//! La interfaz D-Bus del daemon, de punta a punta, en un bus privado.
//!
//! Servicio y cliente se hablan por un par de sockets (`UnixStream::pair`) en
//! modo peer-to-peer: no hace falta bus del sistema ni root. El cliente usa el
//! mismo `PreloadProxy` que `vasak-preload-cli`.

use crate::service::{ConfigSource, EngineTask, PreloadService, planner_from, stats_from_view};
use crate::{Args, OBJECT_PATH, config_source, run, serve, wait_for_shutdown};
use clap::Parser;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;
use tokio::net::UnixStream;
use tokio::time::timeout;
use vasak_preload_config::Config;
use vasak_preload_orchestrator::{
    CandidateExe, EngineView, Error as EngineError, ExeKey, MapSegment, MemStat, ModelDelta,
    Observation, ObservationEvent, Prediction, PredictionSummary, PrefetchPlanner, PrefetchReport,
    ProcfsScanner, Scanner, SqliteRepository, StateRepository, Stores, TickReport,
};
use vasak_preload_protocol::{DaemonState, PreloadProxy};
use zbus::proxy::CacheProperties;
use zbus::{Connection, Guid, connection};

/// Margen para que el motor se apague. Ajustado: el `select!` de `run_until` es
/// `biased` con la cancelación primero y el sueño entre ciclos cede ante ella,
/// así que el apagado no espera al ciclo. 3 s cubre la lentitud del CI.
const APAGADO_MAXIMO: Duration = Duration::from_secs(3);

/// `model_time` que ningún motor de estas pruebas alcanza: se escribe en el
/// estado en disco para saber si después lo pisó un guardado del motor.
const MARCA: u64 = 999_999;

/// Configuración que no depende de la máquina: no escanea `/proc`, no predice
/// ni hace prefetch, y guarda el estado en un directorio temporal.
fn config_de_prueba(dir: &Path) -> Config {
    let mut config = Config::default();
    config.sistema.doscan = false;
    config.sistema.dopredict = false;
    config.sistema.doprefetch = false;
    config.modelo.ciclo = Duration::from_secs(1);
    config.persistencia.estado_path = dir.join("state.sqlite");
    config
}

struct Daemon {
    // El servicio deja de atender cuando se suelta su conexión.
    _servidor: Connection,
    // El cliente tiene que vivir tanto como el proxy.
    _cliente: Connection,
    proxy: PreloadProxy<'static>,
    motor: EngineTask,
}

/// Publica el daemon con `serve` —lo mismo que hace `main` sobre el bus del
/// sistema— en un extremo del par, y conecta el proxy en el otro.
async fn start_daemon(config: Config) -> Daemon {
    start_daemon_with(
        PreloadService::new(config, ConfigSource::Default)
            .await
            .expect("servicio"),
    )
    .await
}

/// Como `start_daemon`, con un servicio ya armado (por ejemplo, con otros archivos
/// de configuración para `ReloadConfig`).
async fn start_daemon_with(service: PreloadService) -> Daemon {
    start_daemon_scanning(service, Box::new(ProcfsScanner::new())).await
}

/// Como `start_daemon_with`, con el escáner a elegir.
async fn start_daemon_scanning(
    service: PreloadService,
    scanner: Box<dyn Scanner + Send + Sync>,
) -> Daemon {
    let (lado_servidor, lado_cliente) = UnixStream::pair().expect("par de sockets");

    let servidor = connection::Builder::unix_stream(lado_servidor)
        .server(Guid::generate())
        .expect("guid")
        .p2p();
    let cliente = connection::Builder::unix_stream(lado_cliente).p2p().build();

    let (servido, cliente) = tokio::join!(serve(servidor, service, scanner), cliente);
    let (servidor, motor) = servido.expect("publicar el servicio y arrancar el motor");
    let cliente = cliente.expect("conectar el cliente");

    // Sin bus no hay `AddMatch` para seguir PropertiesChanged: se leen siempre.
    let proxy = PreloadProxy::builder(&cliente)
        .cache_properties(CacheProperties::No)
        .build()
        .await
        .expect("proxy");

    Daemon {
        _servidor: servidor,
        _cliente: cliente,
        proxy,
        motor,
    }
}

/// Publica el servicio en un extremo del par **sin arrancar el motor**: el
/// tramo entre `serve_at` y `start_engine`, en el que la interfaz ya atiende.
async fn publicar_sin_motor(config: Config) -> (Connection, Connection, PreloadProxy<'static>) {
    let (lado_servidor, lado_cliente) = UnixStream::pair().expect("par de sockets");

    let servicio = PreloadService::new(config, ConfigSource::Default)
        .await
        .expect("servicio");
    let servidor = connection::Builder::unix_stream(lado_servidor)
        .server(Guid::generate())
        .expect("guid")
        .p2p()
        .serve_at(OBJECT_PATH, servicio)
        .expect("publicar")
        .build();
    let cliente = connection::Builder::unix_stream(lado_cliente).p2p().build();

    let (servidor, cliente) = tokio::join!(servidor, cliente);
    let servidor = servidor.expect("publicar el servicio");
    let cliente = cliente.expect("conectar el cliente");

    let proxy = PreloadProxy::builder(&cliente)
        .cache_properties(CacheProperties::No)
        .build()
        .await
        .expect("proxy");

    (servidor, cliente, proxy)
}

