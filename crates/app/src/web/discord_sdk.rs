// SPDX-License-Identifier: AGPL-3.0-only
//! Web API for the EXPERIMENTAL server mode (Social SDK) sign-in.

use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, header};
use remotemusicrp_crypto::Zeroizing;
use remotemusicrp_crypto::tokens::{ct_eq_str, pkce_pair, random_token};
use serde::Deserialize;
use serde_json::json;

use super::session::Ctx;
use super::{ApiError, ApiResult};
use crate::app::App;
use crate::sdk_mode::{self, PendingSignIn};
use crate::settings;

const CALLBACK_PATH: &str = "/discord-callback";

/// The redirect URI for sign-ins started from this browser: the address the
/// UI is open at (already checked against the Host allow-list), or the
/// public origin behind a TLS proxy.
fn redirect_uri(app: &App, headers: &HeaderMap) -> String {
    if let Some(o) = &app.cfg.public_origin {
        return format!("{o}{CALLBACK_PATH}");
    }
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("127.0.0.1")
        .to_ascii_lowercase();
    format!("http://{host}{CALLBACK_PATH}")
}

fn sdk(app: &App) -> ApiResult<&sdk_mode::SdkMode> {
    app.sdk.as_ref().ok_or_else(|| {
        ApiError::bad_request(
            "server mode is off: set [discord] mode = \"social-sdk\" in the config",
        )
    })
}

pub async fn status(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.require_auth(&app)?;
    let signed_in = app
        .db
        .has_secret(settings::SECRET_DISCORD_REFRESH)
        .map_err(ApiError::db)?;
    Ok(Json(json!({
        "mode": if app.sdk.is_some() { "social_sdk" } else { "local_rpc" },
        "compiled": sdk_mode::COMPILED,
        "status": app.sdk.as_ref().map(|s| s.status()),
        "ready": app.sdk.as_ref().is_some_and(|s| s.ready()),
        "online_status": app.sdk.as_ref().and_then(|s| s.online_status()),
        "signed_in": signed_in,
        "redirect_uri": redirect_uri(&app, &headers),
        "error": app.status.borrow().discord_error.clone(),
    })))
}

pub async fn begin(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let session = ctx.guard(&app, &headers)?.clone();
    sdk(&app)?;
    let client_id = app
        .db
        .get_setting(settings::DISCORD_CLIENT_ID)
        .map_err(ApiError::db)?
        .ok_or_else(|| ApiError::bad_request("enter your Discord Application ID first"))?;
    let (verifier, challenge) = pkce_pair().map_err(|_| ApiError::internal("rng"))?;
    let state = random_token().map_err(|_| ApiError::internal("rng"))?;
    let redirect = redirect_uri(&app, &headers);
    let url = sdk_mode::authorize_url(&client_id, &redirect, &challenge, &state);
    *app.sdk_shared
        .pending
        .lock()
        .map_err(|_| ApiError::internal("lock"))? = Some(PendingSignIn {
        state,
        verifier: Zeroizing::new(verifier),
        redirect_uri: redirect.clone(),
        session_hash: session.token_hash,
        created: Instant::now(),
    });
    Ok(Json(json!({ "url": url, "redirect_uri": redirect })))
}

#[derive(Deserialize)]
pub struct FinishBody {
    code: String,
    state: String,
}

fn valid_code(c: &str) -> bool {
    !c.is_empty()
        && c.len() <= 512
        && c.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~'))
}

pub async fn finish(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
    Json(body): Json<FinishBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let session = ctx.guard(&app, &headers)?.clone();
    let sdk = sdk(&app)?;
    let code = body.code.trim().to_owned();
    if !valid_code(&code) {
        return Err(ApiError::bad_request("that sign-in code is not valid"));
    }
    // Single use: take the pending sign-in whatever happens next.
    let pending = app
        .sdk_shared
        .pending
        .lock()
        .map_err(|_| ApiError::internal("lock"))?
        .take()
        .ok_or_else(|| ApiError::bad_request("no sign-in in progress; start again"))?;
    if pending.expired()
        || !ct_eq_str(&pending.state, body.state.trim())
        || !ct_eq_str(&pending.session_hash, &session.token_hash)
    {
        return Err(ApiError::bad_request(
            "this sign-in link is expired or was not started here; start again",
        ));
    }
    sdk.finish_sign_in(&app, pending, code)
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(json!({ "ok": true, "ready": sdk.ready() })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnlineStatusBody {
    status: String,
}

/// Change the account's online status (server mode only).
pub async fn set_online_status(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
    Json(body): Json<OnlineStatusBody>,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    sdk(&app)?
        .set_online_status(&app, body.status.trim())
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn sign_out(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    sdk(&app)?.sign_out(&app).await;
    app.reload.notify_one();
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_validation() {
        assert!(valid_code("AbC123_-.~"));
        assert!(!valid_code(""));
        assert!(!valid_code("a b"));
        assert!(!valid_code("a&state=x"));
    }
}
