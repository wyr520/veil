//! Trojan — HTTPS-identical traffic: a real TLS session, then a SHA-224 password
//! digest followed by a SOCKS-style CONNECT request.

use anyhow::{bail, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::addr::{Destination, ATYP_DOMAIN};

pub const CMD_CONNECT: u8 = 0x01;
pub const CMD_UDP_ASSOCIATE: u8 = 0x03;

const CRLF: [u8; 2] = [0x0d, 0x0a];

/// Hex-encoded SHA-224 of the password (56 ASCII bytes on the wire).
pub fn password_digest(password: &str) -> String {
    crate::crypto::sha224_hex(password.as_bytes())
}

#[derive(Debug, Clone)]
pub struct Request {
    pub cmd: u8,
    pub dest: Destination,
}

impl Request {
    pub fn connect(dest: Destination) -> Self {
        Self { cmd: CMD_CONNECT, dest }
    }

    /// Validate the password digest and parse the request header.
    pub async fn read<R>(rd: &mut R, password: &str) -> Result<Self>
    where
        R: AsyncRead + Unpin,
    {
        let mut digest = [0u8; 56];
        rd.read_exact(&mut digest).await?;
        let mut crlf = [0u8; 2];
        rd.read_exact(&mut crlf).await?;
        if crlf != CRLF {
            bail!("trojan: malformed digest terminator");
        }
        let expected = password_digest(password);
        // Only the first user's digest can be checked here; the caller loops over users
        // by re-reading, so this function is used once the password is already known.
        if !crate::config::constant_time_eq(digest.as_slice(), expected.as_bytes()) {
            bail!("trojan: bad password");
        }

        let cmd = rd.read_u8().await?;
        let atyp = rd.read_u8().await?;
        let host = super::addr::read_host(rd, atyp).await?;
        let port = rd.read_u16().await?;
        let mut end = [0u8; 2];
        rd.read_exact(&mut end).await?;
        if end != CRLF {
            bail!("trojan: malformed request terminator");
        }
        Ok(Self { cmd, dest: Destination { host, port } })
    }

    pub async fn write<W>(&self, wr: &mut W, password: &str) -> Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let mut buf = Vec::with_capacity(80);
        buf.extend_from_slice(password_digest(password).as_bytes());
        buf.extend_from_slice(&CRLF);
        buf.push(self.cmd);
        self.dest.write_to(&mut buf);
        buf.extend_from_slice(&CRLF);
        wr.write_all(&buf).await?;
        wr.flush().await?;
        Ok(())
    }
}

/// Trojan UDP packet: `atyp | addr | port | len(2 BE) | CRLF | payload`.
#[derive(Debug, Clone)]
pub struct UdpPacket {
    pub dest: Destination,
    pub payload: Vec<u8>,
}

impl UdpPacket {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.payload.len() + 24);
        self.dest.write_to(&mut out);
        out.extend_from_slice(&(self.payload.len() as u16).to_be_bytes());
        out.extend_from_slice(&CRLF);
        out.extend_from_slice(&self.payload);
        out
    }

    pub async fn read<R>(rd: &mut R) -> Result<Self>
    where
        R: AsyncRead + Unpin,
    {
        let atyp = rd.read_u8().await?;
        let host = super::addr::read_host(rd, atyp).await?;
        let port = rd.read_u16().await?;
        let len = rd.read_u16().await? as usize;
        let mut crlf = [0u8; 2];
        rd.read_exact(&mut crlf).await?;
        if crlf != CRLF {
            bail!("trojan: malformed udp frame");
        }
        let mut payload = vec![0u8; len];
        rd.read_exact(&mut payload).await?;
        Ok(Self { dest: Destination { host, port }, payload })
    }
}

/// The first 56 bytes of a Trojan session are the password digest; peak at them
/// so the server can pick the matching user before parsing the rest.
pub async fn peek_digest<R>(rd: &mut R) -> Result<[u8; 56]>
where
    R: AsyncRead + Unpin,
{
    let mut digest = [0u8; 56];
    rd.read_exact(&mut digest).await?;
    Ok(digest)
}

/// Parse the remainder of the request after [`peek_digest`] consumed the digest.
pub async fn read_after_digest<R>(rd: &mut R) -> Result<Request>
where
    R: AsyncRead + Unpin,
{
    let mut crlf = [0u8; 2];
    rd.read_exact(&mut crlf).await?;
    if crlf != CRLF {
        bail!("trojan: malformed digest terminator");
    }
    let cmd = rd.read_u8().await?;
    let atyp = rd.read_u8().await?;
    let host = super::addr::read_host(rd, atyp).await?;
    let port = rd.read_u16().await?;
    let mut end = [0u8; 2];
    rd.read_exact(&mut end).await?;
    if end != CRLF {
        bail!("trojan: malformed request terminator");
    }
    Ok(Request { cmd, dest: Destination { host, port } })
}

/// Convenience for tests / clients: encode a full CONNECT request.
pub fn encode_connect(password: &str, dest: &Destination) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(password_digest(password).as_bytes());
    buf.extend_from_slice(&CRLF);
    buf.push(CMD_CONNECT);
    dest.write_to(&mut buf);
    buf.extend_from_slice(&CRLF);
    buf
}

#[allow(dead_code)]
fn _assert_domain_atyp() {
    let _ = ATYP_DOMAIN;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn connect_header_roundtrip() {
        let dest = Destination::new("example.com", 443);
        let raw = encode_connect("s3cret", &dest);
        let mut c = std::io::Cursor::new(raw);
        let digest = peek_digest(&mut c).await.unwrap();
        assert_eq!(
            std::str::from_utf8(&digest).unwrap(),
            password_digest("s3cret")
        );
        let req = read_after_digest(&mut c).await.unwrap();
        assert_eq!(req.cmd, CMD_CONNECT);
        assert_eq!(req.dest.port, 443);
        // stream must be fully consumed
        let mut rest = Vec::new();
        c.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn udp_packet_roundtrip() {
        let p = UdpPacket { dest: Destination::new("8.8.8.8", 53), payload: vec![1, 2, 3] };
        let raw = p.encode();
        let mut c = std::io::Cursor::new(raw);
        let back = UdpPacket::read(&mut c).await.unwrap();
        assert_eq!(back.payload, vec![1, 2, 3]);
        assert_eq!(back.dest.port, 53);
    }
}
