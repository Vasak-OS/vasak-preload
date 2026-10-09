//! Servicio D-Bus para vasak-preload.

#![forbid(unsafe_code)]

use vasak_preload_config::Config;
use vasak_preload_orchestrator::{
    clock::SystemClock, engine::{ControlEvent, PreloadEngine, ReloadBundle, Services},
    observation::DefaultAdmissionPolicy, observation::DefaultModelUpdater,
    observation::ProcfsScanner, persistence::SqliteRepository, prediction::MarkovPredictor,
    prefetch::GreedyPrefetchPlanner, prefetch::NoopPrefetcher, Stores,
};
use vasak_preload_protocol::{DaemonState, DaemonStats, ExeState, ExeStateWrapper};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use zbus::fdo::{Result as FdoResult, Error as FdoError};
use zbus::interface;

/// Handle para controlar el motor desde el servicio.
pub struct EngineHandle {
    control_tx: mpsc::UnboundedSender<ControlEvent>,
    cancel: CancellationToken,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl EngineHandle {
    pub fn new(
        control_tx: mpsc::UnboundedSender<ControlEvent>,
        cancel: CancellationToken,
        task: tokio::task::JoinHandle<()>,
    ) -> Self {
        Self {
            control_tx,
            cancel,
            task: Mutex::new(Some(task)),
        }
    }

    pub fn send(&self, event: ControlEvent) -> Result<(), mpsc::error::SendError<ControlEvent>> {
        self.control_tx.send(event)
    }

    pub async fn cancel(&self) {
        self.cancel.cancel();
        if let Some(task) = self.task.lock().await.take() {
            let _ = task.await;
        }
    }
}

/// Servicio D-Bus que expone la funcionalidad del daemon.
pub struct PreloadService {
    config: RwLock<Config>,
    engine: Mutex<Option<EngineHandle>>,
    stores: Arc<RwLock<Stores>>,
    stats: Arc<Mutex<DaemonStats>>,
}

impl PreloadService {
    /// Crea un nuevo servicio e inicializa el motor.
    pub async fn new(config: Config) -> Result<Self, Box<dyn std::error::Error>> {
        let stores = Arc::new(RwLock::new(orchestrator::Stores::default()));
        let stats = Mutex::new(DaemonStats {
            state: DaemonState::Starting,
            scan_count: 0,
            exe_count: 0,
            map_count: 0,
            edge_count: 0,
            active_count: 0,
            model_time: 0,
            last_tick_scan_id: 0,
            last_tick_exes_added: 0,
            last_tick_maps_added: 0,
            last_tick_edges_added: 0,
            last_tick_edges_updated: 0,
            last_tick_prediction_exes: 0,
            last_tick_prediction_max_prob: 0.0,
            last_tick_prefetch_attempted: 0,
            last_tick_prefetch_succeeded: 0,
            last_tick_prefetch_failed: 0,
            last_tick_prefetch_bytes: 0,
            last_tick_mem_free_pct: 255,
        });

        Ok(Self {
            config: RwLock::new(config),
            engine: Mutex::new(None),
            stores,
            stats,
        })
    }

    /// Inicia el motor en background.
    pub async fn start_engine(&self) -> Result<EngineHandle, Box<dyn std::error::Error>> {
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
        let engine = if config.persistencia.estado_path.exists() {
            PreloadEngine::load(config.clone(), services).await?
        } else {
            PreloadEngine::new(config.clone(), services).await?
        };

        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let stores = self.stores.clone();
        let stats = self.stats.clone();

        // Clonar para el task
        let mut engine = engine;
        let cancel_clone = cancel.clone();

        let task = tokio::spawn(async move {
            if let Err(e) = engine.run_until(cancel_clone, control_rx).await {
                error!("Motor terminó con error: {}", e);
            }
        });

        // Task para actualizar stats periódicamente
        let stores_clone = stores.clone();
        let stats_clone = stats.clone();
        let cancel_stats = cancel.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            while !cancel_stats.is_cancelled() {
                interval.tick().await;
                if cancel_stats.is_cancelled() {
                    break;
                }
                update_stats(&stores_clone, &stats_clone).await;
            }
        });

        let handle = EngineHandle::new(control_tx, cancel, task);
        *self.engine.lock().await = Some(handle);

        // Actualizar estado inicial
        {
            let mut stats = self.stats.lock().await;
            stats.state = DaemonState::Running;
        }
        update_stats(&self.stores, &self.stats).await;

        Ok(self.engine.lock().await.as_ref().unwrap().clone())
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
            persistencia_autosave_secs: config.persistencia.autosave_interval.map(|d| d.as_secs()),
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
        if let Some(engine) = self.engine.lock().await.as_ref() {
            engine.send(ControlEvent::SaveNow)?;
        }
        Ok(())
    }

    async fn reload_config(&self) -> zbus::fdo::Result<()> {
        let new_config = Config::cargar_multiple(vec![
            PathBuf::from("/etc/vasak-preload/config.toml"),
            dirs::config_dir()
                .map(|d| d.join("vasak-preload/config.toml"))
                .unwrap_or_default(),
        ])?;

        if let Some(engine) = self.engine.lock().await.as_ref() {
            let admission = Box::new(DefaultAdmissionPolicy::new(
                new_config.sistema.exeprefix.clone(),
                new_config.sistema.mapprefix.clone(),
                new_config.sistema.max_mapa,
            ));
            let updater = Box::new(DefaultModelUpdater::new());
            let predictor = Box::new(MarkovPredictor::new());
            let planner = Box::new(GreedyPrefetchPlanner::new());
            let prefetcher = Box::new(NoopPrefetcher);

            let bundle = ReloadBundle {
                config: new_config.clone(),
                admission,
                updater,
                predictor,
                planner,
                prefetcher,
            };
            engine.send(ControlEvent::Reload(Box::new(bundle)))?;
        }

        *self.config.write().await = new_config;
        info!("Configuración recargada");
        Ok(())
    }

    async fn get_top_predictions(&self, n: u32) -> zbus::fdo::Result<Vec<(String, f32)>> {
        let stores = self.stores.read().await;
        let config = self.config.read().await;
        let predictor = MarkovPredictor::new();
        let prediction = predictor.predict(&stores);
        let top = prediction.top_n(n as usize);

        let mut result = Vec::new();
        for (exe_id, prob) in top {
            if let Some(exe) = stores.exes.get(exe_id) {
                result.push((exe.key.path().to_string_lossy().to_string(), prob));
            }
        }
        Ok(result)
    }

    async fn get_exe_state(&self, path: String) -> zbus::fdo::Result<ExeStateWrapper> {
        let stores = self.stores.read().await;
        let config = self.config.read().await;

        let exe_key = orchestrator::domain::ExeKey::new(PathBuf::from(&path));
        let Some(exe_id) = stores.exes.id_by_key(&exe_key) else {
            return Ok(ExeStateWrapper::none());
        };

        let exe = stores.exes.get(exe_id).unwrap();
        let predictor = MarkovPredictor::new();
        let prediction = predictor.predict(&stores);
        let prob = prediction.get_probability(exe_id);
        let map_count = stores.exe_maps.maps_for_exe(exe_id).len() as u32;

        Ok(ExeStateWrapper::some(ExeState {
            path,
            running: exe.running,
            total_running_time: exe.total_running_time,
            last_seen_time: exe.last_seen_time,
            prediction_probability: prob,
            map_count,
        }))
    }
}