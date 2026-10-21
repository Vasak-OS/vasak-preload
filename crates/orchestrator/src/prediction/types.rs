//! Tipos de predicción.

#![forbid(unsafe_code)]

use crate::domain::ExeId;
use crate::stores::Stores;
use std::collections::HashMap;

/// Trait para predictores.
pub trait Predictor: Send + Sync {
    fn predict(&self, stores: &Stores) -> Prediction;
}

/// Predicción de probabilidad para cada ejecutable.
#[derive(Debug, Clone, Default)]
pub struct Prediction {
    /// Probabilidad de que cada ejecutable se inicie (0.0 - 1.0).
    pub probabilities: HashMap<ExeId, f32>,
    /// Tiempo de predicción (segundos desde epoch del modelo).
    pub time: u64,
}

impl Prediction {
    /// Crea una predicción vacía.
    pub fn new(time: u64) -> Self {
        Self {
            probabilities: HashMap::new(),
            time,
        }
    }

    /// Establece la probabilidad para un ejecutable.
    pub fn set_probability(&mut self, exe_id: ExeId, prob: f32) {
        if prob > 0.0 {
            self.probabilities.insert(exe_id, prob.clamp(0.0, 1.0));
        } else {
            self.probabilities.remove(&exe_id);
        }
    }

    /// Obtiene la probabilidad para un ejecutable.
    pub fn get_probability(&self, exe_id: ExeId) -> f32 {
        self.probabilities.get(&exe_id).copied().unwrap_or(0.0)
    }

    /// Obtiene los top N ejecutables por probabilidad.
    pub fn top_n(&self, n: usize) -> Vec<(ExeId, f32)> {
        let mut items: Vec<_> = self.probabilities.iter().map(|(k, v)| (*k, *v)).collect();
        items.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        items.into_iter().take(n).collect()
    }

    /// Resumen de la predicción.
    pub fn summarize(&self) -> PredictionSummary {
        PredictionSummary {
            total_exes: self.probabilities.len(),
            max_probability: self.probabilities.values().copied().fold(0.0, f32::max),
            time: self.time,
        }
    }
}

/// Resumen de una predicción.
#[derive(Debug, Clone, Copy)]
pub struct PredictionSummary {
    pub total_exes: usize,
    pub max_probability: f32,
    pub time: u64,
}

impl Default for PredictionSummary {
    fn default() -> Self {
        Self {
            total_exes: 0,
            max_probability: 0.0,
            time: 0,
        }
    }
}
