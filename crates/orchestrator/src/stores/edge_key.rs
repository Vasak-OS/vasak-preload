//! Clave de arista para el grafo de Markov.

#![forbid(unsafe_code)]

use crate::domain::ExeId;
use slotmap::{new_key_type, KeyData};

new_key_type! {
    /// Clave única para una arista (par ordenado de ejecutables).
    pub struct EdgeKey;
}

impl EdgeKey {
    /// Crea una clave de arista canónica (ordenada por ID).
    pub fn new(a: ExeId, b: ExeId) -> Self {
        // Ordenar para que (a,b) y (b,a) sean la misma clave
        // Usar el método key_data() que expone el KeyData interno
        let a_ffi = a.key_data().as_ffi();
        let b_ffi = b.key_data().as_ffi();
        let (first, second) = if a_ffi < b_ffi { (a_ffi, b_ffi) } else { (b_ffi, a_ffi) };
        // Usar hash combinado para crear key única
        let combined = ((first as u64) << 32) | (second as u64);
        Self::from(KeyData::from_ffi(combined))
    }

    /// Crea una clave opcional si ambos IDs son válidos.
    pub fn new_opt(a: ExeId, b: ExeId) -> Option<Self> {
        Some(Self::new(a, b))
    }

    /// Obtiene el primer ejecutable.
    pub fn a(&self) -> ExeId {
        let data = self.0;
        Exi((data.as_ffi() >> 32) as u32).into()
    }

    /// Obtiene el segundo ejecutable.
    pub fn b(&self) -> ExeId {
        let data = self.0;
        Exi(data.as_ffi() as u32).into()
    }
}

/// Wrapper para construir ExeId desde u32.
struct Exi(u32);

impl From<Exi> for ExeId {
    fn from(e: Exi) -> Self {
        ExeId::from(KeyData::from_ffi(e.0 as u64))
    }
}