//! Actualizador del modelo a partir de observaciones.

#![forbid(unsafe_code)]

use crate::domain::{ExeId, ExeKey, MarkovState};
use crate::error::Error;
use crate::observation::{
    AdmissionDecision, AdmissionPolicy, CandidateExe, Completeness, Observation, ObservationEvent,
};
use crate::stores::{EdgeKey, Stores};
use std::collections::HashMap;

/// Cambios en el modelo tras una actualización.
#[derive(Debug, Clone, Default)]
pub struct ModelDelta {
    pub exes_added: u32,
    pub maps_added: u32,
    pub edges_added: u32,
    pub edges_updated: u32,
}

/// Actualizador del modelo a partir de observaciones.
pub trait ModelUpdater: Send + Sync {
    /// Aplica las observaciones al almacén.
    fn apply(
        &mut self,
        stores: &mut Stores,
        observation: &Observation,
        policy: &dyn AdmissionPolicy,
    ) -> Result<ModelDelta, Error>;
}

/// Actualizador de modelo por defecto.
#[derive(Debug, Default)]
pub struct DefaultModelUpdater;

impl DefaultModelUpdater {
    /// Crea un nuevo actualizador.
    pub fn new() -> Self {
        Self
    }
}

impl ModelUpdater for DefaultModelUpdater {
    fn apply(
        &mut self,
        stores: &mut Stores,
        observation: &Observation,
        policy: &dyn AdmissionPolicy,
    ) -> Result<ModelDelta, Error> {
        let mut delta = ModelDelta::default();
        let mut current_time = stores.model_time;
        let mut candidates: HashMap<u32, CandidateExe> = HashMap::new();

        // Procesar eventos
        for event in observation {
            match event {
                ObservationEvent::ObsBegin { time, .. } => {
                    current_time = *time;
                }
                // La memoria la usa el planificador, no el modelo.
                ObservationEvent::MemStat { .. } => {}
                ObservationEvent::CandidateExe { exe } => {
                    candidates.insert(exe.pid, exe.clone());
                }
                ObservationEvent::Map { segment, pid } => {
                    if let Some(candidate) = candidates.get_mut(pid) {
                        candidate.maps.push(segment.clone());
                        candidate.total_size += segment.length;
                    }
                }
                ObservationEvent::ObsEnd { .. } => {}
            }
        }

        // Determinar qué ejecutables están corriendo ahora
        let mut running_exes: HashMap<ExeId, bool> = HashMap::new();

        for (_pid, candidate) in candidates {
            let decision = policy.decide(&candidate);

            match decision {
                AdmissionDecision::Accept { completeness } => {
                    // Asegurar ejecutable en almacén
                    let exe_key = ExeKey::new(candidate.path.clone());
                    let exe_id = stores.ensure_exe(exe_key);

                    // Marcar como corriendo
                    if let Some(exe) = stores.exes.get_mut(exe_id) {
                        exe.mark_running(current_time);
                        running_exes.insert(exe_id, true);
                    }

                    // Procesar mapeos según completitud
                    match completeness {
                        Completeness::ExeOnly => {
                            // Solo ejecutable, sin mapeos
                        }
                        Completeness::Partial | Completeness::Full => {
                            for map in &candidate.maps {
                                let map_id = stores.ensure_map(map.clone());
                                stores.attach_map(exe_id, map_id);
                                delta.maps_added += 1;
                            }
                        }
                    }

                    delta.exes_added += 1;
                }
                AdmissionDecision::Reject { .. } => {
                    // No hacer nada, ya contabilizado en la política
                }
            }
        }

        // Marcar ejecutables que ya no corren
        let exe_ids: Vec<ExeId> = stores.exes.iter().map(|(id, _)| id).collect();
        for exe_id in exe_ids {
            if !running_exes.contains_key(&exe_id)
                && let Some(exe_mut) = stores.exes.get_mut(exe_id)
                && exe_mut.running
            {
                exe_mut.mark_stopped(current_time);
            }
        }

        // Actualizar modelo de Markov. El conjunto activo todavía es el del
        // tick anterior: de ahí sale el estado de partida de cada arista.
        // Salvo tras cargar: el activo reconstruido son los vistos en la
        // ventana, no los que corrían, así que se parte de vacío para no
        // inventar transiciones en el primer tick.
        let was_running = if stores.active_reconstructed {
            std::collections::HashSet::new()
        } else {
            stores.active.exes()
        };
        stores.active_reconstructed = false;
        let dt = current_time.saturating_sub(stores.active.last_update()) as f32;
        let state_of = |running: &dyn Fn(ExeId) -> bool, key: EdgeKey| {
            MarkovState::Neither.transition(running(key.a()), running(key.b()))
        };
        let before = |id: ExeId| was_running.contains(&id);
        let now = |id: ExeId| running_exes.contains_key(&id);

        // Pares nuevos entre lo que corría y lo que corre: sin ellos nunca se
        // aprende que B arranca mientras A corre. Sólo entre ejecutables con
        // mapas (aplicaciones reales): si no, el grafo crece O(n²) con cada
        // proceso del sistema y no se poda hasta la próxima carga.
        let has_maps = |id: ExeId| !stores.exe_maps.maps_for_exe(id).is_empty();
        let mut involved: Vec<ExeId> = was_running
            .iter()
            .copied()
            .filter(|id| has_maps(*id))
            .collect();
        involved.extend(
            running_exes
                .keys()
                .copied()
                .filter(|id| !was_running.contains(id) && has_maps(*id)),
        );
        for (i, &x) in involved.iter().enumerate() {
            for &y in &involved[i + 1..] {
                let key = EdgeKey::new(x, y);
                if stores.ensure_markov_edge(x, y, current_time, state_of(&before, key)) {
                    delta.edges_added += 1;
                }
            }
        }

        // Cada arista observa una vez por tick la transición de su estado
        // anterior al actual, con la orientación de su clave (a <= b).
        let keys: Vec<EdgeKey> = stores.markov.iter().map(|(key, _)| key).collect();
        for key in keys {
            let (from, to) = (state_of(&before, key), state_of(&now, key));
            if let Some(edge) = stores.markov.get_mut(key) {
                edge.observe_transition(from, to, dt);
                delta.edges_updated += 1;
            }
        }

        // Actualizar conjunto activo
        let active_now: Vec<ExeId> = running_exes.keys().copied().collect();
        stores.active.update(&active_now, current_time);

        Ok(delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ExeKey, MapSegment, MemStat};
    use crate::observation::{DefaultAdmissionPolicy, ObservationEvent};
    use crate::stores::Stores;
    use std::path::Path;

    #[test]
    fn actualiza_modelo_basico() {
        let mut stores = Stores::default();
        let mut updater = DefaultModelUpdater::new();
        let policy = DefaultAdmissionPolicy::new(vec![], vec![], 100_000_000);

        let obs = vec![
            ObservationEvent::ObsBegin {
                time: 100,
                scan_id: 1,
            },
            ObservationEvent::MemStat {
                mem: MemStat {
                    total: 1000,
                    free: 500,
                    cached: 200,
                    available: 700,
                    pagein: 0,
                    pageout: 0,
                },
            },
            ObservationEvent::CandidateExe {
                exe: CandidateExe::new("/usr/bin/firefox".into(), 1234),
            },
            ObservationEvent::ObsEnd {
                time: 100,
                scan_id: 1,
                warnings: vec![],
            },
        ];

        let delta = updater.apply(&mut stores, &obs, &policy).unwrap();
        assert_eq!(delta.exes_added, 1);
        assert!(
            stores
                .exes
                .iter()
                .any(|(_, e)| e.key.path() == Path::new("/usr/bin/firefox"))
        );
    }

    use crate::observation::{AdmissionPolicy, RejectReason};
    use crate::stores::EdgeKey;

    fn scan_of(time: u64, running: &[(&str, u32)]) -> Observation {
        let mut events = vec![ObservationEvent::ObsBegin { time, scan_id: 1 }];
        for (path, pid) in running {
            events.push(ObservationEvent::CandidateExe {
                exe: CandidateExe::new((*path).into(), *pid),
            });
            events.push(ObservationEvent::Map {
                segment: MapSegment::new(format!("{path}.lib").into(), 0, 4096, time),
                pid: *pid,
            });
        }
        // Un mapa de un proceso que no es candidato: se ignora.
        events.push(ObservationEvent::Map {
            segment: MapSegment::new("/huérfano".into(), 0, 1, time),
            pid: 9999,
        });
        events.push(ObservationEvent::ObsEnd {
            time,
            scan_id: 1,
            warnings: vec![],
        });
        events
    }

    fn open_policy() -> DefaultAdmissionPolicy {
        DefaultAdmissionPolicy::new(vec![], vec![], 100_000_000)
    }

    /// `early` corre solo en t=0 y `late` arranca en t=20. Con
    /// `late_has_lower_id`, `late` se crea antes en el almacén y tiene el id
    /// menor, así que la arista queda orientada al revés.
    fn early_then_late(late_has_lower_id: bool) -> (Stores, ExeId, ExeId) {
        let mut stores = Stores::default();
        let late_key = ExeKey::new("/usr/bin/late".into());
        let pre_created = late_has_lower_id.then(|| stores.ensure_exe(late_key.clone()));
        let mut updater = DefaultModelUpdater::new();

        let first = updater
            .apply(
                &mut stores,
                &scan_of(0, &[("/usr/bin/early", 1)]),
                &open_policy(),
            )
            .unwrap();
        assert_eq!(first.edges_added, 0);
        assert_eq!(stores.markov.len(), 0);

        let both = [("/usr/bin/early", 1), ("/usr/bin/late", 2)];
        let second = updater
            .apply(&mut stores, &scan_of(20, &both), &open_policy())
            .unwrap();
        // La arista existe desde el tick en que `late` arranca, y se observó una vez.
        assert_eq!(second.edges_added, 1);
        assert_eq!(second.edges_updated, 1);

        let early = stores
            .exes
            .id_by_key(&ExeKey::new("/usr/bin/early".into()))
            .unwrap();
        let late = pre_created.unwrap_or_else(|| stores.exes.id_by_key(&late_key).unwrap());
        assert_eq!(late_has_lower_id, late < early);
        (stores, early, late)
    }

    /// El estado «sólo `x` corre» según la orientación de la clave.
    fn only(key: EdgeKey, x: ExeId) -> MarkovState {
        if key.a() == x {
            MarkovState::OnlyA
        } else {
            MarkovState::OnlyB
        }
    }

    fn check_learns_that_late_starts_while_early_runs(late_has_lower_id: bool) {
        let (stores, early, late) = early_then_late(late_has_lower_id);
        let key = EdgeKey::new(early, late);
        let edge = stores.markov.get(key).expect("la arista existe");

        let only_early = only(key, early);
        let only_late = only(key, late);
        // La probabilidad es la frecuencia: una sola observación de «sólo early»
        // → «ambos» ya marca esa transición con 1, no con 0.1.
        assert!((edge.transition_probability(only_early, MarkovState::Both) - 1.0).abs() < 1e-6);
        assert_eq!(
            edge.transition_probability(only_late, MarkovState::Both),
            0.0
        );
        // El tiempo para dejar el estado usa los 20 s entre los dos escaneos.
        assert!((edge.time_to_leave[only_early as usize] - 2.0).abs() < 1e-6);
        assert_eq!(edge.both_running_time, 0);
    }

    #[test]
    fn la_arista_aprende_que_b_arranca_mientras_a_corre() {
        check_learns_that_late_starts_while_early_runs(false);
    }

    #[test]
    fn la_arista_aprende_lo_mismo_con_los_ids_en_el_orden_contrario() {
        check_learns_that_late_starts_while_early_runs(true);
    }

    #[test]
    fn el_tiempo_juntos_crece_mientras_los_dos_corren() {
        let (mut stores, early, late) = early_then_late(false);
        let mut updater = DefaultModelUpdater::new();
        let both = [("/usr/bin/early", 1), ("/usr/bin/late", 2)];

        for (time, expected) in [(40, 20), (60, 40)] {
            let delta = updater
                .apply(&mut stores, &scan_of(time, &both), &open_policy())
                .unwrap();
            assert_eq!((delta.edges_added, delta.edges_updated), (0, 1));
            let edge = stores.markov.get(EdgeKey::new(early, late)).unwrap();
            assert_eq!(edge.both_running_time, expected);
        }
        let edge = stores.markov.get(EdgeKey::new(early, late)).unwrap();
        assert!(edge.transition_probability(MarkovState::Both, MarkovState::Both) > 0.0);
    }

    #[test]
    fn cada_arista_se_observa_una_sola_vez_por_tick() {
        let mut stores = Stores::default();
        let mut updater = DefaultModelUpdater::new();
        // (tiempo, ejecutables que corren con su pid, aristas nuevas, observadas)
        type Step<'a> = (u64, &'a [(&'a str, u32)], u32, u32);
        let steps: [Step; 3] = [
            (0, &[("/a", 1), ("/b", 2)], 1, 1),
            // `c` arranca: dos aristas nuevas y la de (a, b) ya existente.
            (20, &[("/a", 1), ("/b", 2), ("/c", 3)], 2, 3),
            // Sólo `c` sigue; las tres aristas observan igual.
            (40, &[("/c", 3)], 0, 3),
        ];
        for (time, running, added, updated) in steps {
            let delta = updater
                .apply(&mut stores, &scan_of(time, running), &open_policy())
                .unwrap();
            assert_eq!((delta.edges_added, delta.edges_updated), (added, updated));
            assert_eq!(delta.edges_updated as usize, stores.markov.len());
        }
        assert_eq!(stores.markov.len(), 3);
    }

    #[test]
    fn el_ejecutable_que_deja_de_aparecer_queda_detenido() {
        let mut stores = Stores::default();
        let mut updater = DefaultModelUpdater::new();
        updater
            .apply(&mut stores, &scan_of(0, &[("/a", 1)]), &open_policy())
            .unwrap();
        updater
            .apply(&mut stores, &scan_of(20, &[]), &open_policy())
            .unwrap();

        let a = stores.exes.id_by_key(&ExeKey::new("/a".into())).unwrap();
        let exe = stores.exes.get(a).unwrap();
        assert!(!exe.running);
        assert_eq!(exe.last_seen_time, Some(20));
        assert!(stores.active.is_empty());
    }

    struct FixedDecision(AdmissionDecision);

    impl AdmissionPolicy for FixedDecision {
        fn allow_exe(&self, _path: &Path) -> bool {
            true
        }

        fn allow_map(&self, _path: &Path) -> bool {
            true
        }

        fn decide(&self, _candidate: &CandidateExe) -> AdmissionDecision {
            self.0.clone()
        }
    }

    #[test]
    fn un_candidato_rechazado_no_entra_al_modelo() {
        let mut stores = Stores::default();
        let policy = FixedDecision(AdmissionDecision::Reject {
            reason: RejectReason::ExeNotAllowed,
        });

        let delta = DefaultModelUpdater::new()
            .apply(&mut stores, &scan_of(0, &[("/a", 1)]), &policy)
            .unwrap();

        assert_eq!(delta.exes_added, 0);
        assert!(stores.exes.is_empty());
        assert!(stores.active.is_empty());
    }

    #[test]
    fn no_se_tienden_aristas_entre_ejecutables_sin_mapas() {
        let mut stores = Stores::default();
        let policy = FixedDecision(AdmissionDecision::Accept {
            completeness: Completeness::ExeOnly,
        });
        // Dos ejecutables admitidos pero sin mapas, corriendo juntos: no son
        // aplicaciones reales y no deben tender una arista (el grafo crecería
        // O(n²) con cada proceso del sistema).
        let obs = vec![
            ObservationEvent::ObsBegin {
                time: 0,
                scan_id: 1,
            },
            ObservationEvent::CandidateExe {
                exe: CandidateExe::new("/a".into(), 1),
            },
            ObservationEvent::CandidateExe {
                exe: CandidateExe::new("/b".into(), 2),
            },
            ObservationEvent::ObsEnd {
                time: 0,
                scan_id: 1,
                warnings: vec![],
            },
        ];

        let delta = DefaultModelUpdater::new()
            .apply(&mut stores, &obs, &policy)
            .unwrap();

        assert_eq!(delta.exes_added, 2);
        assert_eq!(delta.edges_added, 0);
        assert_eq!(stores.markov.len(), 0);
    }

    #[test]
    fn tras_cargar_el_primer_tick_no_inventa_transiciones_desde_el_activo_reconstruido() {
        let mut stores = Stores::default();
        let x = stores.ensure_exe(ExeKey::new("/x".into()));
        let y = stores.ensure_exe(ExeKey::new("/y".into()));
        stores.ensure_markov_edge(x, y, 0, MarkovState::Neither);
        // Estado tras `load`: activo reconstruido con x,y (vistos en la ventana,
        // no observados corriendo).
        stores.active.update(&[x, y], 100);
        stores.active_reconstructed = true;
        stores.model_time = 100;

        // Primer tick: no corre nada.
        let empty = vec![
            ObservationEvent::ObsBegin {
                time: 120,
                scan_id: 1,
            },
            ObservationEvent::ObsEnd {
                time: 120,
                scan_id: 1,
                warnings: vec![],
            },
        ];
        DefaultModelUpdater::new()
            .apply(&mut stores, &empty, &open_policy())
            .unwrap();

        let edge = stores.markov.get(EdgeKey::new(x, y)).unwrap();
        // Sin el arreglo se tomaba a x,y como «ambos corrían» y se observaba
        // Both -> Neither, una transición que nunca pasó.
        assert_eq!(
            edge.transition_probability(MarkovState::Both, MarkovState::Neither),
            0.0
        );
        assert!(!stores.active_reconstructed, "la bandera se consume");
    }

    #[test]
    fn segun_la_completitud_se_guardan_o_no_los_mapas() {
        for (completeness, maps) in [
            (Completeness::ExeOnly, 0),
            (Completeness::Partial, 1),
            (Completeness::Full, 1),
        ] {
            let mut stores = Stores::default();
            let policy = FixedDecision(AdmissionDecision::Accept { completeness });

            let delta = DefaultModelUpdater::new()
                .apply(&mut stores, &scan_of(0, &[("/a", 1)]), &policy)
                .unwrap();

            assert_eq!(delta.exes_added, 1);
            assert_eq!(delta.maps_added, maps);
            assert_eq!(stores.maps.len(), maps as usize);
        }
    }
}
