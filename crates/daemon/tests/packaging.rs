//! Comprueba que los archivos de empaquetado dejan arrancar y trabajar al
//! daemon: sin estas invariantes el servicio arranca pero no puede leer
//! /proc/meminfo, ni escribir el estado, ni tomar el nombre en el bus.

use std::path::PathBuf;

/// Raíz del repo, a partir del manifiesto de este crate (`crates/daemon`).
fn packaging_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("packaging")
}

fn read(name: &str) -> String {
    let path = packaging_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("leyendo {}: {e}", path.display()))
}

/// Las directivas efectivas de la unidad, sin comentarios (una línea puede
/// nombrar una directiva al explicarla sin configurarla).
fn directives() -> Vec<String> {
    read("vasak-preload.service")
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .collect()
}

#[test]
fn el_unit_es_de_usuario_y_no_pide_privilegios() {
    let directives = directives();
    let has = |d: &str| directives.iter().any(|l| l == d);
    let sets = |prefijo: &str| directives.iter().any(|l| l.starts_with(prefijo));

    // Corre como la persona de la sesión: nada de otro usuario ni de capacidades.
    // Con `CAP_SYS_PTRACE` fuera de la mesa (vasak-preload#3), el servicio ve
    // sus propios procesos y no necesita ninguna.
    for prohibida in [
        "User=",
        "Group=",
        "AmbientCapabilities=",
        "CapabilityBoundingSet=",
    ] {
        assert!(
            !sets(prohibida),
            "{prohibida} no va en un servicio de usuario"
        );
    }
    assert!(has("NoNewPrivileges=yes"));
    assert!(has("MemoryDenyWriteExecute=yes"));
}

#[test]
fn el_unit_arranca_con_la_sesion_grafica_y_toma_su_nombre_en_el_bus() {
    let directives = directives();
    let has = |d: &str| directives.iter().any(|l| l == d);

    // Antes de iniciar sesión no se precarga nada (vasak-preload#28): la unidad
    // cuelga de la sesión gráfica, no de multi-user.target.
    assert!(has("WantedBy=graphical-session.target"));
    assert!(has("PartOf=graphical-session.target"));
    assert!(!has("WantedBy=multi-user.target"));
    // `Type=dbus` espera a que el servicio tome el nombre en el bus de la sesión.
    assert!(has("Type=dbus"));
    assert!(has("BusName=org.vasakos.Preload"));
}

#[test]
fn el_unit_deja_leer_proc_meminfo_y_el_home() {
    let directives = directives();
    let has = |d: &str| directives.iter().any(|l| l == d);

    // ProcSubset=pid tapa /proc/meminfo, que el escáner necesita.
    assert!(
        !has("ProcSubset=pid"),
        "ProcSubset=pid oculta /proc/meminfo"
    );
    // El modelo vive en ~/.local/state y lo que se precarga puede estar en
    // /home: ProtectHome lo taparía.
    assert!(
        !directives.iter().any(|l| l.starts_with("ProtectHome=")),
        "ProtectHome oculta el estado y los binarios del home"
    );
    // No hay condición sobre un archivo de /etc: el servicio arranca sin
    // configuración (los valores por omisión alcanzan).
    assert!(
        !directives
            .iter()
            .any(|l| l.starts_with("ConditionPathExists=")),
        "sin /etc/vasak-preload/config.toml el servicio no arrancaba"
    );
    // ni toma la configuración de un solo archivo: lee /etc y ~/.config.
    let exec = directives
        .iter()
        .find(|l| l.starts_with("ExecStart="))
        .expect("falta ExecStart");
    assert_eq!(exec, "ExecStart=/usr/bin/vasak-preload");
}

#[test]
fn el_unit_no_usa_red_y_apunta_a_la_organizacion() {
    let directives = directives();
    assert!(
        !directives
            .iter()
            .any(|l| l.starts_with("After=network.target")),
        "After=network.target sobra: el daemon no usa red"
    );
    // La organización es Vasak-OS, no VasakOS.
    assert!(
        directives
            .iter()
            .any(|l| l.starts_with("Documentation=https://github.com/Vasak-OS/")),
        "Documentation= apunta a la organización equivocada"
    );
}

#[test]
fn no_quedan_la_politica_de_dbus_ni_el_usuario_de_sistema() {
    // En el bus de sesión sólo entra la misma persona: la política que dejaba el
    // control a root y el usuario `vasak-preload` eran del daemon de sistema.
    for retirado in ["org.vasakos.Preload.conf", "vasak-preload.sysusers"] {
        assert!(
            !packaging_dir().join(retirado).exists(),
            "{retirado} era del daemon de sistema y ya no va"
        );
    }
}

#[test]
fn la_licencia_esta_definida_y_es_la_del_codigo_del_que_deriva() {
    let license = read("../LICENSE");
    assert!(
        license.contains("GNU GENERAL PUBLIC LICENSE") && license.contains("Version 3"),
        "LICENSE debe ser GPL-3.0 (deriva de vasak-monitor, GPL-3.0)"
    );

    let cargo = read("../Cargo.toml");
    assert!(
        cargo.contains(r#"license = "GPL-3.0-only""#),
        "Cargo.toml debe declarar GPL-3.0-only, no Apache-2.0"
    );
}
