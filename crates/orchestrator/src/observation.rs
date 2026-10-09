//! Observación del sistema: escaneo de procesos y actualización del modelo.

#![forbid(unsafe_code)]

mod admission;
mod event;
mod model_updater;
mod procfs_scanner;

pub use admission::{
    AdmissionDecision, AdmissionPolicy, AdmissionPolicyStats, Completeness, DefaultAdmissionPolicy,
    RejectReason,
};
pub use event::{Observation, ObservationEvent, ScanWarning};
pub use model_updater::{DefaultModelUpdater, ModelDelta, ModelUpdater};
pub use procfs_scanner::ProcfsScanner;

use crate::error::Error;

/// Escáner que observa el sistema.
pub trait Scanner: Send + Sync {
    /// Escanea el sistema y devuelve un stream ordenado de eventos de observación.
    fn scan(&mut self, time: u64, scan_id: u64) -> Result<Observation, Error>;
}

/// Candidato a ejecutable detectado durante el escaneo.
#[derive(Debug, Clone)]
pub struct CandidateExe {
    pub path: std::path::PathBuf,
    pub pid: u32,
    pub maps: Vec<crate::domain::MapSegment>,
    pub total_size: u64,
    pub rejected_maps: Vec<std::path::PathBuf>,
}

impl CandidateExe {
    /// Crea un nuevo candidato.
    pub fn new(path: std::path::PathBuf, pid: u32) -> Self {
        Self {
            path,
            pid,
            maps: Vec::new(),
            total_size: 0,
            rejected_maps: Vec::new(),
        }
    }
}