//! Motor principal de vasak-preload.

#![forbid(unsafe_code)]

use crate::clock::Clock;
use crate::config::Config;
use crate::domain::{ExeKey, MapSegment, MarkovState, MemStat};
use crate::error::Error;
use crate::observation::{AdmissionPolicy, ModelDelta, ModelUpdater, ObservationEvent, Scanner};
use crate::persistence::{
    ExeMapRecord, ExeRecord, MapRecord, MarkovRecord, SNAPSHOT_SCHEMA_VERSION, SnapshotMeta,
    StateRepository, StateSnapshot, StoresSnapshot,
};
use crate::prediction::{Prediction, Predictor};
use crate::prefetch::{PrefetchPlanner, PrefetchReport, Prefetcher};
use crate::stores::Stores;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Servicios que el motor usa (inyección de dependencias).
pub struct Services {
    pub scanner: Box<dyn Scanner + Send + Sync>,
    pub admission: Box<dyn AdmissionPolicy + Send + Sync>,
    pub updater: Box<dyn ModelUpdater + Send + Sync>,
    pub predictor: Box<dyn Predictor + Send + Sync>,
    pub planner: Box<dyn PrefetchPlanner + Send + Sync>,
    pub prefetcher: Box<dyn Prefetcher + Send + Sync>,
    pub repo: Box<dyn StateRepository + Send + Sync>,
    pub clock: Box<dyn Clock + Send + Sync>,
}

/// Paquete de recarga de configuración en caliente.
pub struct ReloadBundle {
    pub config: Config,
    pub admission: Box<dyn AdmissionPolicy + Send + Sync>,
    pub updater: Box<dyn ModelUpdater + Send + Sync>,
    pub predictor: Box<dyn Predictor + Send + Sync>,
    pub planner: Box<dyn PrefetchPlanner + Send + Sync>,
    pub prefetcher: Box<dyn Prefetcher + Send + Sync>,
}

/// Eventos de control para el motor.
pub enum ControlEvent {
    Reload(Box<ReloadBundle>),
    DumpStatus,
    SaveNow,
    /// Pausa (true) o reanuda (false) el prefetch; el escaneo y el modelo siguen.
    SetPaused(bool),
}

/// Reporte de un ciclo (tick) del motor.
#[derive(Debug, Clone)]
pub struct TickReport {
    pub scan_id: u64,
    pub model_delta: ModelDelta,
    pub prediction: crate::prediction::PredictionSummary,
    pub prefetch: PrefetchReport,
    pub memstat: Option<MemStat>,
}

/// Lo que el motor publica para quien lo mira desde afuera (el servicio D-Bus).
#[derive(Debug, Default, Clone)]
pub struct EngineView {
    pub stores: Stores,
    pub last_tick: Option<TickReport>,
    pub paused: bool,
}

/// Motor principal que ejecuta ciclos de escaneo/predicción/prefetch.
pub struct PreloadEngine {
    config: Config,
    services: Services,
    stores: Stores,
    scan_id: u64,
    last_save: Instant,
    paused: bool,
    view: Option<Arc<RwLock<EngineView>>>,
}

impl PreloadEngine {
    /// Crea un nuevo motor con estado vacío (sin cargar persistencia).
    pub async fn new(config: Config, services: Services) -> Result<Self, Error> {
        Ok(Self {
            config,
            services,
            stores: Stores::default(),
            scan_id: 0,
            last_save: Instant::now(),
            paused: false,
            view: None,
        })
    }

    /// Carga el estado desde el repositorio configurado y crea el motor.
    pub async fn load(config: Config, services: Services) -> Result<Self, Error> {
        let snapshot = services.repo.load().await?;
        let stores = Self::stores_from_snapshot(snapshot, config.modelo.ventana_activa.as_secs())?;
        Ok(Self {
            config,
            services,
            stores,
            scan_id: 0,
            last_save: Instant::now(),
            paused: false,
            view: None,
        })
    }

    /// Ejecuta un ciclo completo sin dormir.
    pub async fn tick(&mut self) -> Result<TickReport, Error> {
        self.scan_id = self.scan_id.saturating_add(1);
        let now = self.stores.model_time;

        let observation = if self.config.sistema.doscan {
            self.services.scanner.scan(now, self.scan_id)?
        } else {
            vec![
                ObservationEvent::ObsBegin {
                    time: now,
                    scan_id: self.scan_id,
                },
                ObservationEvent::ObsEnd {
                    time: now,
                    scan_id: self.scan_id,
                    warnings: Vec::new(),
                },
            ]
        };

        let memstat = observation.iter().find_map(|event| match event {
            ObservationEvent::MemStat { mem } => Some(*mem),
            _ => None,
        });

        let model_delta = if self.config.sistema.doscan {
            self.services.updater.apply(
                &mut self.stores,
                &observation,
                self.services.admission.as_ref(),
            )?
        } else {
            ModelDelta::default()
        };

        let prediction = if self.config.sistema.dopredict {
            self.services.predictor.predict(&self.stores)
        } else {
            Prediction::default()
        };

        let plan = if self.config.sistema.doprefetch && !self.paused {
            if let Some(mem) = memstat {
                self.services.planner.plan(&prediction, &self.stores, &mem)
            } else {
                crate::prefetch::PrefetchPlan {
                    maps: Vec::new(),
                    total_bytes: 0,
                    budget_bytes: 0,
                }
            }
        } else {
            crate::prefetch::PrefetchPlan {
                maps: Vec::new(),
                total_bytes: 0,
                budget_bytes: 0,
            }
        };

        let prefetch = self.services.prefetcher.execute(&plan, &self.stores).await;

        // Avanzar tiempo del modelo en un ciclo
        self.stores.model_time = self
            .stores
            .model_time
            .saturating_add(self.config.modelo.ciclo.as_secs());

        Ok(TickReport {
            scan_id: self.scan_id,
            model_delta,
            prediction: prediction.summarize(),
            prefetch,
            memstat,
        })
    }

