//! Modelo de Markov para predicción de transiciones entre aplicaciones.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// Estado de un par de ejecutables en el modelo de Markov.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkovState {
    /// Ninguno de los dos se está ejecutando.
    Neither,
    /// Solo el primero se está ejecutando.
    OnlyA,
    /// Solo el segundo se está ejecutando.
    OnlyB,
    /// Ambos se están ejecutando simultáneamente.
    Both,
}

impl MarkovState {
    /// Transiciona el estado dado qué ejecutables están corriendo ahora.
    pub fn transition(self, a_running: bool, b_running: bool) -> Self {
        match (a_running, b_running) {
            (false, false) => MarkovState::Neither,
            (true, false) => MarkovState::OnlyA,
            (false, true) => MarkovState::OnlyB,
            (true, true) => MarkovState::Both,
        }
    }
}

/// Arista del grafo de Markov entre dos ejecutables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkovEdge {
    /// Tiempo promedio para dejar el estado actual (en segundos).
    pub time_to_leave: [f32; 4],
    /// Probabilidad de transición entre estados.
    pub transition_prob: [[f32; 4]; 4],
    /// Tiempo total en que ambos se ejecutaron simultáneamente (en segundos).
    pub both_running_time: u64,
    /// Cuántas transiciones se observaron desde cada estado. La probabilidad es
    /// la frecuencia incremental, así que tras una sola observación la fila ya
    /// vale ~1 en vez de acercarse de a 0.1.
    #[serde(default)]
    pub observations: [u32; 4],
    /// Tiempo acumulado en el estado actual sin registrar todavía como «tiempo
    /// para dejarlo». Es de ejecución; no se persiste.
    #[serde(skip)]
    pub dwell: [f32; 4],
}

impl Default for MarkovEdge {
    fn default() -> Self {
        Self {
            time_to_leave: [0.0; 4],
            transition_prob: [[0.0; 4]; 4],
            both_running_time: 0,
            observations: [0; 4],
            dwell: [0.0; 4],
        }
    }
}

impl MarkovEdge {
    /// Actualiza la arista con una observación de transición.
    pub fn observe_transition(&mut self, from: MarkovState, to: MarkovState, dt: f32) {
        let from_idx = from as usize;
        let to_idx = to as usize;

        // Probabilidad de transición como frecuencia incremental: la fila
        // converge a la fracción real de veces que se fue a cada estado y ya
        // tras la primera observación marca el destino con ~1, en vez de
        // acercarse de a 0.1 y subestimar las predicciones al principio.
        let n = self.observations[from_idx] as f32;
        for i in 0..4 {
            let target = if i == to_idx { 1.0 } else { 0.0 };
            self.transition_prob[from_idx][i] =
                ((self.transition_prob[from_idx][i] * n) + target) / (n + 1.0);
        }
        self.observations[from_idx] = self.observations[from_idx].saturating_add(1);

        // El tiempo para dejar el estado se acumula mientras el par sigue en él
        // y se registra sólo al cambiar: quedarse en el mismo estado no es
        // dejarlo, así que no cuenta como una salida.
        if from == to {
            self.dwell[from_idx] += dt;
        } else {
            let dwell = self.dwell[from_idx] + dt;
            self.time_to_leave[from_idx] = (self.time_to_leave[from_idx] * 0.9) + (dwell * 0.1);
            self.dwell[from_idx] = 0.0;
            self.dwell[to_idx] = 0.0;
        }

        // Si ambos corren, acumular tiempo
        if matches!((from, to), (MarkovState::Both, MarkovState::Both)) {
            self.both_running_time = self.both_running_time.saturating_add(dt as u64);
        }
    }

    /// Obtiene la probabilidad de transición de un estado a otro.
    pub fn transition_probability(&self, from: MarkovState, to: MarkovState) -> f32 {
        self.transition_prob[from as usize][to as usize]
    }

    /// Obtiene el tiempo esperado para dejar un estado.
    pub fn expected_time_to_leave(&self, state: MarkovState) -> f32 {
        self.time_to_leave[state as usize].max(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_probabilidad_es_la_frecuencia_desde_la_primera_observacion() {
        let mut edge = MarkovEdge::default();
        edge.observe_transition(MarkovState::OnlyA, MarkovState::Both, 10.0);
        // Una sola observación ya marca el destino con 1, no con 0.1.
        assert_eq!(
            edge.transition_probability(MarkovState::OnlyA, MarkovState::Both),
            1.0
        );

        // Dos veces a Both y una a OnlyB desde OnlyA: 2/3 y 1/3.
        edge.observe_transition(MarkovState::OnlyA, MarkovState::Both, 10.0);
        edge.observe_transition(MarkovState::OnlyA, MarkovState::OnlyB, 10.0);
        let both = edge.transition_probability(MarkovState::OnlyA, MarkovState::Both);
        let only_b = edge.transition_probability(MarkovState::OnlyA, MarkovState::OnlyB);
        assert!((both - 2.0 / 3.0).abs() < 1e-6, "{both}");
        assert!((only_b - 1.0 / 3.0).abs() < 1e-6, "{only_b}");
    }

    #[test]
    fn el_tiempo_para_dejar_un_estado_cuenta_solo_al_cambiar() {
        let mut edge = MarkovEdge::default();
        let both = MarkovState::Both as usize;

        // Quedarse en el estado no es dejarlo: no toca time_to_leave.
        edge.observe_transition(MarkovState::Both, MarkovState::Both, 10.0);
        assert_eq!(edge.time_to_leave[both], 0.0);
        edge.observe_transition(MarkovState::Both, MarkovState::Both, 10.0);
        assert_eq!(edge.time_to_leave[both], 0.0);

        // Al cambiar registra todo el tiempo que estuvo: 10 + 10 + 10 = 30.
        edge.observe_transition(MarkovState::Both, MarkovState::OnlyA, 10.0);
        assert!((edge.time_to_leave[both] - 3.0).abs() < 1e-6); // 0*0.9 + 30*0.1
    }
}
