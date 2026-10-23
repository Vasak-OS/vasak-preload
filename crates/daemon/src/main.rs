//! Daemon principal de vasak-preload.
//!
//! Corre como servicio de usuario (`systemd --user`) y expone una interfaz D-Bus
//! en el bus de la sesión para control y monitoreo.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use tokio::signal::unix::{SignalKind, signal};
use tracing::info;
use vasak_preload_config::Config;
use zbus::{Connection, connection};

mod service;
#[cfg(test)]
mod tests;

const BUS_NAME: &str = "org.vasakos.Preload";
const OBJECT_PATH: &str = "/org/vasakos/Preload";

use crate::service::{ConfigSource, EngineTask, PreloadService};
use clap::Parser;
use vasak_preload_orchestrator::{ProcfsScanner, Scanner};

/// Argumentos de línea de comandos.
#[derive(Parser, Debug)]
#[command(
    name = "vasak-preload",
    version,
    about = "VasakOS user service: prefetches binaries based on usage patterns"
)]
struct Args {
    /// Archivo de configuración.
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Directorio de configuración (se cargan todos los *.toml).
    #[arg(long, value_name = "DIR")]
    config_dir: Option<PathBuf>,

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
    let source = config_source(&args);
    let config = source
        .load()
        .map_err(|e| -> Box<dyn std::error::Error> { e })?;

    if args.print_config {
        println!("{}", toml_edit::ser::to_string_pretty(&config)?);
        return Ok(());
    }

    if args.validate_config {
        println!("Configuración válida");
        return Ok(());
    }

    info!("Iniciando vasak-preload");

    // Publicar el servicio en el bus de la sesión: sólo entra quien corre como
    // la misma persona, así que no hay política que escribir. Si el nombre no
    // se puede tomar, el motor no llega a arrancar.
    run(
        connection::Builder::session()?.name(BUS_NAME)?,
        config,
        source,
        wait_for_shutdown(),
    )
    .await
}

/// Publica el servicio, lo atiende hasta que termina `shutdown` y apaga el
/// motor (con su guardado final) antes de volver.
async fn run(
    builder: connection::Builder<'_>,
    config: Config,
    source: ConfigSource,
    shutdown: impl Future<Output = std::io::Result<()>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (_conn, engine) = serve(
        builder,
        PreloadService::new(config, source).await?,
        Box::new(ProcfsScanner::new()),
    )
    .await?;
    info!("Servicio publicado");

    let result = shutdown.await;
    info!("Apagando");
    engine.shutdown().await;

    Ok(result?)
}

/// Publica el servicio en la conexión que arma `builder` y arranca el motor
/// sobre esa misma instancia, para que la interfaz D-Bus vea el motor que corre.
async fn serve(
    builder: connection::Builder<'_>,
    service: PreloadService,
    scanner: Box<dyn Scanner + Send + Sync>,
) -> Result<(Connection, EngineTask), Box<dyn std::error::Error>> {
    let conn = builder.serve_at(OBJECT_PATH, service)?.build().await?;

    let engine = conn
        .object_server()
        .interface::<_, PreloadService>(OBJECT_PATH)
        .await?
        .get()
        .await
        .start_engine(scanner)
        .await?;

    Ok((conn, engine))
}

/// De dónde leer la configuración según los argumentos.
fn config_source(args: &Args) -> ConfigSource {
    if let Some(config_file) = &args.config {
        ConfigSource::File(config_file.clone())
    } else if let Some(config_dir) = &args.config_dir {
        ConfigSource::Dir(config_dir.clone())
    } else {
        ConfigSource::Default
    }
}

/// Espera SIGTERM (systemd) o SIGINT (Ctrl+C).
async fn wait_for_shutdown() -> std::io::Result<()> {
    let mut sigterm = signal(SignalKind::terminate())?;
    tokio::select! {
        _ = sigterm.recv() => {}
        result = tokio::signal::ctrl_c() => result?,
    }
    Ok(())
}
