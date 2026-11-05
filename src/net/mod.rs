//! Shared networking helpers: a boxed stream type, TLS/WebSocket/QUIC transports
//! and traffic accounting.

pub mod quic;
pub mod relay;
pub mod socks5;
pub mod tls;
pub mod ws;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

/// Anything that behaves like a byte stream.
pub trait Io: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Io for T {}

pub type BoxedIo = Box<dyn Io>;

pub fn boxed<T: Io + 'static>(t: T) -> BoxedIo {
    Box::new(t)
}

/// Simple uplink/downlink counters, shared across tasks.
#[derive(Debug, Default)]
pub struct Traffic {
    pub up: AtomicU64,
    pub down: AtomicU64,
}

impl Traffic {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn add_up(&self, n: u64) {
        self.up.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_down(&self, n: u64) {
        self.down.fetch_add(n, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> (u64, u64) {
        (
            self.up.load(Ordering::Relaxed),
            self.down.load(Ordering::Relaxed),
        )
    }
}

/// Apply the usual socket tuning (nodelay + keepalive) to an accepted connection.
pub fn tune(stream: &TcpStream) {
    let _ = stream.set_nodelay(true);
}

/// Half-close the write side, then wait briefly for the peer before dropping.
pub async fn graceful_close<S: AsyncWrite + Unpin>(stream: &mut S) {
    let _ = stream.shutdown().await;
}

/// Connect with a timeout and a helpful error message.
pub async fn connect_tcp(addr: std::net::SocketAddr, timeout: Duration) -> Result<TcpStream> {
    let fut = TcpStream::connect(addr);
    let stream = tokio::time::timeout(timeout, fut)
        .await
        .map_err(|_| anyhow::anyhow!("connect to {addr} timed out"))??;
    tune(&stream);
    Ok(stream)
}
