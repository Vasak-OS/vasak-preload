//! Eventos de observación.

#![forbid(unsafe_code)]

use crate::domain::{MapSegment, MemStat};

/// Evento individual de observación.
#[derive(Debug, Clone)]
pub enum ObservationEvent {
    /// Inicio de un ciclo de observación.
    ObsBegin { time: u64, scan_id: u64 },
    /// Fin de un ciclo de observación.
    ObsEnd {
        time: u64,
        scan_id: u64,
        warnings: Vec<ScanWarning>,
    },
    /// Estadísticas de memoria.
    MemStat { mem: MemStat },
    /// Un ejecutable candidato detectado.
    CandidateExe { exe: super::CandidateExe },
    /// Un mapeo de memoria detectado.
    Map { segment: MapSegment, pid: u32 },
}

/// Advertencias durante el escaneo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanWarning {
    /// No se pudo leer /proc/<pid>/stat.
    StatReadFailed(u32),
    /// No se pudo leer /proc/<pid>/maps.
    MapsReadFailed(u32),
    /// No se pudo leer /proc/<pid>/cmdline.
    CmdlineReadFailed(u32),
    /// Proceso desaparecido entre listado y lectura.
    ProcessGone(u32),
    /// No se pudo leer el proceso desde el iterador.
    ProcessReadFailed,
    /// Mapeo demasiado grande, ignorado.
    MapTooLarge(std::path::PathBuf, u64),
    /// Mapeo rechazado por política.
    MapRejected(std::path::PathBuf),
}

/// Stream de eventos de observación.
pub type Observation = Vec<ObservationEvent>;
