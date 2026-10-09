//! Configuración de vasak-preload.
//!
//! Define la estructura de configuración y su carga desde archivos TOML.

#![forbid(unsafe_code)]

mod error;
mod modelo;
mod persistencia;
mod sistema;

pub use error::Error;
pub use modelo::Modelo;
pub use persistencia::Persistencia;
pub use sistema::Sistema;

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Configuración principal de vasak-preload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Config {
    /// Configuración del modelo de Markov.
    pub modelo: Modelo,
    /// Configuración del sistema y escaneo.
    pub sistema: Sistema,
    /// Configuración de persistencia.
    pub persistencia: Persistencia,
}

impl Config {
    /// Carga la configuración desde un archivo TOML.
    /// Los campos faltantes se rellenan con valores por defecto.
    pub fn cargar(path: impl AsRef<Path>) -> Result<Self, Error> {
        let texto = std::fs::read_to_string(path)?;
        let mut config: Config = toml_edit::de::from_str(&texto)?;
        config.aplicar_defectos();
        Ok(config)
    }

    /// Guarda la configuración en un archivo TOML.
    pub fn guardar(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let toml = toml_edit::ser::to_string_pretty(self)?;
        std::fs::write(path, toml)?;
        Ok(())
    }

    /// Carga la configuración desde múltiples archivos TOML.
    /// Los archivos posteriores sobrescriben a los anteriores.
    pub fn cargar_multiple<T, U>(paths: U) -> Result<Self, Error>
    where
        T: AsRef<Path>,
        U: IntoIterator<Item = T>,
    {
        let mut combinado = toml_edit::DocumentMut::new();
        for path in paths {
            let path = path.as_ref();
            if !path.exists() {
                continue;
            }
            let texto = std::fs::read_to_string(path)?;
            let doc: toml_edit::DocumentMut = texto.parse()?;
            combinar_documento(&mut combinado, doc);
        }
        let mut config: Config = toml_edit::de::from_str(&combinado.to_string())?;
        config.aplicar_defectos();
        Ok(config)
    }

    fn aplicar_defectos(&mut self) {
        // Asegurar que los vectores estén ordenados para semántica de coincidencia de prefijos.
        self.sistema.exeprefix.sort();
        self.sistema.mapprefix.sort();
    }
}

fn combinar_documento(destino: &mut toml_edit::DocumentMut, origen: toml_edit::DocumentMut) {
    for (clave, item) in origen.iter() {
        combinar_item(destino.entry(clave).or_insert(toml_edit::Item::None), item.clone());
    }
}

fn combinar_item(destino: &mut toml_edit::Item, origen: toml_edit::Item) {
    use toml_edit::Item;
    match (destino, origen) {
        (Item::Table(destino_tabla), Item::Table(origen_tabla)) => {
            for (clave, item) in origen_tabla.iter() {
                combinar_item(destino_tabla.entry(clave).or_insert(Item::None), item.clone());
            }
        }
        (Item::ArrayOfTables(destino_array), Item::ArrayOfTables(origen_array)) => {
            for tabla in origen_array.iter() {
                destino_array.push(tabla.clone());
            }
        }
        (destino_item, origen_item) => {
            *destino_item = origen_item;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::tempdir;

    #[test]
    fn ida_y_vuelta() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let mut config = Config::default();
        config.aplicar_defectos();
        config.guardar(&path).unwrap();
        let cargado = Config::cargar(&path).unwrap();

        assert_eq!(config, cargado);
    }

    #[test]
    fn carga_multiple_combina() {
        let dir = tempdir().unwrap();
        let path1 = dir.path().join("a.toml");
        let path2 = dir.path().join("b.toml");

        std::fs::write(&path1, "[modelo]\nciclo = 60\n[sistema]\ndoscan = false\n").unwrap();
        std::fs::write(&path2, "[persistencia]\nautosave_interval = 120\n").unwrap();

        let cfg = Config::cargar_multiple([path1, path2]).unwrap();
        assert_eq!(cfg.modelo.ciclo, Duration::from_secs(60));
        assert!(!cfg.sistema.doscan);
        assert_eq!(
            cfg.persistencia.autosave_interval,
            Some(Duration::from_secs(120))
        );
    }
}