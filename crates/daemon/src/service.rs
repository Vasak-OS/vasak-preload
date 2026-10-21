//! Servicio D-Bus para vasak-preload.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use vasak_preload_config::Config;
use vasak_preload_orchestrator::{
    ControlEvent, DefaultAdmissionPolicy, DefaultModelUpdater, EngineView, ExeKey,
    GreedyPrefetchPlanner, MarkovPredictor, NoopPrefetcher, Predictor, PreloadEngine, ReloadBundle,
    Scanner, Services, SqliteRepository, SystemClock,
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
    state: Mutex<DaemonState>,
    /// Lo que publica el motor: almacenes, último tick y pausa.
    view: Arc<RwLock<EngineView>>,
    /// De dónde sale la configuración: `ReloadConfig` la vuelve a leer del
    /// mismo lugar que el arranque.
    config_source: ConfigSource,
}

/// De dónde se lee la configuración.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSource {
    /// Un único archivo (`--config`). Si falta, es un error.
    File(PathBuf),
    /// Todos los `*.toml` de un directorio, en orden alfabético (`--config-dir`).
    /// Se vuelve a leer el directorio en cada carga.
    Dir(PathBuf),
    /// El archivo del sistema y el del usuario, los que existan.
    Default,
}

impl ConfigSource {
    /// Lee la configuración de esta fuente.
    pub fn load(&self) -> Result<Config, Box<dyn std::error::Error + Send + Sync>> {
        match self {
            ConfigSource::File(path) => Ok(Config::cargar(path)?),
            ConfigSource::Dir(dir) => {
                let mut paths = Vec::new();
                for entry in std::fs::read_dir(dir)? {
                    let path = entry?.path();
                    if path.extension().is_some_and(|ext| ext == "toml") {
                        paths.push(path);
                    }
                }
                paths.sort();
                Ok(Config::cargar_multiple(paths)?)
            }
            ConfigSource::Default => Ok(Config::cargar_multiple([
                PathBuf::from("/etc/vasak-preload/config.toml"),
                dirs::config_dir()
                    .map(|d| d.join("vasak-preload/config.toml"))
                    .unwrap_or_default(),
            ])?),
        }
    }
}

impl PreloadService {
    /// Crea un nuevo servicio. El motor se arranca aparte, con `start_engine`.
    pub async fn new(
        config: Config,
        config_source: ConfigSource,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            config: RwLock::new(config),
            control_tx: Mutex::new(None),
            state: Mutex::new(DaemonState::Starting),
            view: Arc::new(RwLock::new(EngineView::default())),
            config_source,
        })
    }

    /// Inicia el motor en background con el escáner dado (en producción,
    /// `ProcfsScanner`; las pruebas inyectan uno que no depende de `/proc`).
    pub async fn start_engine(
        &self,
        scanner: Box<dyn Scanner + Send + Sync>,
    ) -> Result<EngineTask, Box<dyn std::error::Error>> {
        let config = self.config.read().await.clone();

        // Crear repositorio de persistencia
        let repo = SqliteRepository::new(&config.persistencia.estado_path).await?;

        // Crear servicios
        let services = Services {
            scanner,
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

        engine.publish_to(self.view.clone());

        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();

        let cancel_engine = cancel.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = engine.run_until(cancel_engine, control_rx).await {
                error!("Motor terminó con error: {}", e);
            }
        });

        *self.control_tx.lock().await = Some(control_tx);
        *self.state.lock().await = DaemonState::Running;

        Ok(EngineTask { cancel, task })
    }

    /// La vista que lee la interfaz, para poblarla desde las pruebas.
    #[cfg(test)]
    pub(crate) fn view(&self) -> Arc<RwLock<EngineView>> {
        self.view.clone()
    }

    /// Manda un evento al motor. Falla si el motor todavía no arrancó (el
    /// servicio se publica antes) o si ya terminó.
    async fn send(&self, event: ControlEvent) -> zbus::fdo::Result<()> {
        let not_running = || FdoError::Failed("el motor no está corriendo".into());
        let guard = self.control_tx.lock().await;
        let tx = guard.as_ref().ok_or_else(not_running)?;
        tx.send(event).map_err(|_| not_running())
    }
}

