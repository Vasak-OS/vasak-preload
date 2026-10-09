//! Errores de configuración.

#![forbid(unsafe_code)]

use thiserror::Error;

/// Errores que pueden ocurrir al manejar la configuración.
#[derive(Debug, Error)]
pub enum Error {
    /// Error de E/S al leer o escribir el archivo.
    #[error("error de E/S: {0}")]
    Io(#[from] std::io::Error),

    /// Error al parsear TOML.
    #[error("error al parsear TOML: {0}")]
    TomlParse(#[from] toml_edit::TomlError),

    /// Error al serializar TOML.
    #[error("error al serializar TOML: {0}")]
    TomlSer(#[from] toml_edit::ser::Error),

    /// Error al deserializar TOML.
    #[error("error al deserializar TOML: {0}")]
    TomlDe(#[from] toml_edit::de::Error),

    /// Campo requerido ausente.
    #[error("campo requerido ausente: {0}")]
    CampoRequerido(String),
}