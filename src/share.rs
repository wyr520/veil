//! Share links and subscription payloads (`vless://`, `trojan://`, `vmess://`,
//! `hysteria2://`) compatible with v2rayN, Shadowrocket, Hiddify, sing-box, ...

use base64::Engine;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::{Config, Inbound, Protocol, Transport, User};

#[derive(Debug, Clone)]
pub struct NodeView {
    pub remark: String,
    pub protocol: Protocol,
    pub transport: Transport,
    pub host: String,
    pub port: u16,
    pub sni: String,
    pub path: String,
    pub uuid: Uuid,
    pub password: String,
    pub insecure: bool,
}

impl NodeView {
    pub fn from_inbound(cfg: &Config, ib: &Inbound, user: &User) -> Self {
        let host = if cfg.server.domain.is_empty() {
            ib.listen.ip().to_string()
        } else {
            cfg.server.domain.clone()
        };
        let sni = if ib.sni.is_empty() { host.clone() } else { ib.sni.clone() };
        Self {
            remark: format!("{}-{}", cfg.server.name, ib.tag),
            protocol: ib.protocol,
            transport: ib.transport,
            host,
            port: ib.listen.port(),
            sni,
            path: ib.path.clone(),
            uuid: user.uuid,
            password: user.password.clone(),
            insecure: cfg.server.insecure || cfg.server.cert_mode == crate::config::CertMode::SelfSigned,
        }
    }

    fn security(&self) -> &'static str {
        match self.transport {
            Transport::Ws | Transport::Tls => "tls",
            Transport::Quic => "tls",
            Transport::Raw => "none",
        }
    }

    fn net(&self) -> &'static str {
        match self.transport {
            Transport::Ws => "ws",
            Transport::Quic => "quic",
            _ => "tcp",
        }
    }

    /// Build the URI for this node.
    pub fn uri(&self) -> String {
        let remark = urlencode(&self.remark);
        match self.protocol {
            Protocol::Vless => {
                let mut q: Vec<String> = vec![
                    "encryption=none".into(),
                    format!("security={}", self.security()),
                    format!("type={}", self.net()),
                    format!("sni={}", self.sni),
                    "fp=chrome".into(),
                ];
                if self.transport == Transport::Ws {
                    q.push(format!("path={}", urlencode(&self.path)));
                    q.push(format!("host={}", self.sni));
                }
                if self.insecure {
                    q.push("allowInsecure=1".into());
                }
                format!(
                    "vless://{}@{}:{}?{}#{}",
                    self.uuid,
                    self.host,
                    self.port,
                    q.join("&"),
                    remark
                )
            }
            Protocol::Trojan => {
                let mut q: Vec<String> = vec![
                    format!("security={}", self.security()),
                    format!("type={}", self.net()),
                    format!("sni={}", self.sni),
                ];
                if self.transport == Transport::Ws {
                    q.push(format!("path={}", urlencode(&self.path)));
                    q.push(format!("host={}", self.sni));
                }
                if self.insecure {
                    q.push("allowInsecure=1".into());
                }
                format!(
                    "trojan://{}@{}:{}?{}#{}",
                    urlencode(&self.password),
                    self.host,
                    self.port,
                    q.join("&"),
                    remark
                )
            }
            Protocol::Vmess => {
                let json = serde_json::json!({
                    "v": "2",
                    "ps": self.remark,
                    "add": self.host,
                    "port": self.port.to_string(),
                    "id": self.uuid.to_string(),
                    "aid": "0",
                    "scy": "auto",
                    "net": self.net(),
                    "type": "none",
                    "host": if self.transport == Transport::Ws { self.sni.clone() } else { String::new() },
                    "path": if self.transport == Transport::Ws { self.path.clone() } else { String::new() },
                    "tls": if self.transport == Transport::Raw { "" } else { "tls" },
                    "sni": self.sni,
                    "alpn": "",
                    "allowInsecure": self.insecure,
                });
                format!(
                    "vmess://{}",
                    base64::engine::general_purpose::STANDARD.encode(json.to_string())
                )
            }
            Protocol::Hysteria2 => {
                let mut q = vec![format!("sni={}", self.sni)];
                if self.insecure {
                    q.push("insecure=1".into());
                }
                format!(
                    "hysteria2://{}@{}:{}?{}#{}",
                    urlencode(&self.password),
                    self.host,
                    self.port,
                    q.join("&"),
                    remark
                )
            }
        }
    }
}

/// Keep `- . _ ~` readable in remarks, escape everything a URL parser would choke on.
const FRAGMENT: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'`')
    .add(b'#')
    .add(b'?')
    .add(b'{')
    .add(b'}')
    .add(b'%')
    .add(b'\\')
    .add(b'^')
    .add(b'|')
    .add(b'&')
    .add(b'=');

fn urlencode(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, FRAGMENT).to_string()
}

/// All nodes of `user`, in config order.
pub fn user_nodes(cfg: &Config, user: &User) -> Vec<NodeView> {
    cfg.inbounds
        .iter()
        .filter(|ib| ib.enable)
        .map(|ib| NodeView::from_inbound(cfg, ib, user))
        .collect()
}

/// Base64 subscription body (what clients paste as a "subscription URL").
pub fn subscription(cfg: &Config, user: &User) -> String {
    let body = user_nodes(cfg, user)
        .iter()
        .map(|n| n.uri())
        .collect::<Vec<_>>()
        .join("\n");
    base64::engine::general_purpose::STANDARD.encode(body)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeJson {
    pub tag: String,
    pub protocol: String,
    pub transport: String,
    pub host: String,
    pub port: u16,
    pub sni: String,
    pub path: String,
    pub uri: String,
}

pub fn nodes_json(cfg: &Config, user: &User) -> Vec<NodeJson> {
    cfg.inbounds
        .iter()
        .filter(|ib| ib.enable)
        .map(|ib| {
            let v = NodeView::from_inbound(cfg, ib, user);
            NodeJson {
                tag: ib.tag.clone(),
                protocol: ib.protocol.to_string(),
                transport: ib.transport.as_str().to_string(),
                host: v.host.clone(),
                port: v.port,
                sni: v.sni.clone(),
                path: v.path.clone(),
                uri: v.uri(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vless_uri_shape() {
        let cfg = Config::bootstrap("node.example.com", "");
        let user = &cfg.users[0];
        let n = NodeView::from_inbound(&cfg, &cfg.inbounds[0], user);
        let uri = n.uri();
        assert!(uri.starts_with("vless://"));
        assert!(uri.contains("security=tls"));
        assert!(uri.contains("sni=node.example.com"));
        assert!(uri.ends_with("#veil-node.example.com-vless-tls"));
    }

    #[test]
    fn vmess_json_is_base64() {
        let cfg = Config::bootstrap("node.example.com", "");
        let n = NodeView::from_inbound(&cfg, &cfg.inbounds[3], &cfg.users[0]);
        let uri = n.uri();
        let payload = uri.strip_prefix("vmess://").unwrap();
        let raw = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(v["net"], "ws");
        assert_eq!(v["add"], "node.example.com");
    }

    #[test]
    fn hysteria2_uri() {
        let cfg = Config::bootstrap("node.example.com", "");
        let n = NodeView::from_inbound(&cfg, &cfg.inbounds[4], &cfg.users[0]);
        assert!(n.uri().starts_with("hysteria2://"));
    }
}
