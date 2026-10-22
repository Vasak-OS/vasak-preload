//! Estadísticas de memoria del sistema.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// Estadísticas de memoria del sistema, en bytes (`/proc/meminfo` da kB y se convierten).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemStat {
    /// Memoria total.
    pub total: u64,
    /// Memoria libre.
    pub free: u64,
    /// Memoria en caché.
    pub cached: u64,
    /// Memoria que se puede usar sin hacer swap (`MemAvailable`).
    pub available: u64,
    /// Page-ins desde el último muestreo.
    pub pagein: u64,
    /// Page-outs desde el último muestreo.
    pub pageout: u64,
}

impl MemStat {
    /// Porcentaje de memoria disponible (0-100).
    pub fn free_percent(&self) -> u8 {
        if self.total == 0 {
            return 0;
        }
        (self.available.min(self.total) * 100 / self.total) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem(total: u64, available: u64) -> MemStat {
        MemStat {
            total,
            // `free` y `cached` no cuentan: lo que manda es `available`.
            free: total,
            cached: total,
            available,
            pagein: 0,
            pageout: 0,
        }
    }

    #[test]
    fn el_porcentaje_sale_de_la_memoria_disponible() {
        assert_eq!(mem(1000, 250).free_percent(), 25);
        assert_eq!(mem(1000, 0).free_percent(), 0);
        assert_eq!(mem(1000, 1000).free_percent(), 100);
    }

    #[test]
    fn el_porcentaje_se_limita_a_cien() {
        assert_eq!(mem(1000, 5000).free_percent(), 100);
    }

    #[test]
    fn sin_memoria_total_el_porcentaje_es_cero() {
        assert_eq!(mem(0, 0).free_percent(), 0);
        assert_eq!(mem(0, 100).free_percent(), 0);
    }
}
