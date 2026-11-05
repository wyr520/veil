//! Configuration model, defaults and validation.
//!
//! A veil deployment is described by a single TOML file (`/etc/veil/veil.toml`).
//! Everything the server needs — inbounds, users, TLS material, API — lives here.

use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Wire protocol spoken by an inbound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Vless,
    Trojan,
    Vmess,
    Hysteria2,
}

impl Protocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Protocol::Vless => "vless",
            Protocol::Trojan => "trojan",
            Protocol::Vmess => "vmess",
            Protocol::Hysteria2 => "hysteria2",
        }
    }

    /// Default port for a protocol when the operator does not pick one.
    pub fn default_port(&self) -> u16 {
        match self {
            Protocol::Vless => 443,
            Protocol::Trojan => 443,
            Protocol::Vmess => 443,
            Protocol::Hysteria2 => 8443,
        }
    }

    /// Does this protocol authenticate with a UUID (rather than a password)?
    pub fn uses_uuid(&self) -> bool {
        matches!(self, Protocol::Vless | Protocol::Vmess)
    }
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Protocol {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "vless" => Protocol::Vless,
            "trojan" => Protocol::Trojan,
            "vmess" => Protocol::Vmess,
            "hysteria2" | "hy2" => Protocol::Hysteria2,
            other => bail!("unknown protocol `{other}` (expected vless|trojan|vmess|hysteria2)"),
        })
    }
}

/// Layer stacked in front of the protocol codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    /// Raw TCP, no encryption. Only safe on a loopback / trusted link.
    Raw,
    /// TLS 1.3 (or 1.2) directly wrapping the protocol.
    Tls,
    /// WebSocket inside TLS — looks like a normal web app.
    Ws,
    /// QUIC/UDP. Mandatory for Hysteria2.
    Quic,
}

impl Transport {
    pub fn as_str(&self) -> &'static str {
        match self {
            Transport::Raw => "raw",
            Transport::Tls => "tls",
            Transport::Ws => "ws",
            Transport::Quic => "quic",
        }
    }
}

impl FromStr for Transport {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "raw" | "none" | "tcp" => Transport::Raw,
            "tls" => Transport::Tls,
            "ws" | "websocket" => Transport::Ws,
            "quic" | "udp" => Transport::Quic,
            other => bail!("unknown transport `{other}` (expected raw|tls|ws|quic)"),
        })
    }
}

fn default_true() -> bool {
    true
}

fn default_ws_path() -> String {
    "/veil".to_string()
}

fn default_sni_host() -> String {
    String::new()
}

/// Global server identity and TLS material.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSection {
    /// Human readable node name, used in share links and the UI.
    #[serde(default = "default_node_name")]
    pub name: String,
    /// Public domain pointing at this machine. Used as SNI + in share links.
    #[serde(default)]
    pub domain: String,
    /// PEM certificate chain.
    #[serde(default = "default_cert_path")]
    pub cert: PathBuf,
    /// PEM private key.
    #[serde(default = "default_key_path")]
    pub key: PathBuf,
    /// ECDSA P-256 keys are generated when `veil init` runs.
    #[serde(default)]
    pub cert_mode: CertMode,
    /// Contact address for ACME (informational when cert_mode = acme).
    #[serde(default)]
    pub acme_email: String,
    /// Accept self-signed certificates on the *client* side (testing only).
    #[serde(default)]
    pub insecure: bool,
}

fn default_node_name() -> String {
    "veil-node".to_string()
}

fn default_cert_path() -> PathBuf {
    PathBuf::from("/etc/veil/cert/fullchain.pem")
}

fn default_key_path() -> PathBuf {
    PathBuf::from("/etc/veil/cert/privkey.pem")
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            name: default_node_name(),
            domain: String::new(),
            cert: default_cert_path(),
            key: default_key_path(),
            cert_mode: CertMode::SelfSigned,
            acme_email: String::new(),
            insecure: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CertMode {
    /// Generate a self-signed certificate on first start (client must skip verification).
    #[default]
    SelfSigned,
    /// Operator drops cert/key files (Let's Encrypt, ZeroSSL, ...).
    File,
    /// Reserved: hooks into an ACME client such as certbot / acme.sh.
    Acme,
}

/// One listening socket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inbound {
    pub tag: String,
    pub protocol: Protocol,
    pub listen: SocketAddr,
    #[serde(default = "default_transport")]
    pub transport: Transport,
    #[serde(default = "default_ws_path")]
    pub path: String,
    /// Overrides `server.domain` for the SNI / Host header of this inbound.
    #[serde(default = "default_sni_host")]
    pub sni: String,
    #[serde(default = "default_true")]
    pub enable: bool,
}

