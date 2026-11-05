//! Command line surface. Kept separate from `main` so it can be unit tested.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::config::{Config, User};
use crate::share;

#[derive(Parser, Debug)]
#[command(
    name = "veil",
    version,
    about = "Veil — VLESS / Trojan / VMess / Hysteria2 proxy suite",
    long_about = None
)]
pub struct Cli {
    /// Path to veil.toml (defaults to /etc/veil/veil.toml, then ./veil.toml)
    #[arg(short = 'c', long, global = true, env = "VEIL_CONFIG")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Create a new node config with fresh credentials and a self-signed cert.
    Init(InitArgs),
    /// Run the proxy server.
    Run,
    /// Manage proxy accounts.
    User {
        #[command(subcommand)]
        cmd: UserCmd,
    },
    /// Print share links (vless://, trojan://, vmess://, hysteria2://).
    Link {
        #[arg(short, long)]
        user: Option<String>,
        /// Emit the base64 subscription blob instead of one URI per line.
        #[arg(long)]
        sub: bool,
    },
    /// Validate the configuration and print a summary.
    Check,
    /// Run a local SOCKS5/HTTP proxy that tunnels through this node.
    Client {
        /// Inbound tag to use as the upstream node.
        #[arg(short, long)]
        node: Option<String>,
        /// Local SOCKS5 listen address.
        #[arg(long, default_value = "127.0.0.1:1080")]
        listen: SocketAddr,
        /// Also serve an HTTP CONNECT proxy on this address.
        #[arg(long)]
        http: Option<SocketAddr>,
    },
    /// Measure the round trip time through a node.
    Ping {
        #[arg(short, long)]
        node: Option<String>,
        #[arg(long, default_value = "www.gstatic.com:80")]
        target: String,
        #[arg(long, default_value_t = 3)]
        count: u32,
    },
    /// Certificate helpers.
    Cert {
        #[command(subcommand)]
        cmd: CertCmd,
    },
    /// Remove veil from this machine: service, binary, config, state.
    Uninstall {
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
        /// Keep the config directory (accounts + certificates).
        #[arg(long)]
        keep_config: bool,
    },
    /// Print version and build information.
    Version,
}

#[derive(Subcommand, Debug)]
pub enum UserCmd {
    /// Add a user and print its links.
    Add {
        name: String,
        #[arg(long, default_value = "")]
        email: String,
    },
    /// List users.
    List,
    /// Delete a user.
    Del { name: String },
    /// Rotate a user's UUID, password and subscription token.
    Rotate { name: String },
    /// Enable or disable a user.
    Enable { name: String, enable: bool },
}

#[derive(Subcommand, Debug)]
pub enum CertCmd {
    /// Show the certificate path, SANs and expiry.
    Show,
    /// Regenerate a self-signed certificate.
    SelfSigned {
        #[arg(long)]
        domain: Option<String>,
    },
}

#[derive(clap::Args, Debug)]
pub struct InitArgs {
    /// Public domain pointing at this machine (used for SNI and links).
    #[arg(long, default_value = "")]
    pub domain: String,
    /// Contact e-mail (ACME / notices).
    #[arg(long, default_value = "")]
    pub email: String,
    /// Overwrite an existing config.
    #[arg(long)]
    pub force: bool,
}

fn config_path(cli: &Cli) -> Result<PathBuf> {
    crate::server::resolve_config(cli.config.clone())
}

pub fn init_logging(level: &str) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("veil={level},quinn=warn,rustls=warn")));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

