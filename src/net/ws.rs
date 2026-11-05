//! WebSocket transport. A proxy connection becomes a binary WebSocket stream so
//! that it survives CDNs and looks like an application socket.

use std::io;
use std::pin::Pin;
use std::task::{ready, Context, Poll};

use anyhow::{anyhow, Result};
use futures_util::{Sink, SinkExt, Stream};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{accept_hdr_async, client_async, WebSocketStream};

/// Bidirectional byte stream carried over WebSocket binary messages.
pub struct WsIo<S> {
    inner: WebSocketStream<S>,
    read_buf: Vec<u8>,
    read_pos: usize,
    eof: bool,
}

impl<S> WsIo<S> {
    pub fn into_inner(self) -> WebSocketStream<S> {
        self.inner
    }
}

/// Client handshake. `path` must include the leading `/`.
pub async fn connect<S>(stream: S, host: &str, path: &str, early_data: &[u8]) -> Result<WsIo<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let path = if path.is_empty() { "/" } else { path };
    let url = format!("ws://{host}{path}");
    let mut req = url
        .as_str()
        .into_client_request()
        .map_err(|e| anyhow!("building websocket request: {e}"))?;
    {
        let headers = req.headers_mut();
        headers.insert("User-Agent", HeaderValue::from_static("Mozilla/5.0"));
        headers.insert("Accept-Language", HeaderValue::from_static("en-US,en;q=0.9"));
    }
    let (mut ws, _resp) = client_async(req, stream)
        .await
        .map_err(|e| anyhow!("websocket handshake failed: {e}"))?;
    if !early_data.is_empty() {
        ws.send(Message::Binary(early_data.to_vec().into()))
            .await
            .map_err(|e| anyhow!("websocket write failed: {e}"))?;
    }
    Ok(WsIo {
        inner: ws,
        read_buf: Vec::new(),
        read_pos: 0,
        eof: false,
    })
}

/// Server handshake. Anything that is not a WebSocket upgrade on `path` gets a
/// plain `404`, which is what a normal web server would answer.
pub async fn accept<S>(stream: S, path: &str) -> Result<WsIo<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let wanted = path.to_string();
    let callback = move |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        if req.uri().path() != wanted {
            let mut err = ErrorResponse::new(Some("Not Found".to_string()));
            *err.status_mut() = StatusCode::NOT_FOUND;
            return Err(err);
        }
        Ok(resp)
    };
    let ws = accept_hdr_async(stream, callback)
        .await
        .map_err(|e| anyhow!("websocket upgrade failed: {e}"))?;
    Ok(WsIo {
        inner: ws,
        read_buf: Vec::new(),
        read_pos: 0,
        eof: false,
    })
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for WsIo<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        loop {
            if me.read_pos < me.read_buf.len() {
                let n = std::cmp::min(buf.remaining(), me.read_buf.len() - me.read_pos);
                buf.put_slice(&me.read_buf[me.read_pos..me.read_pos + n]);
                me.read_pos += n;
                if me.read_pos == me.read_buf.len() {
                    me.read_buf.clear();
                    me.read_pos = 0;
                }
                return Poll::Ready(Ok(()));
            }
            if me.eof {
                return Poll::Ready(Ok(()));
            }
            match ready!(Pin::new(&mut me.inner).poll_next(cx)) {
                Some(Ok(Message::Binary(data))) => {
                    me.read_buf = data.to_vec();
                    me.read_pos = 0;
                    if me.read_buf.is_empty() {
                        continue;
                    }
                }
                Some(Ok(Message::Text(text))) => {
                    me.read_buf = text.as_bytes().to_vec();
                    me.read_pos = 0;
                    if me.read_buf.is_empty() {
                        continue;
                    }
                }
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => continue,
                Some(Ok(Message::Close(_))) | Some(Ok(Message::Frame(_))) => {
                    me.eof = true;
                }
                Some(Err(WsError::ConnectionClosed)) | Some(Err(WsError::AlreadyClosed)) => {
                    me.eof = true;
                }
                Some(Err(e)) => {
                    return Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e.to_string())))
                }
                None => me.eof = true,
            }
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for WsIo<S> {
    // Each `write` becomes one binary frame and is flushed immediately: a proxy
    // relay must never sit on buffered bytes, or the tunnel would stall until
    // the connection closes.
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let me = self.get_mut();
        ready!(Pin::new(&mut me.inner).poll_ready(cx))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        Pin::new(&mut me.inner)
            .start_send(Message::Binary(buf.to_vec().into()))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        ready!(Pin::new(&mut me.inner).poll_flush(cx))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        Pin::new(&mut me.inner)
            .poll_flush(cx)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        Pin::new(&mut me.inner)
            .poll_close(cx)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
    }
}

/// Consume and discard a fixed number of bytes (protocol padding).
pub async fn discard<R: AsyncRead + Unpin>(rd: &mut R, n: usize) -> Result<()> {
    if n == 0 {
        return Ok(());
    }
    let mut buf = vec![0u8; n];
    rd.read_exact(&mut buf).await?;
    Ok(())
}
