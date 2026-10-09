//! Tipos del protocolo D-Bus.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use zvariant::Type;

/// Estado del daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[zvariant(signature = "u")]
pub enum DaemonState {
    /// Iniciando.
    Starting = 0,
    /// Ejecutándose normalmente.
    Running = 1,
    /// Pausado (no hace prefetch).
    Paused = 2,
    /// Deteniéndose.
    Stopping = 3,
}

impl Default for DaemonState {
    fn default() -> Self {
        DaemonState::Starting
    }
}

/// Estadísticas del daemon.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct DaemonStats {
    /// Estado actual.
    pub state: DaemonState,
    /// Número de escaneos completados.
    pub scan_count: u64,
    /// Ejecutables rastreados.
    pub exe_count: u32,
    /// Mapeos rastreados.
    pub map_count: u32,
    /// Aristas de Markov.
    pub edge_count: u32,
    /// Ejecutables activos.
    pub active_count: u32,
    /// Tiempo de modelo actual (segundos).
    pub model_time: u64,
    /// Último reporte de tick (usar 0 para indicar ausencia).
    pub last_tick_scan_id: u64,
    pub last_tick_exes_added: u32,
    pub last_tick_maps_added: u32,
    pub last_tick_edges_added: u32,
    pub last_tick_edges_updated: u32,
    pub last_tick_prediction_exes: u32,
    pub last_tick_prediction_max_prob: f32,
    pub last_tick_prefetch_attempted: u32,
    pub last_tick_prefetch_succeeded: u32,
    pub last_tick_prefetch_failed: u32,
    pub last_tick_prefetch_bytes: u64,
    pub last_tick_mem_free_pct: u8, // 255 = no data
}

impl Default for DaemonStats {
    fn default() -> Self {
        Self {
            state: DaemonState::default(),
            scan_count: 0,
            exe_count: 0,
            map_count: 0,
            edge_count: 0,
            active_count: 0,
            model_time: 0,
            last_tick_scan_id: 0,
            last_tick_exes_added: 0,
            last_tick_maps_added: 0,
            last_tick_edges_added: 0,
            last_tick_edges_updated: 0,
            last_tick_prediction_exes: 0,
            last_tick_prediction_max_prob: 0.0,
            last_tick_prefetch_attempted: 0,
            last_tick_prefetch_succeeded: 0,
            last_tick_prefetch_failed: 0,
            last_tick_prefetch_bytes: 0,
            last_tick_mem_free_pct: 255,
        }
    }
}

/// Configuración expuesta via D-Bus.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ConfigSnapshot {
    pub modelo_ciclo_secs: u64,
    pub modelo_ventana_activa_secs: u64,
    pub sistema_doscan: bool,
    pub sistema_dopredict: bool,
    pub sistema_doprefetch: bool,
    pub persistencia_autosave_secs: u64, // 0 = deshabilitado
}

impl Default for ConfigSnapshot {
    fn default() -> Self {
        Self {
            modelo_ciclo_secs: 20,
            modelo_ventana_activa_secs: 3600,
            sistema_doscan: true,
            sistema_dopredict: true,
            sistema_doprefetch: true,
            persistencia_autosave_secs: 300,
        }
    }
}

/// Estado de un ejecutable individual.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Default)]
pub struct ExeState {
    pub path: String,
    pub running: bool,
    pub total_running_time: u64,
    pub last_seen_time: u64, // 0 = never
    pub prediction_probability: f32,
    pub map_count: u32,
}

/// Predicción: par de (ruta_ejecutable, probabilidad).
pub type PredictionEntry = (String, f32);

/// Wrapper para Option<ExeState> que implementa Type para D-Bus.
/// Usa un struct con has_value y un ExeState opcional (usando Default para el caso None).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ExeStateWrapper {
    pub has_value: bool,
    pub inner: ExeState,
}

impl ExeStateWrapper {
    pub fn some(inner: ExeState) -> Self {
        Self {
            has_value: true,
            inner,
        }
    }

    pub fn none() -> Self {
        Self {
            has_value: false,
            inner: ExeState::default(),
        }
    }
}