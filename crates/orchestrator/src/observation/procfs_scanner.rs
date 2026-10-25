//! Escáner de /proc para observar procesos y mapeos de memoria.
//!
//! Aprovecha la lógica de muestreo de procesos de vasak-monitor.

#![forbid(unsafe_code)]

use crate::domain::{MapSegment, MemStat};
use crate::error::Error;
use crate::observation::{CandidateExe, Observation, ObservationEvent, ScanWarning, Scanner};
use procfs::process::{MemoryMaps, Process};
use std::path::{Path, PathBuf};

/// Escáner que lee /proc para obtener procesos y sus mapeos.
#[derive(Debug, Default)]
pub struct ProcfsScanner {
    /// Últimas estadísticas de memoria.
    last_memstat: Option<MemStat>,
}

impl ProcfsScanner {
    /// Crea un nuevo escáner.
    pub fn new() -> Self {
        Self::default()
    }

    /// Lee estadísticas de memoria de /proc/meminfo.
    fn read_memstat(&self) -> Result<MemStat, Error> {
        let content = std::fs::read_to_string("/proc/meminfo")?;
        Ok(Self::parse_meminfo(&content))
    }

    /// Interpreta el contenido de `/proc/meminfo`: los valores vienen en kB y
    /// se devuelven en bytes.
    fn parse_meminfo(content: &str) -> MemStat {
        let mut total = 0u64;
        let mut free = 0u64;
        let mut cached = 0u64;
        let mut available = 0u64;

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
                "MemAvailable:" => available = value,
                _ => {}
            }
        }

        MemStat {
            total,
            free,
            cached,
            available,
            pagein: 0, // Se calcula por delta
            pageout: 0,
        }
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

    /// Convierte MemoryMaps de procfs a MapSegment: el offset es la posición en
    /// el archivo y el largo, los bytes mapeados (no la dirección virtual).
    fn maps_to_segments(maps: MemoryMaps) -> Vec<MapSegment> {
        maps.0
            .into_iter()
            .filter_map(|m| {
                let path = match m.pathname {
                    procfs::process::MMapPath::Path(p) => p,
                    _ => return None, // Skip non-file mappings (heap, stack, etc.)
                };
                // Solo archivos regulares (no heap, stack, anon, etc.)
                if !path.exists() || !path.is_file() {
                    return None;
                }
                let length = m.address.1.saturating_sub(m.address.0);
                Some(MapSegment::new(path, m.offset, length, 0))
            })
            .collect()
    }
}

/// Quién escanea: su uid efectivo y su espacio de montajes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    uid: u32,
    mnt_ns: Option<PathBuf>,
}

impl Identity {
    /// El uid sale del dueño de `/proc/self`, con el mismo criterio que el de
    /// los demás procesos (`Process::uid`).
    fn current() -> Result<Self, Error> {
        Ok(Self {
            uid: Process::myself()?.uid()?,
            mnt_ns: std::fs::read_link("/proc/self/ns/mnt").ok(),
        })
    }

