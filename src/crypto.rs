//! Small crypto helpers shared by the protocol codecs.
//!
//! Only primitives that the proxy protocols themselves mandate live here:
//! HMAC-SHA256 based KDF (VMess), AES-128-GCM body/header sealing, SHA-224
//! password digests (Trojan) and the HMAC-SHA256 handshake used by Hysteria2.

use aes::cipher::{BlockEncrypt, KeyInit};
use aes::Aes128;
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes128Gcm, Nonce};
use anyhow::{anyhow, Result};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha224, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// VMess `KDF`: a nested HMAC-SHA256 construction, not a plain one.
///
/// ```text
/// inner  = HMAC-SHA256(key = "VMess AEAD KDF", msg = "")
/// output = HMAC-SHA256(key = inner, msg = key || path[0] || path[1] || ...)
/// ```
///
/// Skipping the inner step produces a self-consistent protocol that no existing
/// client can talk to — this is the single most common way to get VMess wrong.
pub fn vmess_kdf(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let inner = hmac_sha256(b"VMess AEAD KDF", b"");
    let mut mac = <HmacSha256 as Mac>::new_from_slice(&inner).expect("hmac accepts any key length");
    mac.update(key);
    for p in parts {
        mac.update(p);
    }
    let out = mac.finalize().into_bytes();
    let mut r = [0u8; 32];
    r.copy_from_slice(&out);
    r
}

/// VMess `KDF16`: [`vmess_kdf`] truncated to 16 bytes.
pub fn kdf16(key: &[u8], parts: &[&[u8]]) -> [u8; 16] {
    let full = vmess_kdf(key, parts);
    let mut k = [0u8; 16];
    k.copy_from_slice(&full[..16]);
    k
}

/// AES-128-ECB encryption of a single 16-byte block (VMess auth id).
pub fn aes128_ecb_encrypt(key: &[u8; 16], block: &[u8; 16]) -> [u8; 16] {
    let cipher = Aes128::new(key.into());
    let mut b = (*block).into();
    cipher.encrypt_block(&mut b);
    b.into()
}

/// AES-GCM wants a 12-byte nonce; VMess derives 16 bytes from HMAC, so the
/// first 12 are used (this matches what v2ray/Xray do).
fn nonce12(nonce: &[u8]) -> &[u8] {
    if nonce.len() > 12 {
        &nonce[..12]
    } else {
        nonce
    }
}

/// AES-128-GCM seal: returns ciphertext||tag.
pub fn gcm_seal(key: &[u8; 16], nonce: &[u8], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes128Gcm::new(key.into());
    cipher
        .encrypt(Nonce::from_slice(nonce12(nonce)), Payload { msg: plaintext, aad })
        .map_err(|_| anyhow!("aes-gcm seal failed"))
}

/// AES-128-GCM open: input is ciphertext||tag.
pub fn gcm_open(key: &[u8; 16], nonce: &[u8], aad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes128Gcm::new(key.into());
    cipher
        .decrypt(Nonce::from_slice(nonce12(nonce)), Payload { msg: ciphertext, aad })
        .map_err(|_| anyhow!("aes-gcm open failed"))
}

/// SHA-224 hex digest, the Trojan password representation.
pub fn sha224_hex(input: &[u8]) -> String {
    let mut h = Sha224::new();
    h.update(input);
    hex::encode(h.finalize())
}

/// SHA-256 digest bytes.
pub fn sha256(input: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(input);
    let out = h.finalize();
    let mut r = [0u8; 32];
    r.copy_from_slice(&out);
    r
}

/// MD5 digest bytes (VMess command key derivation — protocol requirement, not a security choice).
pub fn md5(input: &[u8]) -> [u8; 16] {
    use md5::Md5;
    let mut h = Md5::new();
    h.update(input);
    let out = h.finalize();
    let mut r = [0u8; 16];
    r.copy_from_slice(&out);
    r
}

/// Fill a buffer with cryptographically strong random bytes.
pub fn fill_random(buf: &mut [u8]) {
    use rand::RngCore;
    rand::thread_rng().fill_bytes(buf);
}

pub fn random_array<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    fill_random(&mut out);
    out
}

/// HMAC-SHA256 (Hysteria2 `Hysteria-Auth` response signature).
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(msg);
    let out = mac.finalize().into_bytes();
    let mut r = [0u8; 32];
    r.copy_from_slice(&out);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kdf_is_stable() {
        let k = kdf16(b"key", &[b"path"]);
        assert_eq!(hex::encode(k).len(), 32);
        let k2 = kdf16(b"key", &[b"path"]);
        assert_eq!(k, k2);
    }

    #[test]
    fn gcm_roundtrip() {
        let key = random_array::<16>();
        let nonce = random_array::<12>();
        let ct = gcm_seal(&key, &nonce, b"aad", b"hello world").unwrap();
        let pt = gcm_open(&key, &nonce, b"aad", &ct).unwrap();
        assert_eq!(pt, b"hello world");
    }

    #[test]
    fn sha224_matches_reference() {
        // echo -n "password" | sha224sum
        assert_eq!(
            sha224_hex(b"password"),
            "d63dc919e201d7bc4c825630d2cf25fdc93d4b2f0d46706d29038d01"
        );
    }
}
