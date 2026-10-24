//! Ejecutores de prefetch: traen páginas a memoria.

#![forbid(unsafe_code)]

use crate::prefetch::{PrefetchPlan, PrefetchReport};
use async_trait::async_trait;
use nix::fcntl::{AT_FDCWD, OFlag, OpenHow, PosixFadviseAdvice, ResolveFlag, openat2};
use nix::sys::stat::{SFlag, fstat};
use std::os::fd::AsRawFd;
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
///
/// Las rutas salen del modelo, que puede haber escrito cualquier proceso del
/// mismo usuario, así que no se confía en ellas:
/// - se abre primero con `O_PATH` y sin seguir symlinks: `O_PATH` no llama al
///   driver, así que una ruta a un dispositivo o a un FIFO no lo abre ni se
///   bloquea;
/// - sólo se precargan archivos regulares;
/// - se reabre para leer desde el mismo fd (el archivo no puede cambiar en el
///   medio) y el rango se recorta al tamaño del archivo.
fn prefetch_file(path: &Path, offset: u64, len: u64) -> Result<(), std::io::Error> {
    let no_links = ResolveFlag::RESOLVE_NO_SYMLINKS | ResolveFlag::RESOLVE_NO_MAGICLINKS;
    let handle = openat2(
        AT_FDCWD,
        path,
        OpenHow::new()
            .flags(OFlag::O_PATH | OFlag::O_CLOEXEC)
            .resolve(no_links),
    )?;

    let stat = fstat(&handle)?;
    if SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT != SFlag::S_IFREG {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "no es un archivo regular",
        ));
    }

    let size = u64::try_from(stat.st_size).unwrap_or(0);
    let Some(len) = clamp_range(offset, len, size) else {
        return Ok(());
    };

    let file = openat2(
        AT_FDCWD,
        format!("/proc/self/fd/{}", handle.as_raw_fd()).as_str(),
        OpenHow::new()
            .flags(OFlag::O_RDONLY | OFlag::O_NONBLOCK | OFlag::O_NOCTTY | OFlag::O_CLOEXEC),
    )?;
    nix::fcntl::posix_fadvise(
        &file,
        offset as libc::off_t,
        len as libc::off_t,
        PosixFadviseAdvice::POSIX_FADV_WILLNEED,
    )?;
    Ok(())
}

