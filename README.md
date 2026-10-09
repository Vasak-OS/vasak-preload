# vasak-preload

System optimizer for VasakOS: prefetches binaries and shared libraries into memory based on usage patterns to achieve faster application startup times.

## Overview

`vasak-preload` is a daemon that monitors running applications, builds a Markov-based probabilistic model of application transitions, predicts what applications the user might launch next, and prefetches those binaries and their dependencies into memory using `posix_fadvise`.

It's inspired by [preload-rs](https://github.com/arunanshub/preload-rs) but designed specifically for VasakOS with:

- **Integration with existing VasakOS infrastructure**: Uses `vasak-monitor`'s process sampling, zbus for D-Bus IPC, shared configuration patterns
- **D-Bus control interface**: Full control via `vasak-preload-cli` or D-Bus directly
- **Hot config reload**: Change configuration without restarting
- **Persistent state**: SQLite + rkyv for fast serialization
- **Safety first**: Runs with minimal privileges, configurable memory limits

## Architecture

```
┌─────────────────┐     ┌──────────────────┐     ┌─────────────────┐
│  ProcfsScanner  │────▶│  DefaultModel    │────▶│ MarkovPredictor │
│  (observation)  │     │  Updater         │     │  (prediction)   │
└─────────────────┘     └──────────────────┘     └────────┬────────┘
                                                          │
                         ┌──────────────────┐             │
                         │ GreedyPrefetch   │◀────────────┘
                         │ Planner          │
                         └────────┬─────────┘
                                  │
                         ┌────────▼─────────┐
                         │ PosixFadvise     │
                         │ Prefetcher       │
                         └──────────────────┘
```

### Components

| Crate | Purpose |
|-------|---------|
| `vasak-preload-config` | Configuration loading/validation (TOML) |
| `vasak-preload-orchestrator` | Core logic: scanning, modeling, prediction, prefetch |
| `vasak-preload-protocol` | D-Bus interface definitions |
| `vasak-preload-daemon` | System daemon with D-Bus service |
| `vasak-preload-cli` | Command-line client |

## Building

```bash
# From VasakOS root
cargo build --package vasak-preload-daemon --release
cargo build --package vasak-preload-cli --release
```

## Installation

```bash
# Install binaries
sudo cp target/release/vasak-preload /usr/bin/
sudo cp target/release/vasak-preload-cli /usr/bin/

# Install config
sudo mkdir -p /etc/vasak-preload
sudo cp docs/config.example.toml /etc/vasak-preload/config.toml

# Install systemd service
sudo cp packaging/vasak-preload.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now vasak-preload
```

## Usage

### CLI

```bash
# Show daemon status
vasak-preload-cli status

# Show detailed statistics
vasak-preload-cli stats

# Show current configuration
vasak-preload-cli config

# Show top 10 predicted applications
vasak-preload-cli top --n 10

# Check specific executable
vasak-preload-cli exe /usr/bin/firefox

# Control daemon
vasak-preload-cli pause
vasak-preload-cli resume
vasak-preload-cli save
vasak-preload-cli reload
```

### D-Bus

```bash
# Get status
busctl get-property org.vasakos.Preload /org/vasakos/Preload org.vasakos.Preload1 State

# Get stats
busctl call org.vasakos.Preload /org/vasakos/Preload org.vasakos.Preload1 Stats

# Pause prefetch
busctl call org.vasakos.Preload /org/vasakos/Preload org.vasakos.Preload1 Pause

# Get top predictions
busctl call org.vasakos.Preload /org/vasakos/Preload org.vasakos.Preload1 GetTopPredictions u 10
```

## Configuration

See `docs/config.example.toml` for all options. Key settings:

- **modelo.ciclo**: How often to run a scan/predict/prefetch cycle (default: 20s)
- **modelo.ventana_activa**: How long an app stays "active" after last seen (default: 1h)
- **sistema.doprefetch**: Enable/disable actual prefetching (default: true)
- **sistema.mem_libre_min_pct**: Minimum free memory % before prefetch (default: 10%)
- **persistencia.estado_path**: SQLite database path (default: `/var/lib/vasak-preload/state.sqlite`)

## Integration with VasakOS

`vasak-preload` leverages existing VasakOS components:

- **Process sampling**: Reuses `vasak-monitor/src-tauri/src/muestreo/procesos.rs` logic for robust `/proc` parsing
- **IPC**: Uses `zbus` like `vasak-connect-daemon`, `vasak-store-daemon`, `vasak-permissions-daemon`
- **Configuration**: TOML with `toml_edit` like other VasakOS services
- **Observability**: Structured logging with `tracing`, stats exposed via D-Bus
- **Testing**: Property-based tests with `proptest`, unit tests alongside code

## How it Works

1. **Scan**: Every `modelo.ciclo` seconds, read `/proc` for running processes and their memory maps
2. **Filter**: Apply admission policy (prefixes, max map size, memory pressure)
3. **Model**: Update Markov graph with observed transitions between applications
4. **Predict**: Compute probability distribution over applications likely to launch next
5. **Plan**: Select memory maps to prefetch based on prediction probability and memory budget
6. **Prefetch**: Use `posix_fadvise(POSIX_FADV_WILLNEED)` to bring pages into page cache
7. **Persist**: Save model state to SQLite periodically and on shutdown

## Safety

- **Memory pressure awareness**: Won't prefetch if free memory < `mem_libre_min_pct`
- **Size limits**: Ignores mappings larger than `max_mapa` (default 100MiB)
- **Prefix filtering**: Only monitors configured path prefixes (default: all)
- **Noop prefetcher**: Default daemon uses `NoopPrefetcher`; enable `PosixFadvisePrefetcher` explicitly
- **Runs as dedicated user**: Systemd service should run as `vasak-preload` user

## License

Apache-2.0