//! Plan y reporte de prefetch.

#![forbid(unsafe_code)]

use crate::domain::{MapId, MapKey};
use serde::{Deserialize, Serialize};

/// Plan de prefetch: qué mapas traer a memoria.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefetchPlan {
    /// Mapas a prefetch, ordenados por prioridad.
    pub maps: Vec<PrefetchMap>,
    /// Total de bytes a prefetch.
    pub total_bytes: u64,
    /// Presupuesto máximo de bytes.
    pub budget_bytes: u64,
}

/// Un mapa en el plan de prefetch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefetchMap {
    pub map_id: MapId,
    pub map_key: MapKey,
    pub priority: f32,
    pub size: u64,
}

/// Reporte de ejecución de prefetch.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PrefetchReport {
    /// Mapas intentados.
    pub attempted: u32,
    /// Mapas exitosos.
    pub succeeded: u32,
    /// Mapas fallidos.
    pub failed: u32,
    /// Bytes efectivamente prefetched.
    pub bytes_prefetched: u64,
    /// Errores encontrados.
    pub errors: Vec<String>,
}
