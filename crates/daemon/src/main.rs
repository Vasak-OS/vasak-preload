//! Daemon principal de vasak-preload.
//!
//! Expone una interfaz D-Bus para control y monitoreo.

#![forbid(unsafe_code)]

use vasak_preload_config::Config;
use vasak_preload_orchestrator::{
    clock::SystemClock, observation::DefaultAdmissionPolicy, observation::DefaultModelUpdater,
    observation::ProcfsScanner, persistence::SqliteRepository, prediction::MarkovPredictor,
    prefetch::GreedyPrefetchPlanner, prefetch::NoopPrefetcher, engine::{ControlEvent, PreloadEngine, ReloadBundle, Services},
};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use zbus::{connection, zvariant::OwnedObjectPath};

mod service;

use crate::service::PreloadService;
use clap::Parser;

/// Argumentos de línea de comandos.
#[derive(Parser, Debug)]
#[command(name = "vasak-preload", version, about = "VasakOS system optimizer: prefetches binaries based on usage patterns")]
struct Args {
    /// Archivo de configuración.
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Directorio de configuración (se cargan todos los *.toml).
    #[arg(long, value_name = "DIR")]
    config_dir: Option<PathBuf>,

    /// Ejecutar en primer plano (no hacer daemonize).
    #[arg(short, long)]
    foreground: bool,

    /// Imprimir configuración efectiva y salir.
    #[arg(long)]
    print_config: bool,

    /// Solo validar configuración y salir.
    #[arg(long)]
    validate_config: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Inicializar logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,vasak_preload=debug".into()),
        )
        .init();

    let args = Args::parse();

    // Cargar configuración
    let config = load_config(&args)?;

    if args.print_config {
        println!("{}", toml_edit::ser::to_string_pretty(&config)?);
        return Ok(());
    }

    if args.validate_config {
        println!("Configuración válida");
        return Ok(());
    }

    info!("Iniciando vasak-preload");

    // Crear conexión D-Bus
    let conn = connection::Builder::system()?
        .name("org.vasakos.Preload")?
        .serve_at("/org/vasakos/Preload", PreloadService::new(config.clone()).await?)?
        .build()
        .await?;

    info!("Conectado al bus del sistema");

    // Ejecutar el daemon
    if args.foreground {
        run_foreground(conn, config).await?
    } else {
        run_daemon(conn, config).await?
    }

    Ok(())
}

/// Carga la configuración desde archivos.
fn load_config(args: &Args) -> Result<Config, Box<dyn std::error::Error>> {
    if let Some(config_file) = &args.config {
        return Ok(Config::cargar(config_file)?);
    }

    if let Some(config_dir) = &args.config_dir {
        let mut paths = Vec::new();
        for entry in std::fs::read_dir(config_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "toml") {
                paths.push(path);
            }
        }
        paths.sort();
        return Ok(Config::cargar_multiple(paths)?);
    }

    // Rutas por defecto
    let default_paths = vec![
        PathBuf::from("/etc/vasak-preload/config.toml"),
        dirs::config_dir()
            .map(|d| d.join("vasak-preload/config.toml"))
            .unwrap_or_default(),
    ];

    Ok(Config::cargar_multiple(default_paths)?)
}

/// Ejecuta en primer plano (para testing/systemd).
async fn run_foreground(
    conn: connection::Connection,
    config: Config,
) -> Result<(), Box<dyn std::error::Error>> {
    let service = PreloadService::new(config).await?;
    let service = Arc::new(service);

    // Iniciar motor en background
    let engine_handle = service.start_engine().await?;

    // Mantener conexión viva
    let mut signals = conn.try_listen().await?;
    while signals.next().await.is_some() {}

    // Apagar
    engine_handle.cancel().await;
    Ok(())
}

/// Ejecuta como daemon (fork + setsid).
async fn run_daemon(
    conn: connection::Connection,
    config: Config,
) -> Result<(), Box<dyn std::error::Error>> {
    // En Linux, systemd maneja el daemonizing. Si no estamos bajo systemd,
    // podríamos hacer fork manual, pero por simplicidad asumimos systemd.
    run_foreground(conn, config).await
}