    /// Un proceso se observa sólo si es del mismo usuario y del mismo espacio
    /// de montajes: los mapas ajenos no se pueden leer sin ptrace, y en otro
    /// espacio de montajes las rutas apuntan a otros archivos.
    fn observes(&self, uid: u32, mnt_ns: Option<&Path>) -> bool {
        uid == self.uid
            && match &self.mnt_ns {
                Some(own) => mnt_ns == Some(own.as_path()),
                None => true,
            }
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
        let identity = Identity::current()?;
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
            let mnt_ns = std::fs::read_link(format!("/proc/{pid}/ns/mnt")).ok();
            if !proc
                .uid()
                .is_ok_and(|uid| identity.observes(uid, mnt_ns.as_deref()))
            {
                continue;
            }
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
                    let segments = Self::maps_to_segments(maps);
                    for seg in segments {
                        candidate.total_size += seg.length;
                        candidate.maps.push(seg.clone());
                        // Emitir evento Map para el model_updater
                        events.push(ObservationEvent::Map {
                            segment: seg,
                            pid: pid_u32,
                        });
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
        assert!(matches!(
            events.last(),
            Some(ObservationEvent::ObsEnd { .. })
        ));
    }

    use crate::observation::{
        AdmissionDecision, AdmissionPolicy, Completeness, DefaultAdmissionPolicy,
    };
    use procfs::FromBufRead;
    use std::os::unix::fs::MetadataExt;

    /// `MemoryMaps` es `non_exhaustive`: se arma parseando texto con el formato
    /// de `/proc/<pid>/maps`, que además prueba el parser real.
    fn maps_from(text: &str) -> MemoryMaps {
        MemoryMaps::from_buf_read(std::io::Cursor::new(text.to_owned())).unwrap()
    }

    #[test]
    fn los_segmentos_usan_el_offset_del_archivo_y_los_bytes_mapeados() {
        let dir = tempfile::TempDir::new().unwrap();
        let lib = dir.path().join("lib.so");
        std::fs::write(&lib, vec![0u8; 0x4000]).unwrap();
        let lib = lib.display();
        let maps = maps_from(&format!(
            "7f1200000000-7f1200003000 r-xp 00002000 08:01 1234 {lib}\n\
             7f1200010000-7f1200011000 r--p 00000000 08:01 1234 {lib}\n"
        ));

        let segments = ProcfsScanner::maps_to_segments(maps);

        // Offset en el archivo (no la dirección virtual) y largo en bytes (no por la página).
        assert_eq!(segments.len(), 2);
        assert_eq!((segments[0].offset, segments[0].length), (0x2000, 0x3000));
        assert_eq!((segments[1].offset, segments[1].length), (0, 0x1000));
        assert!(
            segments
                .iter()
                .all(|seg| seg.path == Path::new(&lib.to_string()))
        );
    }

    #[test]
    fn los_mapeos_que_no_son_un_archivo_regular_se_descartan() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("lib.so");
        std::fs::write(&file, b"x").unwrap();
        let (file, subdir) = (file.display().to_string(), dir.path().display().to_string());
        let maps = maps_from(&format!(
            "55a000000000-55a000021000 rw-p 00000000 00:00 0 [heap]\n\
             7ffc00000000-7ffc00021000 rw-p 00000000 00:00 0 [stack]\n\
             7f1300000000-7f1300001000 rw-p 00000000 00:00 0 \n\
             7f1400000000-7f1400001000 r--p 00000000 08:01 99 {subdir}\n\
             7f1500000000-7f1500001000 r--p 00000000 08:01 98 /no/existe/lib.so\n\
             7f1600000000-7f1600001000 r--p 00000000 08:01 97 {file}\n"
        ));

        let segments = ProcfsScanner::maps_to_segments(maps);

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].path, Path::new(&file));
    }

    #[test]
    fn un_rango_invertido_no_desborda_el_largo() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("lib.so");
        std::fs::write(&file, b"x").unwrap();
        let maps = maps_from(&format!(
            "7f1200002000-7f1200001000 r--p 00000000 08:01 1 {}\n",
            file.display()
        ));

        let segments = ProcfsScanner::maps_to_segments(maps);

        assert_eq!(segments[0].length, 0);
    }

    #[test]
    fn con_los_largos_corregidos_un_mapeo_chico_entra_bajo_el_limite() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("lib.so");
        std::fs::write(&file, vec![0u8; 1 << 20]).unwrap();
        // 1 MiB mapeado; antes se informaba 4096 veces más y `max_mapa` lo descartaba.
        let maps = maps_from(&format!(
            "7f1200000000-7f1200100000 r--p 00000000 08:01 1 {}\n",
            file.display()
        ));
        let mut candidate = CandidateExe::new("/usr/bin/app".into(), 1);
        candidate.maps = ProcfsScanner::maps_to_segments(maps);
        assert_eq!(candidate.maps[0].length, 1 << 20);

        let policy = DefaultAdmissionPolicy::new(vec![], vec![], 2 << 20);

        assert!(matches!(
            policy.decide(&candidate),
            AdmissionDecision::Accept {
                completeness: Completeness::Full
            }
        ));
    }

    #[test]
    fn solo_se_observa_al_mismo_usuario_en_el_mismo_espacio_de_montajes() {
        let own = Identity {
            uid: 1000,
            mnt_ns: Some("mnt:[4026531841]".into()),
        };
        let ns = |s: &'static str| Some(Path::new(s));

        assert!(own.observes(1000, ns("mnt:[4026531841]")));
        assert!(!own.observes(0, ns("mnt:[4026531841]")), "otro uid");
        assert!(!own.observes(1000, ns("mnt:[4026532000]")), "otro espacio");
        assert!(!own.observes(1000, None), "espacio ilegible");
    }

    #[test]
    fn si_no_se_conoce_el_propio_espacio_de_montajes_decide_solo_el_uid() {
        let own = Identity {
            uid: 1000,
            mnt_ns: None,
        };

        assert!(own.observes(1000, Some(Path::new("mnt:[1]"))));
        assert!(own.observes(1000, None));
        assert!(!own.observes(1001, None));
    }

    #[test]
    fn la_identidad_actual_es_la_del_dueno_de_proc_self() {
        let identity = Identity::current().unwrap();

        assert_eq!(identity.uid, std::fs::metadata("/proc/self").unwrap().uid());
        assert!(identity.observes(identity.uid, identity.mnt_ns.as_deref()));
    }

    #[test]
    fn el_escaner_solo_informa_procesos_del_propio_usuario() {
        let own_uid = std::fs::metadata("/proc/self").unwrap().uid();
        let events = ProcfsScanner::new().scan(0, 1).unwrap();

        let pids: Vec<u32> = events
            .iter()
            .filter_map(|event| match event {
                ObservationEvent::CandidateExe { exe } => Some(exe.pid),
                _ => None,
            })
            .collect();

        assert!(pids.contains(&std::process::id()), "el propio proceso");
        for pid in pids {
            // El proceso pudo terminar entre el escaneo y esta lectura.
            if let Ok(meta) = std::fs::metadata(format!("/proc/{pid}")) {
                assert_eq!(meta.uid(), own_uid, "pid {pid} es de otro usuario");
            }
        }
    }

    #[test]
    fn los_segmentos_del_propio_ejecutable_caen_dentro_del_archivo() {
        let exe = std::env::current_exe().unwrap().canonicalize().unwrap();
        let file_size = std::fs::metadata(&exe).unwrap().len();
        let page = procfs::page_size();
        let events = ProcfsScanner::new().scan(0, 1).unwrap();

        let mine = events
            .iter()
            .find_map(|event| match event {
                ObservationEvent::CandidateExe { exe } if exe.pid == std::process::id() => {
                    Some(exe.clone())
                }
                _ => None,
            })
            .expect("el propio proceso");
        let segments: Vec<_> = mine
            .maps
            .iter()
            .filter(|seg| seg.path.canonicalize().is_ok_and(|p| p == exe))
            .collect();

        assert!(
            !segments.is_empty(),
            "el ejecutable tiene mapeos de archivo"
        );
        for seg in segments {
            assert!(
                seg.offset < file_size,
                "offset {} fuera del archivo",
                seg.offset
            );
            assert_eq!(seg.length % page, 0);
            // A lo sumo el archivo entero redondeado a página; antes eran ×4096.
            assert!(
                seg.length <= file_size.div_ceil(page) * page,
                "largo {}",
                seg.length
            );
        }
        assert_eq!(
            mine.total_size,
            mine.maps.iter().map(|m| m.length).sum::<u64>()
        );
    }

    const SAMPLE_MEMINFO: &str = "MemTotal:       16384000 kB
