//! Política de admisión: qué ejecutables y mapeos rastrear.

#![forbid(unsafe_code)]

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Decisión de admisión para un ejecutable candidato.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionDecision {
    /// Aceptar con completitud dada.
    Accept { completeness: Completeness },
    /// Rechazar con razón.
    Reject { reason: RejectReason },
}

/// Completitud de la información del ejecutable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completeness {
    /// Solo el ejecutable, sin mapeos.
    ExeOnly,
    /// Ejecutable con algunos mapeos.
    Partial,
    /// Ejecutable con todos sus mapeos.
    Full,
}

/// Razón de rechazo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// Ejecutable en lista de denegados.
    ExeDenied,
    /// Mapeo en lista de denegados.
    MapDenied,
    /// Ejecutable no en lista de permitidos (si está configurada).
    ExeNotAllowed,
    /// Mapeo demasiado grande.
    MapTooLarge,
    /// Memoria insuficiente.
    LowMemory,
}

/// Estadísticas de la política de admisión.
#[derive(Debug, Default)]
pub struct AdmissionPolicyStats {
    exe_accepted: AtomicU64,
    exe_rejected: AtomicU64,
    map_accepted: AtomicU64,
    map_rejected: AtomicU64,
}

impl AdmissionPolicyStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_exe_accepted(&self) {
        self.exe_accepted.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_exe_rejected(&self) {
        self.exe_rejected.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_map_accepted(&self) {
        self.map_accepted.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_map_rejected(&self) {
        self.map_rejected.fetch_add(1, Ordering::Relaxed);
    }

    pub fn exe_accepted(&self) -> u64 {
        self.exe_accepted.load(Ordering::Relaxed)
    }

    pub fn exe_rejected(&self) -> u64 {
        self.exe_rejected.load(Ordering::Relaxed)
    }

    pub fn map_accepted(&self) -> u64 {
        self.map_accepted.load(Ordering::Relaxed)
    }

    pub fn map_rejected(&self) -> u64 {
        self.map_rejected.load(Ordering::Relaxed)
    }
}

/// Política de admisión para ejecutables y mapeos.
pub trait AdmissionPolicy: Send + Sync {
    /// Decide si admitir un ejecutable.
    fn allow_exe(&self, path: &Path) -> bool;

    /// Decide si admitir un mapeo.
    fn allow_map(&self, path: &Path) -> bool;

    /// Toma una decisión completa para un candidato.
    fn decide(&self, candidate: &super::CandidateExe) -> AdmissionDecision;

    /// Obtiene estadísticas (opcional).
    fn stats(&self) -> Option<&AdmissionPolicyStats> {
        None
    }
}

/// Política de admisión por defecto: permite todo según configuración.
#[derive(Debug)]
pub struct DefaultAdmissionPolicy {
    exeprefix: Vec<std::path::PathBuf>,
    mapprefix: Vec<std::path::PathBuf>,
    max_map: u64,
    stats: AdmissionPolicyStats,
}

impl DefaultAdmissionPolicy {
    /// Crea una nueva política por defecto.
    pub fn new(
        exeprefix: Vec<std::path::PathBuf>,
        mapprefix: Vec<std::path::PathBuf>,
        max_map: u64,
    ) -> Self {
        Self {
            exeprefix,
            mapprefix,
            max_map,
            stats: AdmissionPolicyStats::new(),
        }
    }

    /// Verifica si una ruta coincide con algún prefijo.
    fn matches_prefix(path: &Path, prefixes: &[std::path::PathBuf]) -> bool {
        if prefixes.is_empty() {
            return true; // Vacío = permitir todo
        }
        prefixes.iter().any(|p| path.starts_with(p))
    }
}

impl AdmissionPolicy for DefaultAdmissionPolicy {
    fn allow_exe(&self, path: &Path) -> bool {
        Self::matches_prefix(path, &self.exeprefix)
    }

    fn allow_map(&self, path: &Path) -> bool {
        Self::matches_prefix(path, &self.mapprefix)
    }

    fn decide(&self, candidate: &super::CandidateExe) -> AdmissionDecision {
        if !self.allow_exe(&candidate.path) {
            self.stats.record_exe_rejected();
            return AdmissionDecision::Reject {
                reason: RejectReason::ExeNotAllowed,
            };
        }

        self.stats.record_exe_accepted();

        let mut accepted_maps = 0;
        let mut rejected_maps = 0;

        for map in &candidate.maps {
            if map.length > self.max_map {
                self.stats.record_map_rejected();
                rejected_maps += 1;
                continue;
            }
            if !self.allow_map(&map.path) {
                self.stats.record_map_rejected();
                rejected_maps += 1;
                continue;
            }
            self.stats.record_map_accepted();
            accepted_maps += 1;
        }

        let completeness = if accepted_maps == 0 {
            Completeness::ExeOnly
        } else if rejected_maps > 0 {
            Completeness::Partial
        } else {
            Completeness::Full
        };

        AdmissionDecision::Accept { completeness }
    }

    fn stats(&self) -> Option<&AdmissionPolicyStats> {
        Some(&self.stats)
    }
}
