//! Planificador de prefetch: selecciona qué mapas traer según predicción y presupuesto.

#![forbid(unsafe_code)]

use crate::prediction::Prediction;
use crate::prefetch::{PrefetchMap, PrefetchPlan};
use crate::stores::Stores;

/// Planificador guloso: selecciona mapas por prioridad hasta agotar presupuesto.
#[derive(Debug, Default)]
pub struct GreedyPrefetchPlanner {
    /// Factor de peso para probabilidad de predicción.
    prediction_weight: f32,
    /// Factor de peso para tamaño del mapa (preferir mapas pequeños).
    size_weight: f32,
    /// Porcentaje de la memoria total que se deja libre (`mem_libre_min_pct`).
    min_free_pct: u8,
}

impl GreedyPrefetchPlanner {
    /// Crea un nuevo planificador guloso.
    pub fn new() -> Self {
        Self {
            prediction_weight: 1.0,
            size_weight: 0.1,
            min_free_pct: 0,
        }
    }

    /// Deja libre al menos este porcentaje de la memoria total.
    pub fn with_min_free_pct(mut self, pct: u8) -> Self {
        self.min_free_pct = pct.min(100);
        self
    }

    /// Crea un planificador con pesos personalizados.
    pub fn with_weights(prediction_weight: f32, size_weight: f32) -> Self {
        Self {
            prediction_weight,
            size_weight,
            min_free_pct: 0,
        }
    }
}

