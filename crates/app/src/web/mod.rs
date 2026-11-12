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

/// LAN mode: a Host header naming this machine on the local network: a
/// private/LAN IP literal or a local name (`.local`, `.lan`, `.home.arpa`,
/// `.internal`) with our port. Public names are still refused, which keeps
/// DNS-rebinding protection (an attacker's domain never matches).
fn lan_host_ok(host: &str, port: u16) -> bool {
    let (name, host_port) = match host.rsplit_once(':') {
        // IPv6 literal without a port, e.g. "[fe80::1]".
        _ if host.ends_with(']') => (host, None),
        Some((n, p)) => (n, p.parse::<u16>().ok()),
        None => (host, None),
    };
    if host_port.unwrap_or(80) != port {
        return false;
    }
    let bare = name.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
        return remotemusicrp_core::config::is_lan_ip(ip);
    }
    let local_suffix = [".local", ".lan", ".home.arpa", ".internal"]
        .iter()
        .any(|s| bare.ends_with(s) && bare.len() > s.len());
    local_suffix
        && bare
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
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
    // LAN mode: only machines on the local network may talk to us at all.
    if app.cfg.lan_access {
        let peer = req
            .extensions()
            .get::<axum::extract::ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip());
        if !peer.is_some_and(remotemusicrp_core::config::is_lan_ip) {
            return ApiError::forbidden("only devices on the local network may connect")
                .into_response();
        }
    }
    let lan_host = app.cfg.lan_access && lan_host_ok(&host, app.cfg.bind.port());
    if !lan_host && !allowed_hosts(&app).contains(&host) {
        return ApiError::new(StatusCode::MISDIRECTED_REQUEST, "unknown host").into_response();
    }
    let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if !safe {
        let origin = req
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        // LAN mode: the page must come from the same LAN address it posts to.
        let same_lan_origin = lan_host && origin.eq_ignore_ascii_case(&format!("http://{host}"));
        if !same_lan_origin
            && !allowed_origins(&app)
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
    // Cover Art Archive images redirect coverartarchive.org → archive.org →
    // <node>.archive.org; every hop must be allowed (`*.archive.org` doesn't
    // cover the bare `archive.org`).
    set(
        h,
        "content-security-policy",
        "default-src 'none'; script-src 'self'; style-src 'self'; font-src 'self'; connect-src 'self'; \
         img-src 'self' data: https://*.mzstatic.com https://coverartarchive.org https://archive.org https://*.archive.org https://cdn.discordapp.com; \
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
        .route("/api/artwork/display-failed", post(api::art_display_failed))
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
        .route(
            "/api/autostart",
            get(api::autostart).put(api::set_autostart),
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

/// This machine's main LAN address, for the log. A UDP "connect" only asks
/// the OS which interface would be used; no packet is sent. The address is
/// from TEST-NET-1 (RFC 5737), which is never routed.
pub fn primary_lan_ip() -> Option<std::net::IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    let ip = s.local_addr().ok()?.ip();
    remotemusicrp_core::config::is_lan_ip(ip)
        .then_some(ip)
        .filter(|ip| !ip.is_loopback())
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
    let port = app.cfg.bind.port();
    let url = app
        .cfg
        .public_origin
        .clone()
        .unwrap_or_else(|| format!("http://127.0.0.1:{port}"));
    tracing::info!(%url, "web UI listening");
    if app.cfg.lan_access {
        match primary_lan_ip() {
            Some(ip) => tracing::warn!(
                url = %format!("http://{ip}:{port}"),
                "LAN access is on: the web UI is reachable from your local network over plain HTTP (not encrypted)"
            ),
            None => tracing::warn!(
                "LAN access is on: the web UI is reachable from your local network over plain HTTP (not encrypted)"
            ),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::lan_host_ok;

    #[test]
    fn lan_hosts() {
        for ok in [
            "192.168.1.20:8484",
            "10.0.0.5:8484",
            "[fe80::1]:8484",
            "[fd00::2]:8484",
            "mypc.local:8484",
            "nas.lan:8484",
            "box.home.arpa:8484",
        ] {
            assert!(lan_host_ok(ok, 8484), "{ok} should be accepted");
        }
        for bad in [
            "8.8.8.8:8484",      // public IP
            "evil.example:8484", // DNS rebinding: attacker's domain
            "192.168.1.20:9999", // wrong port
            "192.168.1.20",      // no port means 80, not ours
            ".local:8484",       // empty name
            "my_pc.local:8484",  // invalid characters
            "attacker.com.local.evil.com:8484",
        ] {
            assert!(!lan_host_ok(bad, 8484), "{bad} should be refused");
        }
        // Port 80: the port may be left out.
        assert!(lan_host_ok("192.168.1.20", 80));
    }
}
