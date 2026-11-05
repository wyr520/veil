//! Protocol codecs. Each module provides both the client and the server half so
//! that the same code powers `veil run` and `veil client`.

pub mod addr;
pub mod hysteria2;
pub mod trojan;
pub mod vless;
pub mod vmess;

pub use addr::{Address, Destination};
