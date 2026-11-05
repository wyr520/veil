//! VMess with AEAD headers (the format used by every modern v2ray/Xray client).
//!
//! Layout of a request:
//! ```text
//! auth_id(16) | enc(header_len)(2+16) | enc(header)(n+16) | body chunks
//! header = ver(1) | body_iv(16) | body_key(16) | resp_v(1) | option(1)
//!          | pad_len(1) | pad | cmd(1) | port(2) | atyp | addr
//! chunk  = len(2 BE) | ciphertext(len) | tag(16)      (AAD = the 2 length bytes)
//! ```
//! The response direction derives its keys by SHA-256 hashing the request
//! direction's key/IV, and starts a fresh chunk counter.

use std::io;
use std::pin::Pin;
use std::task::{ready, Context, Poll};

use anyhow::{anyhow, bail, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use uuid::Uuid;

use crate::crypto::{aes128_ecb_encrypt, gcm_open, gcm_seal, kdf16, md5, sha256};
use crate::proto::addr::Destination;

pub const VERSION: u8 = 1;
pub const CMD_TCP: u8 = 1;
pub const CMD_UDP: u8 = 2;

const CMD_KEY_SALT: &[u8] = b"c48619fe-8f02-49e0-b9e9-edf763e17e21";

const SALT_AUTH_ID: &[u8] = b"AES Auth ID Encryption";

const SALT_REQ_LEN_KEY: &[u8] = b"VMess Header AEAD Key_Length";
const SALT_REQ_LEN_NONCE: &[u8] = b"VMess Header AEAD Nonce_Length";
const SALT_REQ_KEY: &[u8] = b"VMess Header AEAD Key";
const SALT_REQ_NONCE: &[u8] = b"VMess Header AEAD Nonce";

const SALT_RESP_LEN_KEY: &[u8] = b"AEAD Resp Header Len Key";
const SALT_RESP_LEN_NONCE: &[u8] = b"AEAD Resp Header Len Nonce";
const SALT_RESP_KEY: &[u8] = b"AEAD Resp Header Key";
const SALT_RESP_NONCE: &[u8] = b"AEAD Resp Header Nonce";

/// Maximum plaintext carried by one body chunk.
pub const MAX_CHUNK: usize = 16384;

/// Per-user key derived from the UUID.
#[derive(Clone)]
pub struct CmdKey([u8; 16]);

impl CmdKey {
    pub fn new(uuid: &Uuid) -> Self {
        let mut buf = Vec::with_capacity(52);
        buf.extend_from_slice(uuid.as_bytes());
        buf.extend_from_slice(CMD_KEY_SALT);
        CmdKey(md5(&buf))
    }

    pub fn raw(&self) -> &[u8; 16] {
        &self.0
    }
}

impl std::fmt::Debug for CmdKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CmdKey(..)")
    }
}

#[derive(Debug, Clone)]
pub struct RequestHeader {
    pub response_v: u8,
    pub cmd: u8,
    pub dest: Destination,
    pub body_key: [u8; 16],
    pub body_iv: [u8; 16],
}

/// Build the full request header prefix (everything before the first body chunk).
pub fn encode_request(cmd_key: &CmdKey, cmd: u8, dest: &Destination) -> (Vec<u8>, RequestHeader) {
    let nonce: [u8; 16] = crate::crypto::random_array();
    let auth_id = aes128_ecb_encrypt(&kdf16(&cmd_key.0, &[SALT_AUTH_ID]), &nonce);

    let body_iv: [u8; 16] = crate::crypto::random_array();
    let body_key: [u8; 16] = crate::crypto::random_array();
    let mut response_v: u8 = rand_u8();
    if response_v == 0 {
        response_v = 1;
    }
    let pad_len = (rand_u8() as usize) % 16;
    let padding: Vec<u8> = (0..pad_len).map(|_| rand_u8()).collect();

    let mut plain = Vec::with_capacity(64);
    plain.push(VERSION);
    plain.extend_from_slice(&body_iv);
    plain.extend_from_slice(&body_key);
    plain.push(response_v);
    plain.push(0); // option
    plain.push(pad_len as u8);
    plain.extend_from_slice(&padding);
    plain.push(cmd);
    dest.write_port_first(&mut plain);

    let len_key = kdf16(&cmd_key.0, &[SALT_REQ_LEN_KEY, &auth_id]);
    let len_nonce = kdf16(&cmd_key.0, &[SALT_REQ_LEN_NONCE, &auth_id]);
    let hdr_key = kdf16(&cmd_key.0, &[SALT_REQ_KEY, &auth_id]);
    let hdr_nonce = kdf16(&cmd_key.0, &[SALT_REQ_NONCE, &auth_id]);

    let mut out = Vec::with_capacity(34 + plain.len() + 16);
    out.extend_from_slice(&auth_id);
    let enc_len = gcm_seal(&len_key, &len_nonce, &[], &(plain.len() as u16).to_be_bytes())
        .expect("aes-gcm seal cannot fail");
    out.extend_from_slice(&enc_len);
    let enc_hdr = gcm_seal(&hdr_key, &hdr_nonce, &[], &plain).expect("aes-gcm seal cannot fail");
    out.extend_from_slice(&enc_hdr);

    (
        out,
        RequestHeader { response_v, cmd, dest: dest.clone(), body_key, body_iv },
    )
}