MemFree:         2048000 kB
MemAvailable:    8192000 kB
Buffers:          100000 kB
Cached:          4096000 kB
SwapCached:            0 kB
HugePages_Total:       0
";

    #[test]
    fn meminfo_se_interpreta_en_bytes_y_con_la_memoria_disponible() {
        let mem = ProcfsScanner::parse_meminfo(SAMPLE_MEMINFO);

        assert_eq!(mem.total, 16_384_000 * 1024);
        assert_eq!(mem.free, 2_048_000 * 1024);
        assert_eq!(mem.cached, 4_096_000 * 1024);
        // `MemAvailable`, no free + cached (que serían 6 144 000 kB).
        assert_eq!(mem.available, 8_192_000 * 1024);
        assert_eq!((mem.pagein, mem.pageout), (0, 0));
        assert_eq!(mem.free_percent(), 50);
    }

    #[test]
    fn meminfo_ignora_lineas_incompletas_y_valores_ilegibles() {
        let mem = ProcfsScanner::parse_meminfo(
            "MemTotal: 1000 kB\n\nsolo\nMemFree: abc kB\nMemAvailable: 400 kB\n",
        );

        assert_eq!(mem.total, 1000 * 1024);
        assert_eq!(mem.free, 0);
        assert_eq!(mem.available, 400 * 1024);
    }

    #[test]
    fn sin_memavailable_la_disponible_queda_en_cero() {
        let mem = ProcfsScanner::parse_meminfo("MemTotal: 1000 kB\nMemFree: 900 kB\n");

        assert_eq!(mem.available, 0);
    }

    #[test]
    fn la_memoria_del_sistema_se_lee_en_bytes() {
        let mem = ProcfsScanner::new().read_memstat().unwrap();

        // Páginas serían unos 4 millones; bytes, más de 64 MiB en cualquier máquina.
        assert!(mem.total > 64 << 20, "total {}", mem.total);
        assert_eq!(mem.total % 1024, 0);
        assert!(mem.available > 0 && mem.available <= mem.total);
    }
}