/// Recorta `[offset, offset + len)` al tamaño del archivo. `None` si el rango
/// queda vacío (el archivo se achicó desde que se observó).
fn clamp_range(offset: u64, len: u64, size: u64) -> Option<u64> {
    let end = offset.saturating_add(len).min(size);
    (end > offset).then(|| end - offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::MapSegment;
    use crate::prefetch::{PrefetchMap, Prefetcher};
    use crate::stores::Stores;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::Duration;
    use tempfile::TempDir;

    /// Margen para decidir que una llamada se bloqueó.
    const BLOQUEO: Duration = Duration::from_secs(5);

    /// Directorio temporal con la ruta canónica: `RESOLVE_NO_SYMLINKS` también
    /// rechaza un symlink en el camino hacia él (por ejemplo, un `/tmp` enlazado).
    fn tempdir() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        (dir, root)
    }

    fn regular_file(dir: &Path, name: &str, size: usize) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, vec![7u8; size]).unwrap();
        path
    }

    /// Corre `prefetch_file` en otro hilo: si se bloquea, la prueba falla en
    /// lugar de colgar la suite.
    fn prefetch_with_timeout(path: &Path) -> Result<(), std::io::Error> {
        let (tx, rx) = mpsc::channel();
        let path = path.to_path_buf();
        std::thread::spawn(move || {
            let _ = tx.send(prefetch_file(&path, 0, 4096));
        });
        rx.recv_timeout(BLOQUEO)
            .expect("prefetch_file se bloqueó: abrió la ruta en lugar de inspeccionarla")
    }

    fn plan_for(stores: &mut Stores, path: PathBuf, offset: u64, length: u64) -> PrefetchPlan {
        let segment = MapSegment::new(path, offset, length, 0);
        let map_key = segment.key();
        let map_id = stores.ensure_map(segment);
        PrefetchPlan {
            maps: vec![PrefetchMap {
                map_id,
                map_key,
                priority: 1.0,
                size: length,
            }],
            total_bytes: length,
            budget_bytes: length,
        }
    }

    #[test]
    fn un_archivo_regular_se_precarga() {
        let (_dir, root) = tempdir();
        let file = regular_file(&root, "lib.so", 8192);

        prefetch_file(&file, 0, 8192).expect("un archivo regular se precarga");
    }

    #[test]
    fn un_symlink_al_archivo_no_se_sigue() {
        let (_dir, root) = tempdir();
        let file = regular_file(&root, "real", 4096);
        let link = root.join("enlace");
        std::os::unix::fs::symlink(&file, &link).unwrap();

        assert!(prefetch_file(&link, 0, 4096).is_err());
    }

    #[test]
    fn un_symlink_en_un_componente_intermedio_no_se_sigue() {
        let (_dir, root) = tempdir();
        std::fs::create_dir(root.join("real")).unwrap();
        regular_file(&root.join("real"), "lib.so", 4096);
        std::os::unix::fs::symlink(root.join("real"), root.join("enlace")).unwrap();

        assert!(prefetch_file(&root.join("enlace/lib.so"), 0, 4096).is_err());
        // Control: por el camino real sí se precarga.
        prefetch_file(&root.join("real/lib.so"), 0, 4096).unwrap();
    }

    #[test]
    fn un_fifo_sin_escritor_falla_sin_bloquearse() {
        let (_dir, root) = tempdir();
        let fifo = root.join("fifo");
        nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();

        let error = prefetch_with_timeout(&fifo).expect_err("un FIFO no se precarga");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn un_dispositivo_de_caracteres_no_se_precarga() {
        for device in ["/dev/null", "/dev/zero"] {
            let error = prefetch_with_timeout(Path::new(device)).expect_err(device);
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{device}");
            assert!(error.to_string().contains("no es un archivo regular"));
        }
    }

    #[test]
    fn un_directorio_no_se_precarga() {
        let (_dir, root) = tempdir();

        let error = prefetch_file(&root, 0, 4096).expect_err("un directorio no se precarga");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn una_ruta_inexistente_falla() {
        let (_dir, root) = tempdir();

        let error = prefetch_file(&root.join("no-existe"), 0, 4096).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn un_archivo_que_se_achico_no_es_un_error() {
        let (_dir, root) = tempdir();
        let file = regular_file(&root, "chico", 10);

        prefetch_file(&file, 4096, 100).expect("rango fuera del archivo: nada que hacer");
    }

    #[test]
    fn el_rango_dentro_del_archivo_queda_igual() {
        assert_eq!(clamp_range(100, 200, 1000), Some(200));
        assert_eq!(clamp_range(0, 1000, 1000), Some(1000));
    }

    #[test]
    fn el_rango_que_cruza_el_final_se_recorta() {
        assert_eq!(clamp_range(900, 200, 1000), Some(100));
    }

    #[test]
    fn el_rango_sin_bytes_dentro_del_archivo_queda_vacio() {
        assert_eq!(clamp_range(1000, 10, 1000), None);
        assert_eq!(clamp_range(5000, 10, 1000), None);
        assert_eq!(clamp_range(0, 10, 0), None);
    }

    #[test]
    fn el_rango_de_longitud_cero_queda_vacio() {
        assert_eq!(clamp_range(10, 0, 1000), None);
    }

    #[test]
    fn el_desborde_de_offset_mas_longitud_se_satura() {
        assert_eq!(clamp_range(10, u64::MAX, 1000), Some(990));
        assert_eq!(clamp_range(u64::MAX, u64::MAX, 1000), None);
    }

    #[tokio::test]
    async fn el_ejecutor_cuenta_lo_precargado_de_un_archivo_regular() {
        let (_dir, root) = tempdir();
        let file = regular_file(&root, "lib.so", 8192);
        let mut stores = Stores::default();
        let plan = plan_for(&mut stores, file, 0, 8192);

        let report = PosixFadvisePrefetcher.execute(&plan, &stores).await;

        assert_eq!(report.attempted, 1);
        assert_eq!(report.succeeded, 1);
        assert_eq!(report.failed, 0);
        assert_eq!(report.bytes_prefetched, 8192);
        assert!(report.errors.is_empty());
    }

    #[tokio::test]
    async fn el_ejecutor_cuenta_como_fallido_el_archivo_inexistente() {
        let (_dir, root) = tempdir();
        let missing = root.join("no-existe.so");
        let mut stores = Stores::default();
        let plan = plan_for(&mut stores, missing.clone(), 0, 4096);

        let report = PosixFadvisePrefetcher.execute(&plan, &stores).await;

        assert_eq!(report.attempted, 1);
        assert_eq!(report.succeeded, 0);
        assert_eq!(report.failed, 1);
        assert_eq!(report.bytes_prefetched, 0);
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].contains(&missing.display().to_string()));
    }

    #[tokio::test]
    async fn el_ejecutor_cuenta_como_fallido_un_symlink_y_sigue_con_el_resto() {
        let (_dir, root) = tempdir();
        let file = regular_file(&root, "real", 4096);
        let link = root.join("enlace");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        let mut stores = Stores::default();
        let mut plan = plan_for(&mut stores, link, 0, 4096);
        plan.maps.extend(plan_for(&mut stores, file, 0, 4096).maps);

        let report = PosixFadvisePrefetcher.execute(&plan, &stores).await;

        assert_eq!(
            (report.attempted, report.succeeded, report.failed),
            (2, 1, 1)
        );
    }
}