fn rand_u8() -> u8 {
    let b: [u8; 1] = crate::crypto::random_array();
    b[0]
}

/// Server side step 1: try to read the length field with a candidate user key.
/// Returns the plaintext header length when the key belongs to that user.
pub fn try_open_request_len(
    cmd_key: &CmdKey,
    auth_id: &[u8; 16],
    enc_len: &[u8; 18],
) -> Option<usize> {
    let len_key = kdf16(&cmd_key.0, &[SALT_REQ_LEN_KEY, auth_id]);
    let len_nonce = kdf16(&cmd_key.0, &[SALT_REQ_LEN_NONCE, auth_id]);
    let plain = gcm_open(&len_key, &len_nonce, &[], enc_len).ok()?;
    if plain.len() != 2 {
        return None;
    }
    let n = u16::from_be_bytes([plain[0], plain[1]]) as usize;
    if n < 24 || n > 512 {
        return None;
    }
    Some(n)
}

/// Server side step 2: decrypt and parse the header body.
pub fn open_request_header(
    cmd_key: &CmdKey,
    auth_id: &[u8; 16],
    enc_header: &[u8],
) -> Result<RequestHeader> {
    let hdr_key = kdf16(&cmd_key.0, &[SALT_REQ_KEY, auth_id]);
    let hdr_nonce = kdf16(&cmd_key.0, &[SALT_REQ_NONCE, auth_id]);
    let plain = gcm_open(&hdr_key, &hdr_nonce, &[], enc_header)
        .map_err(|_| anyhow!("vmess: header authentication failed"))?;

    let mut pos = 0usize;
    let version = *plain.first().ok_or_else(|| anyhow!("empty vmess header"))?;
    pos += 1;
    if version != VERSION {
        bail!("vmess: unsupported version {version}");
    }
    if plain.len() < pos + 33 {
        bail!("vmess: truncated header");
    }
    let mut body_iv = [0u8; 16];
    body_iv.copy_from_slice(&plain[pos..pos + 16]);
    pos += 16;
    let mut body_key = [0u8; 16];
    body_key.copy_from_slice(&plain[pos..pos + 16]);
    pos += 16;
    let response_v = plain[pos];
    pos += 1;
    let option = plain[pos];
    pos += 1;
    if option & 0x01 != 0 {
        // Chunked-stream "standard" body vs legacy; both are supported here.
    }
    let pad_len = plain[pos] as usize;
    pos += 1;
    if plain.len() < pos + pad_len + 1 {
        bail!("vmess: truncated padding");
    }
    pos += pad_len;
    let cmd = plain[pos];
    pos += 1;
    let port = {
        if plain.len() < pos + 2 {
            bail!("vmess: truncated port");
        }
        let p = u16::from_be_bytes([plain[pos], plain[pos + 1]]);
        pos += 2;
        p
    };
    let atyp = plain[pos];
    pos += 1;
    let host = crate::proto::addr::read_host_slice(&plain, &mut pos, atyp)?;

    Ok(RequestHeader {
        response_v,
        cmd,
        dest: Destination { host, port },
        body_key,
        body_iv,
    })
}

