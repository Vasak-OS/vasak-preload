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

#[test]
fn el_unit_deja_leer_proc_meminfo_y_escribir_el_estado() {
    let unit = read("vasak-preload.service");
    // Directivas efectivas, sin comentarios (una línea puede nombrar una
    // directiva al explicarla sin configurarla).
    let directives: Vec<&str> = unit
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .collect();
    let has = |d: &str| directives.iter().any(|l| *l == d);

    // ProcSubset=pid tapa /proc/meminfo, que el escáner necesita.
    assert!(
        !has("ProcSubset=pid"),
        "ProcSubset=pid oculta /proc/meminfo"
    );
    // Con ProtectSystem=strict hace falta StateDirectory para que
    // /var/lib/vasak-preload sea escribible.
    assert!(
        has("StateDirectory=vasak-preload"),
        "sin StateDirectory el estado queda de sólo lectura"
    );
    // El daemon no usa red.
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
fn hay_politica_de_dbus_que_restringe_el_control_a_root() {
    let policy = read("org.vasakos.Preload.conf");

    // El usuario del servicio puede tomar el nombre en el bus del sistema.
    assert!(
        policy.contains(r#"<allow own="org.vasakos.Preload"/>"#),
        "sin la regla own el daemon no puede tomar el nombre"
    );
    // Los métodos de control no se conceden en el contexto por defecto: no
    // aparecen como send_member fuera de la política de root.
    for control in ["Pause", "Resume", "SaveNow", "ReloadConfig"] {
        assert!(
            !policy.contains(&format!(r#"send_member="{control}""#)),
            "el método de control {control} no debe concederse a cualquiera"
        );
    }
}

#[test]
fn hay_sysusers_que_crea_el_usuario_del_servicio() {
    let sysusers = read("vasak-preload.sysusers");
    assert!(
        sysusers.lines().any(|l| {
            let l = l.trim_start();
            !l.starts_with('#') && l.starts_with("u ") && l.contains("vasak-preload")
        }),
        "falta la línea que crea el usuario vasak-preload"
    );
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
