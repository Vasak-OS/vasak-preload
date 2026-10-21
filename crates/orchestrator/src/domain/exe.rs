//! Representación de un ejecutable en el modelo.

#![forbid(unsafe_code)]

use crate::domain::ids::ExeKey;

/// Un ejecutable rastreado por el modelo.
#[derive(Debug, Clone)]
pub struct Exe {
    /// Clave única del ejecutable.
    pub key: ExeKey,
    /// Tiempo total de ejecución acumulado (en segundos).
    pub total_running_time: u64,
    /// Último tiempo en que se vio ejecutándose (en segundos desde epoch del modelo).
    pub last_seen_time: Option<u64>,
    /// Si está actualmente en ejecución.
    pub running: bool,
}

impl Exe {
    /// Crea un nuevo ejecutable.
    pub fn new(key: ExeKey) -> Self {
        Self {
            key,
            total_running_time: 0,
            last_seen_time: None,
            running: false,
        }
    }

    /// Marca el ejecutable como en ejecución en el tiempo dado.
    pub fn mark_running(&mut self, now: u64) {
        self.running = true;
        self.last_seen_time = Some(now);
    }

    /// Marca el ejecutable como detenido en el tiempo dado.
    pub fn mark_stopped(&mut self, now: u64) {
        self.running = false;
        self.last_seen_time = Some(now);
    }

    /// Actualiza el tiempo de ejecución acumulado.
    pub fn accumulate_runtime(&mut self, delta: u64) {
        self.total_running_time = self.total_running_time.saturating_add(delta);
    }
}
