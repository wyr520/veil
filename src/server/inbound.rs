//! Inbound listeners and per-connection protocol dispatch.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info, warn};

use crate::config::{Inbound, Protocol, Transport};
use crate::net::{boxed, relay::relay, ws, BoxedIo, Traffic};
use crate::proto::{hysteria2, trojan, vless, vmess, Destination};
use crate::server::{outbound, ServerCtx};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(12);

/// Spawn the listener for one inbound. Runs until the process exits.
pub async fn run(ctx: Arc<ServerCtx>, ib: Inbound) -> Result<()> {
    if ib.transport == Transport::Quic {
        run_quic(ctx, ib).await
    } else {
        run_stream(ctx, ib).await
    }
}

async fn run_stream(ctx: Arc<ServerCtx>, ib: Inbound) -> Result<()> {
    let listener = TcpListener::bind(ib.listen)
        .await
        .with_context(|| format!("cannot bind tcp {} for `{}`", ib.listen, ib.tag))?;
    info!(
        tag = %ib.tag,
        listen = %ib.listen,
        protocol = %ib.protocol,
        transport = ib.transport.as_str(),
        "inbound ready"
    );
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                warn!("accept failed on {}: {e}", ib.tag);
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        crate::net::tune(&stream);
        let ctx = ctx.clone();
        let ib = ib.clone();
        tokio::spawn(async move {
            match handle_stream(ctx, ib.clone(), stream).await {
                Ok(()) => debug!(%peer, tag = %ib.tag, "connection closed"),
                Err(e) => debug!(%peer, tag = %ib.tag, "connection dropped: {e:#}"),
            }
        });
    }
}

async fn handle_stream(ctx: Arc<ServerCtx>, ib: Inbound, stream: tokio::net::TcpStream) -> Result<()> {
    let peer = stream.peer_addr().ok();
    let io: BoxedIo = match ib.transport {
        Transport::Raw => boxed(stream),
        Transport::Tls | Transport::Ws => {
            let accepted = tokio::time::timeout(HANDSHAKE_TIMEOUT, ctx.acceptor.accept(stream))
                .await
                .map_err(|_| anyhow::anyhow!("TLS handshake timeout"))?
                .context("TLS handshake failed")?;
            if ib.transport == Transport::Ws {
                boxed(ws::accept(accepted, &ib.path).await?)
            } else {
                boxed(accepted)
            }
        }
        Transport::Quic => unreachable!("quic handled separately"),
    };

    match ib.protocol {
        Protocol::Vless => vless_conn(ctx, io).await,
        Protocol::Trojan => trojan_conn(ctx, io).await,
        Protocol::Vmess => vmess_conn(ctx, io).await,
        Protocol::Hysteria2 => unreachable!("hysteria2 is quic only"),
    }
    .with_context(|| format!("inbound `{}` from {peer:?}", ib.tag))
}

async fn vless_conn(ctx: Arc<ServerCtx>, mut io: BoxedIo) -> Result<()> {
    let req = vless::Request::read(&mut io).await?;
    let user = {
        let cfg = ctx.config.read().await;
        cfg.user_by_uuid(&req.uuid).map(|u| u.name.clone())
    };
    let Some(user) = user else {
        bail!("vless: unknown uuid {}", req.uuid);
    };
    let traffic = ctx.traffic_for(&user);

    match req.cmd {
        vless::CMD_TCP => {
            let out = outbound::connect(&ctx, &req.dest).await?;
            vless::write_response(&mut io).await?;
            relay(io, out, Some(traffic)).await?;
        }
        vless::CMD_UDP => {
            vless::write_response(&mut io).await?;
            vless_udp(io, traffic).await?;
        }
        other => bail!("vless: command {other} is not supported"),
    }
    Ok(())
}

async fn vless_udp(io: BoxedIo, traffic: Arc<Traffic>) -> Result<()> {
    let (mut rd, mut wr) = tokio::io::split(io);
    let socket = Arc::new(outbound::udp_socket().await?);

    let up_sock = socket.clone();
    let up_traffic = traffic.clone();
    let upload = async move {
        loop {
            let frame = match vless::UdpFrame::read(&mut rd).await {
                Ok(f) => f,
                Err(_) => break,
            };
            if let Ok(addrs) = outbound::resolve(&frame.dest).await {
                if let Some(addr) = addrs.first() {
                    up_traffic.add_up(frame.payload.len() as u64);
                    let _ = up_sock.send_to(&frame.payload, addr).await;
                }
            }
        }
    };

    let down_sock = socket.clone();
    let download = async move {
        let mut buf = vec![0u8; 65535];
        loop {
            let (n, from) = match down_sock.recv_from(&mut buf).await {
                Ok(v) => v,
                Err(_) => break,
            };
            traffic.add_down(n as u64);
            let frame = vless::UdpFrame {
                dest: Destination::new(from.ip(), from.port()),
                payload: buf[..n].to_vec(),
            };
            if wr.write_all(&frame.encode()).await.is_err() {
                break;
            }
            let _ = wr.flush().await;
        }
    };

    tokio::select! {
        _ = upload => {}
        _ = download => {}
    }
    Ok(())
}

