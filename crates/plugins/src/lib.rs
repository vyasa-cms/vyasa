//! WASM plugin runtime for Vyasa: wasmtime host with fuel and memory
//! limits, capability broker, priority hook registry, and plugin lifecycle.

#[allow(missing_docs)] // wasmtime bindgen output
/// The WIT world installed plugins are compiled against.
///
/// A release that changes this cannot run plugins built for the old one:
/// the upgrade preflight compares it with the target release's declared
/// contract so an operator learns *before* upgrading which plugins will
/// need rebuilding, rather than finding them degraded afterwards.
pub const CONTRACT_VERSION: &str = "vyasa-plugin-v2";

pub mod blocks;
pub mod broker;
pub mod capabilities;
pub mod dispatch;
pub mod hooks;
pub mod host;
pub mod hostdata;
pub mod lifecycle;
pub mod package;
