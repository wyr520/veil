//! TLS plumbing: certificate generation, server/client configs, and the
//! "accept any certificate" mode used while a node still has a self-signed cert.

use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, ServerConfig, SignatureScheme};

/// Install the `ring` crypto provider exactly once per process.
pub fn init_crypto() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Write a self-signed CA plus a leaf certificate for `domain` if the files are
/// missing. A CA/leaf pair (instead of a single CA:false self-signed cert) is
/// what lets strict clients — Go's crypto/x509, Chrome, iOS — trust the node by
/// importing `<cert_dir>/ca.pem`.
pub fn ensure_self_signed(cert_path: &Path, key_path: &Path, domain: &str) -> Result<bool> {
    if cert_path.exists() && key_path.exists() {
        return Ok(false);
    }
    if let Some(parent) = cert_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    if let Some(parent) = key_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let host = if domain.is_empty() { "localhost" } else { domain };
    let mut sans: Vec<String> = vec![host.to_string()];
    if host.parse::<std::net::IpAddr>().is_ok() {
        sans.push("localhost".to_string());
    } else {
        sans.push("127.0.0.1".to_string());
        sans.push("localhost".to_string());
    }

    // ---- CA ----
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new())
        .context("building CA parameters")?;
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::CrlSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "veil self-signed CA");
    let ca_key = rcgen::KeyPair::generate().context("generating CA key")?;
    let ca_cert = ca_params.self_signed(&ca_key).context("signing CA")?;

    // ---- leaf ----
    let mut leaf_params = rcgen::CertificateParams::new(sans).context("building leaf parameters")?;
    leaf_params.is_ca = rcgen::IsCa::NoCa;
    leaf_params.key_usages = vec![
        rcgen::KeyUsagePurpose::DigitalSignature,
        rcgen::KeyUsagePurpose::KeyEncipherment,
    ];
    leaf_params.extended_key_usages = vec![
        rcgen::ExtendedKeyUsagePurpose::ServerAuth,
        rcgen::ExtendedKeyUsagePurpose::ClientAuth,
    ];
    leaf_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, host);
    let leaf_key = rcgen::KeyPair::generate().context("generating leaf key")?;
    let leaf_cert = leaf_params
        .signed_by(&leaf_key, &ca_cert, &ca_key)
        .context("signing leaf certificate")?;

    std::fs::write(cert_path, format!("{}{}", leaf_cert.pem(), ca_cert.pem()))
        .with_context(|| format!("writing {}", cert_path.display()))?;
    std::fs::write(key_path, leaf_key.serialize_pem())
        .with_context(|| format!("writing {}", key_path.display()))?;
    if let Some(dir) = cert_path.parent() {
        std::fs::write(dir.join("ca.pem"), ca_cert.pem()).ok();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(true)
}

fn read_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("opening certificate {}", path.display()))?;
    let mut rd = BufReader::new(file);
    let certs: Vec<_> = rustls_pemfile::certs(&mut rd)
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("parsing {}", path.display()))?;
    if certs.is_empty() {
        return Err(anyhow!("no certificates found in {}", path.display()));
    }
    Ok(certs)
}

fn read_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("opening private key {}", path.display()))?;
    let mut rd = BufReader::new(file);
    rustls_pemfile::private_key(&mut rd)
        .with_context(|| format!("parsing {}", path.display()))?
        .ok_or_else(|| anyhow!("no private key found in {}", path.display()))
}

/// Server config with ALPN h2/http1.1 so the endpoint looks like a web server.
pub fn server_config(cert_path: &Path, key_path: &Path) -> Result<Arc<ServerConfig>> {
    init_crypto();
    let certs = read_certs(cert_path)?;
    let key = read_key(key_path)?;
    let cfg = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("building TLS server config")?;
    // ALPN is deliberately left empty: many clients (Xray, v2rayN, Shadowrocket)
    // send no ALPN at all, and rustls aborts the handshake when the lists do
    // not overlap. Accepting any (or no) ALPN is what makes the node usable.
    Ok(Arc::new(cfg))
}

/// Server config for the QUIC endpoint (ALPN `h3`, mandatory for Hysteria2).
pub fn quic_server_config(cert_path: &Path, key_path: &Path) -> Result<Arc<ServerConfig>> {
    init_crypto();
    let certs = read_certs(cert_path)?;
    let key = read_key(key_path)?;
    let mut cfg = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("building QUIC TLS config")?;
    cfg.alpn_protocols = vec![crate::proto::hysteria2::ALPN.to_vec()];
    Ok(Arc::new(cfg))
}

/// Client config. `insecure` accepts self-signed certificates.
pub fn client_config(sni: &str, insecure: bool) -> Result<Arc<ClientConfig>> {
    init_crypto();
    let mut cfg = if insecure {
        ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify::new()?))
            .with_no_client_auth()
    } else {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth()
    };
    let _ = sni;
    cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(Arc::new(cfg))
}

/// Client config for the QUIC endpoint.
pub fn quic_client_config(insecure: bool) -> Result<Arc<ClientConfig>> {
    init_crypto();
    let mut cfg = if insecure {
        ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify::new()?))
            .with_no_client_auth()
    } else {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth()
    };
    cfg.alpn_protocols = vec![crate::proto::hysteria2::ALPN.to_vec()];
    Ok(Arc::new(cfg))
}

/// Turn `host` into a `ServerName` (IP literals are supported).
pub fn server_name(host: &str) -> Result<ServerName<'static>> {
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Ok(ServerName::IpAddress(ip.into()));
    }
    ServerName::try_from(host.to_string())
        .map_err(|_| anyhow!("`{host}` is not a valid server name"))
}

#[derive(Debug)]
struct NoVerify(Arc<rustls::crypto::CryptoProvider>);

impl NoVerify {
    fn new() -> Result<Self> {
        let provider = rustls::crypto::CryptoProvider::get_default()
            .cloned()
            .ok_or_else(|| anyhow!("no crypto provider installed"))?;
        Ok(Self(provider))
    }
}

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0
            .signature_verification_algorithms
            .supported_schemes()
    }
}
