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

#[test]
fn un_valor_de_otra_forma_no_se_convierte_en_estado_estadisticas_ni_configuracion() {
    let texto = || OwnedValue::try_from(Value::from("Running")).unwrap();

    assert!(DaemonState::try_from(texto()).is_err());
    assert!(DaemonStats::try_from(texto()).is_err());
    assert!(ConfigSnapshot::try_from(texto()).is_err());
}

#[test]
fn las_cuatro_conversiones_de_las_estructuras_van_y_vuelven() {
    // Directo de `Value` y directo a `OwnedValue`, sin pasar por el otro.
    let stats = DaemonStats {
        exe_count: 5,
        ..DaemonStats::default()
    };
    let vuelta = DaemonStats::try_from(Value::from(stats.clone())).unwrap();
    assert_eq!(vuelta.exe_count, 5);
    let owned = OwnedValue::try_from(stats).unwrap();
    assert_eq!(DaemonStats::try_from(owned).unwrap().exe_count, 5);

    let config = ConfigSnapshot {
        modelo_ciclo_secs: 7,
        ..ConfigSnapshot::default()
    };
    let vuelta = ConfigSnapshot::try_from(Value::from(config.clone())).unwrap();
    assert_eq!(vuelta.modelo_ciclo_secs, 7);
    let owned = OwnedValue::try_from(config).unwrap();
    assert_eq!(
        ConfigSnapshot::try_from(owned).unwrap().modelo_ciclo_secs,
        7
    );
}

#[test]
fn un_campo_de_otro_tipo_no_se_convierte() {
    // Seis campos como la configuración, pero el último es texto.
    let config = Value::from((20u64, 3600u64, true, true, true, "300"));
    assert!(ConfigSnapshot::try_from(config.try_clone().unwrap()).is_err());

    let owned = OwnedValue::try_from(config).unwrap();
    assert!(ConfigSnapshot::try_from(owned).is_err());

    // Las estadísticas con el estado como texto en lugar de `u`, y con el
    // último campo (el porcentaje de memoria, `y`) como `u`.
    for malo in [0, 18] {
        let mut estructura = zvariant::StructureBuilder::new();
        for campo in 0..19 {
            estructura = if campo == malo {
                estructura.add_field("x")
            } else if campo == 0 {
                estructura.add_field(1u32)
            } else {
                estructura.add_field(0u32)
            };
        }
        let stats = Value::from(estructura.build().unwrap());
        let owned = OwnedValue::try_from(stats.try_clone().unwrap()).unwrap();
        assert!(DaemonStats::try_from(stats).is_err(), "campo {malo}");
        assert!(DaemonStats::try_from(owned).is_err(), "campo {malo}");
    }
}

/// Estructura de `n` campos `t` (u64). `n` >= 1 (D-Bus no tiene structs vacíos).
fn struct_de(n: usize) -> Value<'static> {
    let mut b = zvariant::StructureBuilder::new();
    for _ in 0..n {
        b = b.add_field(0u64);
    }
    Value::from(b.build().unwrap())
}

#[test]
fn una_estructura_de_otra_forma_devuelve_error_y_no_entra_en_panico() {
    // Un cli de otra versión manda otra cantidad de campos que la esperada.
    // Antes del arreglo, el derive hacía `fields.remove(0)` sin mirar el largo
    // y entraba en pánico con una estructura más corta; ahora se comprueba la
    // cantidad y se devuelve un error, tanto de más como de menos.
    for n in [1usize, 5, 7] {
        let s = struct_de(n);
        let owned = OwnedValue::try_from(s.try_clone().unwrap()).unwrap();
        assert!(ConfigSnapshot::try_from(s).is_err(), "{n} campos");
        assert!(
            ConfigSnapshot::try_from(owned).is_err(),
            "{n} campos (owned)"
        );
    }

    for n in [1usize, 18, 20] {
        let s = struct_de(n);
        let owned = OwnedValue::try_from(s.try_clone().unwrap()).unwrap();
        assert!(DaemonStats::try_from(s).is_err(), "{n} campos");
        assert!(DaemonStats::try_from(owned).is_err(), "{n} campos (owned)");
    }
}
