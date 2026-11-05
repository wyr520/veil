//! SOCKS-style address encoding shared by VLESS, Trojan and Hysteria2.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncRead, AsyncReadExt};

pub const ATYP_V4: u8 = 1;
pub const ATYP_DOMAIN: u8 = 2;
pub const ATYP_V6: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Address {
    V4(Ipv4Addr),
    V6(Ipv6Addr),
    Domain(String),
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Address::V4(a) => write!(f, "{a}"),
            Address::V6(a) => write!(f, "{a}"),
            Address::Domain(d) => f.write_str(d),
        }
    }
}

impl From<&str> for Address {
    fn from(value: &str) -> Self {
        match value.parse::<IpAddr>() {
            Ok(ip) => Address::from(ip),
            Err(_) => Address::Domain(value.to_string()),
        }
    }
}

impl From<String> for Address {
    fn from(value: String) -> Self {
        Address::from(value.as_str())
    }
}

impl From<IpAddr> for Address {
    fn from(value: IpAddr) -> Self {
        match value {
            IpAddr::V4(v) => Address::V4(v),
            IpAddr::V6(v) => Address::V6(v),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Destination {
    pub host: Address,
    pub port: u16,
}

impl fmt::Display for Destination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.host {
            Address::V6(_) => write!(f, "[{}]:{}", self.host, self.port),
            other => write!(f, "{other}:{}", self.port),
        }
    }
}

impl Destination {
    pub fn new(host: impl Into<Address>, port: u16) -> Self {
        Self { host: host.into(), port }
    }

    /// Parse `host:port`, `[v6]:port` or `v6:port`.
    pub fn parse(s: &str) -> Result<Self> {
        if let Ok(sa) = s.parse::<SocketAddr>() {
            return Ok(Self { host: sa.ip().into(), port: sa.port() });
        }
        let (host, port) = s
            .rsplit_once(':')
            .with_context(|| format!("`{s}` is missing a port"))?;
        let port: u16 = port.parse().with_context(|| format!("bad port in `{s}`"))?;
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let host = match host.parse::<IpAddr>() {
            Ok(ip) => Address::from(ip),
            Err(_) => Address::Domain(host.to_string()),
        };
        Ok(Self { host, port })
    }

    pub fn socket_addr(&self) -> Option<SocketAddr> {
        match &self.host {
            Address::V4(a) => Some(SocketAddr::new(IpAddr::V4(*a), self.port)),
            Address::V6(a) => Some(SocketAddr::new(IpAddr::V6(*a), self.port)),
            Address::Domain(_) => None,
        }
    }

    /// Encode `atyp | addr` (no port).
    pub fn write_host(&self, buf: &mut Vec<u8>) {
        match &self.host {
            Address::V4(a) => {
                buf.push(ATYP_V4);
                buf.extend_from_slice(&a.octets());
            }
            Address::V6(a) => {
                buf.push(ATYP_V6);
                buf.extend_from_slice(&a.octets());
            }
            Address::Domain(d) => {
                buf.push(ATYP_DOMAIN);
                buf.push(d.len().min(255) as u8);
                buf.extend_from_slice(&d.as_bytes()[..d.len().min(255)]);
            }
        }
    }

    /// Encode `port | atyp | addr` (VLESS / VMess order).
    pub fn write_port_first(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.port.to_be_bytes());
        self.write_host(buf);
    }

    /// Decode `port | atyp | addr` (VLESS order).
    pub async fn read_port_first<R>(rd: &mut R) -> Result<Self>
    where
        R: AsyncRead + Unpin,
    {
        let port = rd.read_u16().await.context("reading port")?;
        let atyp = rd.read_u8().await.context("reading address type")?;
        let host = read_host(rd, atyp).await?;
        Ok(Self { host, port })
    }