impl super::PrefetchPlanner for GreedyPrefetchPlanner {
    fn plan(
        &self,
        prediction: &Prediction,
        stores: &Stores,
        memstat: &crate::domain::MemStat,
    ) -> PrefetchPlan {
        let mut candidates = Vec::new();

        // Presupuesto en bytes: la mitad de lo disponible por encima de la reserva.
        let reserve = memstat.total / 100 * u64::from(self.min_free_pct);
        let budget_bytes = memstat.available.saturating_sub(reserve) / 2;

        // Para cada ejecutable con probabilidad > 0, considerar sus mapas
        for (exe_id, prob) in &prediction.probabilities {
            if *prob < 0.01 {
                continue;
            }

            for map_id in stores.exe_maps.maps_for_exe(*exe_id) {
                if let Some(map) = stores.maps.get(map_id) {
                    // Calcular prioridad: probabilidad * peso - tamaño * peso
                    let priority = prob * self.prediction_weight
                        - (map.length as f32 / 1024.0 / 1024.0) * self.size_weight;

                    if priority > 0.0 {
                        candidates.push(PrefetchMap {
                            map_id,
                            map_key: map.key(),
                            priority,
                            size: map.length,
                        });
                    }
                }
            }
        }

        // Ordenar por prioridad descendente
        candidates.sort_by(|a, b| {
            b.priority
                .partial_cmp(&a.priority)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Seleccionar hasta agotar presupuesto
        let mut selected = Vec::new();
        let mut total_bytes = 0u64;

        for candidate in candidates {
            // Un mapa que no entra se salta: los que siguen pueden ser más chicos.
            if total_bytes + candidate.size > budget_bytes {
                continue;
            }
            total_bytes += candidate.size;
            selected.push(candidate);
        }

        PrefetchPlan {
            maps: selected,
            total_bytes,
            budget_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ExeKey, MapSegment, MarkovState, MemStat};
    use crate::prediction::{MarkovPredictor, Predictor};
    use crate::prefetch::PrefetchPlanner;
    use crate::stores::EdgeKey;

    fn mem(total: u64, available: u64) -> MemStat {
        MemStat {
            total,
            free: available,
            cached: 0,
            available,
            pagein: 0,
            pageout: 0,
        }
    }

    /// Un ejecutable con probabilidad 0.9 y un mapa por cada tamaño dado.
    fn stores_with_maps(sizes: &[u64]) -> (Stores, Prediction) {
        let mut stores = Stores::default();
        let exe = stores.ensure_exe(ExeKey::new("/bin/app".into()));
        for (i, size) in sizes.iter().enumerate() {
            let map =
                stores.ensure_map(MapSegment::new(format!("/lib/{i}.so").into(), 0, *size, 0));
            stores.attach_map(exe, map);
        }
        let mut prediction = Prediction::new(0);
        prediction.set_probability(exe, 0.9);
        (stores, prediction)
    }

    #[test]
    fn el_presupuesto_es_la_mitad_de_lo_disponible_por_encima_de_la_reserva() {
        // 10 MB en total, 4 MB disponibles, 10 % reservado (1 MB): (4 - 1) / 2 = 1,5 MB.
        let memstat = mem(10_000_000, 4_000_000);
        let (stores, prediction) = stores_with_maps(&[300_000, 400_000, 1_000_000]);

        let plan =
            GreedyPrefetchPlanner::new()
                .with_min_free_pct(10)
                .plan(&prediction, &stores, &memstat);

        assert_eq!(plan.budget_bytes, 1_500_000);
        // Entran los dos chicos (700 000 B); el de 1 MB pasaría del tope.
        assert_eq!(plan.maps.len(), 2);
        assert_eq!(plan.total_bytes, 700_000);
        assert!(plan.maps.iter().all(|m| m.size <= 400_000));
    }

    #[test]
    fn sin_reserva_el_presupuesto_es_la_mitad_de_lo_disponible() {
        let memstat = mem(10_000_000, 4_000_000);
        let (stores, prediction) = stores_with_maps(&[1_000_000]);

        for planner in [
            GreedyPrefetchPlanner::new(),
            GreedyPrefetchPlanner::with_weights(1.0, 0.1),
        ] {
            let plan = planner.plan(&prediction, &stores, &memstat);
            assert_eq!(plan.budget_bytes, 2_000_000);
            assert_eq!(plan.maps.len(), 1);
        }
    }

    #[test]
    fn con_menos_memoria_que_la_reserva_el_plan_queda_vacio() {
        // Disponible 500 kB, reserva 20 % de 10 MB = 2 MB.
        let memstat = mem(10_000_000, 500_000);
        let (stores, prediction) = stores_with_maps(&[1_000]);

        let plan =
            GreedyPrefetchPlanner::new()
                .with_min_free_pct(20)
                .plan(&prediction, &stores, &memstat);

        assert_eq!(plan.budget_bytes, 0);
        assert!(plan.maps.is_empty());
        assert_eq!(plan.total_bytes, 0);
    }

    #[test]
    fn la_reserva_se_limita_a_toda_la_memoria() {
        // Con 1200 disponibles sobre 1000 de total, una reserva recortada a 100 %
        // deja (1200 - 1000) / 2 = 100; sin recortar, 150 % la dejaría en 0.
        let memstat = mem(1000, 1200);
        let prediction = Prediction::new(0);

        let clipped = GreedyPrefetchPlanner::new().with_min_free_pct(150).plan(
            &prediction,
            &Stores::default(),
            &memstat,
        );

        assert_eq!(clipped.budget_bytes, 100);
    }

    #[test]
    fn los_ejecutables_que_no_corren_reciben_sus_mapas_si_el_predictor_los_espera() {
        let mut stores = Stores::default();
        let running = stores.ensure_exe(ExeKey::new("/bin/running".into()));
        let idle = stores.ensure_exe(ExeKey::new("/bin/idle".into()));
        let idle_map = stores.ensure_map(MapSegment::new("/lib/idle.so".into(), 0, 4096, 0));
        stores.attach_map(idle, idle_map);
        stores.exes.get_mut(running).unwrap().mark_running(100);
        stores.active.update(&[running], 100);
        stores.ensure_markov_edge(running, idle, 100, MarkovState::Neither);
        let key = EdgeKey::new(running, idle);
        let only_running = if key.a() == running {
            MarkovState::OnlyA
        } else {
            MarkovState::OnlyB
        };
        // Históricamente, `idle` arranca cuando `running` ya corre.
        stores.markov.get_mut(key).unwrap().transition_prob[only_running as usize]
            [MarkovState::Both as usize] = 0.3;
        let prediction = MarkovPredictor::new().predict(&stores);
        assert!(prediction.get_probability(idle) > 0.0);
        assert!(!stores.exes.get(idle).unwrap().running);

        let plan = GreedyPrefetchPlanner::new().plan(&prediction, &stores, &mem(1 << 30, 1 << 29));

        assert_eq!(plan.maps.len(), 1);
        assert_eq!(plan.maps[0].map_id, idle_map);
    }

    #[test]
    fn sin_aristas_aprendidas_el_que_no_corre_no_recibe_mapas() {
        let mut stores = Stores::default();
        let running = stores.ensure_exe(ExeKey::new("/bin/running".into()));
        let idle = stores.ensure_exe(ExeKey::new("/bin/idle".into()));
        let idle_map = stores.ensure_map(MapSegment::new("/lib/idle.so".into(), 0, 4096, 0));
        stores.attach_map(idle, idle_map);
        stores.exes.get_mut(running).unwrap().mark_running(100);
        stores.active.update(&[running], 100);
        let prediction = MarkovPredictor::new().predict(&stores);

        let plan = GreedyPrefetchPlanner::new().plan(&prediction, &stores, &mem(1 << 30, 1 << 29));

        assert!(plan.maps.is_empty());
    }

    #[test]
    fn un_mapa_grande_que_no_entra_no_deja_afuera_a_los_chicos_que_siguen() {
        let mut stores = Stores::default();
        let mut prediction = Prediction::new(0);
        let mut add = |path: &str, size: u64, prob: f32| {
            let exe = stores.ensure_exe(ExeKey::new(path.into()));
            let map = stores.ensure_map(MapSegment::new(format!("{path}.so").into(), 0, size, 0));
            stores.attach_map(exe, map);
            prediction.set_probability(exe, prob);
            map
        };
        // El grande tiene prioridad mayor (0.9 - 0.29 contra 0.5 - 0.01) pero
        // pasa del presupuesto de 2 MB; el chico sí entra.
        let big = add("/bin/big", 3_000_000, 0.9);
        let small = add("/bin/small", 100_000, 0.5);

        let plan =
            GreedyPrefetchPlanner::new().plan(&prediction, &stores, &mem(10_000_000, 4_000_000));

        assert_eq!(plan.budget_bytes, 2_000_000);
        assert_eq!(plan.maps.len(), 1);
        assert_eq!(plan.maps[0].map_id, small);
        assert_ne!(plan.maps[0].map_id, big);
        assert_eq!(plan.total_bytes, 100_000);
        assert!(plan.total_bytes <= plan.budget_bytes);
    }
}
