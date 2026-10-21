//! Orquestador principal de vasak-preload.
//!
//! Coordina el escaneo, modelado, predicción y prefetch.

pub mod clock;
pub mod config;
pub mod domain;
pub mod engine;
pub mod error;
pub mod observation;
pub mod persistence;
pub mod prediction;
pub mod prefetch;
pub mod stores;

pub use clock::{Clock, SystemClock};
pub use config::Config;
pub use domain::{Exe, ExeId, ExeKey, MapId, MapKey, MapSegment, MarkovEdge, MarkovState, MemStat};
pub use engine::{ControlEvent, EngineView, PreloadEngine, ReloadBundle, Services, TickReport};
pub use error::Error;
pub use observation::{
    AdmissionDecision, AdmissionPolicy, AdmissionPolicyStats, CandidateExe, Completeness,
    DefaultAdmissionPolicy, DefaultModelUpdater, ModelDelta, ModelUpdater, Observation,
    ObservationEvent, ProcfsScanner, RejectReason, ScanWarning, Scanner,
};
pub use persistence::{NoopRepository, SqliteRepository, StateRepository, StoresSnapshot};
pub use prediction::{MarkovPredictor, Prediction, PredictionSummary, Predictor};
pub use prefetch::{
    GreedyPrefetchPlanner, NoopPrefetcher, PosixFadvisePrefetcher, PrefetchMap, PrefetchPlan,
    PrefetchPlanner, PrefetchReport, Prefetcher,
};
pub use stores::Stores;
