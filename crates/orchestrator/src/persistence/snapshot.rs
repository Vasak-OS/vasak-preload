//! Instantánea del estado para persistencia.

use crate::domain::{ExeKey, MapKey, MapSegment, MarkovEdge, MarkovState};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Versión del esquema de instantánea.
pub const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// Metadatos de la instantánea.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMeta {
    pub schema_version: u32,
    pub app_version: Option<String>,
    /// Timestamp de creación en segundos desde epoch Unix.
    pub created_at: Option<u64>,
}

/// Instantánea del estado del modelo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub model_time: u64,
    pub last_accounting_time: u64,
    pub exes: Vec<ExeRecord>,
    pub maps: Vec<MapRecord>,
    pub exe_maps: Vec<ExeMapRecord>,
    pub markov_edges: Vec<MarkovRecord>,
}

/// Instantánea completa de los almacenes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoresSnapshot {
    pub meta: SnapshotMeta,
    pub state: StateSnapshot,
}

/// Registro de ejecutable para persistencia.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExeRecord {
    pub path: String,
    pub total_running_time: u64,
    pub last_seen_time: Option<u64>,
}

/// Registro de mapeo para persistencia.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapRecord {
    pub path: String,
    pub offset: u64,
    pub length: u64,
    pub update_time: u64,
}

/// Registro de relación ejecutable-mapa para persistencia.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExeMapRecord {
    pub exe_path: String,
    pub map_path: String,
    pub map_offset: u64,
    pub map_length: u64,
    pub prob: f32,
}

/// Registro de arista de Markov para persistencia.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkovRecord {
    pub exe_a: String,
    pub exe_b: String,
    pub time_to_leave: [f32; 4],
    pub transition_prob: [[f32; 4]; 4],
    pub both_running_time: u64,
    /// Observaciones por estado de partida. `default` para leer snapshots
    /// viejos sin el campo (se toman como sin observar).
    #[serde(default)]
    pub observations: [u32; 4],
}