/// Entry point used by `main`.
pub async fn dispatch(cli: Cli) -> Result<()> {
    match &cli.cmd {
        Cmd::Init(args) => cmd_init(&cli, args),
        Cmd::Run => {
            let path = config_path(&cli)?;
            let level = Config::load(&path)
                .map(|c| c.log.level)
                .unwrap_or_else(|_| "info".to_string());
            init_logging(&level);
            crate::server::run(path).await
        }
        Cmd::User { cmd } => cmd_user(&cli, cmd),
        Cmd::Link { user, sub } => cmd_link(&cli, user.as_deref(), *sub),
        Cmd::Check => cmd_check(&cli),
        Cmd::Client { node, listen, http } => {
            init_logging("info");
            let path = config_path(&cli)?;
            let cfg = Config::load(&path)?;
            let client = crate::client::Client::from_config(&cfg, node.as_deref())?;
            let client = std::sync::Arc::new(client);
            println!(
                "tunnelling through {} ({})",
                client.node.remark, client.node.protocol
            );
            crate::client::serve_local(client, *listen, *http).await
        }
        Cmd::Ping { node, target, count } => {
            let path = config_path(&cli)?;
            let cfg = Config::load(&path)?;
            let client = crate::client::Client::from_config(&cfg, node.as_deref())?;
            let mut ok = 0;
            for i in 0..*count {
                match client.probe(target).await {
                    Ok(d) => {
                        ok += 1;
                        println!("probe {}/{}: {:.1} ms", i + 1, count, d.as_secs_f64() * 1000.0);
                    }
                    Err(e) => println!("probe {}/{}: failed ({e})", i + 1, count),
                }
            }
            if ok == 0 {
                bail!("all probes failed");
            }
            Ok(())
        }
        Cmd::Cert { cmd } => cmd_cert(&cli, cmd),
        Cmd::Uninstall { yes, keep_config } => cmd_uninstall(&cli, *yes, *keep_config),
        Cmd::Version => {
            println!("veil {}", crate::VERSION);
            println!("arch: {}-{}", std::env::consts::ARCH, std::env::consts::OS);
            Ok(())
        }
    }
}

fn cmd_init(cli: &Cli, args: &InitArgs) -> Result<()> {
    let path = cli
        .config
        .clone()
        .unwrap_or_else(|| PathBuf::from("/etc/veil/veil.toml"));
    if path.exists() && !args.force {
        bail!(
            "{} already exists (use --force to overwrite)",
            path.display()
        );
    }
    let cfg = {
        let mut cfg = Config::bootstrap(&args.domain, &args.email);
        // Keep certificate material next to the config file so that a custom
        // `-c /tmp/x/veil.toml` does not try to write into /etc.
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                cfg.server.cert = dir.join("cert").join("fullchain.pem");
                cfg.server.key = dir.join("cert").join("privkey.pem");
            }
        }
        cfg
    };
    cfg.validate()?;
    let generated =
        crate::net::tls::ensure_self_signed(&cfg.server.cert, &cfg.server.key, &cfg.server.domain)?;
    cfg.save(&path)?;

    println!("veil node created: {}", path.display());
    println!("  domain      : {}", if cfg.server.domain.is_empty() { "(none)" } else { &cfg.server.domain });
    println!("  certificate : {}{}", cfg.server.cert.display(), if generated { " (self-signed)" } else { "" });
    println!("  admin token : {}", cfg.api.admin_token);
    println!();
    println!("inbounds:");
    for ib in &cfg.inbounds {
        println!(
            "  {:<12} {:<9} {:<5} {}",
            ib.tag,
            ib.protocol.as_str(),
            ib.transport.as_str(),
            ib.listen
        );
    }
    println!();
    println!("links for user `{}`:", cfg.users[0].name);
    for n in share::user_nodes(&cfg, &cfg.users[0]) {
        println!("  {}", n.uri());
    }
    println!();
    println!("next:");
    println!("  veil check -c {}", path.display());
    println!("  veil run   -c {}", path.display());
    Ok(())
}

fn load_for_user(cli: &Cli, user: Option<&str>) -> Result<(PathBuf, Config, String)> {
    let path = config_path(cli)?;
    let cfg = Config::load(&path)?;
    let name = match user {
        Some(u) => u.to_string(),
        None => cfg
            .users
            .first()
            .map(|u| u.name.clone())
            .context("config has no users")?,
    };
    if cfg.user(&name).is_none() {
        bail!("no such user `{name}`");
    }
    Ok((path, cfg, name))
}

