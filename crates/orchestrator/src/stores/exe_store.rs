//! Almacén de ejecutables.

#![forbid(unsafe_code)]

use crate::domain::{Exe, ExeId, ExeKey};
use slotmap::SlotMap;

#[derive(Debug, Default)]
pub struct ExeStore {
    inner: SlotMap<ExeId, Exe>,
    by_key: std::collections::HashMap<ExeKey, ExeId>,
}

impl ExeStore {
    pub fn ensure(&mut self, key: ExeKey) -> ExeId {
        if let Some(&id) = self.by_key.get(&key) {
            return id;
        }
        let exe = Exe::new(key.clone());
        let id = self.inner.insert(exe);
        self.by_key.insert(key, id);
        id
    }

    pub fn get(&self, id: ExeId) -> Option<&Exe> {
        self.inner.get(id)
    }

    pub fn get_mut(&mut self, id: ExeId) -> Option<&mut Exe> {
        self.inner.get_mut(id)
    }

    pub fn id_by_key(&self, key: &ExeKey) -> Option<ExeId> {
        self.by_key.get(key).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (ExeId, &Exe)> {
        self.inner.iter()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}