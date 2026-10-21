//! Planificador de prefetch: selecciona qué mapas traer según predicción y presupuesto.

#![forbid(unsafe_code)]

use crate::domain::{ExeId, MapId, MapSegment};
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
}

impl GreedyPrefetchPlanner {
    /// Crea un nuevo planificador guloso.
    pub fn new() -> Self {
        Self {
            prediction_weight: 1.0,
            size_weight: 0.1,
        }
    }

    /// Crea un planificador con pesos personalizados.
    pub fn with_weights(prediction_weight: f32, size_weight: f32) -> Self {
        Self {
            prediction_weight,
            size_weight,
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

        // Presupuesto: porcentaje de memoria libre
        let free_pct = memstat.free_percent() as f32 / 100.0;
        let budget_bytes = (memstat.available_pages() as f32 * free_pct * 0.5) as u64; // Usar hasta 50% de memoria disponible

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
            if total_bytes + candidate.size > budget_bytes {
                break;
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
