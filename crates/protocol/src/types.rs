//! Tipos del protocolo D-Bus.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use zvariant::{Error, OwnedValue, StructureBuilder, Type, Value};

/// Estado del daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, Value, OwnedValue)]
#[zvariant(signature = "u")]
#[repr(u32)]
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
///
/// Las conversiones a/desde `Value` se escriben a mano (ver abajo): el
/// `#[derive(Value)]` de zvariant entra en pánico al recibir una estructura con
/// menos campos de los esperados (un cli hablando con un daemon de otra
/// versión), en lugar de devolver un error.
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
    pub last_tick_prediction_max_prob: f64,
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
///
/// Conversiones a mano por el mismo motivo que `DaemonStats`.
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

/// Toma los campos de una estructura de D-Bus comprobando primero la cantidad.
/// Es lo que falta en el derive de zvariant: sin este chequeo, leer menos
/// campos de los esperados entra en pánico en vez de devolver un error.
fn struct_fields(value: Value<'_>, expected: usize) -> Result<Vec<Value<'_>>, Error> {
    let Value::Structure(s) = value else {
        return Err(Error::IncorrectType);
    };
    let fields = s.into_fields();
    if fields.len() != expected {
        return Err(Error::IncorrectType);
    }
    Ok(fields)
}

impl From<ConfigSnapshot> for Value<'static> {
    fn from(c: ConfigSnapshot) -> Self {
        Value::from((
            c.modelo_ciclo_secs,
            c.modelo_ventana_activa_secs,
            c.sistema_doscan,
            c.sistema_dopredict,
            c.sistema_doprefetch,
            c.persistencia_autosave_secs,
        ))
    }
}

impl TryFrom<ConfigSnapshot> for OwnedValue {
    type Error = Error;
    fn try_from(c: ConfigSnapshot) -> Result<Self, Self::Error> {
        OwnedValue::try_from(Value::from(c))
    }
}

impl TryFrom<Value<'_>> for ConfigSnapshot {
    type Error = Error;
    fn try_from(value: Value<'_>) -> Result<Self, Self::Error> {
        let mut it = struct_fields(value, 6)?.into_iter();
        let mut next = || it.next().expect("6 campos comprobados");
        Ok(Self {
            modelo_ciclo_secs: u64::try_from(next())?,
            modelo_ventana_activa_secs: u64::try_from(next())?,
            sistema_doscan: bool::try_from(next())?,
            sistema_dopredict: bool::try_from(next())?,
            sistema_doprefetch: bool::try_from(next())?,
            persistencia_autosave_secs: u64::try_from(next())?,
        })
    }
}

impl TryFrom<OwnedValue> for ConfigSnapshot {
    type Error = Error;
    fn try_from(value: OwnedValue) -> Result<Self, Self::Error> {
        Self::try_from(Value::from(value))
    }
}

impl From<DaemonStats> for Value<'static> {
    fn from(s: DaemonStats) -> Self {
        Value::from(
            StructureBuilder::new()
                .add_field(s.state)
                .add_field(s.scan_count)
                .add_field(s.exe_count)
                .add_field(s.map_count)
                .add_field(s.edge_count)
                .add_field(s.active_count)
                .add_field(s.model_time)
                .add_field(s.last_tick_scan_id)
                .add_field(s.last_tick_exes_added)
                .add_field(s.last_tick_maps_added)
                .add_field(s.last_tick_edges_added)
                .add_field(s.last_tick_edges_updated)
                .add_field(s.last_tick_prediction_exes)
                .add_field(s.last_tick_prediction_max_prob)
                .add_field(s.last_tick_prefetch_attempted)
                .add_field(s.last_tick_prefetch_succeeded)
                .add_field(s.last_tick_prefetch_failed)
                .add_field(s.last_tick_prefetch_bytes)
                .add_field(s.last_tick_mem_free_pct)
                .build()
                .expect("19 campos, ninguno vacío"),
        )
    }
}

impl TryFrom<DaemonStats> for OwnedValue {
    type Error = Error;
    fn try_from(s: DaemonStats) -> Result<Self, Self::Error> {
        OwnedValue::try_from(Value::from(s))
    }
}

impl TryFrom<Value<'_>> for DaemonStats {
    type Error = Error;
    fn try_from(value: Value<'_>) -> Result<Self, Self::Error> {
        let mut it = struct_fields(value, 19)?.into_iter();
        let mut next = || it.next().expect("19 campos comprobados");
        Ok(Self {
            state: DaemonState::try_from(next())?,
            scan_count: u64::try_from(next())?,
            exe_count: u32::try_from(next())?,
            map_count: u32::try_from(next())?,
            edge_count: u32::try_from(next())?,
            active_count: u32::try_from(next())?,
            model_time: u64::try_from(next())?,
            last_tick_scan_id: u64::try_from(next())?,
            last_tick_exes_added: u32::try_from(next())?,
            last_tick_maps_added: u32::try_from(next())?,
            last_tick_edges_added: u32::try_from(next())?,
            last_tick_edges_updated: u32::try_from(next())?,
            last_tick_prediction_exes: u32::try_from(next())?,
            last_tick_prediction_max_prob: f64::try_from(next())?,
            last_tick_prefetch_attempted: u32::try_from(next())?,
            last_tick_prefetch_succeeded: u32::try_from(next())?,
            last_tick_prefetch_failed: u32::try_from(next())?,
            last_tick_prefetch_bytes: u64::try_from(next())?,
            last_tick_mem_free_pct: u8::try_from(next())?,
        })
    }
}

impl TryFrom<OwnedValue> for DaemonStats {
    type Error = Error;
    fn try_from(value: OwnedValue) -> Result<Self, Self::Error> {
        Self::try_from(Value::from(value))
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
