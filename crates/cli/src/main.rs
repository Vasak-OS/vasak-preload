//! Cliente CLI para vasak-preload.
//!
//! Permite consultar estado, estadísticas y controlar el daemon via D-Bus.

#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};
use protocol::{DaemonState, ExeState};
use std::path::PathBuf;
use tracing::{error, info};
use zbus::Connection;

#[derive(Parser, Debug)]
#[command(name = "vasak-preload-cli", version, about = "CLI client for vasak-preload daemon")]
struct Cli {
    /// Comando a ejecutar.
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Muestra el estado del daemon.
    Status,
    /// Muestra estadísticas detalladas.
    Stats,
    /// Muestra la configuración actual.
    Config,
    /// Pausa el prefetch.
    Pause,
    /// Reanuda el prefetch.
    Resume,
    /// Fuerza guardado del estado.
    Save,
    /// Recarga la configuración.
    Reload,
    /// Muestra top N predicciones.
    Top {
        /// Número de entradas a mostrar.
        #[arg(short, long, default_value = "10")]
        n: u32,
    },
    /// Muestra estado de un ejecutable específico.
    Exe {
        /// Ruta del ejecutable.
        path: PathBuf,
    },
    /// Ejecuta un tick manual (para testing).
    Tick,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let cli = Cli::parse();

    // Conectar al bus del sistema
    let conn = Connection::system().await?;

    // Obtener proxy del servicio
    let proxy = zbus::Proxy::new(
        &conn,
        "org.vasakos.Preload",
        "/org/vasakos/Preload",
        "org.vasakos.Preload1",
    )
    .await?;

    match cli.command {
        Commands::Status => {
            let state: DaemonState = proxy.get_property("State").await?;
            println!("Estado: {:?}", state);
        }
        Commands::Stats => {
            let stats = proxy.call_method("Stats", &()).await?.body()?;
            print_stats(&stats);
        }
        Commands::Config => {
            let config = proxy.call_method("Config", &()).await?.body()?;
            print_config(&config);
        }
        Commands::Pause => {
            proxy.call_method("Pause", &()).await?;
            info!("Prefetch pausado");
        }
        Commands::Resume => {
            proxy.call_method("Resume", &()).await?;
            info!("Prefetch reanudado");
        }
        Commands::Save => {
            proxy.call_method("SaveNow", &()).await?;
            info!("Guardado solicitado");
        }
        Commands::Reload => {
            proxy.call_method("ReloadConfig", &()).await?;
            info!("Configuración recargada");
        }
        Commands::Top { n } => {
            let predictions: Vec<(String, f32)> = proxy
                .call_method("GetTopPredictions", &n)
                .await?
                .body()?;
            println!("Top {} predicciones:", n);
            for (i, (path, prob)) in predictions.iter().enumerate() {
                println!("  {}. {} ({:.2}%)", i + 1, path, prob * 100.0);
            }
        }
        Commands::Exe { path } => {
            let path_str = path.to_string_lossy().to_string();
            let state: Option<ExeState> = proxy
                .call_method("GetExeState", &path_str)
                .await?
                .body()?;
            if let Some(state) = state {
                print_exe_state(&state);
            } else {
                println!("Ejecutable no encontrado: {}", path_str);
            }
        }
        Commands::Tick => {
            // Para testing: no hay método directo, se haría via control interno
            println!("Tick manual no implementado via D-Bus (usar signal interno)");
        }
    }

    Ok(())
}

fn print_stats(stats: &protocol::DaemonStats) {
    println!("=== Estadísticas de vasak-preload ===");
    println!("Estado: {:?}", stats.state);
    println!("Escaneos: {}", stats.scan_count);
    println!("Ejecutables rastreados: {}", stats.exe_count);
    println!("Mapeos rastreados: {}", stats.map_count);
    println!("Aristas de Markov: {}", stats.edge_count);
    println!("Ejecutables activos: {}", stats.active_count);
    println!("Tiempo de modelo: {}s", stats.model_time);
    if let Some(tick) = &stats.last_tick {
        println!("\n--- Último tick ---");
        println!("Scan ID: {}", tick.scan_id);
        println!("Exes añadidos: {}", tick.exes_added);
        println!("Maps añadidos: {}", tick.maps_added);
        println!("Edges añadidos: {}", tick.edges_added);
        println!("Edges actualizados: {}", tick.edges_updated);
        println!("Predicción exes: {}", tick.prediction_exes);
        println!("Predicción max prob: {:.2}%", tick.prediction_max_prob * 100.0);
        println!("Prefetch intentados: {}", tick.prefetch_attempted);
        println!("Prefetch exitosos: {}", tick.prefetch_succeeded);
        println!("Prefetch fallidos: {}", tick.prefetch_failed);
        println!("Prefetch bytes: {}", tick.prefetch_bytes);
        if let Some(pct) = tick.mem_free_pct {
            println!("Memoria libre: {}%", pct);
        }
    }
}

fn print_config(config: &protocol::ConfigSnapshot) {
    println!("=== Configuración actual ===");
    println!("Modelo ciclo: {}s", config.modelo_ciclo_secs);
    println!("Modelo ventana activa: {}s", config.modelo_ventana_activa_secs);
    println!("Sistema doscan: {}", config.sistema_doscan);
    println!("Sistema dopredict: {}", config.sistema_dopredict);
    println!("Sistema doprefetch: {}", config.sistema_doprefetch);
    if let Some(autosave) = config.persistencia_autosave_secs {
        println!("Autoguardado: {}s", autosave);
    } else {
        println!("Autoguardado: deshabilitado");
    }
}

fn print_exe_state(state: &ExeState) {
    println!("=== Estado de ejecutable ===");
    println!("Ruta: {}", state.path);
    println!("Ejecutándose: {}", state.running);
    println!("Tiempo total: {}s", state.total_running_time);
    if let Some(last) = state.last_seen_time {
        println!("Última vez visto: {}s", last);
    }
    println!("Probabilidad predicción: {:.2}%", state.prediction_probability * 100.0);
    println!("Mapeos: {}", state.map_count);
}