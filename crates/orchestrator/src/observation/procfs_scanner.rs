//! Escáner de /proc para observar procesos y mapeos de memoria.
//!
//! Aprovecha la lógica de muestreo de procesos de vasak-monitor.

#![forbid(unsafe_code)]

use crate::domain::{MapSegment, MemStat};
use crate::error::Error;
use crate::observation::{CandidateExe, Observation, ObservationEvent, ScanWarning, Scanner};
use procfs::process::{Process, MemoryMaps};
use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Escáner que lee /proc para obtener procesos y sus mapeos.
#[derive(Debug, Default)]
pub struct ProcfsScanner {
    /// Caché de tamaño de página del sistema.
    page_size: Option<u64>,
    /// Último tiempo de muestreo para calcular deltas.
    last_sample_time: Option<u64>,
    /// Últimas estadísticas de memoria.
    last_memstat: Option<MemStat>,
}

impl ProcfsScanner {
    /// Crea un nuevo escáner.
    pub fn new() -> Self {
        Self::default()
    }

    /// Obtiene el tamaño de página del sistema.
    fn page_size(&self) -> u64 {
        if let Some(ps) = self.page_size {
            return ps;
        }
        // Leer de /proc/sys/vm/page_size o usar valor por defecto
        let ps = std::fs::read_to_string("/proc/sys/vm/page_size")
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(4096);
        // Note: we don't cache here to avoid &mut self, but it's fine since page size doesn't change
        ps
    }

    /// Lee estadísticas de memoria de /proc/meminfo.
    fn read_memstat(&self) -> Result<MemStat, Error> {
        let content = std::fs::read_to_string("/proc/meminfo")?;
        let mut total = 0u64;
        let mut free = 0u64;
        let mut cached = 0u64;

        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }
            let value: u64 = parts[1].parse().unwrap_or(0) * 1024; // kB a bytes
            match parts[0] {
                "MemTotal:" => total = value,
                "MemFree:" => free = value,
                "Cached:" => cached = value,
                _ => {}
            }
        }

        let page_size = self.page_size();
        Ok(MemStat {
            total: total / page_size,
            free: free / page_size,
            cached: cached / page_size,
            pagein: 0, // Se calcula por delta
            pageout: 0,
        })
    }

    /// Calcula pagein/pageout por delta desde última lectura.
    fn compute_page_delta(&mut self, current: &mut MemStat) {
        if let Some(last) = self.last_memstat {
            current.pagein = current.pagein.saturating_sub(last.pagein);
            current.pageout = current.pageout.saturating_sub(last.pageout);
        }
        self.last_memstat = Some(*current);
    }

    /// Lee mapeos de memoria de un proceso.
    fn read_process_maps(pid: u32) -> Result<MemoryMaps, Error> {
        let proc = Process::new(pid as i32)?;
        Ok(proc.maps()?)
    }

    /// Convierte MemoryMaps de procfs a MapSegment.
    fn maps_to_segments(maps: MemoryMaps, page_size: u64) -> Vec<MapSegment> {
        maps.0.into_iter()
            .filter_map(|m| {
                let path = match m.pathname {
                    procfs::process::MMapPath::Path(p) => p,
                    _ => return None, // Skip non-file mappings (heap, stack, etc.)
                };
                // Solo archivos regulares (no heap, stack, anon, etc.)
                if !path.exists() || !path.is_file() {
                    return None;
                }
                let offset = m.address.0;
                let length = (m.address.1 - m.address.0) * page_size;
                Some(MapSegment::new(path, offset, length, 0))
            })
            .collect()
    }
}

impl Scanner for ProcfsScanner {
    fn scan(&mut self, time: u64, scan_id: u64) -> Result<Observation, Error> {
        let mut events = Vec::new();
        events.push(ObservationEvent::ObsBegin { time, scan_id });

        // Leer memstat
        let mut memstat = self.read_memstat()?;
        self.compute_page_delta(&mut memstat);
        events.push(ObservationEvent::MemStat { mem: memstat });

        // Escanear procesos
        let procs_iter = procfs::process::all_processes()?;
        let mut warnings = Vec::new();

        for proc_result in procs_iter {
            let proc = match proc_result {
                Ok(p) => p,
                Err(_) => {
                    warnings.push(ScanWarning::ProcessReadFailed);
                    continue;
                }
            };
            let pid = proc.pid();
            let pid_u32 = pid as u32;
            let stat = match proc.stat() {
                Ok(s) => s,
                Err(_) => {
                    warnings.push(ScanWarning::StatReadFailed(pid_u32));
                    continue;
                }
            };

            // Obtener nombre del ejecutable desde cmdline
            let exe_path = proc.exe().ok().unwrap_or_else(|| {
                // Fallback: usar nombre de stat
                let name = stat.comm;
                std::path::PathBuf::from(format!("/unknown/{}", name))
            });

            let mut candidate = CandidateExe::new(exe_path.clone(), pid_u32);

            // Leer mapeos
            match Self::read_process_maps(pid_u32) {
                Ok(maps) => {
                    let page_size = self.page_size();
                    let segments = Self::maps_to_segments(maps, page_size);
                    for seg in segments {
                        candidate.total_size += seg.length;
                        candidate.maps.push(seg.clone());
                        // Emitir evento Map para el model_updater
                        events.push(ObservationEvent::Map { segment: seg, pid: pid_u32 });
                    }
                }
                Err(_) => {
                    warnings.push(ScanWarning::MapsReadFailed(pid_u32));
                }
            }

            events.push(ObservationEvent::CandidateExe { exe: candidate });
        }

        events.push(ObservationEvent::ObsEnd {
            time,
            scan_id,
            warnings,
        });

        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaner_crea_eventos() {
        let mut scanner = ProcfsScanner::new();
        let result = scanner.scan(100, 1);
        assert!(result.is_ok());
        let events = result.unwrap();
        assert!(!events.is_empty());
        // Debe tener al menos ObsBegin y ObsEnd
        assert!(matches!(events[0], ObservationEvent::ObsBegin { .. }));
        assert!(matches!(events.last(), Some(ObservationEvent::ObsEnd { .. })));
    }
}