    /// Ejecuta ciclos hasta que se cancela. Maneja autoguardado.
    pub async fn run_until(
        &mut self,
        cancel: CancellationToken,
        mut control_rx: mpsc::UnboundedReceiver<ControlEvent>,
    ) -> Result<(), Error> {
        let mut last_save = Instant::now();
        let autosave = self
            .config
            .persistencia
            .autosave_interval
            .unwrap_or(self.config.sistema.autosave);

        self.publish(None).await;

        loop {
            let tick_start = Instant::now();
            let mut did_tick = false;

            tokio::select! {
                _ = cancel.cancelled() => {
                    if self.config.persistencia.save_on_shutdown {
                        let _ = self.save().await;
                    }
                    info!("apagado solicitado");
                    break;
                }
                Some(event) = control_rx.recv() => {
                    self.handle_control(event).await?;
                    self.publish(None).await;
                }
                result = self.tick() => {
                    let report = result?;
                    self.publish(Some(report)).await;
                    did_tick = true;
                }
            }

            if autosave.as_secs() > 0 {
                let elapsed = last_save.elapsed();
                if elapsed >= autosave {
                    self.save().await?;
                    last_save = Instant::now();
                }
            }

            if did_tick {
                let elapsed = tick_start.elapsed();
                if elapsed < self.config.modelo.ciclo {
                    let sleep_for = self.config.modelo.ciclo - elapsed;
                    self.services.clock.sleep(sleep_for).await;
                }
            }
        }

        Ok(())
    }

    /// Persiste el estado actual via el repositorio configurado.
    pub async fn save(&self) -> Result<(), Error> {
        let snapshot = Self::snapshot_from_stores(&self.stores);
        self.services.repo.save(&snapshot).await
    }

    /// Publica el estado del motor en `view` después de cada tick y de cada
    /// evento de control.
    pub fn publish_to(&mut self, view: Arc<RwLock<EngineView>>) {
        self.view = Some(view);
    }

    async fn publish(&self, last_tick: Option<TickReport>) {
        let Some(view) = &self.view else { return };
        let mut view = view.write().await;
        view.stores = self.stores.clone();
        view.paused = self.paused;
        if last_tick.is_some() {
            view.last_tick = last_tick;
        }
    }

    /// Acceso de solo lectura a los almacenes (útil para tests).
    pub fn stores(&self) -> &Stores {
        &self.stores
    }

    async fn handle_control(&mut self, event: ControlEvent) -> Result<(), Error> {
        match event {
            ControlEvent::Reload(bundle) => {
                self.apply_reload(*bundle);
                info!("configuración recargada");
            }
            ControlEvent::DumpStatus => {
                self.dump_status();
            }
            ControlEvent::SaveNow => {
                self.save().await?;
                self.last_save = Instant::now();
                info!("estado guardado");
            }
            ControlEvent::SetPaused(paused) => {
                self.paused = paused;
                info!(paused, "prefetch pausado/reanudado");
            }
        }
        Ok(())
    }

    fn apply_reload(&mut self, mut bundle: ReloadBundle) {
        if bundle.config.persistencia.estado_path != self.config.persistencia.estado_path {
            warn!(
                actual = ?self.config.persistencia.estado_path,
                solicitado = ?bundle.config.persistencia.estado_path,
                "ignorando cambio de estado_path durante recarga"
            );
            bundle.config.persistencia.estado_path = self.config.persistencia.estado_path.clone();
        }

        self.config = bundle.config;
        self.services.admission = bundle.admission;
        self.services.updater = bundle.updater;
        self.services.predictor = bundle.predictor;
        self.services.planner = bundle.planner;
        self.services.prefetcher = bundle.prefetcher;
    }

    fn dump_status(&self) {
        let exe_count = self.stores.exes.iter().count();
        let map_count = self.stores.maps.iter().count();
        let edge_count = self.stores.markov.iter().count();
        let active_count = self.stores.active.exes().len();

        info!(?self.config, "configuración actual");
        info!(
            exe_count,
            map_count,
            edge_count,
            active_count,
            model_time = self.stores.model_time,
            "resumen de estado"
        );
        if let Some(stats) = self.services.admission.stats() {
            info!(?stats, "estadísticas de política de admisión");
        }
    }

