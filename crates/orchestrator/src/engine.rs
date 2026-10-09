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
use std::time::{Instant, SystemTime};
use tokio::sync::mpsc;
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

/// Motor principal que ejecuta ciclos de escaneo/predicción/prefetch.
pub struct PreloadEngine {
    config: Config,
    services: Services,
    stores: Stores,
    scan_id: u64,
    last_save: Instant,
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

        let plan = if self.config.sistema.doprefetch {
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
                }
                result = self.tick() => {
                    result?;
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
        for (exe_id, exe) in stores.exes.iter() {
            if let Some(last_seen) = exe.last_seen_time
                && stores.model_time.saturating_sub(last_seen) <= active_window
            {
                stores.active.update(&[exe_id], stores.model_time);
            }
        }

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
}
