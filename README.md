# Vasak preload

Precarga de binarios y bibliotecas según patrones de uso (VasakOS)

---

## Tecnología

[![Rust](https://img.shields.io/badge/Rust-1.82%2B-orange?style=flat&logo=rust)](https://www.rust-lang.org/) [![systemd](https://img.shields.io/badge/systemd-256%2B-24283b?style=flat&logo=systemd)](https://systemd.io/) [![D-Bus](https://img.shields.io/badge/D-Bus-1.15%2B-42b883?style=flat&logo=dbus)](https://dbus.freedesktop.org/) [![zbus](https://img.shields.io/badge/zbus-4.0-orange?style=flat&logo=rust)](https://github.com/dbus2/zbus)

## Estado

[![CI](https://github.com/Vasak-OS/vasak-preload/actions/workflows/ci.yml/badge.svg)](https://github.com/Vasak-OS/vasak-preload/actions/workflows/ci.yml)
[![Versión](https://img.shields.io/github/v/release/Vasak-OS/vasak-preload?style=flat&label=Versi%C3%B3n)](https://github.com/Vasak-OS/vasak-preload/releases)
[![Licencia](https://img.shields.io/github/license/Vasak-OS/vasak-preload?style=flat)](LICENSE)
[![Cobertura](https://sonarcloud.io/api/project_badges/measure?project=Vasak-OS_vasak-preload&metric=coverage&style=flat)](https://sonarcloud.io/summary/new_code?id=Vasak-OS_vasak-preload)
[![Quality Gate](https://sonarcloud.io/api/project_badges/measure?project=Vasak-OS_vasak-preload&metric=alert_status&style=flat)](https://sonarcloud.io/summary/new_code?id=Vasak-OS_vasak-preload)
[![Tamaño repo](https://img.shields.io/github/repo-size/Vasak-OS/vasak-preload?style=flat)](https://github.com/Vasak-OS/vasak-preload)

---

## Descripción

Precarga de binarios y bibliotecas según patrones de uso (VasakOS)

**Para el usuario:** Precarga de binarios y bibliotecas según patrones de uso (VasakOS)

**Integración VasakOS:** Integra con el ecosistema VasakOS vía D-Bus y configuración compartida.

---

## Arquitectura (diagramas)

> Copia y pega la plantilla que necesites, descomenta y adapta.

### Flujo de datos

```mermaid
<!--
graph TD
    A[Entrada: Teclado / Ratón / D-Bus / Sistema] --> B[Procesamiento: Core Rust + Tauri Commands]
    B --> C[Salida: UI actualizada / Acciones del sistema]
    style A fill:#e1f5fe
    style C fill:#e8f5e9
-->
```

### Componentes internos

```mermaid
<!--
flowchart LR
    subgraph App[Vasak preload]
        UI[Frontend Vue 3 + vue-libvasak]
        Core[Core Rust: lógica principal]
        IPC[Tauri Commands / Eventos]
        System[Systemd / D-Bus / PipeWire / Wayland]
    end
    UI --> IPC
    IPC --> Core
    Core --> DBus[(D-Bus / zbus)]
    Core --> System
    style UI fill:#f3e5f5
    style Core fill:#e3f2fd
    style DBus fill:#fff3e0
    style System fill:#fce4ec
-->
```

### Integración en el sistema VasakOS

```mermaid
<!--
C4Context
    title Vasak preload en VasakOS
    Person(usuario, "Usuario", "Persona que usa el sistema")
    System_Boundary(c1, "VasakOS") {
        System(app, "Vasak preload", "Precarga de binarios y bibliotecas según patrones de uso (VasakOS)")
        System_Ext(settings, "vasak-settings", "Configuración central")
        System_Ext(keyring, "vasak-keyring", "Secrets y claves")
        System_Ext(accounts, "vasak-accounts", "Cuentas e identidad")
        System_Ext(permissions, "vasak-permissions", "Permisos y portales")
    }
    System_Ext(systemd, "systemd", "Gestión de servicios")
    System_Ext(dbus, "D-Bus", "Bus de mensajes del sistema")
    System_Ext(wayland, "Wayland / Wayfire", "Compositor")
    System_Ext(pipewire, "PipeWire", "Multimedia")
    Rel(usuario, app, "Usa")
    Rel(app, settings, "Lee/escribe config")
    Rel(app, keyring, "Guarda/lee secrets")
    Rel(app, accounts, "Identidad de cuenta")
    Rel(app, permissions, "Solicita permisos")
    Rel(app, systemd, "Activación / sockets")
    Rel(app, dbus, "Expone / consume interfaces")
    Rel(app, wayland, "Superficies / protocolos")
    Rel(app, pipewire, "Streams de audio/video")
-->
```

---

## Instalación

### Arch Linux (repo oficial)

```bash
pacman -S vasak-vasak-preload
```

### Debian / Ubuntu (.deb)

```bash
wget https://github.com/Vasak-OS/vasak-preload/releases/download/v{{VERSION}}/vasak-vasak-preload_{{VERSION}}_amd64.deb
sudo apt install ./vasak-vasak-preload_{{VERSION}}_amd64.deb
```

### Desde fuente

```bash
git clone https://github.com/Vasak-OS/vasak-preload.git
cd vasak-preload

# Build completo
cargo build --release --locked

# Instalar binario
cargo install --locked --path .

# Paquetes nativos
# Arch: makepkg -sf en PKGBUILDS/vasak-preload/
# Debian: cargo deb --no-build --target-dir target/release/debian
```

### Flatpak / AppImage

> *Placeholder: añadir cuando exista paquete Flatpak o AppImage oficial.*

---

## Uso rápido

```bash
# Se lanza automáticamente (systemd --user) o manualmente:
vasak-preload
```

### Atajos / flags principales

| Acción | Comando / Tecla |
|--------|-----------------|
| Acción principal | `vasak-preload` |
| Configuración | `vasak-preload --config` |
| Ayuda | `vasak-preload --help` |

> **Captura:** `assets/demo.gif` *(placeholder — sustituir por GIF real)*

---

## Configuración

### Archivo de configuración

```
~/.config/vasak/vasak-preload.toml
```

### Variables de entorno

| Variable | Descripción | Por defecto |
|----------|-------------|-------------|
| `VASAK_DEBUG` | Activa logs de depuración en consola | `false` |
| `VASAK_PROFILE` | Perfil de rendimiento: `balanced`, `performance`, `power-save` | `balanced` |

### Integración con `tauri-plugin-config-manager`

- Esquemas de color: respeta `--ui-primary`, `--ui-surface`, `--ui-focus`, `--ui-on-surface`, `--ui-border`
- Tema de iconos: usa el tema del sistema (`vasakos-icon-theme`) vía `vicon` lookup
- Radio de bordes: variable CSS `--ui-radius` del esquema activo (por defecto 12px)
- Fuentes: `--ui-font-family`, `--ui-font-size-base` desde el esquema

---

## Desarrollo

### Requisitos

- **Rust:** `rustup` toolchain estable (`rust-toolchain.toml` fija la versión)
- **Node / bun:** `bun` ≥ 1.1 (nunca npm/yarn/pnpm para apps Tauri)
- **Sistema:** `pkg-config`, `libgtk-3-dev`, `libwebkit2gtk-4.1-dev`, `libssl-dev`
- *`libdbus-1-dev`, `libsystemd-dev` para systemd*

### Dependencias de desarrollo

#### Sistema (paquetes Arch / Debian / Fedora)

`base-devel`, `pkgconf`, `libssl-dev`, `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev`, `gstreamer1.0-plugins-base`, `gstreamer1.0-plugins-good`, `libpipewire-0.3-dev`, `libwireplumber-0.5-dev`, `libdbus-1-dev`, `libudev-dev`, `libxkbcommon-dev`, `libwayland-dev`, `meson`, `ninja`, `clang`, `lld`

#### Rust

`rustup` → `stable` + `rustfmt` + `clippy`

#### Node

**Solo `bun`** (nunca npm/yarn/pnpm para apps Tauri). `bun --version` ≥ 1.1

#### Herramientas CLI

`just` (command runner), `cargo-binstall` (opcional), `cargo-llvm-cov` (para cobertura)

### Correr la app en desarrollo

```bash
# Frontend (Vite dev server + Tauri dev)
bun run tauri dev

# Solo frontend (Vite)
bun run dev

# Solo backend (cargo watch)
cargo watch -x "run --bin vasak-preload"
```

### Compilar / Build de release

```bash
# Build completo (frontend + backend + bundle)
bun run tauri build

# Solo frontend (para testear bundle después)
bun run build

# Solo backend Rust (binario optimizado)
cargo build --release --locked

# Paquetes nativos
# Arch: makepkg -sf en PKGBUILDS/vasak-preload/
# Debian: cargo deb --no-build --target-dir src-tauri/target/release/debian
```

### Targets principales

```bash
# Desarrollo (Tauri dev server + hot reload)
bun run dev

# Build producción
bun run build

# Solo frontend
bun run build:frontend

# Solo backend Rust
cargo build --release --locked

# Lint + tipos (pre-commit / CI)
bunx --bun biome check .
bunx vue-tsc --noEmit
cargo fmt --check
```

### Pruebas

```bash
# Suite completa Rust
cargo test -p vasak-preload -p vasak-preload-lib --locked

# Tests de integración (requieren D-Bus)
cargo test -p vasak-preload-integration --locked -- --ignored
```

### Convención de commits

[Conventional Commits](https://www.conventionalcommits.org/es/v1.0.0/):

```
<tipo>(<ámbito>): <descripción corta>

[cuerpo opcional]

[pie opcional: Closes #N]
```

Tipos habituales: `feat`, `fix`, `bugfix`, `docs`, `ci`, `chore`, `security`, `refactor`, `perf`, `test`.

### Cómo abrir PR

1. Rama desde `origin/main`:
   ```bash
   git worktree add -b feat/tema ~/wt/vasak-preload-tema origin/main
   ```
2. Prefijos de rama: `fix/`, `feat/`, `bugfix/`, `docs/`, `ci/`, `chore/`, `security/`
3. Escribe el fix → **`qa` escribe las pruebas** (fallan sin el fix) → suite verde → abre PR
4. Al abrir PR, **cuatro cosas obligatorias**:
   - Asignado a `JoaquinDecima`
   - Etiqueta `enhancement` o `bug` (+ `ci` / `dependencies` si aplica)
   - Añadido al proyecto «VasakOS Roadmap»
   - Status **In Progress**

---

## Arquitectura (texto)

### Crates / módulos principales

| `vasak-preload` | Binario principal / daemon systemd |
| `vasak-preload-lib` | Lógica compartida / biblioteca pública |

### Interfaces D-Bus / zbus expuestas

| `org.vasak.Vasak-preload` | `GetStatus()`, `StatusChanged(dict)`, `Reload()`, `GetConfig()` | Control y estado del servicio |
| `org.vasak.Vasak-preload.Admin` | `SetPolicy(dict)`, `AuditLog()` | API administrativa (solo root) |

### Puntos de extensión

- **Plugins:** Sistema de plugins modular
- **Hooks:** Hooks de ciclo de vida
- **API pública Rust:** API pública del crate principal

### Features / Cargo features

| Feature | Descripción | Default |
|---------|-------------|---------|
| `systemd` | Integración systemd (socket activation) | ✓ |
| `tracing` | Logging estructurado con tracing | ✓ |
| `admin` | API administrativa (solo root) | |
| `pam` | Módulo PAM para autenticación | |

Y para apps Tauri+Vue: features del `tauri.conf.json` → `bundle` / `plugins` / `allowlist` (capabilities).

### Un servicio por persona, sin privilegios

`vasak-preload` corre como **servicio de usuario** (`systemd --user`), sin
capacidades, y arranca con la sesión gráfica (`graphical-session.target`). Cada
persona tiene el suyo: escanea sólo **sus** procesos, guarda **su** modelo de
Markov en `$XDG_STATE_HOME/vasak-preload/state.sqlite` (directorio 0700, archivo
0600: es su historial de uso, en claro) y, cuando la precarga esté activa
([#23](https://github.com/Vasak-OS/vasak-preload/issues/23)), precargará con sus
propias credenciales. **Hoy aprende pero todavía no precarga**: por eso ninguna
receta habilita la unidad hasta que lo haga.
Habla por el **bus de la sesión**, donde sólo entra la misma persona, así que no
hay política de D-Bus que escribir. El page cache es compartido: lo que precarga
una persona le sirve a las demás, sin que ningún dato pase de un uid a otro.

Antes de iniciar sesión no se precarga nada.

Esto es deliberado, no una falla
([#3](https://github.com/Vasak-OS/vasak-preload/issues/3),
[#28](https://github.com/Vasak-OS/vasak-preload/issues/28)). Leer qué archivos
mapean los procesos de **otros usuarios** (`/proc/<pid>/maps`, y también
`/proc/<pid>/exe`) está gateado por el kernel con `ptrace_may_access`, y la
única capacidad que lo abre es `CAP_SYS_PTRACE`, que **no se otorga**: en un
servicio siempre activo dejaría leer la memoria de cualquier proceso (ssh-agent,
gpg-agent, el llavero), justo lo que protege el
[THREAT-MODEL](https://github.com/Vasak-OS/.github/blob/main/THREAT-MODEL.md), a
cambio de una mejora de rendimiento. Con un servicio por persona no hace falta.

- Los procesos en **otro espacio de montajes** (Steam, Flatpak) se ignoran: sus
  rutas no se pueden resolver desde afuera.
- La configuración se lee de `/etc/vasak-preload/config.toml` y
  `~/.config/vasak-preload/config.toml`; sin ninguno, valores por omisión.

```bash
systemctl --user enable --now vasak-preload.service   # a mano, para probar
vasak-preload-cli status                              # habla con el de tu sesión
```

---

## Pruebas y calidad

- **Cobertura objetivo:** ≥ 80 % del código nuevo (SonarCloud Quality Gate)
- **Informes:** GitHub Actions → pestaña *SonarCloud Code Analysis* → enlace a SonarCloud
- **Añadir pruebas:** ver `AGENTS.md` sección 9 (mapa de repos y comandos de prueba)
- **Regla:** toda corrección lleva prueba escrita por `qa` que falla sin el fix

---

## Contribuir

- **Guía:** [.github/CONTRIBUTING.md](https://github.com/Vasak-OS/.github/blob/main/CONTRIBUTING.md)
- **Plantillas de issue:** [.github/ISSUE_TEMPLATE/](https://github.com/Vasak-OS/.github/tree/main/ISSUE_TEMPLATE)
- **Código de conducta:** [CODE_OF_CONDUCT.md](https://github.com/Vasak-OS/.github/blob/main/CODE_OF_CONDUCT.md)

---

## Contribuidores

> **Actualizado automáticamente en CI** — muestra los avatares de quienes han aportado código a este repo.

<div align="center">
  
</div>

[Ver todos los contribuidores](https://github.com/Vasak-OS/vasak-preload/graphs/contributors)

---

## Donadores / Sponsors

[![Sponsors](https://img.shields.io/github/sponsors/Vasak-OS?style=flat&label=Sponsors)](https://github.com/sponsors/Vasak-OS)

> **Backers:** *Actualizado automáticamente en CI desde GitHub Sponsors / OpenCollective / Polar.*

[💖 Donar al proyecto](https://github.com/sponsors/Vasak-OS)

---

## Licencia

`MIT` — Copyright © 2026 **Vasak-OS Contributors**

[Ver licencia completa](LICENSE)
