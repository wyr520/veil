//! VLESS — a lightweight stateless protocol. Encryption is provided entirely by
//! the transport (TLS / XTLS), the header only carries identity and destination.

use anyhow::{bail, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

use super::addr::Destination;

pub const VERSION: u8 = 0;

pub const CMD_TCP: u8 = 1;
pub const CMD_UDP: u8 = 2;
pub const CMD_MUX: u8 = 3;

#[derive(Debug, Clone)]
pub struct Request {
    pub uuid: Uuid,
    pub cmd: u8,
    pub dest: Destination,
}

impl Request {
    pub fn tcp(uuid: Uuid, dest: Destination) -> Self {
        Self { uuid, cmd: CMD_TCP, dest }
    }

    /// Read and validate a client request header.
    pub async fn read<R>(rd: &mut R) -> Result<Self>
    where
        R: AsyncRead + Unpin,
    {
        let version = rd.read_u8().await?;
        if version != VERSION {
            bail!("unsupported vless version {version}");
        }
        let mut uuid = [0u8; 16];
        rd.read_exact(&mut uuid).await?;
        let uuid = Uuid::from_bytes(uuid);

        let addon_len = rd.read_u8().await? as usize;
        if addon_len > 0 {
            let mut skip = vec![0u8; addon_len];
            rd.read_exact(&mut skip).await?;
        }

        let cmd = rd.read_u8().await?;
        let dest = Destination::read_port_first(rd).await?;

        Ok(Self { uuid, cmd, dest })
    }

    pub async fn write<W>(&self, wr: &mut W) -> Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let mut buf = Vec::with_capacity(32);
        buf.push(VERSION);
        buf.extend_from_slice(self.uuid.as_bytes());
        buf.push(0); // no addons
        buf.push(self.cmd);
        self.dest.write_port_first(&mut buf);
        wr.write_all(&buf).await?;
        wr.flush().await?;
        Ok(())
    }
}

/// Server → client response: version + empty addons.
pub async fn write_response<W>(wr: &mut W) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    wr.write_all(&[VERSION, 0x00]).await?;
    wr.flush().await?;
    Ok(())
}

pub async fn read_response<R>(rd: &mut R) -> Result<()>
where
    R: AsyncRead + Unpin,
{
    let version = rd.read_u8().await?;
    if version != VERSION {
        bail!("unsupported vless response version {version}");
    }
    let addon_len = rd.read_u8().await? as usize;
    if addon_len > 0 {
        let mut skip = vec![0u8; addon_len];
        rd.read_exact(&mut skip).await?;
    }
    Ok(())
}

/// One VLESS UDP datagram frame (used when `cmd == CMD_UDP`).
///
/// Wire layout: `len(2 BE) | port(2 BE) | atyp(1) | addr | payload`, where `len`
/// covers everything after itself.
#[derive(Debug, Clone)]
pub struct UdpFrame {
    pub dest: Destination,
    pub payload: Vec<u8>,
}

impl UdpFrame {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.payload.len() + 24);
        out.extend_from_slice(&[0, 0]); // placeholder
        self.dest.write_port_first(&mut out);
        out.extend_from_slice(&self.payload);
        let total = (out.len() - 2) as u16;
        out[0..2].copy_from_slice(&total.to_be_bytes());
        out
    }

    pub async fn read<R>(rd: &mut R) -> Result<Self>
    where
        R: AsyncRead + Unpin,
    {
        let total = rd.read_u16().await? as usize;
        if total < 4 {
            bail!("vless udp frame too short ({total})");
        }
        let mut body = vec![0u8; total];
        rd.read_exact(&mut body).await?;
        let port = u16::from_be_bytes([body[0], body[1]]);
        let atyp = body[2];
        let mut pos = 3usize;
        let host = super::addr::read_host_slice(&body, &mut pos, atyp)?;
        Ok(Self { dest: Destination { host, port }, payload: body[pos..].to_vec() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::addr::Address;

    #[tokio::test]
    async fn header_roundtrip() {
        let id = Uuid::new_v4();
        let req = Request::tcp(id, Destination::new(Address::Domain("a.test".into()), 8443));
        let mut buf = Vec::new();
        req.write(&mut buf).await.unwrap();
        assert_eq!(buf[0], VERSION);
        let mut cursor = std::io::Cursor::new(buf);
        let back = Request::read(&mut cursor).await.unwrap();
        assert_eq!(back.uuid, id);
        assert_eq!(back.dest.port, 8443);
    }

    #[tokio::test]
    async fn udp_frame_roundtrip() {
        let f = UdpFrame {
            dest: Destination::new("1.1.1.1", 53),
            payload: b"dns".to_vec(),
        };
        let raw = f.encode();
        let mut c = std::io::Cursor::new(raw);
        let back = UdpFrame::read(&mut c).await.unwrap();
        assert_eq!(back.dest.port, 53);
        assert_eq!(back.payload, b"dns");
    }
}
