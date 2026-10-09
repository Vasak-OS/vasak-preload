//! Grafo de Markov para transiciones entre ejecutables.

#![forbid(unsafe_code)]

use crate::domain::{ExeId, MarkovEdge, MarkovState};
use crate::stores::EdgeKey;
use slotmap::SlotMap;

#[derive(Debug, Default)]
pub struct MarkovGraph {
    inner: SlotMap<EdgeKey, MarkovEdge>,
}

impl MarkovGraph {
    pub fn ensure_edge(
        &mut self,
        a: ExeId,
        b: ExeId,
        _now: u64,
        _state: MarkovState,
    ) -> bool {
        let key = EdgeKey::new(a, b);
        if self.inner.contains_key(key) {
            return false;
        }
        self.inner.insert(MarkovEdge::default());
        true
    }

    pub fn get(&self, key: EdgeKey) -> Option<&MarkovEdge> {
        self.inner.get(key)
    }

    pub fn get_mut(&mut self, key: EdgeKey) -> Option<&mut MarkovEdge> {
        self.inner.get_mut(key)
    }

    pub fn iter(&self) -> impl Iterator<Item = (EdgeKey, &MarkovEdge)> {
        self.inner.iter()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Poda aristas que involucran ejecutables inactivos.
    pub fn prune_inactive(&mut self, active_exes: &std::collections::HashSet<ExeId>) {
        self.inner.retain(|key, _| {
            active_exes.contains(&key.a()) && active_exes.contains(&key.b())
        });
    }
}