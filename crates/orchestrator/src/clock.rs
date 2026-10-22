//! Abstracción de reloj para testing.

#![forbid(unsafe_code)]

use async_trait::async_trait;
use std::time::{Duration, Instant};

/// Reloj que proporciona tiempo y sleep.
#[async_trait]
pub trait Clock: Send + Sync {
    /// Tiempo actual en segundos desde epoch arbitrario.
    fn now(&self) -> u64;

    /// Duerme por la duración dada.
    async fn sleep(&self, dur: Duration);
}

/// Reloj del sistema real.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock;

#[async_trait]
impl Clock for SystemClock {
    fn now(&self) -> u64 {
        // Usar tiempo monotónico para consistencia
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    async fn sleep(&self, dur: Duration) {
        tokio::time::sleep(dur).await;
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Reloj falso para testing.
    #[derive(Debug, Clone)]
    pub struct FakeClock {
        now: Arc<AtomicU64>,
    }

    impl FakeClock {
        pub fn new(initial: u64) -> Self {
            Self {
                now: Arc::new(AtomicU64::new(initial)),
            }
        }

        pub fn advance(&self, delta: u64) {
            self.now.fetch_add(delta, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl Clock for FakeClock {
        fn now(&self) -> u64 {
            self.now.load(Ordering::SeqCst)
        }

        async fn sleep(&self, _dur: Duration) {
            // No hace nada en tests
        }
    }
}
