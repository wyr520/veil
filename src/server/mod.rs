//! The proxy server: one task per inbound, one task per connection.

pub mod api;
pub mod inbound;
pub mod outbound;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::{Mutex, RwLock};
use tracing::{error, info, warn};

use crate::config::Config;
use crate::net::Traffic;

/// Shared state handed to every inbound and to the API.
pub struct ServerCtx {
    pub config: Arc<RwLock<Config>>,
    pub acceptor: tokio_rustls::TlsAcceptor,
    pub quic_tls: Arc<rustls::ServerConfig>,
    pub traffic: Arc<Traffic>,
    pub per_user: Mutex<HashMap<String, Arc<Traffic>>>,
    pub started: Instant,
    pub config_path: PathBuf,
}

impl ServerCtx {
    pub fn traffic_for(&self, user: &str) -> Arc<Traffic> {
        // The map is tiny; a short critical section on the async mutex is fine.
        if let Ok(mut guard) = self.per_user.try_lock() {
            return guard
                .entry(user.to_string())
                .or_insert_with(Traffic::new)
                .clone();
        }
        Traffic::new()
    }

    pub async fn stats_snapshot(&self) -> Vec<(String, u64, u64)> {
        let guard = self.per_user.lock().await;
        guard
            .iter()
            .map(|(k, v)| {
                let (up, down) = v.snapshot();
                (k.clone(), up, down)
            })
            .collect()
    }

    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
    }
}

/// Load the config, materialise TLS material and run until Ctrl-C.
pub async fn run(config_path: PathBuf) -> Result<()> {
    let cfg = Config::load(&config_path)?;
    let generated = crate::net::tls::ensure_self_signed(
        &cfg.server.cert,
        &cfg.server.key,
        &cfg.server.domain,
    )?;
    if generated {
        warn!(
            "generated a self-signed certificate at {} — clients must enable allowInsecure",
            cfg.server.cert.display()
        );
    }

    let tls = crate::net::tls::server_config(&cfg.server.cert, &cfg.server.key)?;
    let quic_tls = crate::net::tls::quic_server_config(&cfg.server.cert, &cfg.server.key)?;

    info!(
        node = %cfg.server.name,
        domain = %cfg.server.domain,
        users = cfg.users.len(),
        inbounds = cfg.inbounds.len(),
        "veil server starting"
    );

    let api_cfg = cfg.api.clone();
    let inbounds: Vec<_> = cfg.inbounds.iter().filter(|i| i.enable).cloned().collect();

    let ctx = Arc::new(ServerCtx {
        config: Arc::new(RwLock::new(cfg)),
        acceptor: tokio_rustls::TlsAcceptor::from(tls),
        quic_tls,
        traffic: Traffic::new(),
        per_user: Mutex::new(HashMap::new()),
        started: Instant::now(),
        config_path: config_path.clone(),
    });

    let mut handles = Vec::new();
    for ib in inbounds {
        let ctx = ctx.clone();
        handles.push(tokio::spawn(async move {
            inbound::run(ctx, ib).await
        }));
    }

    if api_cfg.enable {
        let ctx = ctx.clone();
        handles.push(tokio::spawn(async move { api::serve(ctx).await }));
    }

    if handles.is_empty() {
        anyhow::bail!("nothing to serve: all inbounds are disabled and the API is off");
    }

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("shutdown signal received");
        }
        _ = async {
            for h in handles {
                if let Err(e) = h.await {
                    error!("task panicked: {e}");
                }
            }
        } => {}
    }
    Ok(())
}

/// Resolve the config path from CLI flags / environment.
pub fn resolve_config(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    if let Ok(p) = std::env::var("VEIL_CONFIG") {
        return Ok(PathBuf::from(p));
    }
    let candidates = [
        PathBuf::from("/etc/veil/veil.toml"),
        PathBuf::from("./veil.toml"),
    ];
    for c in candidates {
        if c.exists() {
            return Ok(c);
        }
    }
    Err(anyhow::anyhow!(
        "no config found; pass --config or run `veil init` first"
    ))
}

/// Reload hooks are intentionally left out of the hot path: `veil` restarts on
/// SIGHUP through systemd, which is simpler and less error prone.
pub async fn reload_note() -> Result<()> {
    tokio::signal::ctrl_c().await.context("signal handler")?;
    Ok(())
}
