//! La interfaz D-Bus del daemon, de punta a punta, en un bus privado.
//!
//! Servicio y cliente se hablan por un par de sockets (`UnixStream::pair`) en
//! modo peer-to-peer: no hace falta bus del sistema ni root. El cliente usa el
//! mismo `PreloadProxy` que `vasak-preload-cli`.

use crate::service::{EngineTask, PreloadService};
use crate::{OBJECT_PATH, run, serve, wait_for_shutdown};
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;
use tokio::net::UnixStream;
use tokio::time::timeout;
use vasak_preload_config::Config;
use vasak_preload_orchestrator::{ExeKey, MapSegment, SqliteRepository, StateRepository};
use vasak_preload_protocol::{DaemonState, PreloadProxy};
use zbus::proxy::CacheProperties;
use zbus::{Connection, Guid, connection};

/// Margen para que el motor se apague. Holgado a propósito: el sueño entre
/// ciclos no se interrumpe y el `select!` de `run_until` no prioriza la
/// cancelación, así que con un ciclo de 1 s el apagado tarda una cantidad
/// geométrica de ciclos (2/3 de las veces elige otro tick). Con 60 ciclos de
/// margen la probabilidad de pasarse es despreciable.
const APAGADO_MAXIMO: Duration = Duration::from_secs(60);

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
    start_daemon_with(PreloadService::new(config).await.expect("servicio")).await
}

/// Como `start_daemon`, con un servicio ya armado (por ejemplo, con otros archivos
/// de configuración para `ReloadConfig`).
async fn start_daemon_with(service: PreloadService) -> Daemon {
    let (lado_servidor, lado_cliente) = UnixStream::pair().expect("par de sockets");

    let servidor = connection::Builder::unix_stream(lado_servidor)
        .server(Guid::generate())
        .expect("guid")
        .p2p();
    let cliente = connection::Builder::unix_stream(lado_cliente).p2p().build();

    let (servido, cliente) = tokio::join!(serve(servidor, service), cliente);
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

    let servicio = PreloadService::new(config).await.expect("servicio");
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
    let dir = TempDir::new().unwrap();
    let daemon = start_daemon(config_de_prueba(dir.path())).await;

    let stats = daemon.proxy.stats().await.unwrap();
    assert_eq!(stats.last_tick_scan_id, 0, "0 = todavía sin ticks");
    assert_eq!(
        stats.last_tick_mem_free_pct, 255,
        "255 = sin dato de memoria"
    );
    assert_eq!(stats.last_tick_prediction_max_prob, 0.0);

    apagar(daemon.motor).await;
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
    let stores = servicio.get().await.stores();
    let mut stores = stores.write().await;

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
        run(servidor, config, async {
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
    // `ReloadConfig` lee archivos que no existen dentro del directorio
    // temporal, así que carga los valores por omisión, que no traen 4321. No
    // depende de la configuración de la máquina.
    let dir = TempDir::new().unwrap();
    let mut config = config_de_prueba(dir.path());
    config.modelo.ventana_activa = Duration::from_secs(4321);
    let service = PreloadService::new(config)
        .await
        .expect("servicio")
        .with_reload_paths(vec![dir.path().join("no-existe.toml")]);
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
