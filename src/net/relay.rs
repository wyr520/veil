//! Byte pump between the client socket and the target socket, with accounting.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::Traffic;

const BUF: usize = 32 * 1024;

async fn pump<R, W>(rd: &mut R, wr: &mut W, counter: &AtomicU64, idle: Duration) -> io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; BUF];
    let mut total = 0u64;
    loop {
        let n = match tokio::time::timeout(idle, rd.read(&mut buf)).await {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "connection idle timeout",
                ))
            }
        };
        if n == 0 {
            break;
        }
        wr.write_all(&buf[..n]).await?;
        // Adapters (WebSocket, VMess AEAD) buffer on write; without an explicit
        // flush a small request would sit in memory until the connection ends.
        wr.flush().await?;
        counter.fetch_add(n as u64, Ordering::Relaxed);
        total += n as u64;
    }
    Ok(total)
}

/// Relay until both directions are done. Returns `(up, down)` byte counts.
pub async fn relay<A, B>(client: A, target: B, traffic: Option<Arc<Traffic>>) -> Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    relay_with_idle(client, target, traffic, Duration::from_secs(300)).await
}

pub async fn relay_with_idle<A, B>(
    client: A,
    target: B,
    traffic: Option<Arc<Traffic>>,
    idle: Duration,
) -> Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let (mut cr, mut cw) = tokio::io::split(client);
    let (mut tr, mut tw) = tokio::io::split(target);

    let up_counter = traffic
        .as_ref()
        .map(|t| &t.up)
        .unwrap_or(&GLOBAL_DUMMY_UP);
    let down_counter = traffic
        .as_ref()
        .map(|t| &t.down)
        .unwrap_or(&GLOBAL_DUMMY_UP);

    let up = async move {
        let n = pump(&mut cr, &mut tw, up_counter, idle).await;
        let _ = tw.shutdown().await;
        n
    };
    let down = async move {
        let n = pump(&mut tr, &mut cw, down_counter, idle).await;
        let _ = cw.shutdown().await;
        n
    };

    let (a, b) = tokio::join!(up, down);
    let up_bytes = match a {
        Ok(n) => n,
        Err(e) if is_benign(&e) => 0,
        Err(e) => return Err(e.into()),
    };
    let down_bytes = match b {
        Ok(n) => n,
        Err(e) if is_benign(&e) => 0,
        Err(e) => return Err(e.into()),
    };
    Ok((up_bytes, down_bytes))
}

static GLOBAL_DUMMY_UP: AtomicU64 = AtomicU64::new(0);

fn is_benign(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::TimedOut
    )
}
