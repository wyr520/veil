//! Veil — a multi-protocol proxy suite.
//!
//! * `veil` binary: `veil init`, `veil run`, `veil client`, `veil user ...`
//! * library: reusable protocol codecs so the same code drives the server, the
//!   desktop client and the mobile app.

pub mod cli;
pub mod client;
pub mod config;
pub mod crypto;
pub mod net;
pub mod proto;
pub mod server;
pub mod share;

/// Crate version, surfaced in `veil version` and `/api/health`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
