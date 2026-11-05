//! QUIC endpoint setup for the Hysteria2 ingress.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};

fn transport() -> Arc<quinn::TransportConfig> {
    let mut t = quinn::TransportConfig::default();
    t.max_idle_timeout(Some(
        Duration::from_secs(90).try_into().expect("idle timeout"),
    ));
    t.keep_alive_interval(Some(Duration::from_secs(20)));
    t.datagram_receive_buffer_size(Some(4 * 1024 * 1024));
    t.datagram_send_buffer_size(4 * 1024 * 1024);
    // Weak-network friendliness: keep the stream window generous but not absurd.
    t.stream_receive_window(quinn::VarInt::from_u32(16 * 1024 * 1024));
    t.receive_window(quinn::VarInt::from_u32(32 * 1024 * 1024));
    Arc::new(t)
}

pub fn server_endpoint(bind: SocketAddr, tls: Arc<rustls::ServerConfig>) -> Result<quinn::Endpoint> {
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls.as_ref().clone())
        .map_err(|e| anyhow::anyhow!("QUIC server crypto: {e}"))?;
    let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    cfg.transport_config(transport());
    quinn::Endpoint::server(cfg, bind)
        .with_context(|| format!("cannot bind QUIC endpoint on {bind}"))
}

pub fn client_endpoint(tls: Arc<rustls::ClientConfig>) -> Result<quinn::Endpoint> {
    let bind: SocketAddr = if is_v6_hint() {
        "[::]:0".parse().unwrap()
    } else {
        "0.0.0.0:0".parse().unwrap()
    };
    let mut endpoint = quinn::Endpoint::client(bind).context("cannot create QUIC client endpoint")?;
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
        .map_err(|e| anyhow::anyhow!("QUIC client crypto: {e}"))?;
    let mut cfg = quinn::ClientConfig::new(Arc::new(crypto));
    cfg.transport_config(transport());
    endpoint.set_default_client_config(cfg);
    Ok(endpoint)
}

fn is_v6_hint() -> bool {
    false
}

/// Connect and wait for the QUIC handshake.
pub async fn connect(
    endpoint: &quinn::Endpoint,
    addr: SocketAddr,
    server_name: &str,
) -> Result<quinn::Connection> {
    let connecting = endpoint
        .connect(addr, server_name)
        .map_err(|e| anyhow::anyhow!("QUIC connect: {e}"))?;
    tokio::time::timeout(Duration::from_secs(10), connecting)
        .await
        .map_err(|_| anyhow::anyhow!("QUIC handshake timed out"))?
        .map_err(|e| anyhow::anyhow!("QUIC handshake failed: {e}"))
}