fn default_transport() -> Transport {
    Transport::Tls
}

/// Where traffic goes once it leaves the proxy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundSection {
    #[serde(default)]
    pub mode: OutboundMode,
    /// Used when `mode = "socks"`.
    #[serde(default)]
    pub socks: String,
    /// Refuse outbound connections to these ports (SMTP abuse, etc.).
    #[serde(default = "default_blocked_ports")]
    pub blocked_ports: Vec<u16>,
    /// Refuse connections to loopback / private ranges.
    #[serde(default)]
    pub block_private: bool,
    /// Outbound connect timeout in seconds.
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout: u64,
}

fn default_blocked_ports() -> Vec<u16> {
    vec![25, 465, 587, 6667]
}

fn default_connect_timeout() -> u64 {
    10
}

impl Default for OutboundSection {
    fn default() -> Self {
        Self {
            mode: OutboundMode::Direct,
            socks: String::new(),
            blocked_ports: default_blocked_ports(),
            block_private: false,
            connect_timeout: default_connect_timeout(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OutboundMode {
    #[default]
    Direct,
    /// Forward everything through an upstream SOCKS5 (chained proxies).
    Socks,
    /// Accept the connection and immediately close it.
    Block,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiSection {
    /// 127.0.0.1 by default: expose it through a reverse proxy if you need it remote.
    #[serde(default = "default_api_listen")]
    pub listen: String,
    /// Bearer token for mutating endpoints.
    #[serde(default)]
    pub admin_token: String,
    /// Path prefix for the subscription endpoint.
    #[serde(default = "default_sub_path")]
    pub sub_path: String,
    #[serde(default = "default_true")]
    pub enable: bool,
}

fn default_api_listen() -> String {
    "127.0.0.1:8080".to_string()
}

fn default_sub_path() -> String {
    "/sub".to_string()
}

impl Default for ApiSection {
    fn default() -> Self {
        Self {
            listen: default_api_listen(),
            admin_token: String::new(),
            sub_path: default_sub_path(),
            enable: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogSection {
    #[serde(default = "default_log_level")]
    pub level: String,
    /// `stdout`, `stderr` or a file path.
    #[serde(default)]
    pub output: String,
}

fn default_log_level() -> String {
    "info".to_string()
}

impl Default for LogSection {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            output: String::new(),
        }
    }
}

/// A proxy account. One row feeds every inbound the user is allowed on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub name: String,
    pub uuid: Uuid,
    /// Trojan / Hysteria2 credential.
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub level: u8,
    #[serde(default = "default_true")]
    pub enable: bool,
    /// Soft quota in bytes (0 = unlimited). Enforced by the stats module.
    #[serde(default)]
    pub quota_bytes: u64,
    /// Per-user subscription token, regenerated by `veil user token`.
    #[serde(default)]
    pub sub_token: String,
}

impl User {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            uuid: Uuid::new_v4(),
            password: random_password(16),
            email: String::new(),
            level: 0,
            enable: true,
            quota_bytes: 0,
            sub_token: random_password(24).to_ascii_lowercase(),
        }
    }
}

/// Cryptographically strong password from the alphanumeric alphabet.
pub fn random_password(len: usize) -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub server: ServerSection,
    #[serde(default)]
    pub log: LogSection,
    #[serde(default)]
    pub outbound: OutboundSection,
    #[serde(default)]
    pub api: ApiSection,
    #[serde(default)]
    pub users: Vec<User>,
    #[serde(default)]
    pub inbounds: Vec<Inbound>,
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read config {}", path.display()))?;
        let cfg: Config = toml::from_str(&raw)
            .with_context(|| format!("invalid TOML in {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))?;
        // Credentials live in this file.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if self.inbounds.is_empty() {
            bail!("config has no [[inbounds]]; run `veil init` or add one");
        }
        let mut seen = std::collections::HashSet::new();
        for ib in &self.inbounds {
            if !seen.insert(ib.tag.clone()) {
                bail!("duplicate inbound tag `{}`", ib.tag);
            }
            match (ib.protocol, ib.transport) {
                (Protocol::Hysteria2, t) if t != Transport::Quic => {
                    bail!("inbound `{}`: hysteria2 requires transport = \"quic\"", ib.tag)
                }
                (p, Transport::Quic) if p != Protocol::Hysteria2 => {
                    bail!("inbound `{}`: only hysteria2 may use transport = \"quic\"", ib.tag)
                }
                _ => {}
            }
            if ib.transport != Transport::Raw && self.server.domain.is_empty() && ib.sni.is_empty() {
                bail!(
                    "inbound `{}` needs TLS but neither server.domain nor inbound.sni is set",
                    ib.tag
                );
            }
        }
        if self.users.is_empty() {
            bail!("config has no users; run `veil user add <name>`");
        }
        for u in &self.users {
            if u.name.trim().is_empty() {
                bail!("a user has an empty name");
            }
            if u.password.is_empty() && !u.uuid.is_nil() {
                // Trojan/Hysteria2 users need a password; derive nothing silently.
            }
        }
        Ok(())
    }

    pub fn user(&self, name: &str) -> Option<&User> {
        self.users.iter().find(|u| u.name == name)
    }

    pub fn user_mut(&mut self, name: &str) -> Option<&mut User> {
        self.users.iter_mut().find(|u| u.name == name)
    }

    /// Lookup tables rebuilt on config reload.
    pub fn user_by_uuid(&self, uuid: &Uuid) -> Option<&User> {
        self.users.iter().find(|u| u.enable && &u.uuid == uuid)
    }

    pub fn user_by_password(&self, password: &str) -> Option<&User> {
        self.users
            .iter()
            .find(|u| u.enable && !u.password.is_empty() && constant_time_eq(u.password.as_bytes(), password.as_bytes()))
    }

    pub fn user_by_sub_token(&self, token: &str) -> Option<&User> {
        self.users
            .iter()
            .find(|u| !u.sub_token.is_empty() && constant_time_eq(u.sub_token.as_bytes(), token.as_bytes()))
    }

    /// Build a ready-to-run single-node config for `domain`.
    pub fn bootstrap(domain: &str, email: &str) -> Self {
        let host = if domain.is_empty() { "127.0.0.1" } else { domain };
        let sni = host.to_string();
        let mut cfg = Config::default();
        cfg.server = ServerSection {
            name: format!("veil-{host}"),
            domain: host.to_string(),
            ..Default::default()
        };
        cfg.server.acme_email = email.to_string();
        cfg.api.admin_token = random_password(32);
        cfg.users = vec![User::new("default")];
        cfg.inbounds = vec![
            Inbound {
                tag: "vless-tls".into(),
                protocol: Protocol::Vless,
                listen: "0.0.0.0:443".parse().unwrap(),
                transport: Transport::Tls,
                path: default_ws_path(),
                sni: sni.clone(),
                enable: true,
            },
            Inbound {
                tag: "vless-ws".into(),
                protocol: Protocol::Vless,
                listen: "0.0.0.0:8443".parse().unwrap(),
                transport: Transport::Ws,
                path: "/veil".into(),
                sni: sni.clone(),
                enable: true,
            },
            Inbound {
                tag: "trojan-tls".into(),
                protocol: Protocol::Trojan,
                listen: "0.0.0.0:8444".parse().unwrap(),
                transport: Transport::Tls,
                path: default_ws_path(),
                sni: sni.clone(),
                enable: true,
            },
            Inbound {
                tag: "vmess-ws".into(),
                protocol: Protocol::Vmess,
                listen: "0.0.0.0:8445".parse().unwrap(),
                transport: Transport::Ws,
                path: "/vm".into(),
                sni: sni.clone(),
                enable: true,
            },
            Inbound {
                tag: "hysteria2".into(),
                protocol: Protocol::Hysteria2,
                listen: "0.0.0.0:8446".parse().unwrap(),
                transport: Transport::Quic,
                path: default_ws_path(),
                sni,
                enable: true,
            },
        ];
        cfg
    }
}

/// Compare two byte strings without early exit.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_is_valid() {
        let cfg = Config::bootstrap("node.example.com", "a@b.c");
        cfg.validate().unwrap();
        assert_eq!(cfg.inbounds.len(), 5);
    }

    #[test]
    fn rejects_hysteria2_over_tcp() {
        let mut cfg = Config::bootstrap("node.example.com", "");
        cfg.inbounds[0].protocol = Protocol::Hysteria2;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn toml_roundtrip() {
        let cfg = Config::bootstrap("node.example.com", "");
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.inbounds.len(), cfg.inbounds.len());
        assert_eq!(back.users[0].uuid, cfg.users[0].uuid);
    }
}
