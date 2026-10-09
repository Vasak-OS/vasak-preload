//! Definiciones del protocolo D-Bus para vasak-preload.

#![forbid(unsafe_code)]

mod proxy;
mod types;

pub use proxy::{PreloadProxy, PreloadProxyBlocking};
pub use types::*;