    /// Encode `atyp | addr | port` into `buf`.
    pub fn write_to(&self, buf: &mut Vec<u8>) {
        match &self.host {
            Address::V4(a) => {
                buf.push(ATYP_V4);
                buf.extend_from_slice(&a.octets());
            }
            Address::V6(a) => {
                buf.push(ATYP_V6);
                buf.extend_from_slice(&a.octets());
            }
            Address::Domain(d) => {
                buf.push(ATYP_DOMAIN);
                buf.push(d.len().min(255) as u8);
                buf.extend_from_slice(&d.as_bytes()[..d.len().min(255)]);
            }
        }
        buf.extend_from_slice(&self.port.to_be_bytes());
    }

    pub async fn read_from<R>(rd: &mut R) -> Result<Self>
    where
        R: AsyncRead + Unpin,
    {
        let atyp = rd.read_u8().await.context("reading address type")?;
        let host = read_host(rd, atyp).await?;
        let port = rd.read_u16().await.context("reading port")?;
        Ok(Self { host, port })
    }
}

/// Read the address body for a known `atyp` (port is *not* consumed).
pub async fn read_host<R>(rd: &mut R, atyp: u8) -> Result<Address>
where
    R: AsyncRead + Unpin,
{
    Ok(match atyp {
        ATYP_V4 => {
            let mut b = [0u8; 4];
            rd.read_exact(&mut b).await?;
            Address::V4(Ipv4Addr::from(b))
        }
        ATYP_V6 => {
            let mut b = [0u8; 16];
            rd.read_exact(&mut b).await?;
            Address::V6(Ipv6Addr::from(b))
        }
        ATYP_DOMAIN => {
            let len = rd.read_u8().await? as usize;
            if len == 0 {
                bail!("zero-length domain name");
            }
            let mut b = vec![0u8; len];
            rd.read_exact(&mut b).await?;
            Address::Domain(
                String::from_utf8(b).context("domain name is not valid UTF-8")?,
            )
        }
        other => bail!("unknown address type {other}"),
    })
}

/// Same as [`read_host`] but over a byte slice, returning the cursor position.
pub fn read_host_slice(buf: &[u8], pos: &mut usize, atyp: u8) -> Result<Address> {
    match atyp {
        ATYP_V4 => {
            if buf.len() < *pos + 4 {
                bail!("truncated ipv4 address");
            }
            let b: [u8; 4] = buf[*pos..*pos + 4].try_into().unwrap();
            *pos += 4;
            Ok(Address::V4(Ipv4Addr::from(b)))
        }
        ATYP_V6 => {
            if buf.len() < *pos + 16 {
                bail!("truncated ipv6 address");
            }
            let b: [u8; 16] = buf[*pos..*pos + 16].try_into().unwrap();
            *pos += 16;
            Ok(Address::V6(Ipv6Addr::from(b)))
        }
        ATYP_DOMAIN => {
            if buf.len() < *pos + 1 {
                bail!("truncated domain length");
            }
            let len = buf[*pos] as usize;
            *pos += 1;
            if len == 0 || buf.len() < *pos + len {
                bail!("truncated domain name");
            }
            let d = String::from_utf8(buf[*pos..*pos + len].to_vec())
                .context("domain name is not valid UTF-8")?;
            *pos += len;
            Ok(Address::Domain(d))
        }
        other => bail!("unknown address type {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_domain() {
        let d = Destination::new("example.com", 443);
        let mut buf = Vec::new();
        d.write_to(&mut buf);
        assert_eq!(buf[0], ATYP_DOMAIN);
        let mut pos = 1usize;
        let host = read_host_slice(&buf, &mut pos, buf[0]).unwrap();
        assert_eq!(host, Address::Domain("example.com".into()));
        assert_eq!(&buf[pos..], &[0x01, 0xbb]);
    }

    #[test]
    fn parse_variants() {
        assert_eq!(Destination::parse("1.2.3.4:80").unwrap().port, 80);
        assert_eq!(
            Destination::parse("[::1]:8080").unwrap().host,
            Address::V6(Ipv6Addr::LOCALHOST)
        );
        assert!(Destination::parse("nope").is_err());
    }
}