    fn snapshot_from_stores(stores: &Stores) -> StoresSnapshot {
        let mut exes = Vec::new();
        for (_, exe) in stores.exes.iter() {
            exes.push(ExeRecord {
                path: exe.key.path().to_string_lossy().into_owned(),
                total_running_time: exe.total_running_time,
                last_seen_time: exe.last_seen_time,
            });
        }

        let mut maps = Vec::new();
        for (_, map) in stores.maps.iter() {
            maps.push(MapRecord {
                path: map.path.to_string_lossy().into_owned(),
                offset: map.offset,
                length: map.length,
                update_time: map.update_time,
            });
        }

        let mut exe_maps = Vec::new();
        for (exe_id, exe) in stores.exes.iter() {
            for map_id in stores.exe_maps.maps_for_exe(exe_id) {
                if let Some(map) = stores.maps.get(map_id) {
                    exe_maps.push(ExeMapRecord {
                        exe_path: exe.key.path().to_string_lossy().into_owned(),
                        map_path: map.path.to_string_lossy().into_owned(),
                        map_offset: map.offset,
                        map_length: map.length,
                        prob: 1.0,
                    });
                }
            }
        }

        let mut markov_edges = Vec::new();
        for (key, edge) in stores.markov.iter() {
            let Some(exe_a) = stores.exes.get(key.a()) else {
                continue;
            };
            let Some(exe_b) = stores.exes.get(key.b()) else {
                continue;
            };
            markov_edges.push(MarkovRecord {
                exe_a: exe_a.key.path().to_string_lossy().into_owned(),
                exe_b: exe_b.key.path().to_string_lossy().into_owned(),
                time_to_leave: edge.time_to_leave,
                transition_prob: edge.transition_prob,
                both_running_time: edge.both_running_time,
            });
        }

        StoresSnapshot {
            meta: SnapshotMeta {
                schema_version: SNAPSHOT_SCHEMA_VERSION,
                app_version: None,
                created_at: Some(
                    SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                ),
            },
            state: StateSnapshot {
                model_time: stores.model_time,
                last_accounting_time: stores.last_accounting_time,
                exes,
                maps,
                exe_maps,
                markov_edges,
            },
        }
    }

