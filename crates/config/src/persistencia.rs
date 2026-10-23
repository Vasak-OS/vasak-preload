//! Configuración de persistencia.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use serde_with::{DurationSeconds, serde_as};
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

/// Dónde guarda el modelo cada persona: bajo su `$XDG_STATE_HOME`.
///
/// Es un servicio de usuario, así que no escribe en `/var/lib`. Sin directorio de
/// estado (no hay `HOME`) la ruta queda relativa: systemd arranca los servicios
/// de usuario en `$HOME`, y fallar con una ruta sin permisos es peor que guardar
/// ahí.
fn estado_defecto() -> PathBuf {
    estado_bajo(dirs::state_dir())
}

fn estado_bajo(base: Option<PathBuf>) -> PathBuf {
    base.unwrap_or_default()
        .join("vasak-preload")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_estado_va_en_un_directorio_propio_del_estado_del_usuario() {
        // Un servicio de usuario no puede escribir en /var/lib: el modelo de
        // cada persona vive bajo su `$XDG_STATE_HOME`.
        assert_eq!(
            estado_bajo(Some(PathBuf::from("/home/ana/.local/state"))),
            PathBuf::from("/home/ana/.local/state/vasak-preload/state.sqlite")
        );
    }

    #[test]
    fn sin_directorio_de_estado_no_se_cae_a_var_lib() {
        // /var/lib/vasak-preload era del daemon de sistema: desde un servicio de
        // usuario no se puede escribir, y el motor no arrancaría.
        let ruta = estado_bajo(None);
        assert!(!ruta.starts_with("/var/lib"), "{ruta:?}");
        assert!(ruta.ends_with("vasak-preload/state.sqlite"), "{ruta:?}");
    }

    #[test]
    fn el_valor_por_omision_no_apunta_a_var_lib() {
        let ruta = Persistencia::default().estado_path;
        assert!(!ruta.starts_with("/var/lib"), "{ruta:?}");
        assert!(ruta.ends_with("vasak-preload/state.sqlite"), "{ruta:?}");
    }
}