async fn trojan_conn(ctx: Arc<ServerCtx>, mut io: BoxedIo) -> Result<()> {
    let digest = trojan::peek_digest(&mut io).await?;
    let user = {
        let cfg = ctx.config.read().await;
        cfg.users
            .iter()
            .filter(|u| u.enable && !u.password.is_empty())
            .find(|u| {
                crate::config::constant_time_eq(
                    &digest,
                    trojan::password_digest(&u.password).as_bytes(),
                )
            })
            .map(|u| u.name.clone())
    };
    let Some(user) = user else {
        bail!("trojan: no user matches the presented password");
    };
    let req = trojan::read_after_digest(&mut io).await?;
    let traffic = ctx.traffic_for(&user);

    match req.cmd {
        trojan::CMD_CONNECT => {
            let out = outbound::connect(&ctx, &req.dest).await?;
            relay(io, out, Some(traffic)).await?;
        }
        trojan::CMD_UDP_ASSOCIATE => {
            trojan_udp(io, traffic).await?;
        }
        other => bail!("trojan: command {other} is not supported"),
    }
    Ok(())
}

async fn trojan_udp(io: BoxedIo, traffic: Arc<Traffic>) -> Result<()> {
    let (mut rd, mut wr) = tokio::io::split(io);
    let socket = Arc::new(outbound::udp_socket().await?);

    let up_sock = socket.clone();
    let up_traffic = traffic.clone();
    let upload = async move {
        loop {
            let packet = match trojan::UdpPacket::read(&mut rd).await {
                Ok(p) => p,
                Err(_) => break,
            };
            if let Ok(addrs) = outbound::resolve(&packet.dest).await {
                if let Some(addr) = addrs.first() {
                    up_traffic.add_up(packet.payload.len() as u64);
                    let _ = up_sock.send_to(&packet.payload, addr).await;
                }
            }
        }
    };

    let down_sock = socket.clone();
    let download = async move {
        let mut buf = vec![0u8; 65535];
        loop {
            let (n, from) = match down_sock.recv_from(&mut buf).await {
                Ok(v) => v,
                Err(_) => break,
            };
            traffic.add_down(n as u64);
            let packet = trojan::UdpPacket {
                dest: Destination::new(from.ip(), from.port()),
                payload: buf[..n].to_vec(),
            };
            if wr.write_all(&packet.encode()).await.is_err() {
                break;
            }
            let _ = wr.flush().await;
        }
    };

    tokio::select! {
        _ = upload => {}
        _ = download => {}
    }
    Ok(())
}

async fn vmess_conn(ctx: Arc<ServerCtx>, mut io: BoxedIo) -> Result<()> {
    let mut prefix = [0u8; 34];
    io.read_exact(&mut prefix).await?;
    let auth_id: [u8; 16] = prefix[..16].try_into().expect("16 bytes");
    let enc_len: [u8; 18] = prefix[16..].try_into().expect("18 bytes");

    let found = {
        let cfg = ctx.config.read().await;
        let mut found = None;
        for u in cfg.users.iter().filter(|u| u.enable) {
            let key = vmess::CmdKey::new(&u.uuid);
            if let Some(n) = vmess::try_open_request_len(&key, &auth_id, &enc_len) {
                found = Some((u.name.clone(), key, n));
                break;
            }
        }
        found
    };
    let Some((user, cmd_key, header_len)) = found else {
        tracing::trace!("vmess handshake prefix: {}", hex::encode(prefix));
        bail!("vmess: no user matches this handshake");
    };

    let mut enc_header = vec![0u8; header_len + 16];
    io.read_exact(&mut enc_header).await?;
    let req = vmess::open_request_header(&cmd_key, &auth_id, &enc_header)?;
    if req.cmd != vmess::CMD_TCP {
        bail!("vmess: command {} is not supported (tcp only)", req.cmd);
    }

    let traffic = ctx.traffic_for(&user);
    let out = outbound::connect(&ctx, &req.dest).await?;
    io.write_all(&vmess::encode_response(&cmd_key, req.response_v)).await?;
    io.flush().await?;

    let stream = vmess::server_stream(io, &req);
    relay(stream, out, Some(traffic)).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Hysteria2 / QUIC
// ---------------------------------------------------------------------------

type SessionMap = Arc<Mutex<HashMap<u32, mpsc::Sender<(Destination, Vec<u8>)>>>>;

async fn run_quic(ctx: Arc<ServerCtx>, ib: Inbound) -> Result<()> {
    let endpoint = crate::net::quic::server_endpoint(ib.listen, ctx.quic_tls.clone())?;
    info!(tag = %ib.tag, listen = %ib.listen, protocol = "hysteria2", "QUIC inbound ready");
    while let Some(incoming) = endpoint.accept().await {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            match incoming.await {
                Ok(conn) => {
                    let remote = conn.remote_address();
                    if let Err(e) = hysteria2_conn(ctx, conn).await {
                        debug!(%remote, "hysteria2 connection closed: {e}");
                    }
                }
                Err(e) => warn!("QUIC handshake failed: {e}"),
            }
        });
    }
    Ok(())
}

