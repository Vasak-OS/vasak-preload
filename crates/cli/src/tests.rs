//! Lo que imprime `vasak-preload-cli` a partir de lo que llega por D-Bus.
//!
//! D-Bus no tiene `Option`: el protocolo marca la ausencia con valores
//! centinela (0 o 255), y el cliente no tiene que mostrarlos como datos.

use crate::{Commands, format_config, format_exe_state, format_stats, run};
use std::sync::{Arc, Mutex};
use tokio::net::UnixStream;
use vasak_preload_protocol::{
    ConfigSnapshot, DaemonState, DaemonStats, ExeState, ExeStateWrapper, PreloadProxy,
};
use zbus::proxy::CacheProperties;
use zbus::{Connection, Guid, connection, interface};

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

/// Un daemon falso con la misma interfaz: responde valores fijos y anota qué
/// métodos le llamaron.
struct FakePreload {
    calls: Arc<Mutex<Vec<&'static str>>>,
    /// Si `SaveNow` responde como un daemon sin motor.
    save_fails: bool,
}

impl FakePreload {
    fn record(&self, method: &'static str) {
        self.calls.lock().unwrap().push(method);
    }
}

#[interface(name = "org.vasakos.Preload1")]
impl FakePreload {
    #[zbus(property)]
    async fn state(&self) -> DaemonState {
        DaemonState::Paused
    }

    #[zbus(property)]
    async fn stats(&self) -> DaemonStats {
        DaemonStats {
            exe_count: 9,
            ..DaemonStats::default()
        }
    }

    #[zbus(property)]
    async fn config(&self) -> ConfigSnapshot {
        ConfigSnapshot {
            modelo_ciclo_secs: 33,
            ..ConfigSnapshot::default()
        }
    }

    async fn pause(&self) {
        self.record("Pause");
    }

    async fn resume(&self) {
        self.record("Resume");
    }

    async fn save_now(&self) -> zbus::fdo::Result<()> {
        self.record("SaveNow");
        if self.save_fails {
            return Err(zbus::fdo::Error::Failed(
                "el motor no está corriendo".into(),
            ));
        }
        Ok(())
    }

    async fn reload_config(&self) {
        self.record("ReloadConfig");
    }

    async fn get_top_predictions(&self, n: u32) -> Vec<(String, f32)> {
        vec![
            ("/usr/bin/bash".to_string(), 0.5),
            ("/usr/bin/vim".to_string(), 0.25),
        ]
        .into_iter()
        .take(n as usize)
        .collect()
    }

    async fn get_exe_state(&self, path: String) -> ExeStateWrapper {
        if path == "/usr/bin/bash" {
            ExeStateWrapper::some(ExeState {
                path,
                total_running_time: 40,
                ..ExeState::default()
            })
        } else {
            ExeStateWrapper::none()
        }
    }
}

struct Daemon {
    _servidor: Connection,
    _cliente: Connection,
    proxy: PreloadProxy<'static>,
    calls: Arc<Mutex<Vec<&'static str>>>,
}

/// Sirve `FakePreload` en un extremo de un par de sockets peer-to-peer y
/// conecta el proxy del cli en el otro.
async fn falso() -> Daemon {
    falso_con(false).await
}

async fn falso_con(save_fails: bool) -> Daemon {
    let (lado_servidor, lado_cliente) = UnixStream::pair().expect("par de sockets");
    let calls = Arc::new(Mutex::new(Vec::new()));

    let servidor = connection::Builder::unix_stream(lado_servidor)
        .server(Guid::generate())
        .expect("guid")
        .p2p()
        .serve_at(
            "/org/vasakos/Preload",
            FakePreload {
                calls: calls.clone(),
                save_fails,
            },
        )
        .expect("publicar")
        .build();
    let cliente = connection::Builder::unix_stream(lado_cliente).p2p().build();
    let (servidor, cliente) = tokio::join!(servidor, cliente);
    let (servidor, cliente) = (servidor.expect("servidor"), cliente.expect("cliente"));

    let proxy = PreloadProxy::builder(&cliente)
        .cache_properties(CacheProperties::No)
        .build()
        .await
        .expect("proxy");

    Daemon {
        _servidor: servidor,
        _cliente: cliente,
        proxy,
        calls,
    }
}

#[tokio::test]
async fn status_muestra_el_estado_del_daemon() {
    let daemon = falso().await;
    let salida = run(&daemon.proxy, Commands::Status).await.unwrap();
    assert_eq!(salida, "Estado: Paused\n");
}

#[tokio::test]
async fn stats_y_config_muestran_lo_que_publica_el_daemon() {
    let daemon = falso().await;

    let stats = run(&daemon.proxy, Commands::Stats).await.unwrap();
    assert!(stats.contains("Ejecutables rastreados: 9"), "{stats}");

    let config = run(&daemon.proxy, Commands::Config).await.unwrap();
    assert!(config.contains("Modelo ciclo: 33s"), "{config}");
}

#[tokio::test]
async fn los_comandos_de_control_llaman_a_su_metodo_sin_imprimir() {
    let daemon = falso().await;

    for comando in [
        Commands::Pause,
        Commands::Resume,
        Commands::Save,
        Commands::Reload,
    ] {
        assert_eq!(run(&daemon.proxy, comando).await.unwrap(), "");
    }
    assert_eq!(
        *daemon.calls.lock().unwrap(),
        vec!["Pause", "Resume", "SaveNow", "ReloadConfig"]
    );
}

#[tokio::test]
async fn un_error_del_daemon_llega_a_quien_corre_el_comando() {
    let daemon = falso_con(true).await;

    let error = run(&daemon.proxy, Commands::Save).await.unwrap_err();
    assert!(
        error.to_string().contains("el motor no está corriendo"),
        "{error}"
    );
    assert_eq!(*daemon.calls.lock().unwrap(), vec!["SaveNow"]);
}

#[tokio::test]
async fn top_numera_las_predicciones_en_porcentaje() {
    let daemon = falso().await;

    let salida = run(&daemon.proxy, Commands::Top { n: 2 }).await.unwrap();
    assert_eq!(
        salida,
        "Top 2 predicciones:\n  1. /usr/bin/bash (50.00%)\n  2. /usr/bin/vim (25.00%)\n"
    );
}

#[tokio::test]
async fn exe_muestra_un_ejecutable_conocido() {
    let daemon = falso().await;

    let salida = run(
        &daemon.proxy,
        Commands::Exe {
            path: "/usr/bin/bash".into(),
        },
    )
    .await
    .unwrap();
    assert!(salida.contains("Ruta: /usr/bin/bash"), "{salida}");
    assert!(salida.contains("Tiempo total: 40s"), "{salida}");
}

#[tokio::test]
async fn exe_avisa_cuando_el_ejecutable_no_esta_en_el_modelo() {
    let daemon = falso().await;

    let salida = run(
        &daemon.proxy,
        Commands::Exe {
            path: "/usr/bin/nada".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(salida, "Ejecutable no encontrado: /usr/bin/nada\n");
}

#[tokio::test]
async fn tick_avisa_que_no_existe_por_d_bus() {
    let daemon = falso().await;
    let salida = run(&daemon.proxy, Commands::Tick).await.unwrap();
    assert!(salida.contains("no implementado"), "{salida}");
}
