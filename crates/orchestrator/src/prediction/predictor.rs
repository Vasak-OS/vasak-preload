//! Predictor basado en modelo de Markov.

#![forbid(unsafe_code)]

use crate::domain::{ExeId, MarkovEdge, MarkovState};
use crate::prediction::types::{Prediction, Predictor};
use crate::stores::Stores;
use std::collections::HashMap;

/// Predictor que usa el modelo de Markov para predecir próximos ejecutables.
#[derive(Debug)]
pub struct MarkovPredictor {
    /// Factor de decaimiento para predicciones antiguas.
    decay_factor: f32,
}

impl Default for MarkovPredictor {
    fn default() -> Self {
        // `derive(Default)` dejaba `decay_factor = 0.0`, un predictor que nunca
        // daba probabilidad por arista. El default es el mismo que `new`.
        Self::new()
    }
}

impl MarkovPredictor {
    /// Crea un nuevo predictor de Markov.
    pub fn new() -> Self {
        Self { decay_factor: 0.95 }
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

        let running = |id: ExeId| stores.exes.get(id).is_some_and(|e| e.running);

        // Probabilidad base: el ejecutable activo tiene alta probabilidad de seguir corriendo
        let base_prob: f32 = 0.8;
        for &source in &active_exes {
            prediction.set_probability(source, base_prob);
        }

        // Para cada arista, el extremo que no corre recibe la probabilidad de
        // pasar a un estado en el que corre. Las aristas se combinan como
        // eventos independientes: 1 - Π(1 - p).
        let mut not_starting: HashMap<ExeId, f32> = HashMap::new();
        for (key, edge) in stores.markov.iter() {
            let (a, b) = (key.a(), key.b());
            let state = MarkovState::Neither.transition(running(a), running(b));
            let both = edge.transition_probability(state, MarkovState::Both);
            for (target, alone) in [(a, MarkovState::OnlyA), (b, MarkovState::OnlyB)] {
                if running(target) {
                    continue;
                }
                let p = (edge.transition_probability(state, alone) + both) * self.decay_factor;
                *not_starting.entry(target).or_insert(1.0) *= 1.0 - p.clamp(0.0, 1.0);
            }
        }
        for (target, q) in not_starting {
            let p = 1.0 - q;
            if p > 0.01 {
                prediction.set_probability(target, p.max(prediction.get_probability(target)));
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
    use crate::domain::{ExeKey, MapSegment, MarkovState, MemStat};
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

    use crate::stores::EdgeKey;

    /// `running` corre y es activo; `idle` no. Su arista aprendió estas
    /// probabilidades de transición desde «sólo corre `running`». Con
    /// `idle_has_lower_id`, `idle` se crea primero y la clave queda al revés.
    fn running_next_to_idle(
        idle_has_lower_id: bool,
        to_both: f32,
        to_only_idle: f32,
    ) -> (Stores, ExeId, ExeId) {
        let mut stores = Stores::default();
        let (idle, running) = if idle_has_lower_id {
            let idle = stores.ensure_exe(ExeKey::new("/bin/idle".into()));
            (idle, stores.ensure_exe(ExeKey::new("/bin/running".into())))
        } else {
            let running = stores.ensure_exe(ExeKey::new("/bin/running".into()));
            (stores.ensure_exe(ExeKey::new("/bin/idle".into())), running)
        };
        stores.exes.get_mut(running).unwrap().mark_running(100);
        stores.active.update(&[running], 100);

        stores.ensure_markov_edge(running, idle, 100, MarkovState::Neither);
        let key = EdgeKey::new(running, idle);
        let (only_running, only_idle) = if key.a() == running {
            (MarkovState::OnlyA, MarkovState::OnlyB)
        } else {
            (MarkovState::OnlyB, MarkovState::OnlyA)
        };
        let edge = stores.markov.get_mut(key).unwrap();
        edge.transition_prob[only_running as usize][MarkovState::Both as usize] = to_both;
        edge.transition_prob[only_running as usize][only_idle as usize] = to_only_idle;
        (stores, running, idle)
    }

    fn approx(left: f32, right: f32) {
        assert!((left - right).abs() < 1e-5, "{left} != {right}");
    }

    #[test]
    fn el_que_no_corre_recibe_probabilidad_de_lo_que_la_arista_aprendio() {
        for idle_has_lower_id in [false, true] {
            // 0.05 de que arranque junto al que corre y 0.05 de que lo reemplace.
            let (stores, running, idle) = running_next_to_idle(idle_has_lower_id, 0.05, 0.05);

            let prediction = MarkovPredictor::new().predict(&stores);

            approx(prediction.get_probability(running), 0.8);
            approx(prediction.get_probability(idle), 0.1 * 0.95);
        }
    }

    #[test]
    fn el_predictor_por_defecto_tambien_predice_por_arista() {
        // `default()` tenía decay 0.0 y nunca daba probabilidad por arista.
        let (stores, _, idle) = running_next_to_idle(false, 0.1, 0.0);
        let p = MarkovPredictor::default()
            .predict(&stores)
            .get_probability(idle);
        approx(p, 0.1 * 0.95);
    }

    #[test]
    fn sin_nada_aprendido_el_que_no_corre_queda_sin_probabilidad() {
        let (stores, _, idle) = running_next_to_idle(false, 0.0, 0.0);
        assert_eq!(
            MarkovPredictor::new()
                .predict(&stores)
                .get_probability(idle),
            0.0
        );
    }

    #[test]
    fn lo_aprendido_por_debajo_del_umbral_se_descarta() {
        // 0.01 / 0.95 = 0.0105: justo por debajo de 0.01 tras el decaimiento.
        let (stores, _, idle) = running_next_to_idle(false, 0.0105, 0.0);
        assert_eq!(
            MarkovPredictor::new()
                .predict(&stores)
                .get_probability(idle),
            0.0
        );
    }

    #[test]
    fn el_decaimiento_escala_lo_aprendido_y_se_limita_a_uno() {
        let (stores, _, idle) = running_next_to_idle(false, 0.1, 0.0);

        let half = MarkovPredictor::with_decay(0.5).predict(&stores);
        approx(half.get_probability(idle), 0.05);
        let capped = MarkovPredictor::with_decay(7.0).predict(&stores);
        approx(capped.get_probability(idle), 0.1);
    }

    #[test]
    fn dos_aristas_hacia_el_mismo_ejecutable_se_combinan_como_eventos_independientes() {
        let mut stores = Stores::default();
        let running = stores.ensure_exe(ExeKey::new("/bin/running".into()));
        let target = stores.ensure_exe(ExeKey::new("/bin/target".into()));
        let other = stores.ensure_exe(ExeKey::new("/bin/other".into()));
        stores.exes.get_mut(running).unwrap().mark_running(100);
        stores.active.update(&[running], 100);

        // running -> target: aprendió 0.05 de arrancar juntos desde «sólo running».
        stores.ensure_markov_edge(running, target, 100, MarkovState::Neither);
        let key = EdgeKey::new(running, target);
        let only_running = if key.a() == running {
            MarkovState::OnlyA
        } else {
            MarkovState::OnlyB
        };
        stores.markov.get_mut(key).unwrap().transition_prob[only_running as usize]
            [MarkovState::Both as usize] = 0.05;
        // other <-> target: ninguno corre; aprendió 0.04 de arrancar los dos.
        stores.ensure_markov_edge(other, target, 100, MarkovState::Neither);
        stores
            .markov
            .get_mut(EdgeKey::new(other, target))
            .unwrap()
            .transition_prob[MarkovState::Neither as usize][MarkovState::Both as usize] = 0.04;

        let prediction = MarkovPredictor::new().predict(&stores);

        let (p1, p2) = (0.05 * 0.95_f32, 0.04 * 0.95_f32);
        approx(
            prediction.get_probability(target),
            1.0 - (1.0 - p1) * (1.0 - p2),
        );
        approx(prediction.get_probability(other), p2);
        approx(prediction.get_probability(running), 0.8);
    }

    #[test]
    fn el_que_ya_corre_no_recibe_probabilidad_por_arista() {
        let mut stores = Stores::default();
        let active = stores.ensure_exe(ExeKey::new("/bin/active".into()));
        let also_running = stores.ensure_exe(ExeKey::new("/bin/also".into()));
        stores.exes.get_mut(active).unwrap().mark_running(100);
        stores.exes.get_mut(also_running).unwrap().mark_running(100);
        stores.active.update(&[active], 100);
        stores.ensure_markov_edge(active, also_running, 100, MarkovState::Neither);
        stores
            .markov
            .get_mut(EdgeKey::new(active, also_running))
            .unwrap()
            .transition_prob[MarkovState::Both as usize][MarkovState::Both as usize] = 0.9;

        let prediction = MarkovPredictor::new().predict(&stores);

        assert_eq!(prediction.get_probability(also_running), 0.0);
    }

    #[test]
    fn las_probabilidades_se_normalizan_para_sumar_a_lo_sumo_uno() {
        let mut stores = Stores::default();
        let target = stores.ensure_exe(ExeKey::new("/bin/target".into()));
        let mut runners = Vec::new();
        for path in ["/bin/one", "/bin/two"] {
            let id = stores.ensure_exe(ExeKey::new(path.into()));
            stores.exes.get_mut(id).unwrap().mark_running(100);
            stores.ensure_markov_edge(id, target, 100, MarkovState::Neither);
            let key = EdgeKey::new(id, target);
            let only_runner = if key.a() == id {
                MarkovState::OnlyA
            } else {
                MarkovState::OnlyB
            };
            stores.markov.get_mut(key).unwrap().transition_prob[only_runner as usize]
                [MarkovState::Both as usize] = 0.6;
            runners.push(id);
        }
        stores.active.update(&runners, 100);

        let prediction = MarkovPredictor::new().predict(&stores);

        let sum: f32 = prediction.probabilities.values().sum();
        approx(sum, 1.0);
        // Antes de normalizar: corredores 0.8 y 0.8, objetivo 1 - 0.43^2.
        let raw_target = 1.0 - (1.0 - 0.6 * 0.95_f32).powi(2);
        approx(
            prediction.get_probability(target) / prediction.get_probability(runners[0]),
            raw_target / 0.8,
        );
    }
}
