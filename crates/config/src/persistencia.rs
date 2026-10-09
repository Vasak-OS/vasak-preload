//! Configuración de persistencia.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DurationSeconds};
use std::path::PathBuf;
use std::time::Duration;

/// Configuración de persistencia del estado.
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Persistencia {
    /// Ruta al archivo de estado (SQLite).
    #[serde(default = "estado_defecto")]
    pub estado_path: PathBuf,

    /// Intervalo de autoguardado (en segundos). None = usar valor de sistema.
    #[serde_as(as = "Option<DurationSeconds<u64>>")]
    #[serde(default)]
    pub autosave_interval: Option<Duration>,

    /// Si guardar al apagar.
    #[serde(default = "verdadero")]
    pub save_on_shutdown: bool,

    /// Versión del esquema de la base de datos.
    #[serde(default = "version_esquema_defecto")]
    pub version_esquema: u32,
}

fn estado_defecto() -> PathBuf {
    dirs::state_dir()
        .unwrap_or_else(|| PathBuf::from("/var/lib/vasak-preload"))
        .join("state.sqlite")
}

fn verdadero() -> bool {
    true
}

fn version_esquema_defecto() -> u32 {
    1
}

impl Default for Persistencia {
    fn default() -> Self {
        Self {
            estado_path: estado_defecto(),
            autosave_interval: None,
            save_on_shutdown: verdadero(),
            version_esquema: version_esquema_defecto(),
        }
    }
}