//! Cliente CLI para vasak-preload.
//!
//! Permite consultar estado, estadísticas y controlar el daemon via D-Bus.

#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};
use std::fmt::Write as _;
use std::path::PathBuf;
use tracing::info;
use vasak_preload_protocol::{ConfigSnapshot, DaemonStats, ExeState, PreloadProxy};
use zbus::Connection;

#[cfg(test)]
mod tests;

#[derive(Parser, Debug)]
#[command(
    name = "vasak-preload-cli",
    version,
    about = "CLI client for vasak-preload daemon"
)]
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
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let cli = Cli::parse();

    // Conectar al bus del sistema
    let conn = Connection::system().await?;

    let proxy = PreloadProxy::new(&conn).await?;

    print!("{}", run(&proxy, cli.command).await?);

    Ok(())
}

/// Ejecuta `command` contra el daemon y devuelve lo que hay que imprimir.
async fn run(
    proxy: &PreloadProxy<'_>,
    command: Commands,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut out = String::new();
    match command {
        Commands::Status => {
            let state = proxy.state().await?;
            let _ = writeln!(out, "Estado: {:?}", state);
        }
        Commands::Stats => out = format_stats(&proxy.stats().await?),
        Commands::Config => out = format_config(&proxy.config().await?),
        Commands::Pause => {
            proxy.pause().await?;
            info!("Prefetch pausado");
        }
        Commands::Resume => {
            proxy.resume().await?;
            info!("Prefetch reanudado");
        }
        Commands::Save => {
            proxy.save_now().await?;
            info!("Guardado solicitado");
        }
        Commands::Reload => {
            proxy.reload_config().await?;
            info!("Configuración recargada");
        }
        Commands::Top { n } => {
            let predictions = proxy.get_top_predictions(n).await?;
            let _ = writeln!(out, "Top {} predicciones:", n);
            for (i, (path, prob)) in predictions.iter().enumerate() {
                let _ = writeln!(out, "  {}. {} ({:.2}%)", i + 1, path, prob * 100.0);
            }
        }
        Commands::Exe { path } => {
            let path_str = path.to_string_lossy().to_string();
            let state = proxy.get_exe_state(&path_str).await?;
            if state.has_value {
                out = format_exe_state(&state.inner);
            } else {
                let _ = writeln!(out, "Ejecutable no encontrado: {}", path_str);
            }
        }
        Commands::Tick => {
            // Para testing: no hay método directo, se haría via control interno
            let _ = writeln!(
                out,
                "Tick manual no implementado via D-Bus (usar signal interno)"
            );
        }
    }

    Ok(out)
}

fn format_stats(stats: &DaemonStats) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "=== Estadísticas de vasak-preload ===");
    let _ = writeln!(out, "Estado: {:?}", stats.state);
    let _ = writeln!(out, "Escaneos: {}", stats.scan_count);
    let _ = writeln!(out, "Ejecutables rastreados: {}", stats.exe_count);
    let _ = writeln!(out, "Mapeos rastreados: {}", stats.map_count);
    let _ = writeln!(out, "Aristas de Markov: {}", stats.edge_count);
    let _ = writeln!(out, "Ejecutables activos: {}", stats.active_count);
    let _ = writeln!(out, "Tiempo de modelo: {}s", stats.model_time);
    // last_tick_scan_id == 0: todavía no hubo ningún tick.
    if stats.last_tick_scan_id != 0 {
        let _ = writeln!(out, "\n--- Último tick ---");
        let _ = writeln!(out, "Scan ID: {}", stats.last_tick_scan_id);
        let _ = writeln!(out, "Exes añadidos: {}", stats.last_tick_exes_added);
        let _ = writeln!(out, "Maps añadidos: {}", stats.last_tick_maps_added);
        let _ = writeln!(out, "Edges añadidos: {}", stats.last_tick_edges_added);
        let _ = writeln!(out, "Edges actualizados: {}", stats.last_tick_edges_updated);
        let _ = writeln!(out, "Predicción exes: {}", stats.last_tick_prediction_exes);
        let _ = writeln!(
            out,
            "Predicción max prob: {:.2}%",
            stats.last_tick_prediction_max_prob * 100.0
        );
        let _ = writeln!(
            out,
            "Prefetch intentados: {}",
            stats.last_tick_prefetch_attempted
        );
        let _ = writeln!(
            out,
            "Prefetch exitosos: {}",
            stats.last_tick_prefetch_succeeded
        );
        let _ = writeln!(
            out,
            "Prefetch fallidos: {}",
            stats.last_tick_prefetch_failed
        );
        let _ = writeln!(out, "Prefetch bytes: {}", stats.last_tick_prefetch_bytes);
        // 255: sin dato de memoria.
        if stats.last_tick_mem_free_pct != 255 {
            let _ = writeln!(out, "Memoria libre: {}%", stats.last_tick_mem_free_pct);
        }
    }
    out
}

fn format_config(config: &ConfigSnapshot) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "=== Configuración actual ===");
    let _ = writeln!(out, "Modelo ciclo: {}s", config.modelo_ciclo_secs);
    let _ = writeln!(
        out,
        "Modelo ventana activa: {}s",
        config.modelo_ventana_activa_secs
    );
    let _ = writeln!(out, "Sistema doscan: {}", config.sistema_doscan);
    let _ = writeln!(out, "Sistema dopredict: {}", config.sistema_dopredict);
    let _ = writeln!(out, "Sistema doprefetch: {}", config.sistema_doprefetch);
    // 0: autoguardado deshabilitado.
    if config.persistencia_autosave_secs != 0 {
        let _ = writeln!(out, "Autoguardado: {}s", config.persistencia_autosave_secs);
    } else {
        let _ = writeln!(out, "Autoguardado: deshabilitado");
    }
    out
}

fn format_exe_state(state: &ExeState) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "=== Estado de ejecutable ===");
    let _ = writeln!(out, "Ruta: {}", state.path);
    let _ = writeln!(out, "Ejecutándose: {}", state.running);
    let _ = writeln!(out, "Tiempo total: {}s", state.total_running_time);
    // 0: nunca visto.
    if state.last_seen_time != 0 {
        let _ = writeln!(out, "Última vez visto: {}s", state.last_seen_time);
    }
    let _ = writeln!(
        out,
        "Probabilidad predicción: {:.2}%",
        state.prediction_probability * 100.0
    );
    let _ = writeln!(out, "Mapeos: {}", state.map_count);
    out
}
