//! Predicción de qué aplicaciones se ejecutarán próximamente.

#![forbid(unsafe_code)]

mod predictor;
mod types;

pub use predictor::MarkovPredictor;
pub use types::{Prediction, PredictionSummary, Predictor};