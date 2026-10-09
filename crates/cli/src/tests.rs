//! Lo que imprime `vasak-preload-cli` a partir de lo que llega por D-Bus.
//!
//! D-Bus no tiene `Option`: el protocolo marca la ausencia con valores
//! centinela (0 o 255), y el cliente no tiene que mostrarlos como datos.

use crate::{format_config, format_exe_state, format_stats};
use vasak_preload_protocol::{ConfigSnapshot, DaemonState, DaemonStats, ExeState};

#[test]
fn sin_ticks_las_estadisticas_no_muestran_el_ultimo_tick() {
    let stats = DaemonStats {
        state: DaemonState::Running,
        model_time: 40,
        ..DaemonStats::default()
    };

    let salida = format_stats(&stats);

    assert!(salida.contains("Estado: Running"));
    assert!(salida.contains("Tiempo de modelo: 40s"));
    assert!(!salida.contains("Último tick"), "{salida}");
    assert!(!salida.contains("Scan ID"), "{salida}");
}

#[test]
fn con_un_tick_las_estadisticas_muestran_sus_campos() {
    let stats = DaemonStats {
        last_tick_scan_id: 7,
        last_tick_exes_added: 3,
        last_tick_prediction_max_prob: 0.5,
        last_tick_prefetch_bytes: 4096,
        last_tick_mem_free_pct: 42,
        ..DaemonStats::default()
    };

    let salida = format_stats(&stats);

    assert!(salida.contains("--- Último tick ---"));
    assert!(salida.contains("Scan ID: 7"));
    assert!(salida.contains("Exes añadidos: 3"));
    assert!(salida.contains("Predicción max prob: 50.00%"));
    assert!(salida.contains("Prefetch bytes: 4096"));
    assert!(salida.contains("Memoria libre: 42%"));
}

#[test]
fn sin_dato_de_memoria_no_se_muestra_un_255_por_ciento() {
    let stats = DaemonStats {
        last_tick_scan_id: 1,
        last_tick_mem_free_pct: 255,
        ..DaemonStats::default()
    };

    let salida = format_stats(&stats);

    assert!(salida.contains("Scan ID: 1"));
    assert!(!salida.contains("Memoria libre"), "{salida}");
}

#[test]
fn el_autoguardado_en_cero_se_muestra_deshabilitado() {
    let config = ConfigSnapshot {
        persistencia_autosave_secs: 0,
        ..ConfigSnapshot::default()
    };

    assert!(format_config(&config).contains("Autoguardado: deshabilitado"));
}

#[test]
fn el_autoguardado_con_intervalo_se_muestra_en_segundos() {
    let config = ConfigSnapshot {
        persistencia_autosave_secs: 300,
        ..ConfigSnapshot::default()
    };

    let salida = format_config(&config);

    assert!(salida.contains("Autoguardado: 300s"));
    assert!(!salida.contains("deshabilitado"));
}

#[test]
fn un_ejecutable_nunca_visto_no_muestra_la_ultima_vez() {
    let estado = ExeState {
        path: "/usr/bin/foo".into(),
        last_seen_time: 0,
        ..ExeState::default()
    };

    let salida = format_exe_state(&estado);

    assert!(salida.contains("Ruta: /usr/bin/foo"));
    assert!(!salida.contains("Última vez visto"), "{salida}");
}

#[test]
fn un_ejecutable_visto_muestra_la_ultima_vez() {
    let estado = ExeState {
        path: "/usr/bin/foo".into(),
        last_seen_time: 120,
        prediction_probability: 0.25,
        ..ExeState::default()
    };

    let salida = format_exe_state(&estado);

    assert!(salida.contains("Última vez visto: 120s"));
    assert!(salida.contains("Probabilidad predicción: 25.00%"));
}
