//! Los tipos que el daemon publica como propiedades D-Bus tienen que poder
//! convertirse a `Value` y volver desde `OwnedValue`: es lo que zbus 5 exige
//! para una `#[zbus(property)]`, del lado del servicio y del lado del proxy.

use vasak_preload_protocol::{ConfigSnapshot, DaemonState, DaemonStats};
use zvariant::{OwnedValue, Type, Value};

const ESTADOS: [(DaemonState, u32); 4] = [
    (DaemonState::Starting, 0),
    (DaemonState::Running, 1),
    (DaemonState::Paused, 2),
    (DaemonState::Stopping, 3),
];

#[test]
fn el_estado_del_daemon_viaja_como_un_u32() {
    assert_eq!(DaemonState::SIGNATURE.to_string(), "u");
    for (estado, numero) in ESTADOS {
        assert_eq!(Value::from(estado), Value::U32(numero), "{estado:?}");
    }
}

#[test]
fn el_estado_del_daemon_ida_y_vuelta_por_owned_value() {
    for (estado, _) in ESTADOS {
        let owned = OwnedValue::try_from(Value::from(estado)).expect("a OwnedValue");
        assert_eq!(
            DaemonState::try_from(owned).expect("desde OwnedValue"),
            estado
        );
    }
}

#[test]
fn un_u32_fuera_de_rango_no_es_un_estado() {
    let owned = OwnedValue::from(7u32);
    assert!(DaemonState::try_from(owned).is_err());
}

#[test]
fn las_estadisticas_ida_y_vuelta_conservan_cada_campo() {
    let stats = DaemonStats {
        state: DaemonState::Paused,
        scan_count: 11,
        exe_count: 12,
        map_count: 13,
        edge_count: 14,
        active_count: 15,
        model_time: 16,
        last_tick_scan_id: 17,
        last_tick_exes_added: 18,
        last_tick_maps_added: 19,
        last_tick_edges_added: 20,
        last_tick_edges_updated: 21,
        last_tick_prediction_exes: 22,
        last_tick_prediction_max_prob: 0.123_456_789_012_345,
        last_tick_prefetch_attempted: 23,
        last_tick_prefetch_succeeded: 24,
        last_tick_prefetch_failed: 25,
        last_tick_prefetch_bytes: 1 << 40,
        last_tick_mem_free_pct: 255,
    };

    let owned = OwnedValue::try_from(Value::from(stats.clone())).expect("a OwnedValue");
    let vuelta = DaemonStats::try_from(owned).expect("desde OwnedValue");

    assert_eq!(vuelta.state, stats.state);
    assert_eq!(vuelta.scan_count, 11);
    assert_eq!(vuelta.exe_count, 12);
    assert_eq!(vuelta.map_count, 13);
    assert_eq!(vuelta.edge_count, 14);
    assert_eq!(vuelta.active_count, 15);
    assert_eq!(vuelta.model_time, 16);
    assert_eq!(vuelta.last_tick_scan_id, 17);
    assert_eq!(vuelta.last_tick_exes_added, 18);
    assert_eq!(vuelta.last_tick_maps_added, 19);
    assert_eq!(vuelta.last_tick_edges_added, 20);
    assert_eq!(vuelta.last_tick_edges_updated, 21);
    assert_eq!(vuelta.last_tick_prediction_exes, 22);
    // En el cable es un `d`: la probabilidad no pierde precisión al ir y volver.
    assert_eq!(vuelta.last_tick_prediction_max_prob, 0.123_456_789_012_345);
    assert_eq!(vuelta.last_tick_prefetch_attempted, 23);
    assert_eq!(vuelta.last_tick_prefetch_succeeded, 24);
    assert_eq!(vuelta.last_tick_prefetch_failed, 25);
    assert_eq!(vuelta.last_tick_prefetch_bytes, 1 << 40);
    assert_eq!(vuelta.last_tick_mem_free_pct, 255);
}

#[test]
fn las_estadisticas_se_firman_como_una_estructura_de_d_bus() {
    assert_eq!(DaemonStats::SIGNATURE.to_string(), "(utuuuuttuuuuuduuuty)");
}

#[test]
fn la_configuracion_ida_y_vuelta_conserva_cada_campo() {
    let config = ConfigSnapshot {
        modelo_ciclo_secs: 20,
        modelo_ventana_activa_secs: 3600,
        sistema_doscan: true,
        sistema_dopredict: false,
        sistema_doprefetch: true,
        persistencia_autosave_secs: 0,
    };

    let owned = OwnedValue::try_from(Value::from(config.clone())).expect("a OwnedValue");
    let vuelta = ConfigSnapshot::try_from(owned).expect("desde OwnedValue");

    assert_eq!(vuelta.modelo_ciclo_secs, 20);
    assert_eq!(vuelta.modelo_ventana_activa_secs, 3600);
    assert!(vuelta.sistema_doscan);
    assert!(!vuelta.sistema_dopredict);
    assert!(vuelta.sistema_doprefetch);
    assert_eq!(vuelta.persistencia_autosave_secs, 0);
    assert_eq!(ConfigSnapshot::SIGNATURE.to_string(), "(ttbbbt)");
}
