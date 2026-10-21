//! Grafo de Markov para transiciones entre ejecutables.

#![forbid(unsafe_code)]

use crate::domain::{ExeId, MarkovEdge, MarkovState};
use crate::stores::EdgeKey;
use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct MarkovGraph {
    inner: HashMap<EdgeKey, MarkovEdge>,
}

impl MarkovGraph {
    pub fn ensure_edge(&mut self, a: ExeId, b: ExeId, _now: u64, _state: MarkovState) -> bool {
        let key = EdgeKey::new(a, b);
        if self.inner.contains_key(&key) {
            return false;
        }
        self.inner.insert(key, MarkovEdge::default());
        true
    }

    pub fn get(&self, key: EdgeKey) -> Option<&MarkovEdge> {
        self.inner.get(&key)
    }

    pub fn get_mut(&mut self, key: EdgeKey) -> Option<&mut MarkovEdge> {
        self.inner.get_mut(&key)
    }

    pub fn iter(&self) -> impl Iterator<Item = (EdgeKey, &MarkovEdge)> {
        self.inner.iter().map(|(key, edge)| (*key, edge))
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Poda aristas que involucran ejecutables inactivos.
    pub fn prune_inactive(&mut self, active_exes: &std::collections::HashSet<ExeId>) {
        self.inner
            .retain(|key, _| active_exes.contains(&key.a()) && active_exes.contains(&key.b()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slotmap::SlotMap;
    use std::collections::HashSet;

    /// Tres ids que difieren en índice y en versión: `reutilizado` ocupa el
    /// índice de `borrado` con otra versión, y `lejano` está varios índices más
    /// adelante. Con claves recortadas a 32 bits se confundirían.
    fn distinct_ids() -> (ExeId, ExeId, ExeId) {
        let mut slots: SlotMap<ExeId, ()> = SlotMap::with_key();
        let borrado = slots.insert(());
        slots.remove(borrado);
        let reutilizado = slots.insert(());
        for _ in 0..5 {
            slots.insert(());
        }
        let lejano = slots.insert(());
        assert_ne!(borrado, reutilizado);
        (borrado, reutilizado, lejano)
    }

    #[test]
    fn la_arista_se_encuentra_con_el_par_en_cualquier_orden() {
        let (a, b, _) = distinct_ids();
        let mut graph = MarkovGraph::default();

        assert!(graph.ensure_edge(a, b, 0, MarkovState::Neither));

        assert!(graph.get(EdgeKey::new(a, b)).is_some());
        assert!(graph.get(EdgeKey::new(b, a)).is_some());
        assert!(graph.get_mut(EdgeKey::new(b, a)).is_some());
    }

    #[test]
    fn crear_dos_veces_el_mismo_par_no_duplica_la_arista() {
        let (a, b, _) = distinct_ids();
        let mut graph = MarkovGraph::default();

        assert!(graph.ensure_edge(a, b, 0, MarkovState::Neither));
        assert!(!graph.ensure_edge(a, b, 1, MarkovState::Neither));
        assert!(!graph.ensure_edge(b, a, 2, MarkovState::Neither));

        assert_eq!(graph.len(), 1);
    }

    #[test]
    fn los_cambios_sobre_la_arista_se_conservan() {
        let (a, b, _) = distinct_ids();
        let mut graph = MarkovGraph::default();
        graph.ensure_edge(a, b, 0, MarkovState::Neither);

        graph.get_mut(EdgeKey::new(a, b)).unwrap().both_running_time = 77;

        assert_eq!(graph.get(EdgeKey::new(b, a)).unwrap().both_running_time, 77);
    }

    #[test]
    fn la_clave_devuelve_los_ids_originales_aunque_difieran_en_version_e_indice() {
        let (a, b, c) = distinct_ids();
        let mut graph = MarkovGraph::default();
        graph.ensure_edge(a, b, 0, MarkovState::Neither);
        graph.ensure_edge(b, c, 0, MarkovState::Neither);

        let mut pairs: Vec<_> = graph
            .iter()
            .map(|(key, _)| {
                let mut pair = [key.a(), key.b()];
                pair.sort();
                pair
            })
            .collect();
        pairs.sort();

        let mut expected = [[a, b], [b, c]];
        expected.iter_mut().for_each(|pair| pair.sort());
        expected.sort();
        assert_eq!(pairs, expected);
        // Las dos puntas son distintas: no se pierde una por el recorte.
        assert!(pairs.iter().all(|[x, y]| x != y));
    }

    #[test]
    fn podar_conserva_solo_las_aristas_con_los_dos_extremos_activos() {
        let (a, b, c) = distinct_ids();
        let mut graph = MarkovGraph::default();
        graph.ensure_edge(a, b, 0, MarkovState::Neither);
        graph.ensure_edge(a, c, 0, MarkovState::Neither);
        graph.ensure_edge(b, c, 0, MarkovState::Neither);

        graph.prune_inactive(&HashSet::from([a, b]));

        assert_eq!(graph.len(), 1);
        assert!(graph.get(EdgeKey::new(a, b)).is_some());
        assert!(graph.get(EdgeKey::new(a, c)).is_none());
        assert!(graph.get(EdgeKey::new(b, c)).is_none());
    }

    #[test]
    fn podar_con_el_conjunto_vacio_quita_todas_las_aristas() {
        let (a, b, _) = distinct_ids();
        let mut graph = MarkovGraph::default();
        graph.ensure_edge(a, b, 0, MarkovState::Neither);

        graph.prune_inactive(&HashSet::new());

        assert_eq!(graph.len(), 0);
    }
}
