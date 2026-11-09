// SPDX-License-Identifier: AGPL-3.0-only
//! Sessions, CSRF and per-request context.
//!
//! - Session tokens are 256-bit random, sent only as an `HttpOnly`,
//!   `SameSite=Strict` cookie (`__Host-` + `Secure` behind HTTPS), and stored
//!   server-side only as SHA-256 hashes.
//! - Every session has its own CSRF token, required in `X-CSRF-Token` on all
//!   state-changing requests (in addition to the Origin check).
//! - Lifetimes (owner decision): 1 h idle / 12 h total by default; 30 days
//!   with "remember me".

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, header};
use remotemusicrp_crypto::tokens::{ct_eq_str, looks_like_token, random_token, token_hash};
use remotemusicrp_storage::SessionRow;

use super::ApiError;
use crate::app::App;
use crate::{now_unix, settings};

pub const IDLE_SECS: i64 = 3600;
pub const MAX_SECS: i64 = 12 * 3600;
pub const REMEMBER_SECS: i64 = 30 * 24 * 3600;
const TOUCH_EVERY_SECS: i64 = 60;

pub fn cookie_name(app: &App) -> &'static str {
    if app.cfg.public_origin.is_some() {
        "__Host-rsrp_session"
    } else {
        "rsrp_session"
    }
}

/// Whether a password must be presented to use the UI.
pub fn auth_required(app: &App) -> bool {
    app.cfg.is_exposed() || settings::password_hash(&app.db).is_some()
}

/// Per-request context: client IP and the (validated) session, if any.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub ip: IpAddr,
    pub session: Option<SessionRow>,
}

impl Ctx {
    pub fn require_session(&self) -> Result<&SessionRow, ApiError> {
        self.session
            .as_ref()
            .ok_or_else(|| ApiError::unauthorized("session expired; reload the page"))
    }

    /// Session that is allowed to use the app (logged in, or no password
    /// needed on this localhost-only install).
    pub fn require_auth(&self, app: &App) -> Result<&SessionRow, ApiError> {
        let s = self.require_session()?;
        if s.authenticated || !auth_required(app) {
            Ok(s)
        } else {
            Err(ApiError::unauthorized("please log in"))
        }
    }

    pub fn check_csrf(&self, headers: &HeaderMap) -> Result<&SessionRow, ApiError> {
        let s = self.require_session()?;
        let sent = headers
            .get("x-csrf-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if ct_eq_str(sent, &s.csrf) {
            Ok(s)
        } else {
            Err(ApiError::forbidden("invalid CSRF token; reload the page"))
        }
    }

    /// Auth + CSRF: the guard for every state-changing endpoint.
    pub fn guard(&self, app: &App, headers: &HeaderMap) -> Result<&SessionRow, ApiError> {
        self.check_csrf(headers)?;
        self.require_auth(app)
    }
}

fn cookie_token(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_owned())
        .filter(|v| looks_like_token(v))
}

/// Client IP. Behind the TLS reverse proxy (public_origin set) and only when
/// the direct peer is loopback, the rightmost `X-Forwarded-For` entry (the
/// one our proxy appended) is used.
pub fn client_ip(app: &App, peer: IpAddr, headers: &HeaderMap) -> IpAddr {
    if app.cfg.public_origin.is_some()
        && peer.is_loopback()
        && let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').next())
            .and_then(|v| v.trim().parse::<IpAddr>().ok())
    {
        return ip;
    }
    peer
}

fn load_session(app: &App, headers: &HeaderMap) -> Option<SessionRow> {
    let token = cookie_token(headers, cookie_name(app))?;
    let hash = token_hash(&token);
    let row = app.db.get_session(&hash).ok()??;
    let now = now_unix() as i64;
    let idle = if row.remember {
        REMEMBER_SECS
    } else {
        IDLE_SECS
    };
    if now >= row.expires_at || now - row.last_seen >= idle {
        let _ = app.db.delete_session(&hash);
        return None;
    }
    if now - row.last_seen >= TOUCH_EVERY_SECS {
        let _ = app.db.touch_session(&hash, now);
    }
    Some(row)
}

impl FromRequestParts<Arc<App>> for Ctx {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        app: &Arc<App>,
    ) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip())
            .ok_or_else(|| ApiError::internal("missing peer address"))?;
        Ok(Ctx {
            ip: client_ip(app, peer, &parts.headers),
            session: load_session(app, &parts.headers),
        })
    }
}

/// Create a session and return `(row, Set-Cookie value)`.
pub fn create_session(
    app: &App,
    authenticated: bool,
    remember: bool,
    headers: &HeaderMap,
) -> Result<(SessionRow, HeaderValue), ApiError> {
    let token = random_token().map_err(|_| ApiError::internal("rng failure"))?;
    let csrf = random_token().map_err(|_| ApiError::internal("rng failure"))?;
    let now = now_unix() as i64;
    let max = if remember { REMEMBER_SECS } else { MAX_SECS };
    let row = SessionRow {
        token_hash: token_hash(&token),
        csrf,
        authenticated,
        remember,
        created_at: now,
        last_seen: now,
        expires_at: now + max,
        label: browser_label(headers),
    };
    app.db.insert_session(&row).map_err(ApiError::db)?;
    let secure = if app.cfg.public_origin.is_some() {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "{}={token}; Path=/; HttpOnly; SameSite=Strict{secure}; Max-Age={max}",
        cookie_name(app)
    );
    let value = HeaderValue::from_str(&cookie).map_err(|_| ApiError::internal("cookie"))?;
    Ok((row, value))
}

pub fn clear_cookie(app: &App) -> HeaderValue {
    let secure = if app.cfg.public_origin.is_some() {
        "; Secure"
    } else {
        ""
    };
    HeaderValue::from_str(&format!(
        "{}=; Path=/; HttpOnly; SameSite=Strict{secure}; Max-Age=0",
        cookie_name(app)
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("rsrp_session=; Max-Age=0"))
}

/// Coarse, non-identifying label for the sessions list.
fn browser_label(headers: &HeaderMap) -> String {
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let browser = if ua.contains("Edg/") {
        "Edge"
    } else if ua.contains("Firefox/") {
        "Firefox"
    } else if ua.contains("Chrome/") {
        "Chrome"
    } else if ua.contains("Safari/") {
        "Safari"
    } else {
        "Browser"
    };
    let os = if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Android") {
        "Android"
    } else if ua.contains("iPhone") || ua.contains("iPad") {
        "iOS"
    } else if ua.contains("Mac OS") {
        "macOS"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        "unknown OS"
    };
    format!("{browser} on {os}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_parsing() {
        let mut h = HeaderMap::new();
        let tok = "a".repeat(43);
        h.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("x=1; rsrp_session={tok}; y=2"))
                .unwrap_or(HeaderValue::from_static("")),
        );
        assert_eq!(
            cookie_token(&h, "rsrp_session").as_deref(),
            Some(tok.as_str())
        );
        assert_eq!(cookie_token(&h, "other"), None);
        let mut bad = HeaderMap::new();
        bad.insert(
            header::COOKIE,
            HeaderValue::from_static("rsrp_session=not-a-token"),
        );
        assert_eq!(cookie_token(&bad, "rsrp_session"), None);
    }

    #[test]
    fn labels() {
        let mut h = HeaderMap::new();
        h.insert(
            header::USER_AGENT,
            HeaderValue::from_static(
                "Mozilla/5.0 (Windows NT 10.0) AppleWebKit Chrome/140 Safari/537",
            ),
        );
        assert_eq!(browser_label(&h), "Chrome on Windows");
    }
}