    fn stores_from_snapshot(snapshot: StoresSnapshot, active_window: u64) -> Result<Stores, Error> {
        let mut stores = Stores {
            model_time: snapshot.state.model_time,
            last_accounting_time: snapshot.state.last_accounting_time,
            ..Default::default()
        };

        for map in snapshot.state.maps {
            let segment = MapSegment::new(
                PathBuf::from(map.path),
                map.offset,
                map.length,
                map.update_time,
            );
            stores.ensure_map(segment);
        }

        for exe in snapshot.state.exes {
            let exe_key = ExeKey::new(PathBuf::from(exe.path));
            let exe_id = stores.ensure_exe(exe_key);
            if let Some(exe_mut) = stores.exes.get_mut(exe_id) {
                exe_mut.total_running_time = exe.total_running_time;
                exe_mut.last_seen_time = exe.last_seen_time;
            }
        }

        // Reconstruir conjunto activo basado en last_seen_time y ventana
        let active: Vec<_> = stores
            .exes
            .iter()
            .filter(|(_, exe)| {
                exe.last_seen_time.is_some_and(|last_seen| {
                    stores.model_time.saturating_sub(last_seen) <= active_window
                })
            })
            .map(|(exe_id, _)| exe_id)
            .collect();
        stores.active.update(&active, stores.model_time);

        for record in snapshot.state.exe_maps {
            let exe_key = ExeKey::new(PathBuf::from(record.exe_path));
            let exe_id = stores
                .exes
                .id_by_key(&exe_key)
                .ok_or_else(|| Error::ExeMissing(exe_key.path().clone()))?;
            // Buscar el mapa por path, offset y length
            let map_id = stores
                .maps
                .iter()
                .find(|(_, m)| {
                    m.path.to_string_lossy() == record.map_path
                        && m.offset == record.map_offset
                        && m.length == record.map_length
                })
                .map(|(id, _)| id)
                .ok_or_else(|| Error::MapMissing(PathBuf::from(record.map_path.clone())))?;
            stores.attach_map(exe_id, map_id);
        }

        for record in snapshot.state.markov_edges {
            let exe_a_key = ExeKey::new(PathBuf::from(record.exe_a));
            let exe_b_key = ExeKey::new(PathBuf::from(record.exe_b));
            let a = stores
                .exes
                .id_by_key(&exe_a_key)
                .ok_or_else(|| Error::ExeMissing(exe_a_key.path().clone()))?;
            let b = stores
                .exes
                .id_by_key(&exe_b_key)
                .ok_or_else(|| Error::ExeMissing(exe_b_key.path().clone()))?;
            let state = MarkovState::Neither;
            let key = crate::stores::EdgeKey::new(a, b);
            if stores.ensure_markov_edge(a, b, stores.model_time, state)
                && let Some(edge) = stores.markov.get_mut(key)
            {
                edge.time_to_leave = record.time_to_leave;
                edge.transition_prob = record.transition_prob;
                edge.both_running_time = record.both_running_time;
            }
        }

        let active = stores.active.exes();
        stores.markov.prune_inactive(&active);

        Ok(stores)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::test::FakeClock;
    use crate::config::{Config, Modelo, Persistencia, Sistema};
    use crate::domain::{ExeKey, MapKey, MapSegment, MarkovState, MemStat};
    use crate::observation::{AdmissionDecision, AdmissionPolicy, CandidateExe, Completeness};
    use crate::observation::{DefaultAdmissionPolicy, DefaultModelUpdater};
    use crate::observation::{ModelUpdater, Observation, ObservationEvent, Scanner};
    use crate::persistence::NoopRepository;
    use crate::prediction::{Prediction, Predictor};
    use crate::prefetch::{
        NoopPrefetcher, PrefetchPlan, PrefetchPlanner, PrefetchReport, Prefetcher,
    };
    use crate::stores::EdgeKey;
    use async_trait::async_trait;
    use proptest::prelude::*;
    use std::collections::{HashMap, HashSet};
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    #[derive(Debug, Clone)]
    struct Recording {
        id: u32,
        hits: Arc<AtomicU32>,
    }

    impl Recording {
        fn record(&self) {
            self.hits.store(self.id, Ordering::SeqCst);
        }
    }

    #[derive(Debug, Default)]
    struct StaticScanner;

    impl Scanner for StaticScanner {
        fn scan(&mut self, time: u64, scan_id: u64) -> Result<Observation, Error> {
            Ok(vec![
                ObservationEvent::ObsBegin { time, scan_id },
                ObservationEvent::MemStat {
                    mem: MemStat {
                        total: 1,
                        free: 1,
                        cached: 1,
                        pagein: 0,
                        pageout: 0,
                    },
                },
                ObservationEvent::ObsEnd {
                    time,
                    scan_id,
                    warnings: Vec::new(),
                },
            ])
        }
    }

    impl AdmissionPolicy for Recording {
        fn allow_exe(&self, _path: &Path) -> bool {
            self.record();
            true
        }

        fn allow_map(&self, _path: &Path) -> bool {
            self.record();
            true
        }

        fn decide(&self, _candidate: &CandidateExe) -> AdmissionDecision {
            self.record();
            AdmissionDecision::Accept {
                completeness: Completeness::Full,
            }
        }
    }

    impl ModelUpdater for Recording {
        fn apply(
            &mut self,
            _stores: &mut Stores,
            _observation: &Observation,
            policy: &dyn AdmissionPolicy,
        ) -> Result<ModelDelta, Error> {
            self.record();
            let candidate = CandidateExe::new(std::path::PathBuf::from("/bin/test"), 0);
            let _ = policy.decide(&candidate);
            Ok(ModelDelta::default())
        }
    }

    impl Predictor for Recording {
        fn predict(&self, _stores: &Stores) -> Prediction {
            self.record();
            Prediction::default()
        }
    }

    impl PrefetchPlanner for Recording {
        fn plan(
            &self,
            _prediction: &Prediction,
            _stores: &Stores,
            _memstat: &MemStat,
        ) -> PrefetchPlan {
            self.record();
            PrefetchPlan {
                maps: Vec::new(),
                total_bytes: 0,
                budget_bytes: 0,
            }
        }
    }

    #[async_trait]
    impl Prefetcher for Recording {
        async fn execute(&self, _plan: &PrefetchPlan, _stores: &Stores) -> PrefetchReport {
            self.record();
            PrefetchReport::default()
        }
    }

    #[tokio::test]
    async fn recarga_intercambia_servicios_runtime() {
        let mut config = Config::default();
        config.sistema.doscan = true;
        config.sistema.dopredict = true;
        config.modelo.ciclo = Duration::from_secs(1);

        let admission_hits = Arc::new(AtomicU32::new(0));
        let updater_hits = Arc::new(AtomicU32::new(0));
        let predictor_hits = Arc::new(AtomicU32::new(0));
        let planner_hits = Arc::new(AtomicU32::new(0));
        let prefetcher_hits = Arc::new(AtomicU32::new(0));

        let services = Services {
            scanner: Box::new(StaticScanner),
            admission: Box::new(Recording {
                id: 1,
                hits: admission_hits.clone(),
            }),
            updater: Box::new(Recording {
                id: 1,
                hits: updater_hits.clone(),
            }),
            predictor: Box::new(Recording {
                id: 1,
                hits: predictor_hits.clone(),
            }),
            planner: Box::new(Recording {
                id: 1,
                hits: planner_hits.clone(),
            }),
            prefetcher: Box::new(Recording {
                id: 1,
                hits: prefetcher_hits.clone(),
            }),
            repo: Box::new(NoopRepository),
            clock: Box::new(crate::clock::SystemClock),
        };

        let mut engine = PreloadEngine::new(config.clone(), services)
            .await
            .expect("motor");
        engine.tick().await.expect("tick");

        assert_eq!(admission_hits.load(Ordering::SeqCst), 1);
        assert_eq!(updater_hits.load(Ordering::SeqCst), 1);
        assert_eq!(predictor_hits.load(Ordering::SeqCst), 1);
        assert_eq!(planner_hits.load(Ordering::SeqCst), 1);
        assert_eq!(prefetcher_hits.load(Ordering::SeqCst), 1);

        let bundle = ReloadBundle {
            config: config.clone(),
            admission: Box::new(Recording {
                id: 2,
                hits: admission_hits.clone(),
            }),
            updater: Box::new(Recording {
                id: 2,
                hits: updater_hits.clone(),
            }),
            predictor: Box::new(Recording {
                id: 2,
                hits: predictor_hits.clone(),
            }),
            planner: Box::new(Recording {
                id: 2,
                hits: planner_hits.clone(),
            }),
            prefetcher: Box::new(Recording {
                id: 2,
                hits: prefetcher_hits.clone(),
            }),
        };

        engine.apply_reload(bundle);
        engine.tick().await.expect("tick");

        assert_eq!(admission_hits.load(Ordering::SeqCst), 2);
        assert_eq!(updater_hits.load(Ordering::SeqCst), 2);
        assert_eq!(predictor_hits.load(Ordering::SeqCst), 2);
        assert_eq!(planner_hits.load(Ordering::SeqCst), 2);
        assert_eq!(prefetcher_hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn motor_con_reloj_falso() {
        let mut config = Config::default();
        config.sistema.doscan = false;
        config.sistema.dopredict = false;
        config.sistema.doprefetch = false;
        config.modelo.ciclo = Duration::from_secs(1);

        let clock = FakeClock::new(1000);
        let services = Services {
            scanner: Box::new(StaticScanner),
            admission: Box::new(DefaultAdmissionPolicy::new(vec![], vec![], 100_000_000)),
            updater: Box::new(DefaultModelUpdater::new()),
            predictor: Box::new(crate::prediction::MarkovPredictor::new()),
            planner: Box::new(crate::prefetch::GreedyPrefetchPlanner::new()),
            prefetcher: Box::new(NoopPrefetcher),
            repo: Box::new(NoopRepository),
            clock: Box::new(clock.clone()),
        };

        let mut engine = PreloadEngine::new(config, services).await.unwrap();
        engine.tick().await.unwrap();

        // El tiempo del modelo es lógico: avanza un ciclo por tick, sin mirar
        // el reloj (que sólo se usa para dormir entre ticks).
        assert_eq!(engine.stores.model_time, 1);
    }

    /// Servicios para un motor que sólo se guarda y se carga: nada escanea.
    fn servicios_con_repo(repo: crate::persistence::SqliteRepository) -> Services {
        Services {
            scanner: Box::new(StaticScanner),
            admission: Box::new(DefaultAdmissionPolicy::new(vec![], vec![], 100_000_000)),
            updater: Box::new(DefaultModelUpdater::new()),
            predictor: Box::new(crate::prediction::MarkovPredictor::new()),
            planner: Box::new(crate::prefetch::GreedyPrefetchPlanner::new()),
            prefetcher: Box::new(NoopPrefetcher),
            repo: Box::new(repo),
            clock: Box::new(crate::clock::SystemClock),
        }
    }

    /// Lo que importa de los almacenes, sin los ids (que cambian al cargar):
    /// cada ejecutable con sus tiempos y los mapas que tiene enganchados.
    fn ejecutables_con_mapas(
        stores: &Stores,
    ) -> Vec<(String, u64, Option<u64>, Vec<(String, u64, u64, u64)>)> {
        let mut resultado: Vec<_> = stores
            .exes
            .iter()
            .map(|(exe_id, exe)| {
                let mut mapas: Vec<_> = stores
                    .exe_maps
                    .maps_for_exe(exe_id)
                    .into_iter()
                    .map(|map_id| {
                        let mapa = stores.maps.get(map_id).expect("mapa enganchado existe");
                        (
                            mapa.path.to_string_lossy().into_owned(),
                            mapa.offset,
                            mapa.length,
                            mapa.update_time,
                        )
                    })
                    .collect();
                mapas.sort();
                (
                    exe.key.path().to_string_lossy().into_owned(),
                    exe.total_running_time,
                    exe.last_seen_time,
                    mapas,
                )
            })
            .collect();
        resultado.sort();
        resultado
    }

    #[tokio::test]
    async fn guardar_y_cargar_conserva_ejecutables_mapas_y_sus_enganches() {
        let dir = tempfile::TempDir::new().unwrap();
        let ruta = dir.path().join("state.sqlite");
        let config = Config::default();

        let repo = crate::persistence::SqliteRepository::new(&ruta)
            .await
            .unwrap();
        let mut original = PreloadEngine::new(config.clone(), servicios_con_repo(repo))
            .await
            .unwrap();
        {
            let stores = &mut original.stores;
            stores.model_time = 500;
            let bash = stores.ensure_exe(ExeKey::new("/usr/bin/bash".into()));
            let vim = stores.ensure_exe(ExeKey::new("/usr/bin/vim".into()));
            let _sin_mapas = stores.ensure_exe(ExeKey::new("/usr/bin/true".into()));
            stores.exes.get_mut(bash).unwrap().total_running_time = 40;
            stores.exes.get_mut(bash).unwrap().last_seen_time = Some(490);
            stores.exes.get_mut(vim).unwrap().total_running_time = 7;

            // Segmentos que difieren de `libc_a` en un solo campo (offset,
            // longitud o ruta): el reenganche tiene que mirar los tres. Se
            // crean después de `libc_a` para que una búsqueda que ignore un
            // campo encuentre `libc_a` primero y enganche el equivocado.
            let libc_a =
                stores.ensure_map(MapSegment::new("/usr/lib/libc.so.6".into(), 0, 4096, 11));
            let otro_offset =
                stores.ensure_map(MapSegment::new("/usr/lib/libc.so.6".into(), 4096, 4096, 12));
            let otra_longitud =
                stores.ensure_map(MapSegment::new("/usr/lib/libc.so.6".into(), 0, 8192, 13));
            let otra_ruta = stores.ensure_map(MapSegment::new(
                "/usr/lib/libncursesw.so.6".into(),
                0,
                4096,
                14,
            ));
            stores.attach_map(bash, libc_a);
            stores.attach_map(vim, otro_offset);
            stores.attach_map(vim, otra_longitud);
            stores.attach_map(vim, otra_ruta);
        }
        original.save().await.unwrap();
        let esperado = ejecutables_con_mapas(&original.stores);

        let repo = crate::persistence::SqliteRepository::new(&ruta)
            .await
            .unwrap();
        let cargado = PreloadEngine::load(config, servicios_con_repo(repo))
            .await
            .unwrap();

        assert_eq!(cargado.stores.model_time, 500);
        assert_eq!(cargado.stores.maps.len(), 4);
        assert_eq!(ejecutables_con_mapas(&cargado.stores), esperado);
    }

    /// Escáner que informa siempre los mismos ejecutables, cada uno con un
    /// mapa propio, y estadísticas de memoria (50 % libre): no depende de `/proc`.
    struct FixedScanner(Vec<(&'static str, u32)>);

    impl Scanner for FixedScanner {
        fn scan(&mut self, time: u64, scan_id: u64) -> Result<Observation, Error> {
            let mut events = vec![
                ObservationEvent::ObsBegin { time, scan_id },
                ObservationEvent::MemStat {
                    mem: MemStat {
                        total: 1000,
                        free: 300,
                        cached: 200,
                        pagein: 0,
                        pageout: 0,
                    },
                },
            ];
            for (path, pid) in &self.0 {
                events.push(ObservationEvent::CandidateExe {
                    exe: CandidateExe::new(path.into(), *pid),
                });
                events.push(ObservationEvent::Map {
                    segment: MapSegment::new(format!("{path}.lib").into(), 0, 4096, time),
                    pid: *pid,
                });
            }
            events.push(ObservationEvent::ObsEnd {
                time,
                scan_id,
                warnings: Vec::new(),
            });
            Ok(events)
        }
    }

    /// Planificador que pide siempre el primer mapa que conozca.
    struct FirstMapPlanner;

    impl PrefetchPlanner for FirstMapPlanner {
        fn plan(&self, _: &Prediction, stores: &Stores, _: &MemStat) -> PrefetchPlan {
            let maps = stores
                .maps
                .iter()
                .take(1)
                .map(|(map_id, map)| crate::prefetch::PrefetchMap {
                    map_id,
                    map_key: map.key(),
                    priority: 1.0,
                    size: map.length,
                })
                .collect();
            PrefetchPlan {
                maps,
                total_bytes: 4096,
                budget_bytes: 4096,
            }
        }
    }

    /// Prefetcher que anota cuántos mapas tenía cada plan que recibió.
    struct PlanLogPrefetcher(Arc<std::sync::Mutex<Vec<usize>>>);

    #[async_trait]
    impl Prefetcher for PlanLogPrefetcher {
        async fn execute(&self, plan: &PrefetchPlan, _stores: &Stores) -> PrefetchReport {
            self.0.lock().unwrap().push(plan.maps.len());
            PrefetchReport::default()
        }
    }

    /// Prefetcher que nunca termina: deja al motor clavado dentro del primer tick.
    struct PendingPrefetcher;

    #[async_trait]
    impl Prefetcher for PendingPrefetcher {
        async fn execute(&self, _plan: &PrefetchPlan, _stores: &Stores) -> PrefetchReport {
            std::future::pending().await
        }
    }

    /// Reloj cuyo sueño dura un milisegundo real: el motor no gira sin ceder el
    /// hilo, y una prueba que falla termina por su `timeout` en lugar de colgarse.
    struct BriefSleepClock;

    #[async_trait]
    impl crate::clock::Clock for BriefSleepClock {
        fn now(&self) -> u64 {
            0
        }

        async fn sleep(&self, _dur: Duration) {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    fn scanning_services(prefetcher: Box<dyn Prefetcher + Send + Sync>) -> Services {
        Services {
            scanner: Box::new(FixedScanner(vec![
                ("/usr/bin/bash", 1),
                ("/usr/bin/vim", 2),
            ])),
            admission: Box::new(DefaultAdmissionPolicy::new(vec![], vec![], 100_000_000)),
            updater: Box::new(DefaultModelUpdater::new()),
            predictor: Box::new(crate::prediction::MarkovPredictor::new()),
            planner: Box::new(FirstMapPlanner),
            prefetcher,
            repo: Box::new(NoopRepository),
            clock: Box::new(BriefSleepClock),
        }
    }

    #[tokio::test]
    async fn con_el_motor_en_pausa_el_prefetcher_recibe_un_plan_vacio_y_al_reanudar_vuelve_a_recibir_mapas()
     {
        let plans = Arc::new(std::sync::Mutex::new(Vec::new()));
        let services = scanning_services(Box::new(PlanLogPrefetcher(plans.clone())));
        let mut engine = PreloadEngine::new(Config::default(), services)
            .await
            .unwrap();

        engine.tick().await.unwrap();
        engine
            .handle_control(ControlEvent::SetPaused(true))
            .await
            .unwrap();
        engine.tick().await.unwrap();
        engine
            .handle_control(ControlEvent::SetPaused(false))
            .await
            .unwrap();
        engine.tick().await.unwrap();

        // Pausado sigue escaneando y modelando: lo único que se omite es el plan.
        assert_eq!(*plans.lock().unwrap(), vec![1, 0, 1]);
        assert_eq!(engine.stores.exes.len(), 2);
    }

    #[tokio::test]
    async fn publicar_deja_en_la_vista_los_almacenes_el_ultimo_tick_y_la_pausa() {
        let services = scanning_services(Box::new(NoopPrefetcher));
        let mut engine = PreloadEngine::new(Config::default(), services)
            .await
            .unwrap();
        // Sin vista, publicar no hace nada.
        engine.publish(None).await;

        let view = Arc::new(RwLock::new(EngineView::default()));
        engine.publish_to(view.clone());
        let report = engine.tick().await.unwrap();
        engine.publish(Some(report)).await;
        {
            let v = view.read().await;
            assert_eq!(v.stores.exes.len(), 2);
            assert_eq!(v.stores.maps.len(), 2);
            let tick = v.last_tick.as_ref().expect("último tick");
            assert_eq!(tick.scan_id, 1);
            assert_eq!(tick.model_delta.exes_added, 2);
            assert_eq!(tick.memstat.map(|m| m.free_percent()), Some(50));
            assert!(!v.paused);
        }

        // Un evento de control publica sin pisar el último tick.
        engine
            .handle_control(ControlEvent::SetPaused(true))
            .await
            .unwrap();
        engine.publish(None).await;
        let v = view.read().await;
        assert!(v.paused);
        assert_eq!(v.last_tick.as_ref().map(|t| t.scan_id), Some(1));
    }

    #[tokio::test]
    async fn run_until_publica_al_arrancar_y_despues_de_un_evento_con_el_tick_trabado() {
        let services = scanning_services(Box::new(PendingPrefetcher));
        let mut engine = PreloadEngine::new(Config::default(), services)
            .await
            .unwrap();
        engine.stores.model_time = 500;
        engine
            .stores
            .ensure_exe(ExeKey::new("/usr/bin/cargado".into()));
        let view = Arc::new(RwLock::new(EngineView::default()));
        engine.publish_to(view.clone());

        let cancel = CancellationToken::new();
        let (tx, rx) = mpsc::unbounded_channel();
        let task = tokio::spawn({
            let cancel = cancel.clone();
            async move { engine.run_until(cancel, rx).await }
        });

        // El primer tick nunca termina (el prefetcher no vuelve): lo que hay en
        // la vista lo publicó el arranque.
        tokio::time::timeout(Duration::from_secs(10), async {
            while view.read().await.stores.exes.len() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("el motor no publicó al arrancar");
        let v = view.read().await;
        assert_eq!(v.stores.model_time, 500);
        assert!(v.last_tick.is_none());
        drop(v);

        // Sin ningún tick que publique, la pausa sólo puede llegar a la vista
        // por el evento de control.
        tx.send(ControlEvent::SetPaused(true)).unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while !view.read().await.paused {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("el evento de control no se publicó");
        assert!(view.read().await.last_tick.is_none());

        cancel.cancel();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn run_until_publica_despues_de_cada_tick_y_de_cada_evento_de_control() {
        let services = scanning_services(Box::new(NoopPrefetcher));
        let mut engine = PreloadEngine::new(Config::default(), services)
            .await
            .unwrap();
        let view = Arc::new(RwLock::new(EngineView::default()));
        engine.publish_to(view.clone());

        let cancel = CancellationToken::new();
        let (tx, rx) = mpsc::unbounded_channel();
        let task = tokio::spawn({
            let cancel = cancel.clone();
            async move { engine.run_until(cancel, rx).await }
        });

        tokio::time::timeout(Duration::from_secs(10), async {
            while view.read().await.last_tick.is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("el motor no publicó ningún tick");
        assert_eq!(view.read().await.stores.exes.len(), 2);

        tx.send(ControlEvent::SetPaused(true)).unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while !view.read().await.paused {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("la pausa no llegó a la vista");

        cancel.cancel();
        task.await.unwrap().unwrap();
        assert!(view.read().await.last_tick.is_some());
    }

    #[tokio::test]
    async fn al_cargar_quedan_activos_todos_los_ejecutables_dentro_de_la_ventana() {
        let dir = tempfile::TempDir::new().unwrap();
        let ruta = dir.path().join("state.sqlite");
        let mut config = Config::default();
        config.modelo.ventana_activa = Duration::from_secs(100);

        let repo = crate::persistence::SqliteRepository::new(&ruta)
            .await
            .unwrap();
        let mut original = PreloadEngine::new(config.clone(), servicios_con_repo(repo))
            .await
            .unwrap();
        {
            let stores = &mut original.stores;
            stores.model_time = 500;
            // Tres dentro de la ventana (500 - last_seen <= 100), uno fuera y uno sin verse.
            for (path, last_seen) in [
                ("/usr/bin/bash", Some(490)),
                ("/usr/bin/vim", Some(480)),
                ("/usr/bin/zsh", Some(495)),
                ("/usr/bin/viejo", Some(100)),
                ("/usr/bin/nunca", None),
            ] {
                let id = stores.ensure_exe(ExeKey::new(path.into()));
                stores.exes.get_mut(id).unwrap().last_seen_time = last_seen;
            }
        }
        original.save().await.unwrap();

        let repo = crate::persistence::SqliteRepository::new(&ruta)
            .await
            .unwrap();
        let cargado = PreloadEngine::load(config, servicios_con_repo(repo))
            .await
            .unwrap();
        let stores = &cargado.stores;

        let path_of = |id| {
            stores
                .exes
                .get(id)
                .unwrap()
                .key
                .path()
                .to_string_lossy()
                .into_owned()
        };
        let mut active: Vec<_> = stores.active.exes().into_iter().map(path_of).collect();
        active.sort();
        assert_eq!(active, ["/usr/bin/bash", "/usr/bin/vim", "/usr/bin/zsh"]);
    }

    #[tokio::test]
    async fn dos_ejecutables_que_corren_juntos_quedan_unidos_por_una_sola_arista() {
        let services = scanning_services(Box::new(NoopPrefetcher));
        let mut engine = PreloadEngine::new(Config::default(), services)
            .await
            .unwrap();

        // Primer tick: no había ejecutables activos, así que no hay con quién unir.
        let first = engine.tick().await.unwrap();
        assert_eq!(first.model_delta.edges_added, 0);
        assert_eq!(engine.stores.markov.len(), 0);

        // Segundo: ya eran activos; la arista (bash, vim) se crea una sola vez
        // aunque el par se recorra en los dos órdenes.
        let second = engine.tick().await.unwrap();
        assert_eq!(second.model_delta.edges_added, 1);
        assert_eq!(second.model_delta.edges_updated, 1);
        assert_eq!(engine.stores.markov.len(), 1);

        // Tercero: la arista ya existe y no se duplica.
        let third = engine.tick().await.unwrap();
        assert_eq!(third.model_delta.edges_added, 0);
        assert_eq!(third.model_delta.edges_updated, 2);
        assert_eq!(engine.stores.markov.len(), 1);

        let stores = &engine.stores;
        let (key, _) = stores.markov.iter().next().unwrap();
        let mut paths =
            [key.a(), key.b()].map(|id| stores.exes.get(id).unwrap().key.path().to_path_buf());
        paths.sort();
        assert_eq!(
            paths,
            [
                PathBuf::from("/usr/bin/bash"),
                PathBuf::from("/usr/bin/vim")
            ]
        );
    }

    #[tokio::test]
    async fn al_cargar_se_conservan_las_aristas_entre_activos_con_sus_campos_y_se_podan_las_de_inactivos()
     {
        let dir = tempfile::TempDir::new().unwrap();
        let ruta = dir.path().join("state.sqlite");
        let mut config = Config::default();
        config.modelo.ventana_activa = Duration::from_secs(100);

        let repo = crate::persistence::SqliteRepository::new(&ruta)
            .await
            .unwrap();
        let mut original = PreloadEngine::new(config.clone(), servicios_con_repo(repo))
            .await
            .unwrap();
        {
            let stores = &mut original.stores;
            stores.model_time = 500;
            let mut id = |path: &str, last_seen| {
                let id = stores.ensure_exe(ExeKey::new(path.into()));
                stores.exes.get_mut(id).unwrap().last_seen_time = Some(last_seen);
                id
            };
            let bash = id("/usr/bin/bash", 490);
            let vim = id("/usr/bin/vim", 480);
            let zsh = id("/usr/bin/zsh", 495);
            let viejo = id("/usr/bin/viejo", 100);

            for (a, b) in [(bash, vim), (bash, zsh), (bash, viejo)] {
                stores.ensure_markov_edge(a, b, 500, MarkovState::Neither);
            }
            let edge = stores.markov.get_mut(EdgeKey::new(vim, bash)).unwrap();
            edge.time_to_leave = [1.5, 2.5, 3.5, 4.5];
            edge.transition_prob[2][3] = 0.25;
            edge.both_running_time = 321;
        }
        original.save().await.unwrap();

        let repo = crate::persistence::SqliteRepository::new(&ruta)
            .await
            .unwrap();
        let cargado = PreloadEngine::load(config, servicios_con_repo(repo))
            .await
            .unwrap();
        let stores = &cargado.stores;

        let id_of = |path: &str| stores.exes.id_by_key(&ExeKey::new(path.into())).unwrap();
        let (bash, vim, zsh, viejo) = (
            id_of("/usr/bin/bash"),
            id_of("/usr/bin/vim"),
            id_of("/usr/bin/zsh"),
            id_of("/usr/bin/viejo"),
        );
        assert_eq!(stores.markov.len(), 2);
        assert!(stores.markov.get(EdgeKey::new(bash, zsh)).is_some());
        assert!(stores.markov.get(EdgeKey::new(bash, viejo)).is_none());

        let edge = stores.markov.get(EdgeKey::new(bash, vim)).unwrap();
        assert_eq!(edge.time_to_leave, [1.5, 2.5, 3.5, 4.5]);
        assert_eq!(edge.transition_prob[2][3], 0.25);
        assert_eq!(edge.both_running_time, 321);
    }
}
