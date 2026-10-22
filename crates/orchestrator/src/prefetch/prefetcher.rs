//! Ejecutores de prefetch: traen páginas a memoria.

#![allow(unsafe_code)]

use crate::prefetch::{PrefetchPlan, PrefetchReport};
use async_trait::async_trait;
use nix::fcntl::{OFlag, open};
use nix::sys::stat::Mode;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use tokio::task;

/// Prefetcher que no hace nada (para testing).
#[derive(Debug, Default)]
pub struct NoopPrefetcher;

#[async_trait]
impl super::Prefetcher for NoopPrefetcher {
    async fn execute(
        &self,
        plan: &PrefetchPlan,
        _stores: &crate::stores::Stores,
    ) -> PrefetchReport {
        PrefetchReport {
            attempted: plan.maps.len() as u32,
            succeeded: plan.maps.len() as u32,
            failed: 0,
            bytes_prefetched: plan.total_bytes,
            errors: Vec::new(),
        }
    }
}

/// Prefetcher que usa posix_fadvise para traer páginas a memoria.
#[derive(Debug, Default)]
pub struct PosixFadvisePrefetcher;

#[async_trait]
impl super::Prefetcher for PosixFadvisePrefetcher {
    async fn execute(&self, plan: &PrefetchPlan, stores: &crate::stores::Stores) -> PrefetchReport {
        let mut report = PrefetchReport::default();

        for prefetch_map in &plan.maps {
            report.attempted += 1;

            if let Some(map) = stores.maps.get(prefetch_map.map_id) {
                let result = task::spawn_blocking({
                    let path = map.path.clone();
                    let offset = map.offset;
                    let len = map.length;
                    move || prefetch_file(&path, offset, len)
                })
                .await;

                match result {
                    Ok(Ok(())) => {
                        report.succeeded += 1;
                        report.bytes_prefetched += prefetch_map.size;
                    }
                    Ok(Err(e)) => {
                        report.failed += 1;
                        report.errors.push(format!("{}: {}", map.path.display(), e));
                    }
                    Err(e) => {
                        report.failed += 1;
                        report.errors.push(format!("task error: {}", e));
                    }
                }
            } else {
                report.failed += 1;
                report
                    .errors
                    .push(format!("mapa no encontrado: {:?}", prefetch_map.map_id));
            }
        }

        report
    }
}

/// Prefetchea un archivo usando posix_fadvise.
fn prefetch_file(path: &Path, offset: u64, len: u64) -> Result<(), std::io::Error> {
    let fd = open(path, OFlag::O_RDONLY | OFlag::O_CLOEXEC, Mode::empty())?;

    // POSIX_FADV_WILLNEED = 3
    const POSIX_FADV_WILLNEED: i32 = 3;

    // Llamar a posix_fadvise
    let result = unsafe {
        libc::posix_fadvise(
            fd.as_raw_fd(),
            offset as libc::off_t,
            len as libc::off_t,
            POSIX_FADV_WILLNEED,
        )
    };

    if result != 0 {
        return Err(std::io::Error::from_raw_os_error(result));
    }

    Ok(())
}
