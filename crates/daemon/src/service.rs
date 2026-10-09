//! Servicio D-Bus para vasak-preload.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use vasak_preload_config::Config;
use vasak_preload_orchestrator::{
    ControlEvent, DefaultAdmissionPolicy, DefaultModelUpdater, ExeKey, GreedyPrefetchPlanner,
    MarkovPredictor, NoopPrefetcher, Predictor, PreloadEngine, ProcfsScanner, ReloadBundle,
    Services, SqliteRepository, Stores, SystemClock,
};
use vasak_preload_protocol::{ConfigSnapshot, DaemonState, DaemonStats, ExeState, ExeStateWrapper};
use zbus::fdo::Error as FdoError;
use zbus::interface;

/// Tarea del motor en background. Quien la tiene es quien la apaga.
pub struct EngineTask {
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl EngineTask {
    /// Pide al motor que se detenga y espera a que termine (incluye el guardado final).
    pub async fn shutdown(self) {
        self.cancel.cancel();
        let _ = self.task.await;
    }
}

/// Servicio D-Bus que expone la funcionalidad del daemon.
pub struct PreloadService {
    config: RwLock<Config>,
    control_tx: Mutex<Option<mpsc::UnboundedSender<ControlEvent>>>,
    stores: Arc<RwLock<Stores>>,
    stats: Arc<Mutex<DaemonStats>>,
}

impl PreloadService {
    /// Crea un nuevo servicio. El motor se arranca aparte, con `start_engine`.
    pub async fn new(config: Config) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            config: RwLock::new(config),
            control_tx: Mutex::new(None),
            stores: Arc::new(RwLock::new(Stores::default())),
            stats: Arc::new(Mutex::new(DaemonStats::default())),
        })
    }

    /// Inicia el motor en background.
    pub async fn start_engine(&self) -> Result<EngineTask, Box<dyn std::error::Error>> {
        let config = self.config.read().await.clone();

        // Crear repositorio de persistencia
        let repo = SqliteRepository::new(&config.persistencia.estado_path).await?;

        // Crear servicios
        let services = Services {
            scanner: Box::new(ProcfsScanner::new()),
            admission: Box::new(DefaultAdmissionPolicy::new(
                config.sistema.exeprefix.clone(),
                config.sistema.mapprefix.clone(),
                config.sistema.max_mapa,
            )),
            updater: Box::new(DefaultModelUpdater::new()),
            predictor: Box::new(MarkovPredictor::new()),
            planner: Box::new(GreedyPrefetchPlanner::new()),
            prefetcher: Box::new(NoopPrefetcher), // Usar NoopPrefetcher por seguridad inicial
            repo: Box::new(repo),
            clock: Box::new(SystemClock),
        };

        // Cargar o crear motor
        let mut engine = if config.persistencia.estado_path.exists() {
            PreloadEngine::load(config.clone(), services).await?
        } else {
            PreloadEngine::new(config.clone(), services).await?
        };

        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();

        let cancel_engine = cancel.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = engine.run_until(cancel_engine, control_rx).await {
                error!("Motor terminó con error: {}", e);
            }
        });

        // Task para actualizar stats periódicamente
        let stores = self.stores.clone();
        let stats = self.stats.clone();
        let cancel_stats = cancel.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            while !cancel_stats.is_cancelled() {
                interval.tick().await;
                if cancel_stats.is_cancelled() {
                    break;
                }
                update_stats(&stores, &stats).await;
            }
        });

        *self.control_tx.lock().await = Some(control_tx);

        // Actualizar estado inicial
        self.stats.lock().await.state = DaemonState::Running;
        update_stats(&self.stores, &self.stats).await;

        Ok(EngineTask { cancel, task })
    }

    /// Manda un evento al motor, si está corriendo.
    async fn send(&self, event: ControlEvent) -> zbus::fdo::Result<()> {
        if let Some(tx) = self.control_tx.lock().await.as_ref() {
            tx.send(event)
                .map_err(|_| FdoError::Failed("el motor no está corriendo".into()))?;
        }
        Ok(())
    }
}

/// Actualiza las estadísticas desde los almacenes.
async fn update_stats(stores: &Arc<RwLock<Stores>>, stats: &Arc<Mutex<DaemonStats>>) {
    let stores = stores.read().await;
    let mut stats = stats.lock().await;

    stats.scan_count = stores.model_time;
    stats.exe_count = stores.exes.len() as u32;
    stats.map_count = stores.maps.len() as u32;
    stats.edge_count = stores.markov.len() as u32;
    stats.active_count = stores.active.exes().len() as u32;
    stats.model_time = stores.model_time;
}

