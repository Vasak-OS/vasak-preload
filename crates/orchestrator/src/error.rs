//! Errores del orquestador.

#![forbid(unsafe_code)]

use procfs::ProcError;
use thiserror::Error;

/// Errores que pueden ocurrir en el orquestador.
#[derive(Debug, Error)]
pub enum Error {
    /// Error de E/S.
    #[error("error de E/S: {0}")]
    Io(#[from] std::io::Error),

    /// Error de base de datos SQL.
    #[error("error de base de datos: {0}")]
    Sql(#[from] sqlx::Error),

    /// Error de serialización JSON.
    #[error("error de serialización: {0}")]
    SerdeJson(#[from] serde_json::Error),

    /// Error de procfs.
    #[error("error de procfs: {0}")]
    Procfs(#[from] ProcError),

    /// Error de configuración.
    #[error("error de configuración: {0}")]
    Config(#[from] crate::config::Error),

    /// Ejecutable no encontrado en el almacén.
    #[error("ejecutable no encontrado: {0}")]
    ExeMissing(std::path::PathBuf),

    /// Mapeo no encontrado en el almacén.
    #[error("mapeo no encontrado: {0}")]
    MapMissing(std::path::PathBuf),

    /// Error de nix (syscalls).
    #[error("error de syscall: {0}")]
    Nix(#[from] nix::Error),

    /// Error genérico.
    #[error("{0}")]
    Other(String),
}