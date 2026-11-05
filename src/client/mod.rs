//! Client side: a local SOCKS5 (and optional HTTP) proxy that tunnels through
//! one of the configured nodes. This also powers the mobile app's core.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tracing::{debug, info};

use crate::config::{Config, Inbound, Transport};
use crate::net::{boxed, relay::relay, socks5, BoxedIo};
use crate::proto::{hysteria2, trojan, vless, vmess, Destination};
use crate::share::NodeView;

struct QuicSession {
    /// Kept alive for the lifetime of the connection even though it is not read again.
    #[allow(dead_code)]
    endpoint: quinn::Endpoint,
    conn: quinn::Connection,
}

/// A ready-to-use tunnel to one node.
pub struct Client {
    pub node: NodeView,
    connector: tokio_rustls::TlsConnector,
    quic: Mutex<Option<Arc<QuicSession>>>,
    insecure: bool,
}

impl Client {
    pub fn new(node: NodeView) -> Result<Self> {
        let insecure = node.insecure;
        let cfg = crate::net::tls::client_config(&node.sni, insecure)?;
        Ok(Self {
            node,
            connector: tokio_rustls::TlsConnector::from(cfg),
            quic: Mutex::new(None),
            insecure,
        })
    }

    pub fn from_config(cfg: &Config, tag: Option<&str>) -> Result<Self> {
        let ib: &Inbound = match tag {
            Some(t) => cfg
                .inbounds
                .iter()
                .find(|i| i.tag == t)
                .with_context(|| format!("no inbound tagged `{t}`"))?,
            None => cfg
                .inbounds
                .iter()
                .find(|i| i.enable)
                .context("config has no enabled inbound")?,
        };
        let user = cfg.users.first().context("config has no users")?;
        Ok(Self::new(NodeView::from_inbound(cfg, ib, user))?)
    }

    fn addr(&self) -> String {
        format!("{}:{}", self.node.host, self.node.port)
    }

    async fn quic_session(&self) -> Result<Arc<QuicSession>> {
        let mut guard = self.quic.lock().await;
        if let Some(s) = guard.as_ref() {
            if s.conn.close_reason().is_none() {
                return Ok(s.clone());
            }
        }
        let tls = crate::net::tls::quic_client_config(self.insecure)?;
        let endpoint = crate::net::quic::client_endpoint(tls)?;
        let resolved: Vec<SocketAddr> = tokio::net::lookup_host(self.addr())
            .await
            .with_context(|| format!("cannot resolve {}", self.addr()))?
            .collect();
        let addr = resolved
            .iter()
            .find(|a| a.is_ipv4())
            .or_else(|| resolved.first())
            .copied()
            .context("no address")?;
        let conn = crate::net::quic::connect(&endpoint, addr, &self.node.sni).await?;

        // authenticate on the control stream
        let (mut send, mut recv) = conn.open_bi().await.context("opening control stream")?;
        hysteria2::write_auth(&mut send, &self.node.password).await?;
        hysteria2::read_auth_ok(&mut recv).await?;

        let session = Arc::new(QuicSession { endpoint, conn });
        *guard = Some(session.clone());
        Ok(session)
    }

    /// Establish a tunnelled stream to `dest`, already past the protocol handshake.
    pub async fn dial(&self, dest: &Destination) -> Result<BoxedIo> {
        if self.node.transport == Transport::Quic {
            let session = self.quic_session().await?;
            let (mut send, recv) = session
                .conn
                .open_bi()
                .await
                .context("opening QUIC stream")?;
            hysteria2::write_open(&mut send, dest).await?;
            return Ok(boxed(tokio::io::join(recv, send)));
        }

        let tcp = TcpStream::connect(self.addr())
            .await
            .with_context(|| format!("cannot connect to {}", self.addr()))?;
        crate::net::tune(&tcp);

        let io: BoxedIo = match self.node.transport {
            Transport::Raw => boxed(tcp),
            Transport::Tls | Transport::Ws => {
                let server_name = crate::net::tls::server_name(&self.node.sni)?;
                let tls = self
                    .connector
                    .connect(server_name, tcp)
                    .await
                    .context("TLS handshake with node failed")?;
                if self.node.transport == Transport::Ws {
                    boxed(crate::net::ws::connect(tls, &self.node.sni, &self.node.path, &[]).await?)
                } else {
                    boxed(tls)
                }
            }
            Transport::Quic => unreachable!(),
        };

        match self.node.protocol {
            crate::config::Protocol::Vless => {
                let req = vless::Request::tcp(self.node.uuid, dest.clone());
                let mut io = io;
                req.write(&mut io).await?;
                vless::read_response(&mut io).await?;
                Ok(io)
            }
            crate::config::Protocol::Trojan => {
                let req = trojan::Request::connect(dest.clone());
                let mut io = io;
                req.write(&mut io, &self.node.password).await?;
                Ok(io)
            }
            crate::config::Protocol::Vmess => {
                let key = vmess::CmdKey::new(&self.node.uuid);
                let (header, hdr) = vmess::encode_request(&key, vmess::CMD_TCP, dest);
                let mut io = io;
                io.write_all(&header).await?;
                io.flush().await?;
                vmess::read_response(&mut io, &key).await?;
                Ok(boxed(vmess::client_stream(io, &hdr)))
            }
            crate::config::Protocol::Hysteria2 => bail!("hysteria2 needs the QUIC transport"),
        }
    }