fn cmd_user(cli: &Cli, cmd: &UserCmd) -> Result<()> {
    let path = config_path(cli)?;
    let mut cfg = Config::load(&path)?;
    match cmd {
        UserCmd::Add { name, email } => {
            if cfg.user(name).is_some() {
                bail!("user `{name}` already exists");
            }
            let mut user = User::new(name);
            user.email = email.clone();
            cfg.users.push(user.clone());
            cfg.save(&path)?;
            println!("user `{name}` created");
            println!("  uuid     : {}", user.uuid);
            println!("  password : {}", user.password);
            println!("  sub token: {}", user.sub_token);
            println!();
            for n in share::user_nodes(&cfg, &user) {
                println!("{}", n.uri());
            }
        }
        UserCmd::List => {
            println!("{:<16} {:<10} {:<38} {}", "NAME", "ENABLED", "UUID", "EMAIL");
            for u in &cfg.users {
                println!(
                    "{:<16} {:<10} {:<38} {}",
                    u.name,
                    if u.enable { "yes" } else { "no" },
                    u.uuid,
                    u.email
                );
            }
        }
        UserCmd::Del { name } => {
            let before = cfg.users.len();
            cfg.users.retain(|u| &u.name != name);
            if cfg.users.len() == before {
                bail!("no such user `{name}`");
            }
            cfg.save(&path)?;
            println!("user `{name}` removed");
        }
        UserCmd::Rotate { name } => {
            let user = cfg.user_mut(name).with_context(|| format!("no such user `{name}`"))?;
            user.uuid = uuid::Uuid::new_v4();
            user.password = crate::config::random_password(16);
            user.sub_token = crate::config::random_password(24).to_ascii_lowercase();
            let snapshot = user.clone();
            cfg.save(&path)?;
            println!("rotated `{name}`");
            println!("  uuid     : {}", snapshot.uuid);
            println!("  password : {}", snapshot.password);
        }
        UserCmd::Enable { name, enable } => {
            let user = cfg.user_mut(name).with_context(|| format!("no such user `{name}`"))?;
            user.enable = *enable;
            cfg.save(&path)?;
            println!("user `{name}` {}", if *enable { "enabled" } else { "disabled" });
        }
    }
    Ok(())
}

fn cmd_link(cli: &Cli, user: Option<&str>, sub: bool) -> Result<()> {
    let (_, cfg, name) = load_for_user(cli, user)?;
    let u = cfg.user(&name).expect("checked");
    if sub {
        println!("{}", share::subscription(&cfg, u));
    } else {
        for n in share::user_nodes(&cfg, u) {
            println!("{}", n.uri());
        }
    }
    Ok(())
}

fn cmd_check(cli: &Cli) -> Result<()> {
    let path = config_path(cli)?;
    let cfg = Config::load(&path)?;
    println!("config : {}", path.display());
    println!("node   : {}", cfg.server.name);
    println!("domain : {}", if cfg.server.domain.is_empty() { "(none)" } else { &cfg.server.domain });
    println!("cert   : {}", cfg.server.cert.display());
    println!("users  : {}", cfg.users.len());
    println!("inbounds:");
    for ib in &cfg.inbounds {
        println!(
            "  {:<14} {:<9} {:<5} {:<22} path={} sni={}",
            ib.tag,
            ib.protocol.as_str(),
            ib.transport.as_str(),
            ib.listen.to_string(),
            ib.path,
            if ib.sni.is_empty() { "-" } else { &ib.sni }
        );
    }
    println!("ok: configuration is valid");
    Ok(())
}

/// SHA-256 of the DER encoding of the first certificate in a PEM chain.
fn leaf_pin(cert_path: &std::path::Path) -> Result<String> {
    let file = std::fs::File::open(cert_path)?;
    let mut rd = std::io::BufReader::new(file);
    let certs: Vec<_> = rustls_pemfile::certs(&mut rd).collect::<Result<Vec<_>, _>>()?;
    let first = certs.first().context("certificate file is empty")?;
    Ok(hex::encode(crate::crypto::sha256(first.as_ref())))
}

