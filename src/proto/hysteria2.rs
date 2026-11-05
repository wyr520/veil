//! Hysteria2 style QUIC data plane.
//!
//! One QUIC connection carries:
//! * a **control stream** that authenticates the user (password),
//! * one **bi-directional stream per proxied TCP connection**,
//! * **QUIC datagrams** for UDP relay, keyed by a 32-bit session id.
//!
//! The ALPN is `h3` so the handshake is indistinguishable from ordinary HTTP/3
//! traffic on the wire. See `docs/INTEROP.md` for the differences with the
//! reference `hysteria` implementation.

use anyhow::{bail, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::addr::Destination;

pub const ALPN: &[u8] = b"h3";
pub const MAGIC: &[u8; 3] = b"HY2";
pub const VERSION: u8 = 1;
pub const AUTH_OK: &[u8] = b"OK";

/// Write the control-stream authentication frame.
pub async fn write_auth<W: AsyncWrite + Unpin>(wr: &mut W, password: &str) -> Result<()> {
    if password.len() > u16::MAX as usize {
        bail!("password too long");
    }
    let mut buf = Vec::with_capacity(8 + password.len());
    buf.extend_from_slice(MAGIC);
    buf.push(VERSION);
    buf.extend_from_slice(&(password.len() as u16).to_be_bytes());
    buf.extend_from_slice(password.as_bytes());
    wr.write_all(&buf).await?;
    wr.flush().await?;
    Ok(())
}

/// Read the control-stream authentication frame, returning the password.
pub async fn read_auth<R: AsyncRead + Unpin>(rd: &mut R) -> Result<String> {
    let mut head = [0u8; 6];
    rd.read_exact(&mut head).await?;
    if &head[..3] != MAGIC {
        bail!("hysteria2: bad magic (not a veil client?)");
    }
    if head[3] != VERSION {
        bail!("hysteria2: unsupported version {}", head[3]);
    }
    let len = u16::from_be_bytes([head[4], head[5]]) as usize;
    if len == 0 || len > 1024 {
        bail!("hysteria2: bad password length");
    }
    let mut pw = vec![0u8; len];
    rd.read_exact(&mut pw).await?;
    String::from_utf8(pw).map_err(|_| anyhow::anyhow!("hysteria2: password is not UTF-8"))
}

pub async fn write_auth_ok<W: AsyncWrite + Unpin>(wr: &mut W) -> Result<()> {
    wr.write_all(AUTH_OK).await?;
    wr.flush().await?;
    Ok(())
}

pub async fn read_auth_ok<R: AsyncRead + Unpin>(rd: &mut R) -> Result<()> {
    let mut buf = [0u8; 2];
    rd.read_exact(&mut buf).await?;
    if &buf != AUTH_OK {
        bail!("hysteria2: authentication rejected");
    }
    Ok(())
}

/// First frame on a TCP stream: the destination address.
pub async fn write_open<W: AsyncWrite + Unpin>(wr: &mut W, dest: &Destination) -> Result<()> {
    let mut buf = Vec::with_capacity(24);
    dest.write_to(&mut buf);
    wr.write_all(&buf).await?;
    wr.flush().await?;
    Ok(())
}

pub async fn read_open<R: AsyncRead + Unpin>(rd: &mut R) -> Result<Destination> {
    Destination::read_from(rd).await
}

/// Datagram layout used for UDP relay: `session(4 LE) | [dest] | payload`.
/// Server → client omits the destination.
pub fn encode_udp(session: u32, dest: Option<&Destination>, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 32);
    out.extend_from_slice(&session.to_le_bytes());
    if let Some(d) = dest {
        d.write_to(&mut out);
    }
    out.extend_from_slice(payload);
    out
}

pub fn decode_udp_with_addr(buf: &[u8]) -> Result<(u32, Destination, &[u8])> {
    if buf.len() < 8 {
        bail!("hysteria2: short udp datagram");
    }
    let session = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let mut pos = 4usize;
    let atyp = buf[pos];
    pos += 1;
    let host = super::addr::read_host_slice(buf, &mut pos, atyp)?;
    if buf.len() < pos + 2 {
        bail!("hysteria2: truncated udp port");
    }
    let port = u16::from_be_bytes([buf[pos], buf[pos + 1]]);
    pos += 2;
    Ok((session, Destination { host, port }, &buf[pos..]))
}

pub fn decode_udp_reply(buf: &[u8]) -> Result<(u32, &[u8])> {
    if buf.len() < 4 {
        bail!("hysteria2: short udp reply");
    }
    let session = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    Ok((session, &buf[4..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::addr::Address;

    #[test]
    fn udp_frames_roundtrip() {
        let dest = Destination::new(Address::Domain("dns.test".into()), 53);
        let raw = encode_udp(7, Some(&dest), b"query");
        let (sid, d, payload) = decode_udp_with_addr(&raw).unwrap();
        assert_eq!(sid, 7);
        assert_eq!(d, dest);
        assert_eq!(payload, b"query");

        let reply = encode_udp(7, None, b"answer");
        let (sid, payload) = decode_udp_reply(&reply).unwrap();
        assert_eq!(sid, 7);
        assert_eq!(payload, b"answer");
    }

    #[tokio::test]
    async fn auth_roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(256);
        write_auth(&mut a, "hunter2").await.unwrap();
        let pw = read_auth(&mut b).await.unwrap();
        assert_eq!(pw, "hunter2");
    }
}