/// El error tiene que ser `org.freedesktop.DBus.Error.Failed`.
fn es_failed(error: &zbus::Error) -> bool {
    matches!(
        error,
        zbus::Error::MethodError(nombre, _, _)
            if nombre.as_str() == "org.freedesktop.DBus.Error.Failed"
    )
}

/// Deja `MARCA` como `model_time` del estado en disco.
async fn marcar(repo: &SqliteRepository) {
    let mut snapshot = repo.load().await.unwrap();
    snapshot.state.model_time = MARCA;
    repo.save(&snapshot).await.unwrap();
}

async fn model_time_en_disco(repo: &SqliteRepository) -> u64 {
    repo.load().await.unwrap().state.model_time
}

async fn apagar(motor: EngineTask) {
    timeout(APAGADO_MAXIMO, motor.shutdown())
        .await
        .expect("el motor no terminó al pedirle que se apague");
}

#[tokio::test]
async fn la_interfaz_publicada_ve_el_motor_que_corre() {
    let dir = TempDir::new().unwrap();
    let daemon = start_daemon(config_de_prueba(dir.path())).await;

    // Con dos instancias del servicio (una publicada, otra con el motor), la
    // publicada se quedaba en `Starting` para siempre.
    assert_eq!(daemon.proxy.state().await.unwrap(), DaemonState::Running);
    assert_eq!(
        daemon.proxy.stats().await.unwrap().state,
        DaemonState::Running
    );

    apagar(daemon.motor).await;
}

#[tokio::test]
async fn las_estadisticas_llegan_por_d_bus_con_sus_valores_de_ausencia() {
    // Sin motor no hay ticks: con uno corriendo, el primero ya los llena.
    let dir = TempDir::new().unwrap();
    let (_servidor, _cliente, proxy) = publicar_sin_motor(config_de_prueba(dir.path())).await;

    let stats = proxy.stats().await.unwrap();
    assert_eq!(stats.last_tick_scan_id, 0, "0 = todavía sin ticks");
    assert_eq!(
        stats.last_tick_mem_free_pct, 255,
        "255 = sin dato de memoria"
    );
    assert_eq!(stats.last_tick_prediction_max_prob, 0.0);
}

#[tokio::test]
async fn la_configuracion_llega_por_d_bus() {
    let dir = TempDir::new().unwrap();
    let mut config = config_de_prueba(dir.path());
    config.modelo.ventana_activa = Duration::from_secs(1234);
    config.sistema.dopredict = true;
    config.persistencia.autosave_interval = Some(Duration::from_secs(42));
    let daemon = start_daemon(config).await;

    let snapshot = daemon.proxy.config().await.unwrap();
    assert_eq!(snapshot.modelo_ciclo_secs, 1);
    assert_eq!(snapshot.modelo_ventana_activa_secs, 1234);
    assert!(!snapshot.sistema_doscan);
    assert!(snapshot.sistema_dopredict);
    assert!(!snapshot.sistema_doprefetch);
    assert_eq!(snapshot.persistencia_autosave_secs, 42);

    apagar(daemon.motor).await;
}

#[tokio::test]
async fn sin_intervalo_propio_la_configuracion_informa_el_autoguardado_del_sistema() {
    // `persistencia.autosave_interval = None` quiere decir «usar el del
    // sistema», y el motor autoguarda con ese. Publicar 0 hacía que
    // `vasak-preload-cli config` dijera «deshabilitado» con el autoguardado
    // andando cada cinco minutos.
    let dir = TempDir::new().unwrap();
    let mut config = config_de_prueba(dir.path());
    config.persistencia.autosave_interval = None;
    config.sistema.autosave = Duration::from_secs(300);
    let daemon = start_daemon(config).await;

    let snapshot = daemon.proxy.config().await.unwrap();
    assert_eq!(snapshot.persistencia_autosave_secs, 300);

    apagar(daemon.motor).await;
}

#[tokio::test]
async fn un_ejecutable_desconocido_vuelve_sin_valor() {
    let dir = TempDir::new().unwrap();
    let daemon = start_daemon(config_de_prueba(dir.path())).await;

    let estado = daemon
        .proxy
        .get_exe_state("/usr/bin/no-existe-en-el-modelo")
        .await
        .unwrap();
    assert!(!estado.has_value);
    assert!(estado.inner.path.is_empty());

    apagar(daemon.motor).await;
}

#[tokio::test]
async fn pausar_y_reanudar_se_ven_en_el_estado() {
    let dir = TempDir::new().unwrap();
    let daemon = start_daemon(config_de_prueba(dir.path())).await;

    daemon.proxy.pause().await.unwrap();
    assert_eq!(daemon.proxy.state().await.unwrap(), DaemonState::Paused);
    daemon.proxy.resume().await.unwrap();
    assert_eq!(daemon.proxy.state().await.unwrap(), DaemonState::Running);

    apagar(daemon.motor).await;
}