    /// Round-trip measurement through the tunnel: TCP connect + HTTP HEAD.
    pub async fn probe(&self, target: &str) -> Result<Duration> {
        let dest = Destination::parse(target)?;
        let started = Instant::now();
        let mut io = tokio::time::timeout(Duration::from_secs(8), self.dial(&dest))
            .await
            .map_err(|_| anyhow::anyhow!("timeout"))??;
        let req = format!(
            "HEAD / HTTP/1.1\r\nHost: {}\r\nUser-Agent: veil\r\nConnection: close\r\n\r\n",
            dest.host
        );
        io.write_all(req.as_bytes()).await?;
        io.flush().await?;
        let mut buf = [0u8; 64];
        let n = tokio::time::timeout(Duration::from_secs(8), io.read(&mut buf))
            .await
            .map_err(|_| anyhow::anyhow!("timeout"))??;
        if n == 0 {
            bail!("server closed the connection");
        }
        Ok(started.elapsed())
    }
}

/// Run the local SOCKS5 listener (and optionally an HTTP CONNECT listener).
pub async fn serve_local(client: Arc<Client>, listen: SocketAddr, http: Option<SocketAddr>) -> Result<()> {
    if let Some(http_addr) = http {
        let client = client.clone();
        tokio::spawn(async move {
            if let Err(e) = serve_http(client, http_addr).await {
                tracing::warn!("HTTP proxy stopped: {e}");
            }
        });
    }

    let listener = TcpListener::bind(listen)
        .await
        .with_context(|| format!("cannot bind local proxy on {listen}"))?;
    info!("local SOCKS5 proxy listening on socks5://{listen}");
    loop {
        let (stream, peer) = listener.accept().await?;
        let client = client.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_socks(client, stream).await {
                debug!(%peer, "socks session ended: {e}");
            }
        });
    }
}

async fn handle_socks(client: Arc<Client>, mut stream: TcpStream) -> Result<()> {
    let req = socks5::accept(&mut stream).await?;
    match req {
        socks5::SocksRequest::Connect(dest) => match client.dial(&dest).await {
            Ok(remote) => {
                socks5::reply(&mut stream, 0x00, None).await.ok();
                relay(stream, remote, None).await?;
            }
            Err(e) => {
                socks5::reply(&mut stream, 0x05, None).await.ok();
                return Err(e);
            }
        },
        socks5::SocksRequest::UdpAssociate => {
            socks5::reply(&mut stream, 0x07, None).await.ok();
            bail!("UDP associate is not supported by this client build");
        }
    }
    Ok(())
}

async fn serve_http(client: Arc<Client>, listen: SocketAddr) -> Result<()> {
    let listener = TcpListener::bind(listen).await?;
    info!("local HTTP proxy listening on http://{listen}");
    loop {
        let (stream, _) = listener.accept().await?;
        let client = client.clone();
        tokio::spawn(async move {
            let _ = handle_http(client, stream).await;
        });
    }
}

async fn handle_http(client: Arc<Client>, mut stream: TcpStream) -> Result<()> {
    let mut buf = vec![0u8; 8192];
    let n = stream.read(&mut buf).await?;
    if n == 0 {
        return Ok(());
    }
    let head = String::from_utf8_lossy(&buf[..n]).to_string();
    let first = head.lines().next().unwrap_or_default();
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();

    if method.eq_ignore_ascii_case("CONNECT") {
        let dest = Destination::parse(target)?;
        let mut remote = client.dial(&dest).await?;
        stream
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        stream.flush().await?;
        relay(stream, &mut remote, None).await?;
        return Ok(());
    }

    // Plain HTTP proxying: rewrite the request line to an origin-form path.
    let url = url::Url::parse(target)?;
    let host = url.host_str().context("no host in request")?;
    let port = url.port_or_known_default().unwrap_or(80);
    let dest = Destination::new(host, port);
    let path = if url.path().is_empty() { "/" } else { url.path() };
    let mut remote = client.dial(&dest).await?;
    let mut out = format!("{method} {path} HTTP/1.1\r\n");
    for line in head.lines().skip(1) {
        if line.to_ascii_lowercase().starts_with("proxy-connection") {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    remote.write_all(out.as_bytes()).await?;
    relay(stream, remote, None).await?;
    Ok(())
}
