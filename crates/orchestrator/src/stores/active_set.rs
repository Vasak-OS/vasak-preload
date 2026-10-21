//! Conjunto de ejecutables activos.

#![forbid(unsafe_code)]

use crate::domain::ExeId;
use std::collections::HashSet;

#[derive(Debug, Default, Clone)]
pub struct ActiveSet {
    exes: HashSet<ExeId>,
    last_update: u64,
}

impl ActiveSet {
    pub fn update(&mut self, exes: &[ExeId], now: u64) {
        self.exes = exes.iter().copied().collect();
        self.last_update = now;
    }

    pub fn exes(&self) -> HashSet<ExeId> {
        self.exes.clone()
    }

    pub fn contains(&self, exe_id: ExeId) -> bool {
        self.exes.contains(&exe_id)
    }

    pub fn len(&self) -> usize {
        self.exes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.exes.is_empty()
    }

    pub fn last_update(&self) -> u64 {
        self.last_update
    }
}
