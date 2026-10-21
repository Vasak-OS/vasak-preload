//! Identificadores tipados para entidades del modelo.

#![allow(unsafe_code)]

use serde::{Deserialize, Serialize, de, ser};
use slotmap::{Key, KeyData};
use std::path::PathBuf;

/// Identificador único de un ejecutable.
#[derive(Copy, Clone, Default, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct ExeId(pub KeyData);

impl From<KeyData> for ExeId {
    fn from(data: KeyData) -> Self {
        ExeId(data)
    }
}

// Implementar el trait Key de slotmap para que funcione con SlotMap
unsafe impl Key for ExeId {
    fn data(&self) -> KeyData {
        self.0
    }
}

/// Identificador único de un mapeo de memoria.
#[derive(Copy, Clone, Default, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct MapId(pub KeyData);

impl From<KeyData> for MapId {
    fn from(data: KeyData) -> Self {
        MapId(data)
    }
}

// Implementar el trait Key de slotmap para que funcione con SlotMap
unsafe impl Key for MapId {
    fn data(&self) -> KeyData {
        self.0
    }
}

// Serialización manual para MapId usando la representación u64 de KeyData
impl Serialize for MapId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        serializer.serialize_u64(self.0.as_ffi())
    }
}

impl<'de> Deserialize<'de> for MapId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        let ffi = u64::deserialize(deserializer)?;
        Ok(MapId(KeyData::from_ffi(ffi)))
    }
}

/// Clave para buscar un ejecutable por su ruta.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ExeKey {
    path: PathBuf,
}

impl ExeKey {
    /// Crea una nueva clave de ejecutable.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Obtiene la ruta del ejecutable.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl AsRef<PathBuf> for ExeKey {
    fn as_ref(&self) -> &PathBuf {
        &self.path
    }
}

impl AsRef<std::path::Path> for ExeKey {
    fn as_ref(&self) -> &std::path::Path {
        &self.path
    }
}

/// Clave para buscar un mapeo de memoria.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MapKey {
    pub path: PathBuf,
    pub offset: u64,
    pub length: u64,
}

impl MapKey {
    /// Crea una nueva clave de mapeo.
    pub fn new(path: PathBuf, offset: u64, length: u64) -> Self {
        Self {
            path,
            offset,
            length,
        }
    }
}
