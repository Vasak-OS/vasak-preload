//! Planificación y ejecución de prefetch.

mod plan;
mod planner;
mod prefetcher;

pub use plan::{PrefetchMap, PrefetchPlan, PrefetchReport};
pub use planner::GreedyPrefetchPlanner;
pub use prefetcher::{NoopPrefetcher, PosixFadvisePrefetcher};

/// Trait para planificadores de prefetch.
pub trait PrefetchPlanner: Send + Sync {
    fn plan(
        &self,
        prediction: &crate::prediction::Prediction,
        stores: &crate::stores::Stores,
        memstat: &crate::domain::MemStat,
    ) -> PrefetchPlan;
}

/// Trait para prefetchers.
#[async_trait::async_trait]
pub trait Prefetcher: Send + Sync {
    async fn execute(&self, plan: &PrefetchPlan, stores: &crate::stores::Stores) -> PrefetchReport;
}