/// Server → client response header: `enc(len)(2+16) | enc(4)(4+16)`.
pub fn encode_response(cmd_key: &CmdKey, response_v: u8) -> Vec<u8> {
    let len_key = kdf16(&cmd_key.0, &[SALT_RESP_LEN_KEY]);
    let len_nonce = kdf16(&cmd_key.0, &[SALT_RESP_LEN_NONCE]);
    let hdr_key = kdf16(&cmd_key.0, &[SALT_RESP_KEY]);
    let hdr_nonce = kdf16(&cmd_key.0, &[SALT_RESP_NONCE]);

    let plain = [response_v, 0u8, 0u8, 0u8]; // ver, option, cmd=none, cmd_len=0
    let mut out = Vec::with_capacity(40);
    out.extend_from_slice(
        &gcm_seal(&len_key, &len_nonce, &[], &(plain.len() as u16).to_be_bytes())
            .expect("seal"),
    );
    out.extend_from_slice(&gcm_seal(&hdr_key, &hdr_nonce, &[], &plain).expect("seal"));
    out
}

/// Client side: consume the 38-byte response header.
pub async fn read_response<R>(rd: &mut R, cmd_key: &CmdKey) -> Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut enc_len = [0u8; 18];
    rd.read_exact(&mut enc_len).await?;
    let len_key = kdf16(&cmd_key.0, &[SALT_RESP_LEN_KEY]);
    let len_nonce = kdf16(&cmd_key.0, &[SALT_RESP_LEN_NONCE]);
    let plain_len = gcm_open(&len_key, &len_nonce, &[], &enc_len)?;
    let n = u16::from_be_bytes([plain_len[0], plain_len[1]]) as usize;
    if n > 512 {
        bail!("vmess: absurd response header length {n}");
    }
    let mut enc = vec![0u8; n + 16];
    rd.read_exact(&mut enc).await?;
    let hdr_key = kdf16(&cmd_key.0, &[SALT_RESP_KEY]);
    let hdr_nonce = kdf16(&cmd_key.0, &[SALT_RESP_NONCE]);
    let plain = gcm_open(&hdr_key, &hdr_nonce, &[], &enc)?;
    if plain.len() < 3 {
        bail!("vmess: short response header");
    }
    if plain[2] != 0 {
        bail!("vmess: server responded with command {}", plain[2]);
    }
    Ok(())
}

fn derive_response_key(k: &[u8; 16]) -> [u8; 16] {
    let h = sha256(k);
    let mut out = [0u8; 16];
    out.copy_from_slice(&h[..16]);
    out
}

struct BodyWriter {
    key: [u8; 16],
    iv: [u8; 16],
    counter: u16,
}

impl BodyWriter {
    fn new(key: [u8; 16], iv: [u8; 16]) -> Self {
        Self { key, iv, counter: 0 }
    }

    fn nonce(&self) -> [u8; 12] {
        let mut n = [0u8; 12];
        n[0..2].copy_from_slice(&self.counter.to_be_bytes());
        n[2..12].copy_from_slice(&self.iv[2..12]);
        n
    }

    /// Encode one chunk (including the 2-byte length prefix) into `out`.
    fn seal_chunk(&mut self, data: &[u8], out: &mut Vec<u8>) {
        let len = (data.len() as u16).to_be_bytes();
        out.extend_from_slice(&len);
        let sealed = gcm_seal(&self.key, &self.nonce(), &len, data).expect("seal");
        out.extend_from_slice(&sealed);
        self.counter = self.counter.wrapping_add(1);
    }
}

struct BodyReader {
    key: [u8; 16],
    iv: [u8; 16],
    counter: u16,
    /// 2-byte length prefix
    hdr: [u8; 2],
    hdr_filled: usize,
    /// ciphertext + tag of the current chunk
    body: Vec<u8>,
    body_filled: usize,
    need: usize,
}

impl BodyReader {
    fn new(key: [u8; 16], iv: [u8; 16]) -> Self {
        Self {
            key,
            iv,
            counter: 0,
            hdr: [0; 2],
            hdr_filled: 0,
            body: Vec::new(),
            body_filled: 0,
            need: 0,
        }
    }

    fn nonce(&self) -> [u8; 12] {
        let mut n = [0u8; 12];
        n[0..2].copy_from_slice(&self.counter.to_be_bytes());
        n[2..12].copy_from_slice(&self.iv[2..12]);
        n
    }

    fn open_chunk(&mut self) -> io::Result<Vec<u8>> {
        let aad = self.hdr;
        let plain = gcm_open(&self.key, &self.nonce(), &aad, &self.body)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "vmess: body tag mismatch"))?;
        self.counter = self.counter.wrapping_add(1);
        Ok(plain)
    }
}