async fn hysteria2_conn(ctx: Arc<ServerCtx>, conn: quinn::Connection) -> Result<()> {
    let (mut send, mut recv) = tokio::time::timeout(HANDSHAKE_TIMEOUT, conn.accept_bi())
        .await
        .map_err(|_| anyhow::anyhow!("no control stream"))??;

    let password = hysteria2::read_auth(&mut recv).await?;
    let user = {
        let cfg = ctx.config.read().await;
        cfg.user_by_password(&password).map(|u| u.name.clone())
    };
    let Some(user) = user else {
        conn.close(1u32.into(), b"auth failed");
        bail!("hysteria2: bad password");
    };
    hysteria2::write_auth_ok(&mut send).await?;
    let traffic = ctx.traffic_for(&user);
    let sessions: SessionMap = Arc::new(Mutex::new(HashMap::new()));

    loop {
        tokio::select! {
            bi = conn.accept_bi() => {
                let (send, recv) = match bi {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let ctx = ctx.clone();
                let traffic = traffic.clone();
                tokio::spawn(async move {
                    if let Err(e) = hysteria2_tcp(ctx, send, recv, traffic).await {
                        debug!("hysteria2 stream: {e}");
                    }
                });
            }
            dg = conn.read_datagram() => {
                let data = match dg {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let (session, dest, payload) = match hysteria2::decode_udp_with_addr(&data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let tx = {
                    let mut guard = sessions.lock().await;
                    if let Some(tx) = guard.get(&session) {
                        tx.clone()
                    } else {
                        let (tx, rx) = mpsc::channel::<(Destination, Vec<u8>)>(256);
                        guard.insert(session, tx.clone());
                        let conn = conn.clone();
                        let traffic = traffic.clone();
                        tokio::spawn(udp_session(conn, session, rx, traffic));
                        tx
                    }
                };
                if tx.send((dest, payload.to_vec())).await.is_ok() {
                    traffic.add_up(payload.len() as u64);
                }
            }
        }
    }
    Ok(())
}

async fn hysteria2_tcp(
    ctx: Arc<ServerCtx>,
    send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    traffic: Arc<Traffic>,
) -> Result<()> {
    let dest = hysteria2::read_open(&mut recv).await?;
    let out = outbound::connect(&ctx, &dest).await?;
    let stream = tokio::io::join(recv, send);
    relay(stream, out, Some(traffic)).await?;
    Ok(())
}

async fn udp_session(
    conn: quinn::Connection,
    session: u32,
    mut rx: mpsc::Receiver<(Destination, Vec<u8>)>,
    traffic: Arc<Traffic>,
) {
    let Ok(socket) = tokio::net::UdpSocket::bind("0.0.0.0:0").await else {
        return;
    };
    let socket = Arc::new(socket);

    let reply_socket = socket.clone();
    let reply_conn = conn.clone();
    let reply_traffic = traffic.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65535];
        loop {
            let Ok((n, _from)) = reply_socket.recv_from(&mut buf).await else {
                break;
            };
            reply_traffic.add_down(n as u64);
            let frame = hysteria2::encode_udp(session, None, &buf[..n]);
            if reply_conn.send_datagram(frame.into()).is_err() {
                break;
            }
        }
    });

    while let Some((dest, payload)) = rx.recv().await {
        if let Ok(addrs) = outbound::resolve(&dest).await {
            if let Some(addr) = addrs.first() {
                let _ = socket.send_to(&payload, addr).await;
            }
        }
    }
}
