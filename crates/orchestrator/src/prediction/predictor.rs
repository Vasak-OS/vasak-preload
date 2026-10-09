//! Predictor basado en modelo de Markov.

#![forbid(unsafe_code)]

use crate::domain::{ExeId, MarkovEdge, MarkovState};
use crate::prediction::types::{Prediction, Predictor};
use crate::stores::Stores;

/// Predictor que usa el modelo de Markov para predecir próximos ejecutables.
#[derive(Debug, Default)]
pub struct MarkovPredictor {
    /// Factor de decaimiento para predicciones antiguas.
    decay_factor: f32,
}

impl MarkovPredictor {
    /// Crea un nuevo predictor de Markov.
    pub fn new() -> Self {
        Self {
            decay_factor: 0.95,
        }
    }

    /// Crea un predictor con factor de decaimiento personalizado.
    pub fn with_decay(decay: f32) -> Self {
        Self {
            decay_factor: decay.clamp(0.0, 1.0),
        }
    }
}

impl Predictor for MarkovPredictor {
    fn predict(&self, stores: &Stores) -> Prediction {
        let mut prediction = Prediction::new(stores.model_time);
        let active_exes: Vec<ExeId> = stores.active.exes().into_iter().collect();

        if active_exes.is_empty() {
            return prediction;
        }

        // Para cada ejecutable activo, propagar probabilidad a sus vecinos
        for &source in &active_exes {
            // Probabilidad base: el ejecutable activo tiene alta probabilidad de seguir corriendo
            let base_prob: f32 = 0.8;
            prediction.set_probability(source, base_prob.max(prediction.get_probability(source)));

            // Propagar a través de aristas de Markov
            for &target in &active_exes {
                if source == target {
                    continue;
                }

                if let Some(edge_key) = crate::stores::EdgeKey::new_opt(source, target) {
                    if let Some(edge) = stores.markov.get(edge_key) {
                        // Probabilidad de transición desde el estado actual
                        let current_state = if stores.exes.get(source).map(|e| e.running).unwrap_or(false) {
                            if stores.exes.get(target).map(|e| e.running).unwrap_or(false) {
                                MarkovState::Both
                            } else {
                                MarkovState::OnlyA
                            }
                        } else if stores.exes.get(target).map(|e| e.running).unwrap_or(false) {
                            MarkovState::OnlyB
                        } else {
                            MarkovState::Neither
                        };

                        let trans_prob = edge.transition_probability(current_state, MarkovState::OnlyB);
                        let propagated = base_prob * trans_prob * self.decay_factor;

                        if propagated > 0.01 {
                            let current = prediction.get_probability(target);
                            prediction.set_probability(target, current.max(propagated));
                        }
                    }
                }
            }
        }

        // Normalizar probabilidades para que sumen <= 1.0
        let sum: f32 = prediction.probabilities.values().sum();
        if sum > 1.0 {
            for prob in prediction.probabilities.values_mut() {
                *prob /= sum;
            }
        }

        prediction
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ExeKey, MapSegment, MemStat, MarkovState};
    use crate::observation::{DefaultAdmissionPolicy, DefaultModelUpdater, ObservationEvent};
    use crate::stores::Stores;

    #[test]
    fn predictor_devuelve_vacio_sin_activos() {
        let stores = Stores::default();
        let predictor = MarkovPredictor::new();
        let pred = predictor.predict(&stores);
        assert_eq!(pred.probabilities.len(), 0);
    }

    #[test]
    fn predictor_asigna_probabilidad_a_activos() {
        let mut stores = Stores::default();
        let exe_id = stores.ensure_exe(ExeKey::new("/bin/test".into()));
        if let Some(exe) = stores.exes.get_mut(exe_id) {
            exe.mark_running(100);
        }
        stores.active.update(&[exe_id], 100);

        let predictor = MarkovPredictor::new();
        let pred = predictor.predict(&stores);
        assert!(pred.get_probability(exe_id) > 0.0);
    }
}