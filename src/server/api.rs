//! Small management API: subscriptions, node lists, traffic counters and user
//! provisioning. Bound to loopback by default.

use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use tracing::info;

use crate::config::User;
use crate::server::ServerCtx;
use crate::share;

pub async fn serve(ctx: Arc<ServerCtx>) -> Result<()> {
    let listen = {
        let cfg = ctx.config.read().await;
        cfg.api.listen.clone()
    };
    let app = Router::new()
        .route("/", get(root))
        .route("/api/health", get(health))
        .route("/api/nodes/:user", get(nodes))
        .route("/api/stats", get(stats))
        .route("/api/users", get(list_users).post(create_user))
        .route(&format!("/sub/:token"), get(subscription))
        .with_state(ctx);

    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("cannot bind API on {listen}"))?;
    info!(%listen, "management API ready");
    axum::serve(listener, app).await.context("API server failed")?;
    Ok(())
}

async fn root() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        "<!doctype html><html><head><title>Welcome to nginx!</title></head>\
         <body><h1>Welcome to nginx!</h1><p>If you see this page, the web server is successfully installed.</p></body></html>",
    )
}

async fn health(State(ctx): State<Arc<ServerCtx>>) -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": ctx.uptime().as_secs(),
        "users": ctx.config.read().await.users.len(),
    }))
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

fn admin_ok(ctx: &ServerCtx, headers: &HeaderMap) -> Result<(), Response> {
    let token = ctx.config.blocking_read().api.admin_token.clone();
    if token.is_empty() {
        return Err(err(
            StatusCode::FORBIDDEN,
            "api.admin_token is empty: management endpoints are disabled",
        ));
    }
    let presented = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim_start_matches("Bearer ").trim())
        .unwrap_or_default();
    if crate::config::constant_time_eq(presented.as_bytes(), token.as_bytes()) {
        Ok(())
    } else {
        Err(err(StatusCode::UNAUTHORIZED, "bad admin token"))
    }
}

async fn nodes(
    State(ctx): State<Arc<ServerCtx>>,
    Path(user): Path<String>,
) -> Response {
    let cfg = ctx.config.read().await;
    match cfg.user(&user) {
        Some(u) => Json(share::nodes_json(&cfg, u)).into_response(),
        None => err(StatusCode::NOT_FOUND, "no such user"),
    }
}

async fn subscription(State(ctx): State<Arc<ServerCtx>>, Path(token): Path<String>) -> Response {
    let cfg = ctx.config.read().await;
    match cfg.user_by_sub_token(&token) {
        Some(u) => (
            StatusCode::OK,
            [("content-type", "text/plain; charset=utf-8")],
            share::subscription(&cfg, u),
        )
            .into_response(),
        None => err(StatusCode::NOT_FOUND, "invalid subscription token"),
    }
}

async fn stats(State(ctx): State<Arc<ServerCtx>>) -> Response {
    let snapshot = ctx.stats_snapshot().await;
    let list: Vec<_> = snapshot
        .into_iter()
        .map(|(name, up, down)| json!({"user": name, "up": up, "down": down}))
        .collect();
    Json(json!({
        "uptime_secs": ctx.uptime().as_secs(),
        "users": list,
    }))
    .into_response()
}

async fn list_users(State(ctx): State<Arc<ServerCtx>>, headers: HeaderMap) -> Response {
    if let Err(r) = admin_ok(&ctx, &headers) {
        return r;
    }
    let cfg = ctx.config.read().await;
    let users: Vec<_> = cfg
        .users
        .iter()
        .map(|u| {
            json!({
                "name": u.name,
                "uuid": u.uuid,
                "enable": u.enable,
                "email": u.email,
                "sub_token": u.sub_token,
            })
        })
        .collect();
    Json(users).into_response()
}

async fn create_user(
    State(ctx): State<Arc<ServerCtx>>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if let Err(r) = admin_ok(&ctx, &headers) {
        return r;
    }
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if name.is_empty() {
        return err(StatusCode::BAD_REQUEST, "`name` is required");
    }
    let path = ctx.config_path.clone();
    let mut cfg = ctx.config.write().await;
    if cfg.user(&name).is_some() {
        return err(StatusCode::CONFLICT, "user already exists");
    }
    let user = User::new(&name);
    cfg.users.push(user.clone());
    if let Err(e) = cfg.save(&path) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("cannot persist: {e}"));
    }
    (StatusCode::CREATED, Json(json!({ "user": name, "uuid": user.uuid }))).into_response()
}
