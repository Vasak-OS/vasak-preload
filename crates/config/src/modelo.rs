//! Configuración del modelo de Markov.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DurationSeconds};
use std::time::Duration;

/// Configuración del modelo de predicción.
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Modelo {
    /// Intervalo entre ciclos del modelo (en segundos).
    #[serde_as(as = "DurationSeconds<u64>")]
    #[serde(default = "ciclo_defecto")]
    pub ciclo: Duration,

    /// Ventana de tiempo para considerar una aplicación "activa" (en segundos).
    #[serde_as(as = "DurationSeconds<u64>")]
    #[serde(default = "ventana_activa_defecto")]
    pub ventana_activa: Duration,

    /// Tamaño máximo del modelo (número de estados).
    #[serde(default = "max_estados_defecto")]
    pub max_estados: usize,

    /// Umbral de probabilidad mínima para considerar una transición.
    #[serde(default = "umbral_transicion_defecto")]
    pub umbral_transicion: f32,
}

fn ciclo_defecto() -> Duration {
    Duration::from_secs(20)
}

fn ventana_activa_defecto() -> Duration {
    Duration::from_secs(3600) // 1 hora
}

fn max_estados_defecto() -> usize {
    2000
}

fn umbral_transicion_defecto() -> f32 {
    0.01
}

impl Default for Modelo {
    fn default() -> Self {
        Self {
            ciclo: ciclo_defecto(),
            ventana_activa: ventana_activa_defecto(),
            max_estados: max_estados_defecto(),
            umbral_transicion: umbral_transicion_defecto(),
        }
    }
}