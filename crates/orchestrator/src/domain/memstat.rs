//! Estadísticas de memoria del sistema.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// Estadísticas de memoria del sistema (en páginas).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemStat {
    /// Memoria total (páginas).
    pub total: u64,
    /// Memoria libre (páginas).
    pub free: u64,
    /// Memoria en caché (páginas).
    pub cached: u64,
    /// Page-ins desde el último muestreo.
    pub pagein: u64,
    /// Page-outs desde el último muestreo.
    pub pageout: u64,
}

impl MemStat {
    /// Memoria disponible (free + cached) en páginas.
    pub fn available_pages(&self) -> u64 {
        self.free.saturating_add(self.cached)
    }

    /// Porcentaje de memoria libre (0-100).
    pub fn free_percent(&self) -> u8 {
        if self.total == 0 {
            return 0;
        }
        ((self.available_pages() * 100) / self.total) as u8
    }
}