/// Implementación de la interfaz D-Bus.
#[interface(name = "org.vasakos.Preload1")]
impl PreloadService {
    #[zbus(property)]
    async fn state(&self) -> zbus::fdo::Result<DaemonState> {
        Ok(self.stats.lock().await.state)
    }

    #[zbus(property)]
    async fn stats(&self) -> zbus::fdo::Result<DaemonStats> {
        Ok(self.stats.lock().await.clone())
    }

    #[zbus(property)]
    async fn config(&self) -> zbus::fdo::Result<ConfigSnapshot> {
        let config = self.config.read().await;
        Ok(ConfigSnapshot {
            modelo_ciclo_secs: config.modelo.ciclo.as_secs(),
            modelo_ventana_activa_secs: config.modelo.ventana_activa.as_secs(),
            sistema_doscan: config.sistema.doscan,
            sistema_dopredict: config.sistema.dopredict,
            sistema_doprefetch: config.sistema.doprefetch,
            // El intervalo que usa el motor: sin uno propio de persistencia,
            // el del sistema. 0 sólo si de verdad está deshabilitado.
            persistencia_autosave_secs: config
                .persistencia
                .autosave_interval
                .unwrap_or(config.sistema.autosave)
                .as_secs(),
        })
    }

    async fn pause(&self) -> zbus::fdo::Result<()> {
        let mut stats = self.stats.lock().await;
        stats.state = DaemonState::Paused;
        info!("Prefetch pausado");
        Ok(())
    }

    async fn resume(&self) -> zbus::fdo::Result<()> {
        let mut stats = self.stats.lock().await;
        stats.state = DaemonState::Running;
        info!("Prefetch reanudado");
        Ok(())
    }

    async fn save_now(&self) -> zbus::fdo::Result<()> {
        self.send(ControlEvent::SaveNow).await
    }

    async fn reload_config(&self) -> zbus::fdo::Result<()> {
        let new_config = Config::cargar_multiple(vec![
            PathBuf::from("/etc/vasak-preload/config.toml"),
            dirs::config_dir()
                .map(|d| d.join("vasak-preload/config.toml"))
                .unwrap_or_default(),
        ])
        .map_err(|e| FdoError::Failed(format!("configuración inválida: {e}")))?;

        let bundle = ReloadBundle {
            config: new_config.clone(),
            admission: Box::new(DefaultAdmissionPolicy::new(
                new_config.sistema.exeprefix.clone(),
                new_config.sistema.mapprefix.clone(),
                new_config.sistema.max_mapa,
            )),
            updater: Box::new(DefaultModelUpdater::new()),
            predictor: Box::new(MarkovPredictor::new()),
            planner: Box::new(GreedyPrefetchPlanner::new()),
            prefetcher: Box::new(NoopPrefetcher),
        };
        self.send(ControlEvent::Reload(Box::new(bundle))).await?;

        *self.config.write().await = new_config;
        info!("Configuración recargada");
        Ok(())
    }

    async fn get_top_predictions(&self, n: u32) -> zbus::fdo::Result<Vec<(String, f32)>> {
        let stores = self.stores.read().await;
        let prediction = MarkovPredictor::new().predict(&stores);

        let mut result = Vec::new();
        for (exe_id, prob) in prediction.top_n(n as usize) {
            if let Some(exe) = stores.exes.get(exe_id) {
                result.push((exe.key.path().to_string_lossy().to_string(), prob));
            }
        }
        Ok(result)
    }

    async fn get_exe_state(&self, path: String) -> zbus::fdo::Result<ExeStateWrapper> {
        let stores = self.stores.read().await;

        let exe_key = ExeKey::new(PathBuf::from(&path));
        let Some(exe) = stores
            .exes
            .id_by_key(&exe_key)
            .and_then(|id| stores.exes.get(id).map(|exe| (id, exe)))
        else {
            return Ok(ExeStateWrapper::none());
        };
        let (exe_id, exe) = exe;

        let prediction = MarkovPredictor::new().predict(&stores);

        Ok(ExeStateWrapper::some(ExeState {
            path,
            running: exe.running,
            total_running_time: exe.total_running_time,
            last_seen_time: exe.last_seen_time.unwrap_or(0),
            prediction_probability: prediction.get_probability(exe_id),
            map_count: stores.exe_maps.maps_for_exe(exe_id).len() as u32,
        }))
    }
}
