//! Proxy tipado de la interfaz D-Bus del daemon.
//!
//! Vive en el protocolo, y no en el cliente, para que el cliente y las pruebas
//! del daemon hablen con la interfaz a través de la misma definición.

#![forbid(unsafe_code)]

use crate::types::{ConfigSnapshot, DaemonState, DaemonStats, ExeStateWrapper};
use zbus::proxy;

/// Proxy tipado de `org.vasakos.Preload1`.
#[proxy(
    interface = "org.vasakos.Preload1",
    default_service = "org.vasakos.Preload",
    default_path = "/org/vasakos/Preload"
)]
pub trait Preload {
    #[zbus(property)]
    fn state(&self) -> zbus::Result<DaemonState>;
    #[zbus(property)]
    fn stats(&self) -> zbus::Result<DaemonStats>;
    #[zbus(property)]
    fn config(&self) -> zbus::Result<ConfigSnapshot>;
    fn pause(&self) -> zbus::Result<()>;
    fn resume(&self) -> zbus::Result<()>;
    fn save_now(&self) -> zbus::Result<()>;
    fn reload_config(&self) -> zbus::Result<()>;
    fn get_top_predictions(&self, n: u32) -> zbus::Result<Vec<(String, f32)>>;
    fn get_exe_state(&self, path: &str) -> zbus::Result<ExeStateWrapper>;
}
