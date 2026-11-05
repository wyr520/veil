//! Minimal SOCKS5: the local inbound used by `veil client`, and the upstream
//! dialer used when the server chains through another proxy.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::proto::{addr::Address, Destination};

pub const CMD_CONNECT: u8 = 0x01;
pub const CMD_UDP_ASSOCIATE: u8 = 0x03;

#[derive(Debug)]
pub enum SocksRequest {
    Connect(Destination),
    UdpAssociate,
}

async fn read_destination<R: AsyncRead + Unpin>(
    rd: &mut R,
    atyp: u8,
) -> Result<Destination> {
    let host = crate::proto::addr::read_host(rd, atyp).await?;
    let port = rd.read_u16().await?;
    Ok(Destination { host, port })
}

/// Server side of the SOCKS5 greeting + request.
pub async fn accept<S>(stream: &mut S) -> Result<SocksRequest>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let ver = stream.read_u8().await?;
    if ver != 5 {
        bail!("socks5: unsupported version {ver}");
    }
    let nmethods = stream.read_u8().await? as usize;
    let mut methods = vec![0u8; nmethods];
    stream.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        stream.write_all(&[0x05, 0xff]).await?;
        bail!("socks5: client does not offer no-auth");
    }
    stream.write_all(&[0x05, 0x00]).await?;

    let ver = stream.read_u8().await?;
    if ver != 5 {
        bail!("socks5: bad request version {ver}");
    }
    let cmd = stream.read_u8().await?;
    let _rsv = stream.read_u8().await?;
    let atyp = stream.read_u8().await?;
    let dest = read_destination(stream, atyp).await?;
    match cmd {
        CMD_CONNECT => Ok(SocksRequest::Connect(dest)),
        CMD_UDP_ASSOCIATE => Ok(SocksRequest::UdpAssociate),
        other => {
            reply(stream, 0x07, None).await.ok();
            bail!("socks5: command {other} not supported");
        }
    }
}

/// Send a SOCKS5 reply. `code` 0 means success.
pub async fn reply<S>(stream: &mut S, code: u8, bind: Option<SocketAddr>) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    let mut out = vec![0x05, code, 0x00];
    match bind {
        Some(SocketAddr::V4(a)) => {
            out.push(0x01);
            out.extend_from_slice(&a.ip().octets());
            out.extend_from_slice(&a.port().to_be_bytes());
        }
        Some(SocketAddr::V6(a)) => {
            out.push(0x04);
            out.extend_from_slice(&a.ip().octets());
            out.extend_from_slice(&a.port().to_be_bytes());
        }
        None => {
            out.extend_from_slice(&[0x01, 0, 0, 0, 0, 0, 0]);
        }
    }
    stream.write_all(&out).await?;
    stream.flush().await?;
    Ok(())
}

/// Client side: open a tunnel to `dest` through a SOCKS5 proxy.
pub async fn connect_through(
    proxy: SocketAddr,
    dest: &Destination,
    timeout: Duration,
) -> Result<TcpStream> {
    let mut stream = tokio::time::timeout(timeout, TcpStream::connect(proxy))
        .await
        .context("socks5: connect timeout")?
        .with_context(|| format!("socks5: cannot reach {proxy}"))?;
    stream.set_nodelay(true).ok();

    stream.write_all(&[0x05, 0x01, 0x00]).await?;
    let mut resp = [0u8; 2];
    stream.read_exact(&mut resp).await?;
    if resp != [0x05, 0x00] {
        bail!("socks5: proxy refused no-auth");
    }

    let mut req = vec![0x05, CMD_CONNECT, 0x00];
    dest.write_to(&mut req);
    stream.write_all(&req).await?;

    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await?;
    if head[1] != 0x00 {
        bail!("socks5: proxy returned error code {}", head[1]);
    }
    // Drain the bound address.
    let atyp = head[3];
    let host = crate::proto::addr::read_host(&mut stream, atyp).await?;
    let _ = host;
    let _port = stream.read_u16().await?;
    Ok(stream)
}

/// Client side UDP associate: returns the relay socket and the stream that must
/// be kept open for the association to live.
pub async fn udp_associate(
    proxy: SocketAddr,
    timeout: Duration,
) -> Result<(tokio::net::UdpSocket, TcpStream)> {
    let mut stream = tokio::time::timeout(timeout, TcpStream::connect(proxy))
        .await
        .context("socks5: connect timeout")??;
    stream.write_all(&[0x05, 0x01, 0x00]).await?;
    let mut resp = [0u8; 2];
    stream.read_exact(&mut resp).await?;

    let bind = Destination::new(Address::V4(std::net::Ipv4Addr::UNSPECIFIED), 0);
    let mut req = vec![0x05, CMD_UDP_ASSOCIATE, 0x00];
    bind.write_to(&mut req);
    stream.write_all(&req).await?;

    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await?;
    if head[1] != 0x00 {
        bail!("socks5: udp associate refused");
    }
    let atyp = head[3];
    let host = crate::proto::addr::read_host(&mut stream, atyp).await?;
    let port = stream.read_u16().await?;
    let relay_addr = match host {
        Address::V4(a) => SocketAddr::new(a.into(), port),
        Address::V6(a) => SocketAddr::new(a.into(), port),
        Address::Domain(d) => {
            let s = format!("{d}:{port}");
            tokio::net::lookup_host(s).await?.next().context("no addr")?
        }
    };
    let bind_ip = if relay_addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
    let socket = tokio::net::UdpSocket::bind(bind_ip).await?;
    Ok((socket, stream))
}

/// Encode a SOCKS5 UDP datagram header.
pub fn encode_udp_datagram(dest: &Destination, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0x00, 0x00, 0x00];
    dest.write_to(&mut out);
    out.extend_from_slice(payload);
    out
}

/// Split a SOCKS5 UDP datagram into destination + payload.
pub fn decode_udp_datagram(buf: &[u8]) -> Result<(Destination, &[u8])> {
    if buf.len() < 4 {
        bail!("socks5: short datagram");
    }
    let mut pos = 3usize;
    let atyp = buf[pos];
    pos += 1;
    let host = crate::proto::addr::read_host_slice(buf, &mut pos, atyp)?;
    if buf.len() < pos + 2 {
        bail!("socks5: truncated datagram");
    }
    let port = u16::from_be_bytes([buf[pos], buf[pos + 1]]);
    pos += 2;
    Ok((Destination { host, port }, &buf[pos..]))
}
