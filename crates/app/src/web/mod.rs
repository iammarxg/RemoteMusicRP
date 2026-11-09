// SPDX-License-Identifier: AGPL-3.0-only
//! Web UI server: JSON API + embedded single-page app.
//!
//! Hardening applied to every response/request:
//! - strict CSP (no inline scripts, no third-party scripts/styles), plus
//!   nosniff, no-referrer, frame-ancestors none, COOP/CORP, Permissions-Policy,
//!   HSTS behind HTTPS;
//! - `Host` allow-list (blocks DNS rebinding against the localhost UI);
//! - `Origin` must match for every state-changing request (blocks CSRF from
//!   other sites), in addition to per-session CSRF tokens;
//! - small request body limit; `no-store` on API responses.

mod api;
mod assets;
mod auth;
mod backup;
mod discord_sdk;
pub mod session;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use tokio::sync::watch;

use crate::app::App;

const BODY_LIMIT: usize = 16 * 1024;
/// Blocklists can be long (2 × 200 entries of up to 200 characters).
const RULES_BODY_LIMIT: usize = 192 * 1024;
/// An encrypted full export is base64 (about 4/3 of the rules limit).
const IMPORT_BODY_LIMIT: usize = 320 * 1024;

/// JSON error `{ "error": "..." }`. Messages are user-facing and never
/// contain secrets.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
    pub fn bad_request(m: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, m)
    }
    pub fn unauthorized(m: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, m)
    }
    pub fn forbidden(m: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, m)
    }
    pub fn internal(m: impl Into<String>) -> Self {
        let m = m.into();
        tracing::error!(error = %m, "internal error");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
    }
    pub fn db(e: remotemusicrp_storage::StorageError) -> Self {
        Self::internal(e.to_string())
    }
    pub fn locked() -> Self {
        Self::new(
            StatusCode::LOCKED,
            "instance is locked; enter the passphrase",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            axum::Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

fn loopback_hosts(app: &App) -> Vec<String> {
    let port = app.cfg.bind.port();
    vec![
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("[::1]:{port}"),
    ]
}

/// Host header values we answer to. On an exposed install the direct
/// address is not accepted: traffic must come through the TLS proxy
/// (public_origin). Loopback hosts stay valid for local health checks.
fn allowed_hosts(app: &App) -> Vec<String> {
    let mut hosts = loopback_hosts(app);
    if let Some(o) = &app.cfg.public_origin {
        let h = o.trim_start_matches("https://").to_owned();
        if let Some(stripped) = h.strip_suffix(":443") {
            hosts.push(stripped.to_owned());
        }
        hosts.push(h);
    }
    hosts
}

/// Origins allowed to make state-changing requests.
fn allowed_origins(app: &App) -> Vec<String> {
    let mut v: Vec<String> = loopback_hosts(app)
        .into_iter()
        .map(|h| format!("http://{h}"))
        .collect();
    if let Some(o) = &app.cfg.public_origin {
        v.push(o.clone());
    }
    v
}

async fn guard_requests(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let host = host.to_ascii_lowercase();
    if !allowed_hosts(&app).contains(&host) {
        return ApiError::new(StatusCode::MISDIRECTED_REQUEST, "unknown host").into_response();
    }
    let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if !safe {
        let origin = req
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !allowed_origins(&app)
            .iter()
            .any(|o| o.eq_ignore_ascii_case(origin))
        {
            return ApiError::forbidden("cross-origin request blocked").into_response();
        }
    }
    let is_api = req.uri().path().starts_with("/api/");
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    let set = |h: &mut axum::http::HeaderMap, k: &'static str, v: &'static str| {
        h.insert(k, HeaderValue::from_static(v));
    };
    set(
        h,
        "content-security-policy",
        "default-src 'none'; script-src 'self'; style-src 'self'; font-src 'self'; connect-src 'self'; \
         img-src 'self' data: https://*.mzstatic.com https://coverartarchive.org https://*.archive.org https://cdn.discordapp.com; \
         manifest-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
    );
    set(h, "x-content-type-options", "nosniff");
    set(h, "referrer-policy", "no-referrer");
    set(h, "x-frame-options", "DENY");
    set(h, "cross-origin-opener-policy", "same-origin");
    set(h, "cross-origin-resource-policy", "same-origin");
    set(
        h,
        "permissions-policy",
        "camera=(), microphone=(), geolocation=(), payment=(), usb=(), interest-cohort=()",
    );
    if app.cfg.public_origin.is_some() {
        set(h, "strict-transport-security", "max-age=31536000");
    }
    if is_api {
        set(h, "cache-control", "no-store");
    }
    resp
}

pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/state", get(api::state))
        .route("/api/unlock", post(auth::unlock))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/setup/password", post(auth::setup_password))
        .route("/api/setup/skip-password", post(auth::skip_password))
        .route("/api/setup/complete", post(api::setup_complete))
        .route("/api/status", get(api::status))
        .route("/api/connections", get(api::connections))
        .route(
            "/api/connections/lastfm",
            post(api::connect_lastfm).delete(api::disconnect_lastfm),
        )
        .route(
            "/api/connections/discord",
            post(api::connect_discord).delete(api::disconnect_discord),
        )
        .route(
            "/api/presence",
            get(api::get_presence).put(api::put_presence),
        )
        .route("/api/presence/pause", post(api::pause))
        .route("/api/discord/sdk", get(discord_sdk::status))
        .route("/api/discord/sdk/begin", post(discord_sdk::begin))
        .route("/api/discord/sdk/finish", post(discord_sdk::finish))
        .route("/api/discord/sdk/sign-out", post(discord_sdk::sign_out))
        .route(
            "/api/discord/sdk/online-status",
            post(discord_sdk::set_online_status),
        )
        .route(
            "/api/rules",
            get(api::get_rules)
                .put(api::put_rules)
                .layer(DefaultBodyLimit::max(RULES_BODY_LIMIT)),
        )
        .route("/api/activity", get(api::activity))
        .route("/api/settings", get(api::instance_settings))
        .route(
            "/api/email",
            get(api::get_email)
                .put(api::put_email)
                .delete(api::delete_email),
        )
        .route("/api/email/test", post(api::test_email))
        .route("/api/backup/export", post(backup::export))
        .route(
            "/api/backup/import",
            post(backup::import).layer(DefaultBodyLimit::max(IMPORT_BODY_LIMIT)),
        )
        .route("/api/about", get(api::about))
        .route("/api/security", get(auth::security))
        .route("/api/security/password", post(auth::change_password))
        .route("/api/security/totp/begin", post(auth::totp_begin))
        .route("/api/security/totp/confirm", post(auth::totp_confirm))
        .route("/api/security/totp/disable", post(auth::totp_disable))
        .route("/api/security/recovery", post(auth::recovery_regenerate))
        .route("/api/security/sessions", delete(auth::logout_others))
        .route("/api/security/rotate-keys", post(auth::rotate_keys))
        .route(
            "/api/danger/delete-everything",
            post(auth::delete_everything),
        )
        .route("/api/updates", get(api::updates))
        .route("/api/updates/check", post(api::updates_check))
        .route("/api/updates/install", post(api::updates_install))
        .route(
            "/api/updates/settings",
            axum::routing::put(api::updates_settings),
        )
        .fallback(assets::serve)
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(middleware::from_fn_with_state(app.clone(), guard_requests))
        .with_state(app)
}

pub async fn serve(app: Arc<App>, mut shutdown: watch::Receiver<bool>) -> Result<(), String> {
    // Retry briefly: after an update the old process may still hold the port.
    let mut attempt = 0;
    let listener = loop {
        match tokio::net::TcpListener::bind(app.cfg.bind).await {
            Ok(l) => break l,
            Err(e) if attempt < 20 && e.kind() == std::io::ErrorKind::AddrInUse => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            Err(e) => return Err(format!("cannot listen on {}: {}", app.cfg.bind, e.kind())),
        }
    };
    let url = app
        .cfg
        .public_origin
        .clone()
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", app.cfg.bind.port()));
    tracing::info!(%url, "web UI listening");
    axum::serve(
        listener,
        router(app).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = shutdown.changed().await;
    })
    .await
    .map_err(|e| format!("web server: {}", e.kind()))
}
