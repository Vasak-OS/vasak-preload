//! Configuración del sistema y escaneo.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DurationSeconds};
use std::path::PathBuf;
use std::time::Duration;

/// Configuración del sistema y escaneo de procesos.
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Sistema {
    /// Si se debe escanear procesos activos.
    #[serde(default = "verdadero")]
    pub doscan: bool,

    /// Si se debe ejecutar predicción.
    #[serde(default = "verdadero")]
    pub dopredict: bool,

    /// Si se debe ejecutar prefetch.
    #[serde(default = "verdadero")]
    pub doprefetch: bool,

    /// Intervalo de autoguardado (en segundos). 0 = deshabilitado.
    #[serde_as(as = "DurationSeconds<u64>")]
    #[serde(default = "autosave_defecto")]
    pub autosave: Duration,

    /// Prefijos de ejecutables a monitorear (vacío = todos).
    #[serde(default)]
    pub exeprefix: Vec<PathBuf>,

    /// Prefijos de mapeos de memoria a monitorear (vacío = todos).
    #[serde(default)]
    pub mapprefix: Vec<PathBuf>,

    /// Tamaño máximo de mapa a considerar para prefetch (en bytes).
    #[serde(default = "max_mapa_defecto")]
    pub max_mapa: u64,

    /// Umbral de memoria libre mínima para hacer prefetch (porcentaje 0-100).
    #[serde(default = "mem_libre_defecto")]
    pub mem_libre_min_pct: u8,

    /// Tamaño de página del sistema (para cálculos).
    #[serde(default = "tamano_pagina_defecto")]
    pub tamano_pagina: u64,
}

fn verdadero() -> bool {
    true
}

fn autosave_defecto() -> Duration {
    Duration::from_secs(300) // 5 minutos
}

fn max_mapa_defecto() -> u64 {
    100 * 1024 * 1024 // 100 MiB
}

fn mem_libre_defecto() -> u8 {
    10 // 10%
}

fn tamano_pagina_defecto() -> u64 {
    4096
}

impl Default for Sistema {
    fn default() -> Self {
        Self {
            doscan: verdadero(),
            dopredict: verdadero(),
            doprefetch: verdadero(),
            autosave: autosave_defecto(),
            exeprefix: Vec::new(),
            mapprefix: Vec::new(),
            max_mapa: max_mapa_defecto(),
            mem_libre_min_pct: mem_libre_defecto(),
            tamano_pagina: tamano_pagina_defecto(),
        }
    }
}