/// Adapts a VMess body stream into a plaintext `AsyncRead + AsyncWrite`.
pub struct AeadStream<S> {
    inner: S,
    reader: BodyReader,
    writer: BodyWriter,
    read_buf: Vec<u8>,
    read_pos: usize,
    write_buf: Vec<u8>,
    pending: Vec<u8>,
    pending_pos: usize,
    read_eof: bool,
}

impl<S> AeadStream<S> {
    pub fn into_inner(self) -> S {
        self.inner
    }
}

/// Client view: writes with the request key/IV, reads with the derived response keys.
pub fn client_stream<S>(inner: S, req: &RequestHeader) -> AeadStream<S> {
    AeadStream {
        inner,
        reader: BodyReader::new(derive_response_key(&req.body_key), derive_response_key(&req.body_iv)),
        writer: BodyWriter::new(req.body_key, req.body_iv),
        read_buf: Vec::new(),
        read_pos: 0,
        write_buf: Vec::new(),
        pending: Vec::new(),
        pending_pos: 0,
        read_eof: false,
    }
}

/// Server view: reads with the request key/IV, writes with the derived response keys.
pub fn server_stream<S>(inner: S, req: &RequestHeader) -> AeadStream<S> {
    AeadStream {
        inner,
        reader: BodyReader::new(req.body_key, req.body_iv),
        writer: BodyWriter::new(derive_response_key(&req.body_key), derive_response_key(&req.body_iv)),
        read_buf: Vec::new(),
        read_pos: 0,
        write_buf: Vec::new(),
        pending: Vec::new(),
        pending_pos: 0,
        read_eof: false,
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for AeadStream<S> {
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
            if me.read_eof {
                return Poll::Ready(Ok(()));
            }

            // Phase 1: the two plaintext length bytes.
            if me.reader.need == 0 {
                if me.reader.hdr_filled < 2 {
                    let start = me.reader.hdr_filled;
                    let mut tmp = ReadBuf::new(&mut me.reader.hdr[start..]);
                    ready!(Pin::new(&mut me.inner).poll_read(cx, &mut tmp))?;
                    let got = tmp.filled().len();
                    if got == 0 {
                        me.read_eof = true;
                        return Poll::Ready(Ok(()));
                    }
                    me.reader.hdr_filled += got;
                    continue;
                }
                let len = u16::from_be_bytes(me.reader.hdr) as usize;
                me.reader.hdr_filled = 0;
                if len == 0 {
                    // zero-length chunk marks a clean half-close
                    me.read_eof = true;
                    return Poll::Ready(Ok(()));
                }
                me.reader.need = len + 16;
                me.reader.body.clear();
                me.reader.body.resize(me.reader.need, 0);
                me.reader.body_filled = 0;
                continue;
            }

            // Phase 2: ciphertext + tag.
            if me.reader.body_filled < me.reader.need {
                let start = me.reader.body_filled;
                let mut tmp = ReadBuf::new(&mut me.reader.body[start..]);
                ready!(Pin::new(&mut me.inner).poll_read(cx, &mut tmp))?;
                let got = tmp.filled().len();
                if got == 0 {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "vmess: truncated chunk",
                    )));
                }
                me.reader.body_filled += got;
                continue;
            }

            let plain = me.reader.open_chunk()?;
            me.reader.need = 0;
            me.read_buf = plain;
            me.read_pos = 0;
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for AeadStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let me = self.get_mut();
        // Opportunistically flush what is already buffered to bound memory.
        if me.write_buf.len() >= MAX_CHUNK * 4 {
            ready!(me.drive_flush(cx))?;
        }
        me.write_buf.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        me.drive_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        ready!(me.drive_flush(cx))?;
        if me.pending_pos >= me.pending.len() {
            me.pending.clear();
            me.pending_pos = 0;
            // zero-length chunk = FIN
            me.writer.seal_chunk(&[], &mut me.pending);
        }
        while me.pending_pos < me.pending.len() {
            let n = ready!(Pin::new(&mut me.inner)
                .poll_write(cx, &me.pending[me.pending_pos..]))?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            me.pending_pos += n;
        }
        me.pending.clear();
        me.pending_pos = 0;
        Pin::new(&mut me.inner).poll_shutdown(cx)
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AeadStream<S> {
    fn drive_flush(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        loop {
            if self.pending_pos < self.pending.len() {
                let n = ready!(Pin::new(&mut self.inner)
                    .poll_write(cx, &self.pending[self.pending_pos..]))?;
                if n == 0 {
                    return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
                }
                self.pending_pos += n;
                continue;
            }
            if self.write_buf.is_empty() {
                self.pending.clear();
                self.pending_pos = 0;
                return Poll::Ready(Ok(()));
            }
            let take = std::cmp::min(MAX_CHUNK, self.write_buf.len());
            let chunk: Vec<u8> = self.write_buf.drain(..take).collect();
            self.pending.clear();
            self.pending_pos = 0;
            self.writer.seal_chunk(&chunk, &mut self.pending);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::addr::Address;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn cmd_key_is_deterministic() {
        let u = Uuid::parse_str("b831381d-6324-4d53-ad4f-8cda48b30811").unwrap();
        let a = CmdKey::new(&u);
        let b = CmdKey::new(&u);
        assert_eq!(a.0, b.0);
    }

    #[test]
    fn request_header_roundtrip() {
        let uuid = Uuid::new_v4();
        let key = CmdKey::new(&uuid);
        let dest = Destination::new(Address::Domain("example.org".into()), 443);
        let (bytes, hdr) = encode_request(&key, CMD_TCP, &dest);
        assert!(bytes.len() > 34);

        let auth_id: [u8; 16] = bytes[..16].try_into().unwrap();
        let enc_len: [u8; 18] = bytes[16..34].try_into().unwrap();
        let n = try_open_request_len(&key, &auth_id, &enc_len).expect("len opens");
        let enc_hdr = &bytes[34..34 + n + 16];
        let parsed = open_request_header(&key, &auth_id, enc_hdr).unwrap();
        assert_eq!(parsed.dest, dest);
        assert_eq!(parsed.body_key, hdr.body_key);
        assert_eq!(parsed.body_iv, hdr.body_iv);
        assert_eq!(parsed.response_v, hdr.response_v);
    }

    #[test]
    fn wrong_key_rejected() {
        let key = CmdKey::new(&Uuid::new_v4());
        let other = CmdKey::new(&Uuid::new_v4());
        let (bytes, _) = encode_request(&key, CMD_TCP, &Destination::new("a.com", 80));
        let auth_id: [u8; 16] = bytes[..16].try_into().unwrap();
        let enc_len: [u8; 18] = bytes[16..34].try_into().unwrap();
        assert!(try_open_request_len(&other, &auth_id, &enc_len).is_none());
    }

    #[tokio::test]
    async fn body_stream_roundtrip() {
        let uuid = Uuid::new_v4();
        let key = CmdKey::new(&uuid);
        let (mut a, mut b) = tokio::io::duplex(4096);
        let dest = Destination::new("example.net", 80);
        let (hdr_bytes, client_hdr) = encode_request(&key, CMD_TCP, &dest);

        // The request header travels in the clear (it is already AEAD sealed),
        // so it is written before the body adapter wraps the socket.
        a.write_all(&hdr_bytes).await.unwrap();

        let server_task = tokio::spawn(async move {
            let mut prefix = [0u8; 34];
            b.read_exact(&mut prefix).await.unwrap();
            let auth_id: [u8; 16] = prefix[..16].try_into().unwrap();
            let enc_len: [u8; 18] = prefix[16..].try_into().unwrap();
            let n = try_open_request_len(&key, &auth_id, &enc_len).unwrap();
            let mut enc = vec![0u8; n + 16];
            b.read_exact(&mut enc).await.unwrap();
            let req = open_request_header(&key, &auth_id, &enc).unwrap();
            let mut server = server_stream(&mut b, &req);
            let mut got = vec![0u8; 5];
            server.read_exact(&mut got).await.unwrap();
            got
        });

        let mut client = AeadStream {
            inner: &mut a,
            reader: BodyReader::new(
                derive_response_key(&client_hdr.body_key),
                derive_response_key(&client_hdr.body_iv),
            ),
            writer: BodyWriter::new(client_hdr.body_key, client_hdr.body_iv),
            read_buf: Vec::new(),
            read_pos: 0,
            write_buf: Vec::new(),
            pending: Vec::new(),
            pending_pos: 0,
            read_eof: false,
        };

        client.write_all(b"hello").await.unwrap();
        client.flush().await.unwrap();
        let got = server_task.await.unwrap();
        assert_eq!(&got, b"hello");
    }
}
