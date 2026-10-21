//! Almacén de mapeos de memoria.

#![forbid(unsafe_code)]

use crate::domain::{MapId, MapKey, MapSegment};
use slotmap::SlotMap;

#[derive(Debug, Default, Clone)]
pub struct MapStore {
    inner: SlotMap<MapId, MapSegment>,
    by_key: std::collections::HashMap<MapKey, MapId>,
}

impl MapStore {
    pub fn ensure(&mut self, segment: MapSegment) -> MapId {
        let key = segment.key();
        if let Some(&id) = self.by_key.get(&key) {
            // Actualizar tiempo
            if let Some(existing) = self.inner.get_mut(id) {
                existing.update_time = segment.update_time;
            }
            return id;
        }
        let id = self.inner.insert(segment);
        self.by_key.insert(key, id);
        id
    }

    pub fn ensure_with_flag(&mut self, segment: MapSegment) -> (MapId, bool) {
        let key = segment.key();
        if let Some(&id) = self.by_key.get(&key) {
            if let Some(existing) = self.inner.get_mut(id) {
                existing.update_time = segment.update_time;
            }
            return (id, false);
        }
        let id = self.inner.insert(segment);
        self.by_key.insert(key, id);
        (id, true)
    }

    pub fn get(&self, id: MapId) -> Option<&MapSegment> {
        self.inner.get(id)
    }

    pub fn get_mut(&mut self, id: MapId) -> Option<&mut MapSegment> {
        self.inner.get_mut(id)
    }

    pub fn id_by_key(&self, key: &MapKey) -> Option<MapId> {
        self.by_key.get(key).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (MapId, &MapSegment)> {
        self.inner.iter()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }
}
