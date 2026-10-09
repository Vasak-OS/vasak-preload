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
}

impl Default for MarkovEdge {
    fn default() -> Self {
        Self {
            time_to_leave: [0.0; 4],
            transition_prob: [[0.0; 4]; 4],
            both_running_time: 0,
        }
    }
}

impl MarkovEdge {
    /// Actualiza la arista con una observación de transición.
    pub fn observe_transition(&mut self, from: MarkovState, to: MarkovState, dt: f32) {
        let from_idx = from as usize;
        let to_idx = to as usize;

        // Actualizar tiempo para dejar el estado
        self.time_to_leave[from_idx] =
            (self.time_to_leave[from_idx] * 0.9) + (dt * 0.1);

        // Actualizar probabilidad de transición
        for i in 0..4 {
            let factor = if i == to_idx { 1.0 } else { 0.0 };
            self.transition_prob[from_idx][i] =
                (self.transition_prob[from_idx][i] * 0.9) + (factor * 0.1);
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