fn cmd_cert(cli: &Cli, cmd: &CertCmd) -> Result<()> {    let path = config_path(cli)?;
    let mut cfg = Config::load(&path)?;
    match cmd {
        CertCmd::Show => {
            println!("certificate: {}", cfg.server.cert.display());
            println!("private key: {}", cfg.server.key.display());
            println!("mode       : {:?}", cfg.server.cert_mode);
            if let Ok(meta) = std::fs::metadata(&cfg.server.cert) {
                println!("size       : {} bytes", meta.len());
            }
            if let Ok(pin) = leaf_pin(&cfg.server.cert) {
                println!("sha256 pin : {pin}");
                println!();
                println!("clients that cannot disable certificate checks (Xray >= 25,");
                println!("v2rayN) accept a self-signed node by pinning this value:");
                println!("  \"tlsSettings\": {{ \"pinnedPeerCertSha256\": \"{pin}\" }}");
            }
            if let Some(dir) = cfg.server.cert.parent() {
                let ca = dir.join("ca.pem");
                if ca.exists() {
                    println!();
                    println!("CA bundle  : {}", ca.display());
                    println!("             import this file into the OS trust store to make");
                    println!("             browsers and strict TLS clients trust the node.");
                }
            }
        }
        CertCmd::SelfSigned { domain } => {
            if let Some(d) = domain {
                cfg.server.domain = d.clone();
            }
            std::fs::remove_file(&cfg.server.cert).ok();
            std::fs::remove_file(&cfg.server.key).ok();
            crate::net::tls::ensure_self_signed(&cfg.server.cert, &cfg.server.key, &cfg.server.domain)?;
            cfg.save(&path)?;
            println!("self-signed certificate written to {}", cfg.server.cert.display());
        }
    }
    Ok(())
}

/// `veil uninstall` — remove the service, the binary and (optionally) the config.
///
/// Deliberately shell-based instead of re-implementing systemd: the goal is to
/// leave the machine exactly as `deploy/install.sh` would have found it.
#[cfg(unix)]
fn cmd_uninstall(cli: &Cli, yes: bool, keep_config: bool) -> Result<()> {
    use std::io::Write;
    use std::process::Command;

    if unsafe { libc::geteuid() } != 0 {
        bail!("run as root: sudo veil uninstall");
    }

    let config_dir = cli
        .config
        .as_ref()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("/etc/veil"));
    let state_dir = PathBuf::from("/var/lib/veil");
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("/usr/local/bin/veil"));

    println!("将要删除：");
    println!("  服务     veil（停止 + 取消开机自启）");
    println!("  程序     {}", exe.display());
    println!("  运行状态 {}", state_dir.display());
    if keep_config {
        println!("  配置     {}（保留）", config_dir.display());
    } else {
        println!("  配置     {}（含全部账号凭据与证书）", config_dir.display());
    }
    if !yes {
        print!("\n确认请输入 yes: ");
        std::io::stdout().flush().ok();
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if answer.trim() != "yes" {
            println!("已取消");
            return Ok(());
        }
    }

    // systemd
    let _ = Command::new("systemctl")
        .args(["disable", "--now", "veil"])
        .status();
    let _ = std::fs::remove_file("/etc/systemd/system/veil.service");
    let _ = Command::new("systemctl").arg("daemon-reload").status();

    // OpenRC (Alpine)
    let _ = Command::new("rc-service").args(["veil", "stop"]).status();
    let _ = Command::new("rc-update")
        .args(["del", "veil", "default"])
        .status();
    let _ = std::fs::remove_file("/etc/init.d/veil");

    // Fallback for a process started by hand.
    let _ = Command::new("pkill").args(["-f", "/usr/local/bin/veil run"]).status();

    for path in ["/usr/local/bin/veil"] {
        match std::fs::remove_file(path) {
            Ok(()) => println!("  removed {path}"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => println!("  could not remove {path}: {e}"),
        }
    }
    if exe != PathBuf::from("/usr/local/bin/veil") {
        let _ = std::fs::remove_file(&exe);
    }
    let _ = std::fs::remove_dir_all(&state_dir);
    if keep_config {
        println!("  配置保留在 {}", config_dir.display());
    } else {
        let _ = std::fs::remove_dir_all(&config_dir);
    }
    let _ = std::fs::remove_file("/etc/sysctl.d/99-veil.conf");

    // Best effort: drop the service account.
    let _ = Command::new("userdel").arg("veil").status();

    println!("\nveil 已卸载。");
    Ok(())
}

#[cfg(not(unix))]
fn cmd_uninstall(_cli: &Cli, _yes: bool, _keep_config: bool) -> Result<()> {
    bail!("`veil uninstall` targets Linux; on Windows just delete the binary and the config folder")
}