#[tokio::test]
async fn guardar_ahora_persiste_el_estado_del_motor() {
    let dir = TempDir::new().unwrap();
    let config = config_de_prueba(dir.path());
    let repo = SqliteRepository::new(&config.persistencia.estado_path)
        .await
        .unwrap();
    let daemon = start_daemon(config).await;
    marcar(&repo).await;

    daemon.proxy.save_now().await.unwrap();

    // El guardado lo hace el motor en su tarea, después de responder: se
    // espera a que pise la marca.
    timeout(APAGADO_MAXIMO, async {
        while model_time_en_disco(&repo).await == MARCA {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("SaveNow no llegó a guardar el estado del motor");

    apagar(daemon.motor).await;
}

#[tokio::test]
async fn el_apagado_termina_el_motor_y_guarda() {
    let dir = TempDir::new().unwrap();
    let config = config_de_prueba(dir.path());
    let repo = SqliteRepository::new(&config.persistencia.estado_path)
        .await
        .unwrap();
    let daemon = start_daemon(config).await;
    marcar(&repo).await;

    apagar(daemon.motor).await;

    // `save_on_shutdown` viene encendido por omisión: al volver `shutdown`
    // el guardado final ya está en disco.
    assert_ne!(model_time_en_disco(&repo).await, MARCA);

    // Con el motor terminado nadie recibe eventos: SaveNow tiene que fallar
    // con un error de D-Bus, no colgarse ni responder bien.
    let error = daemon.proxy.save_now().await.unwrap_err();
    match error {
        zbus::Error::MethodError(nombre, mensaje, _) => {
            assert_eq!(nombre.as_str(), "org.freedesktop.DBus.Error.Failed");
            assert_eq!(mensaje.as_deref(), Some("el motor no está corriendo"));
        }
        otro => panic!("se esperaba org.freedesktop.DBus.Error.Failed, llegó {otro:?}"),
    }
}

#[tokio::test]
async fn guardar_ahora_antes_de_arrancar_el_motor_falla() {
    let dir = TempDir::new().unwrap();
    let (_servidor, _cliente, proxy) = publicar_sin_motor(config_de_prueba(dir.path())).await;

    // Nadie va a guardar nada: responder Ok sería mentirle a quien pidió el
    // guardado.
    let error = proxy.save_now().await.unwrap_err();
    assert!(
        es_failed(&error),
        "se esperaba Error.Failed, llegó {error:?}"
    );
    match error {
        zbus::Error::MethodError(_, mensaje, _) => {
            assert_eq!(mensaje.as_deref(), Some("el motor no está corriendo"));
        }
        otro => panic!("se esperaba un error de método, llegó {otro:?}"),
    }
}

#[tokio::test]
async fn recargar_antes_de_arrancar_el_motor_no_cambia_la_configuracion() {
    let dir = TempDir::new().unwrap();
    let mut config = config_de_prueba(dir.path());
    // Un valor que no trae ninguna configuración real: si la recarga la
    // reemplazara, se notaría.
    config.modelo.ventana_activa = Duration::from_secs(4321);
    let (_servidor, _cliente, proxy) = publicar_sin_motor(config).await;

    // `ReloadConfig` lee `/etc/vasak-preload/config.toml` y la del usuario. Si
    // la de la máquina es inválida falla antes, también con Error.Failed; en
    // los dos casos la configuración publicada tiene que quedar igual.
    let error = proxy.reload_config().await.unwrap_err();
    assert!(
        es_failed(&error),
        "se esperaba Error.Failed, llegó {error:?}"
    );
    assert_eq!(
        proxy.config().await.unwrap().modelo_ventana_activa_secs,
        4321
    );
}

/// Puebla los almacenes que lee la interfaz: `/usr/bin/bash` corriendo, activo
/// y con dos mapas; `/usr/bin/vim` conocido pero inactivo.
async fn poblar_almacenes(servidor: &Connection) {
    let servicio = servidor
        .object_server()
        .interface::<_, PreloadService>(OBJECT_PATH)
        .await
        .expect("servicio publicado");
    let view = servicio.get().await.view();
    let mut view = view.write().await;
    let stores = &mut view.stores;

    let bash = stores.ensure_exe(ExeKey::new("/usr/bin/bash".into()));
    let vim = stores.ensure_exe(ExeKey::new("/usr/bin/vim".into()));
    let exe = stores.exes.get_mut(bash).unwrap();
    exe.mark_running(490);
    exe.total_running_time = 40;
    stores.exes.get_mut(vim).unwrap().total_running_time = 3;
    let libc = stores.ensure_map(MapSegment::new("/usr/lib/libc.so.6".into(), 0, 4096, 1));
    let ld = stores.ensure_map(MapSegment::new("/usr/lib/ld-linux.so.2".into(), 0, 2048, 1));
    stores.attach_map(bash, libc);
    stores.attach_map(bash, ld);
    stores.active.update(&[bash], 490);
}

#[tokio::test]
async fn las_predicciones_nombran_los_ejecutables_por_su_ruta() {
    let dir = TempDir::new().unwrap();
    let (servidor, _cliente, proxy) = publicar_sin_motor(config_de_prueba(dir.path())).await;
    poblar_almacenes(&servidor).await;

    // Sólo bash está activo: el predictor le da la probabilidad base.
    let top = proxy.get_top_predictions(5).await.unwrap();
    assert_eq!(top, vec![("/usr/bin/bash".to_string(), 0.8)]);
    assert!(proxy.get_top_predictions(0).await.unwrap().is_empty());
}

#[tokio::test]
async fn un_ejecutable_conocido_vuelve_con_su_estado() {
    let dir = TempDir::new().unwrap();
    let (servidor, _cliente, proxy) = publicar_sin_motor(config_de_prueba(dir.path())).await;
    poblar_almacenes(&servidor).await;

    let bash = proxy.get_exe_state("/usr/bin/bash").await.unwrap();
    assert!(bash.has_value);
    assert_eq!(bash.inner.path, "/usr/bin/bash");
    assert!(bash.inner.running);
    assert_eq!(bash.inner.total_running_time, 40);
    assert_eq!(bash.inner.last_seen_time, 490);
    assert_eq!(bash.inner.map_count, 2);
    assert_eq!(bash.inner.prediction_probability, 0.8);

    // Conocido pero nunca visto: 0 en `last_seen_time`, sin mapas ni
    // probabilidad.
    let vim = proxy.get_exe_state("/usr/bin/vim").await.unwrap();
    assert!(vim.has_value);
    assert!(!vim.inner.running);
    assert_eq!(vim.inner.total_running_time, 3);
    assert_eq!(vim.inner.last_seen_time, 0);
    assert_eq!(vim.inner.map_count, 0);
    assert_eq!(vim.inner.prediction_probability, 0.0);
}

/// Corre `run` —lo que hace `main` después de cargar la configuración— en un
/// extremo del par, con `resultado_espera` en lugar de las señales. Mientras
/// tanto, del otro lado, comprueba que el daemon atiende, marca el estado en
/// disco y recién entonces da la señal. Devuelve lo que devolvió `run`.
async fn correr_hasta_el_apagado(
    config: Config,
    resultado_espera: std::io::Result<()>,
) -> Result<(), String> {
    let repo = SqliteRepository::new(&config.persistencia.estado_path)
        .await
        .unwrap();
    let (lado_servidor, lado_cliente) = UnixStream::pair().expect("par de sockets");
    let servidor = connection::Builder::unix_stream(lado_servidor)
        .server(Guid::generate())
        .expect("guid")
        .p2p();
    let (senal, apagado) = tokio::sync::oneshot::channel();

    let daemon = async {
        run(servidor, config, ConfigSource::Default, async {
            apagado.await.expect("emisor vivo")
        })
        .await
        .map_err(|e| e.to_string())
    };
    let cliente = async {
        let cliente = connection::Builder::unix_stream(lado_cliente)
            .p2p()
            .build()
            .await
            .expect("conectar el cliente");
        let proxy = PreloadProxy::builder(&cliente)
            .cache_properties(CacheProperties::No)
            .build()
            .await
            .expect("proxy");
        // La interfaz atiende desde `serve_at`, antes de que el motor
        // arranque: se espera a verlo corriendo.
        while proxy.state().await.unwrap() != DaemonState::Running {
            tokio::task::yield_now().await;
        }
        marcar(&repo).await;
        senal.send(resultado_espera).unwrap();
        cliente
    };

    let (resultado, _cliente) = timeout(APAGADO_MAXIMO, async { tokio::join!(daemon, cliente) })
        .await
        .expect("run no volvió después de la señal");

    // `run` no vuelve hasta que el motor hizo su guardado final.
    assert_ne!(model_time_en_disco(&repo).await, MARCA);
    resultado
}

#[tokio::test]
async fn el_daemon_atiende_hasta_el_apagado_y_guarda_al_salir() {
    let dir = TempDir::new().unwrap();
    let resultado = correr_hasta_el_apagado(config_de_prueba(dir.path()), Ok(())).await;
    assert_eq!(resultado, Ok(()));
}

#[tokio::test]
async fn si_esperar_la_senal_falla_el_motor_igual_se_apaga() {
    let dir = TempDir::new().unwrap();
    let resultado = correr_hasta_el_apagado(
        config_de_prueba(dir.path()),
        Err(std::io::Error::other("sin manejador de señales")),
    )
    .await;
    assert_eq!(resultado, Err("sin manejador de señales".to_string()));
}

/// Hace que `espera` registre sus manejadores (el primer `poll` llama a
/// `signal`) y le manda `senal` al propio proceso.
async fn senal_a_mi_mismo(
    espera: &mut std::pin::Pin<&mut impl Future<Output = std::io::Result<()>>>,
    senal: nix::sys::signal::Signal,
) {
    tokio::select! {
        biased;
        _ = espera.as_mut() => panic!("terminó sin ninguna señal"),
        _ = std::future::ready(()) => {}
    }
    nix::sys::signal::kill(nix::unistd::Pid::this(), senal).expect("kill");
}

#[tokio::test]
async fn sigterm_y_sigint_terminan_la_espera_de_apagado() {
    // Las dos en la misma prueba: los manejadores son de todo el proceso, y
    // una señal mandada por otra prueba en paralelo despertaría a ésta.
    // Antes del primer `kill` hay un manejador de tokio instalado para cada
    // señal, así que ninguna puede matar al proceso de pruebas.
    let espera = wait_for_shutdown();
    tokio::pin!(espera);
    senal_a_mi_mismo(&mut espera, nix::sys::signal::Signal::SIGTERM).await;
    timeout(Duration::from_secs(10), espera)
        .await
        .expect("SIGTERM no terminó la espera")
        .unwrap();

    let espera = wait_for_shutdown();
    tokio::pin!(espera);
    senal_a_mi_mismo(&mut espera, nix::sys::signal::Signal::SIGINT).await;
    timeout(Duration::from_secs(10), espera)
        .await
        .expect("SIGINT no terminó la espera")
        .unwrap();
}

#[tokio::test]
async fn recargar_con_el_motor_corriendo_reemplaza_la_configuracion() {
    // `ReloadConfig` lee los `*.toml` del directorio temporal: uno solo, con
    // ciclo de 1 s (con el de 20 s por omisión el apagado del motor tarda
    // ciclos enteros) y sin `ventana_activa`, que vuelve a su valor por omisión.
    // No depende de la configuración de la máquina.
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("config.toml"), "[modelo]\nciclo = 1\n").unwrap();
    let mut config = config_de_prueba(dir.path());
    config.modelo.ventana_activa = Duration::from_secs(4321);
    let service = PreloadService::new(config, ConfigSource::Dir(dir.path().to_path_buf()))
        .await
        .expect("servicio");
    let daemon = start_daemon_with(service).await;

    daemon.proxy.reload_config().await.unwrap();
    assert_ne!(
        daemon
            .proxy
            .config()
            .await
            .unwrap()
            .modelo_ventana_activa_secs,
        4321
    );

    apagar(daemon.motor).await;
}

/// Escribe en `path` una configuración que no depende de la máquina (sin
/// escaneo, predicción ni prefetch; estado en `state_dir`) con la
/// `ventana_activa` pedida.
fn write_config(path: &Path, state_dir: &Path, ventana_activa: u64) {
    let estado = state_dir.join("state.sqlite");
    std::fs::write(
        path,
        format!(
            "[modelo]\nciclo = 1\nventana_activa = {ventana_activa}\n\
             [sistema]\ndoscan = false\ndopredict = false\ndoprefetch = false\n\
             [persistencia]\nestado_path = {estado:?}\n"
        ),
    )
    .unwrap();
}

/// Arranca el daemon como `main`: lee la configuración de `source` y le pasa
/// la misma fuente al servicio para las recargas.
async fn start_daemon_from(source: ConfigSource) -> Daemon {
    let config = source.load().expect("configuración inicial");
    start_daemon_with(PreloadService::new(config, source).await.expect("servicio")).await
}

async fn published_window(daemon: &Daemon) -> u64 {
    daemon
        .proxy
        .config()
        .await
        .unwrap()
        .modelo_ventana_activa_secs
}

#[tokio::test]
async fn recargar_relee_el_archivo_elegido_con_config() {
    let dir = TempDir::new().unwrap();
    let archivo = dir.path().join("elegido.toml");
    write_config(&archivo, dir.path(), 111);
    let daemon = start_daemon_from(ConfigSource::File(archivo.clone())).await;
    assert_eq!(published_window(&daemon).await, 111);

    // Quien arrancó con `--config` edita ese archivo y pide recargar: tiene
    // que verse lo que escribió, no lo de `/etc` ni lo del usuario.
    write_config(&archivo, dir.path(), 222);
    daemon.proxy.reload_config().await.unwrap();
    assert_eq!(published_window(&daemon).await, 222);

    apagar(daemon.motor).await;
}

#[tokio::test]
async fn recargar_relee_el_directorio_y_el_orden_alfabetico_decide() {
    let dir = TempDir::new().unwrap();
    let conf = dir.path().join("conf.d");
    std::fs::create_dir(&conf).unwrap();
    write_config(&conf.join("10-base.toml"), dir.path(), 111);
    let daemon = start_daemon_from(ConfigSource::Dir(conf.clone())).await;
    assert_eq!(published_window(&daemon).await, 111);

    // Dos archivos nuevos, uno antes y otro después de `10-base` en orden
    // alfabético. El de antes queda pisado; el de después pisa.
    std::fs::write(
        conf.join("05-antes.toml"),
        "[modelo]\nciclo = 7\nventana_activa = 999\n",
    )
    .unwrap();
    std::fs::write(
        conf.join("20-despues.toml"),
        "[modelo]\nventana_activa = 222\n[persistencia]\nautosave_interval = 42\n",
    )
    .unwrap();
    // Lo que no termina en `.toml` no se lee.
    std::fs::write(conf.join("99-notas.txt"), "[modelo]\nventana_activa = 5\n").unwrap();

    daemon.proxy.reload_config().await.unwrap();
    let config = daemon.proxy.config().await.unwrap();
    assert_eq!(config.modelo_ventana_activa_secs, 222);
    assert_eq!(config.modelo_ciclo_secs, 1, "10-base pisa a 05-antes");
    assert_eq!(config.persistencia_autosave_secs, 42);

    apagar(daemon.motor).await;
}

#[tokio::test]
async fn sin_el_archivo_elegido_la_recarga_falla_y_no_vuelve_a_los_valores_por_omision() {
    let dir = TempDir::new().unwrap();
    let archivo = dir.path().join("elegido.toml");
    write_config(&archivo, dir.path(), 111);
    let daemon = start_daemon_from(ConfigSource::File(archivo.clone())).await;

    std::fs::remove_file(&archivo).unwrap();
    let error = daemon.proxy.reload_config().await.unwrap_err();
    assert!(
        es_failed(&error),
        "se esperaba Error.Failed, llegó {error:?}"
    );
    assert_eq!(published_window(&daemon).await, 111);

    apagar(daemon.motor).await;
}

fn source_from(args: &[&str]) -> ConfigSource {
    let args = Args::try_parse_from(std::iter::once("vasak-preload").chain(args.iter().copied()))
        .expect("argumentos válidos");
    config_source(&args)
}

#[test]
fn los_argumentos_eligen_de_donde_se_lee_la_configuracion() {
    assert_eq!(
        source_from(&["--config", "/srv/preload.toml"]),
        ConfigSource::File("/srv/preload.toml".into())
    );
    assert_eq!(
        source_from(&["--config-dir", "/srv/preload.d"]),
        ConfigSource::Dir("/srv/preload.d".into())
    );
    assert_eq!(source_from(&[]), ConfigSource::Default);
    // Con los dos, manda el archivo, como antes en `load_config`.
    assert_eq!(
        source_from(&[
            "--config-dir",
            "/srv/preload.d",
            "--config",
            "/srv/preload.toml"
        ]),
        ConfigSource::File("/srv/preload.toml".into())
    );
}

/// Escáner que informa siempre `bash` (con dos mapas) y `vim`, y 50 % de
/// memoria libre: el motor del daemon lo usa en lugar de leer `/proc`.
struct FixedScanner;

impl Scanner for FixedScanner {
    fn scan(&mut self, time: u64, scan_id: u64) -> Result<Observation, EngineError> {
        let map = |path: &str, pid| ObservationEvent::Map {
            segment: MapSegment::new(path.into(), 0, 4096, time),
            pid,
        };
        Ok(vec![
            ObservationEvent::ObsBegin { time, scan_id },
            ObservationEvent::MemStat {
                mem: MemStat {
                    total: 1000,
                    free: 300,
                    cached: 200,
                    available: 500,
                    pagein: 0,
                    pageout: 0,
                },
            },
            ObservationEvent::CandidateExe {
                exe: CandidateExe::new("/usr/bin/bash".into(), 1),
            },
            map("/usr/lib/libc.so.6", 1),
            map("/usr/lib/libreadline.so.8", 1),
            ObservationEvent::CandidateExe {
                exe: CandidateExe::new("/usr/bin/vim".into(), 2),
            },
            ObservationEvent::ObsEnd {
                time,
                scan_id,
                warnings: Vec::new(),
            },
        ])
    }
}

#[tokio::test]
async fn la_interfaz_informa_lo_que_el_motor_observo() {
    let dir = TempDir::new().unwrap();
    let mut config = config_de_prueba(dir.path());
    config.sistema.doscan = true;
    config.sistema.dopredict = true;
    let service = PreloadService::new(config, ConfigSource::Default)
        .await
        .expect("servicio");
    let daemon = start_daemon_scanning(service, Box::new(FixedScanner)).await;

    // El motor publica al terminar cada tick: se espera al primero.
    let stats = timeout(APAGADO_MAXIMO, async {
        loop {
            let stats = daemon.proxy.stats().await.unwrap();
            if stats.last_tick_scan_id != 0 {
                break stats;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("el motor no llegó a publicar un tick");

    assert_eq!(stats.state, DaemonState::Running);
    assert_eq!(stats.exe_count, 2);
    assert_eq!(stats.map_count, 2);
    assert!(stats.scan_count >= 1);
    assert_eq!(stats.last_tick_exes_added, 2);
    assert_eq!(stats.last_tick_maps_added, 2);
    assert_eq!(stats.last_tick_prediction_exes, 2);
    assert!(stats.last_tick_prediction_max_prob > 0.0);
    assert_eq!(stats.last_tick_mem_free_pct, 50);

    let top = daemon.proxy.get_top_predictions(5).await.unwrap();
    let mut paths: Vec<_> = top.iter().map(|(path, _)| path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, ["/usr/bin/bash", "/usr/bin/vim"]);
    assert!(top.iter().all(|(_, prob)| *prob > 0.0));

    let bash = daemon.proxy.get_exe_state("/usr/bin/bash").await.unwrap();
    assert!(bash.has_value);
    assert!(bash.inner.running);
    assert_eq!(bash.inner.map_count, 2);
    assert!(bash.inner.prediction_probability > 0.0);

    apagar(daemon.motor).await;
}

#[tokio::test]
async fn pausar_llega_al_motor_y_reanudar_lo_revierte() {
    let dir = TempDir::new().unwrap();
    let service = PreloadService::new(config_de_prueba(dir.path()), ConfigSource::Default)
        .await
        .expect("servicio");
    let daemon = start_daemon_scanning(service, Box::new(FixedScanner)).await;
    let view = daemon
        ._servidor
        .object_server()
        .interface::<_, PreloadService>(OBJECT_PATH)
        .await
        .unwrap()
        .get()
        .await
        .view();
    let wait_for_paused = |expected: bool| {
        let view = view.clone();
        async move {
            timeout(APAGADO_MAXIMO, async {
                while view.read().await.paused != expected {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("el motor no cambió su pausa")
        }
    };

    daemon.proxy.pause().await.unwrap();
    wait_for_paused(true).await;
    daemon.proxy.resume().await.unwrap();
    wait_for_paused(false).await;

    apagar(daemon.motor).await;
}

#[tokio::test]
async fn pausar_o_reanudar_sin_motor_falla_y_no_cambia_el_estado() {
    let dir = TempDir::new().unwrap();
    let (_servidor, _cliente, proxy) = publicar_sin_motor(config_de_prueba(dir.path())).await;
    assert_eq!(proxy.state().await.unwrap(), DaemonState::Starting);

    for error in [
        proxy.pause().await.unwrap_err(),
        proxy.resume().await.unwrap_err(),
    ] {
        assert!(
            es_failed(&error),
            "se esperaba Error.Failed, llegó {error:?}"
        );
        assert_eq!(proxy.state().await.unwrap(), DaemonState::Starting);
    }
}

fn full_tick() -> TickReport {
    TickReport {
        scan_id: 7,
        model_delta: ModelDelta {
            exes_added: 1,
            maps_added: 2,
            edges_added: 3,
            edges_updated: 4,
        },
        prediction: PredictionSummary {
            total_exes: 5,
            max_probability: 0.5,
            time: 99,
        },
        prefetch: PrefetchReport {
            attempted: 6,
            succeeded: 5,
            failed: 1,
            bytes_prefetched: 1 << 33,
            errors: Vec::new(),
        },
        memstat: Some(MemStat {
            total: 1000,
            free: 300,
            cached: 200,
            available: 500,
            pagein: 0,
            pageout: 0,
        }),
    }
}

#[test]
fn sin_ultimo_tick_las_estadisticas_dejan_los_valores_de_ausencia() {
    let stats = stats_from_view(DaemonState::Paused, &EngineView::default());

    assert_eq!(stats.state, DaemonState::Paused);
    assert_eq!(stats.scan_count, 0);
    assert_eq!(stats.exe_count, 0);
    assert_eq!(stats.last_tick_scan_id, 0);
    assert_eq!(stats.last_tick_mem_free_pct, 255);
}

#[test]
fn las_estadisticas_toman_los_contadores_de_los_almacenes_y_del_ultimo_tick() {
    let mut view = EngineView::default();
    view.stores.model_time = 42;
    let bash = view.stores.ensure_exe(ExeKey::new("/usr/bin/bash".into()));
    view.stores.ensure_exe(ExeKey::new("/usr/bin/vim".into()));
    let libc = view
        .stores
        .ensure_map(MapSegment::new("/usr/lib/libc.so.6".into(), 0, 4096, 1));
    view.stores.attach_map(bash, libc);
    view.stores.active.update(&[bash], 42);
    view.last_tick = Some(full_tick());

    let stats = stats_from_view(DaemonState::Running, &view);

    assert_eq!(stats.state, DaemonState::Running);
    assert_eq!(stats.exe_count, 2);
    assert_eq!(stats.map_count, 1);
    assert_eq!(stats.active_count, 1);
    assert_eq!(stats.model_time, 42);
    assert_eq!(stats.scan_count, 7);
    assert_eq!(stats.last_tick_scan_id, 7);
    assert_eq!(stats.last_tick_exes_added, 1);
    assert_eq!(stats.last_tick_maps_added, 2);
    assert_eq!(stats.last_tick_edges_added, 3);
    assert_eq!(stats.last_tick_edges_updated, 4);
    assert_eq!(stats.last_tick_prediction_exes, 5);
    assert_eq!(stats.last_tick_prediction_max_prob, 0.5);
    assert_eq!(stats.last_tick_prefetch_attempted, 6);
    assert_eq!(stats.last_tick_prefetch_succeeded, 5);
    assert_eq!(stats.last_tick_prefetch_failed, 1);
    assert_eq!(stats.last_tick_prefetch_bytes, 1 << 33);
    assert_eq!(stats.last_tick_mem_free_pct, 50);
}

#[test]
fn un_tick_sin_datos_de_memoria_deja_el_porcentaje_en_255() {
    let view = EngineView {
        last_tick: Some(TickReport {
            memstat: None,
            ..full_tick()
        }),
        ..EngineView::default()
    };

    let stats = stats_from_view(DaemonState::Running, &view);

    assert_eq!(stats.last_tick_scan_id, 7);
    assert_eq!(stats.last_tick_mem_free_pct, 255);
}

#[test]
fn el_planificador_del_servicio_respeta_la_reserva_de_la_configuracion() {
    let memstat = MemStat {
        total: 1000,
        free: 600,
        cached: 0,
        available: 600,
        pagein: 0,
        pageout: 0,
    };
    let budget = |min_free_pct: u8| {
        let mut config = Config::default();
        config.sistema.mem_libre_min_pct = min_free_pct;
        planner_from(&config)
            .plan(&Prediction::new(0), &Stores::default(), &memstat)
            .budget_bytes
    };

    // Reserva de 20 % de 1000 = 200: (600 - 200) / 2. Sin reserva sería 300.
    assert_eq!(budget(20), 200);
    assert_eq!(budget(0), 300);
    assert_eq!(budget(60), 0);
}

/// Como `FixedScanner`, pero el único mapa apunta a un archivo que no existe:
/// el prefetcher real lo cuenta como fallido; `NoopPrefetcher` lo daba por
/// bueno sin tocar el disco.
struct MissingMapScanner;

impl Scanner for MissingMapScanner {
    fn scan(&mut self, time: u64, scan_id: u64) -> Result<Observation, EngineError> {
        Ok(vec![
            ObservationEvent::ObsBegin { time, scan_id },
            ObservationEvent::MemStat {
                mem: MemStat {
                    total: 1000,
                    free: 300,
                    cached: 200,
                    available: 500,
                    pagein: 0,
                    pageout: 0,
                },
            },
            ObservationEvent::CandidateExe {
                exe: CandidateExe::new("/usr/bin/bash".into(), 1),
            },
            ObservationEvent::Map {
                segment: MapSegment::new(
                    "/usr/lib/vasak-preload-no-existe.so".into(),
                    0,
                    100,
                    time,
                ),
                pid: 1,
            },
            ObservationEvent::ObsEnd {
                time,
                scan_id,
                warnings: Vec::new(),
            },
        ])
    }
}

#[tokio::test]
async fn el_motor_del_servicio_precarga_de_verdad_y_cuenta_lo_que_falla() {
    let dir = TempDir::new().unwrap();
    let mut config = config_de_prueba(dir.path());
    config.sistema.doscan = true;
    config.sistema.dopredict = true;
    config.sistema.doprefetch = true;
    // El presupuesto (la mitad de lo disponible, 500 B según el escáner) tiene
    // que alcanzar para el mapa de 100 B.
    config.sistema.mem_libre_min_pct = 0;
    let service = PreloadService::new(config, ConfigSource::Default)
        .await
        .expect("servicio");
    let daemon = start_daemon_scanning(service, Box::new(MissingMapScanner)).await;

    let stats = timeout(APAGADO_MAXIMO, async {
        loop {
            let stats = daemon.proxy.stats().await.unwrap();
            if stats.last_tick_prefetch_attempted > 0 {
                break stats;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("el motor no llegó a planificar una precarga");

    assert_eq!(stats.last_tick_prefetch_failed, 1);
    assert_eq!(stats.last_tick_prefetch_succeeded, 0);
    assert_eq!(stats.last_tick_prefetch_bytes, 0);

    apagar(daemon.motor).await;
}

#[tokio::test]
async fn el_motor_recargado_tambien_precarga_de_verdad() {
    // Arranca sin precarga y la recarga la enciende: lo que ejecuta el plan
    // después es el prefetcher que armó `ReloadConfig`.
    let dir = TempDir::new().unwrap();
    let estado = dir.path().join("state.sqlite");
    let archivo = dir.path().join("config.toml");
    std::fs::write(
        &archivo,
        format!(
            "[modelo]\nciclo = 1\n\
             [sistema]\ndoscan = true\ndopredict = true\ndoprefetch = true\n\
             mem_libre_min_pct = 0\n\
             [persistencia]\nestado_path = {estado:?}\n"
        ),
    )
    .unwrap();
    let source = ConfigSource::File(archivo);
    let mut inicial = config_de_prueba(dir.path());
    inicial.sistema.doscan = true;
    inicial.sistema.dopredict = true;
    let service = PreloadService::new(inicial, source)
        .await
        .expect("servicio");
    let daemon = start_daemon_scanning(service, Box::new(MissingMapScanner)).await;

    daemon.proxy.reload_config().await.unwrap();

    let stats = timeout(APAGADO_MAXIMO, async {
        loop {
            let stats = daemon.proxy.stats().await.unwrap();
            if stats.last_tick_prefetch_attempted > 0 {
                break stats;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("el motor recargado no llegó a planificar una precarga");

    assert_eq!(stats.last_tick_prefetch_failed, 1);
    assert_eq!(stats.last_tick_prefetch_succeeded, 0);

    apagar(daemon.motor).await;
}
