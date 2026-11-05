//! Outbound side: where proxied traffic actually goes.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tokio::net::TcpStream;

use crate::config::OutboundMode;
use crate::proto::Destination;
use crate::server::ServerCtx;

/// Resolve a destination into socket addresses (domains go through the system resolver).
pub async fn resolve(dest: &Destination) -> Result<Vec<SocketAddr>> {
    if let Some(sa) = dest.socket_addr() {
        return Ok(vec![sa]);
    }
    let host = dest.host.to_string();
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), dest.port))
        .await
        .with_context(|| format!("DNS lookup failed for {host}"))?
        .collect();
    if addrs.is_empty() {
        bail!("no addresses for {host}");
    }
    Ok(addrs)
}

fn is_private(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
        }
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
    }
}

/// Open a TCP connection to `dest`, honouring the outbound policy.
pub async fn connect(ctx: &ServerCtx, dest: &Destination) -> Result<TcpStream> {
    let (mode, socks, blocked, block_private, timeout) = {
        let cfg = ctx.config.read().await;
        (
            cfg.outbound.mode,
            cfg.outbound.socks.clone(),
            cfg.outbound.blocked_ports.clone(),
            cfg.outbound.block_private,
            Duration::from_secs(cfg.outbound.connect_timeout.max(1)),
        )
    };

    if blocked.contains(&dest.port) {
        bail!("port {} is blocked by policy", dest.port);
    }
    match mode {
        OutboundMode::Block => bail!("outbound is disabled (mode = block)"),
        OutboundMode::Socks => {
            let proxy: SocketAddr = socks
                .parse()
                .with_context(|| format!("invalid outbound.socks `{socks}`"))?;
            return crate::net::socks5::connect_through(proxy, dest, timeout).await;
        }
        OutboundMode::Direct => {}
    }

    let addrs = resolve(dest).await?;
    if block_private && addrs.iter().any(|a| is_private(&a.ip())) {
        bail!("destination {} is private (block_private = true)", dest);
    }

    let mut last_err = None;
    for addr in addrs {
        match crate::net::connect_tcp(addr, timeout).await {
            Ok(s) => return Ok(s),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no route to {dest}")))
}

/// Bind a UDP socket suitable for relaying to arbitrary destinations.
pub async fn udp_socket() -> Result<tokio::net::UdpSocket> {
    Ok(tokio::net::UdpSocket::bind("0.0.0.0:0").await?)
}
