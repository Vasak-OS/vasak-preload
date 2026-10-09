//! Actualizador del modelo a partir de observaciones.

#![forbid(unsafe_code)]

use crate::domain::{ExeId, ExeKey, MapId, MapKey, MapSegment, MarkovEdge, MarkovState};
use crate::error::Error;
use crate::observation::{AdmissionDecision, AdmissionPolicy, CandidateExe, Completeness, Observation, ObservationEvent, ScanWarning};
use crate::stores::Stores;
use std::collections::HashMap;
use std::path::Path;

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
        let mut memstat = None;

        // Procesar eventos
        for event in observation {
            match event {
                ObservationEvent::ObsBegin { time, .. } => {
                    current_time = *time;
                }
                ObservationEvent::MemStat { mem } => {
                    memstat = Some(*mem);
                }
                ObservationEvent::CandidateExe { exe } => {
                    candidates.insert(exe.pid, exe.clone());
                }
                ObservationEvent::Map { segment, pid } => {
                    if let Some(candidate) = candidates.get_mut(&pid) {
                        candidate.maps.push(segment.clone());
                        candidate.total_size += segment.length;
                    }
                }
                ObservationEvent::ObsEnd { .. } => {}
            }
        }

        // Determinar qué ejecutables están corriendo ahora
        let mut running_exes: HashMap<ExeId, bool> = HashMap::new();

        for (pid, candidate) in candidates {
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
            if !running_exes.contains_key(&exe_id) {
                if let Some(exe_mut) = stores.exes.get_mut(exe_id) {
                    if exe_mut.running {
                        exe_mut.mark_stopped(current_time);
                    }
                }
            }
        }

        // Actualizar modelo de Markov
        let active_exes: Vec<ExeId> = stores.active.exes().into_iter().collect();
        for &a in &active_exes {
            for &b in &active_exes {
                if a == b {
                    continue;
                }
                let a_running = running_exes.get(&a).copied().unwrap_or(false);
                let b_running = running_exes.get(&b).copied().unwrap_or(false);

                let state = MarkovState::Neither.transition(a_running, b_running);
                let was_new = stores.ensure_markov_edge(a, b, current_time, state);

                if let Some(edge_key) = crate::stores::EdgeKey::new_opt(a, b) {
                    if let Some(edge) = stores.markov.get_mut(edge_key) {
                        if was_new {
                            delta.edges_added += 1;
                        } else {
                            delta.edges_updated += 1;
                        }
                    }
                }
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

    #[test]
    fn actualiza_modelo_basico() {
        let mut stores = Stores::default();
        let mut updater = DefaultModelUpdater::new();
        let policy = DefaultAdmissionPolicy::new(vec![], vec![], 100_000_000);

        let obs = vec![
            ObservationEvent::ObsBegin { time: 100, scan_id: 1 },
            ObservationEvent::MemStat {
                mem: MemStat {
                    total: 1000,
                    free: 500,
                    cached: 200,
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
        assert!(stores.exes.iter().any(|(_, e)| e.key.path() == Path::new("/usr/bin/firefox")));
    }
}