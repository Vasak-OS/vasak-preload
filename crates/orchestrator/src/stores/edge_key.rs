//! Clave de arista para el grafo de Markov.

#![forbid(unsafe_code)]

use crate::domain::ExeId;

/// Clave de una arista: el par de ejecutables, ordenado para que (a, b) y
/// (b, a) sean la misma arista.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EdgeKey {
    a: ExeId,
    b: ExeId,
}

impl EdgeKey {
    /// Crea una clave de arista canónica (ordenada por ID).
    pub fn new(a: ExeId, b: ExeId) -> Self {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        Self { a, b }
    }

    /// Crea una clave opcional si ambos IDs son válidos.
    pub fn new_opt(a: ExeId, b: ExeId) -> Option<Self> {
        Some(Self::new(a, b))
    }

    /// Obtiene el primer ejecutable.
    pub fn a(&self) -> ExeId {
        self.a
    }

    /// Obtiene el segundo ejecutable.
    pub fn b(&self) -> ExeId {
        self.b
    }
}