/// Arma las estadísticas a partir de lo que publicó el motor.
pub(crate) fn stats_from_view(state: DaemonState, view: &EngineView) -> DaemonStats {
    let stores = &view.stores;
    let mut stats = DaemonStats {
        state,
        exe_count: stores.exes.len() as u32,
        map_count: stores.maps.len() as u32,
        edge_count: stores.markov.len() as u32,
        active_count: stores.active.len() as u32,
        model_time: stores.model_time,
        ..DaemonStats::default()
    };
    if let Some(tick) = &view.last_tick {
        stats.scan_count = tick.scan_id;
        stats.last_tick_scan_id = tick.scan_id;
        stats.last_tick_exes_added = tick.model_delta.exes_added;
        stats.last_tick_maps_added = tick.model_delta.maps_added;
        stats.last_tick_edges_added = tick.model_delta.edges_added;
        stats.last_tick_edges_updated = tick.model_delta.edges_updated;
        stats.last_tick_prediction_exes = tick.prediction.total_exes as u32;
        stats.last_tick_prediction_max_prob = f64::from(tick.prediction.max_probability);
        stats.last_tick_prefetch_attempted = tick.prefetch.attempted;
        stats.last_tick_prefetch_succeeded = tick.prefetch.succeeded;
        stats.last_tick_prefetch_failed = tick.prefetch.failed;
        stats.last_tick_prefetch_bytes = tick.prefetch.bytes_prefetched;
        if let Some(mem) = &tick.memstat {
            stats.last_tick_mem_free_pct = mem.free_percent();
        }
    }
    stats
}

/// Implementación de la interfaz D-Bus.
#[interface(name = "org.vasakos.Preload1")]
impl PreloadService {
    #[zbus(property)]
    async fn state(&self) -> zbus::fdo::Result<DaemonState> {
        Ok(*self.state.lock().await)
    }

    #[zbus(property)]
    async fn stats(&self) -> zbus::fdo::Result<DaemonStats> {
        let state = *self.state.lock().await;
        Ok(stats_from_view(state, &*self.view.read().await))
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
        // El candado cubre el envío y la escritura: con Pause y Resume a la vez,
        // el estado queda en el mismo orden que los eventos que recibe el motor.
        let mut state = self.state.lock().await;
        self.send(ControlEvent::SetPaused(true)).await?;
        *state = DaemonState::Paused;
        info!("Prefetch pausado");
        Ok(())
    }

    async fn resume(&self) -> zbus::fdo::Result<()> {
        let mut state = self.state.lock().await;
        self.send(ControlEvent::SetPaused(false)).await?;
        *state = DaemonState::Running;
        info!("Prefetch reanudado");
        Ok(())
    }

    async fn save_now(&self) -> zbus::fdo::Result<()> {
        self.send(ControlEvent::SaveNow).await
    }

    async fn reload_config(&self) -> zbus::fdo::Result<()> {
        let new_config = self
            .config_source
            .load()
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
        let view = self.view.read().await;
        let stores = &view.stores;
        let prediction = MarkovPredictor::new().predict(stores);

        let mut result = Vec::new();
        for (exe_id, prob) in prediction.top_n(n as usize) {
            if let Some(exe) = stores.exes.get(exe_id) {
                result.push((exe.key.path().to_string_lossy().to_string(), prob));
            }
        }
        Ok(result)
    }

    async fn get_exe_state(&self, path: String) -> zbus::fdo::Result<ExeStateWrapper> {
        let view = self.view.read().await;
        let stores = &view.stores;

        let exe_key = ExeKey::new(PathBuf::from(&path));
        let Some(exe) = stores
            .exes
            .id_by_key(&exe_key)
            .and_then(|id| stores.exes.get(id).map(|exe| (id, exe)))
        else {
            return Ok(ExeStateWrapper::none());
        };
        let (exe_id, exe) = exe;

        let prediction = MarkovPredictor::new().predict(stores);

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
