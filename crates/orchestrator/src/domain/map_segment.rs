//! Segmento de mapeo de memoria.

#![forbid(unsafe_code)]

use crate::domain::ids::MapKey;
use std::path::PathBuf;

/// Un segmento de memoria mapeada (archivo + offset + longitud).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MapSegment {
    /// Ruta del archivo mapeado.
    pub path: PathBuf,
    /// Offset dentro del archivo (en bytes).
    pub offset: u64,
    /// Longitud del segmento (en bytes).
    pub length: u64,
    /// Tiempo de última actualización (en segundos desde epoch del modelo).
    pub update_time: u64,
}

impl MapSegment {
    /// Crea un nuevo segmento de mapeo.
    pub fn new(path: PathBuf, offset: u64, length: u64, update_time: u64) -> Self {
        Self {
            path,
            offset,
            length,
            update_time,
        }
    }

    /// Obtiene la clave de este segmento.
    pub fn key(&self) -> MapKey {
        MapKey::new(self.path.clone(), self.offset, self.length)
    }

    /// Tamaño del segmento en bytes.
    pub fn size(&self) -> u64 {
        self.length
    }
}
