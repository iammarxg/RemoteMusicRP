// SPDX-License-Identifier: AGPL-3.0-only
//! Authentication and security endpoints.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use remotemusicrp_core::config::KeyStoreConfig;
use remotemusicrp_crypto::keystore::{self, KeyStore, PassphraseFileKeyStore};
use remotemusicrp_crypto::totp::{self, TotpSecret};
use remotemusicrp_crypto::{SecretString, password, recovery};
use remotemusicrp_storage::LogLevel;
use serde::Deserialize;
use serde_json::json;

use super::session::{Ctx, clear_cookie, create_session};
use super::{ApiError, ApiResult};
use crate::app::App;
use crate::{now_unix, settings};

fn secret(s: String) -> SecretString {
    SecretString::from(s)
}

async fn verify_password(app: &App, pw: SecretString) -> bool {
    let Some(hash) = settings::password_hash(&app.db) else {
        return false;
    };
    let Ok(_permit) = app.hash_permits.acquire().await else {
        return false;
    };
    tokio::task::spawn_blocking(move || password::verify(&pw, &hash))
        .await
        .unwrap_or(false)
}

async fn hash_password(app: &App, pw: SecretString) -> ApiResult<String> {
    let _permit = app
        .hash_permits
        .acquire()
        .await
        .map_err(|_| ApiError::internal("semaphore"))?;
    tokio::task::spawn_blocking(move || password::hash(&pw))
        .await
        .map_err(|_| ApiError::internal("hash task failed"))?
        .map_err(|_| ApiError::internal("hash failed"))
}

fn throttle_check(app: &App, ctx: &Ctx) -> ApiResult<()> {
    let locked = app
        .lockout
        .lock()
        .map_err(|_| ApiError::internal("lock"))?
        .locked_for(ctx.ip);
    match locked {
        Some(d) => Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "too many failed attempts; try again in {} s",
                d.as_secs().max(1)
            ),
        )),
        None => Ok(()),
    }
}

fn record_failure(app: &App, ctx: &Ctx, what: &str) {
    let lock = app
        .lockout
        .lock()
        .ok()
        .and_then(|mut l| l.record_failure(ctx.ip));
    let msg = match lock {
        Some(d) => format!(
            "failed {what} from {}; locked for {} s",
            ctx.ip,
            d.as_secs()
        ),
        None => format!("failed {what} from {}", ctx.ip),
    };
    let _ = app.db.log(LogLevel::Warn, "security", &msg);
}

fn record_success(app: &App, ctx: &Ctx) {
    if let Ok(mut l) = app.lockout.lock() {
        l.record_success(ctx.ip);
    }
}

fn with_cookie(body: serde_json::Value, cookie: axum::http::HeaderValue) -> Response {
    let mut resp = Json(body).into_response();
    resp.headers_mut().append(header::SET_COOKIE, cookie);
    resp
}

/// Replace the caller's session with a fresh authenticated one (session
/// fixation defence) and return the response carrying the new cookie.
fn rotate_into_authenticated(
    app: &App,
    ctx: &Ctx,
    headers: &HeaderMap,
    remember: bool,
    body: serde_json::Value,
) -> ApiResult<Response> {
    if let Some(old) = &ctx.session {
        let _ = app.db.delete_session(&old.token_hash);
    }
    let (_, cookie) = create_session(app, true, remember, headers)?;
    Ok(with_cookie(body, cookie))
}

// ---- unlock (passphrase mode) -------------------------------------------

#[derive(Deserialize)]
pub struct UnlockBody {
    passphrase: String,
}

pub async fn unlock(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
    Json(body): Json<UnlockBody>,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.check_csrf(&headers)?;
    let KeyStoreConfig::Passphrase(path) = app.cfg.key_store.clone() else {
        return Err(ApiError::bad_request(
            "this instance does not use a passphrase",
        ));
    };
    if !app.is_locked() {
        return Ok(Json(json!({ "ok": true })));
    }
    throttle_check(&app, &ctx)?;
    if body.passphrase.chars().count() < 12 {
        return Err(ApiError::bad_request(
            "passphrase must be at least 12 characters",
        ));
    }
    let store = PassphraseFileKeyStore::new(path, secret(body.passphrase));
    let _permit = app
        .hash_permits
        .acquire()
        .await
        .map_err(|_| ApiError::internal("semaphore"))?;
    let loaded =
        tokio::task::spawn_blocking(move || keystore::load_or_create(&store).map(|kr| (kr, store)))
            .await
            .map_err(|_| ApiError::internal("unlock task failed"))?;
    match loaded {
        Ok((kr, store)) => {
            record_success(&app, &ctx);
            *app.keyring
                .write()
                .map_err(|_| ApiError::internal("lock"))? = Some(kr);
            *app.key_store
                .lock()
                .map_err(|_| ApiError::internal("lock"))? = Some(Box::new(store));
            let _ = app.db.log(LogLevel::Info, "security", "instance unlocked");
            app.reload.notify_one();
            Ok(Json(json!({ "ok": true })))
        }
        Err(remotemusicrp_crypto::Error::Decrypt) => {
            record_failure(&app, &ctx, "unlock");
            Err(ApiError::unauthorized("wrong passphrase"))
        }
        Err(e) => Err(ApiError::internal(e.to_string())),
    }
}

// ---- login / logout --------------------------------------------------------

#[derive(Deserialize)]
pub struct LoginBody {
    password: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    remember: bool,
}

pub async fn login(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> ApiResult<Response> {
    ctx.check_csrf(&headers)?;
    throttle_check(&app, &ctx)?;
    if settings::password_hash(&app.db).is_none() {
        return Err(ApiError::bad_request("no password has been set yet"));
    }
    let pw_ok = verify_password(&app, secret(body.password)).await;
    let totp_on = settings::get_bool(&app.db, settings::TOTP_ENABLED);
    if pw_ok && totp_on {
        let code = body.code.unwrap_or_default();
        if code.trim().is_empty() {
            // Password was right; ask for the second factor without
            // counting this as a failure.
            return Ok((
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "enter your 2FA code", "need_code": true })),
            )
                .into_response());
        }
        if !check_second_factor(&app, &code)? {
            record_failure(&app, &ctx, "2FA login");
            return Err(ApiError::unauthorized("incorrect password or code"));
        }
    } else if !pw_ok {
        record_failure(&app, &ctx, "login");
        return Err(ApiError::unauthorized("incorrect password or code"));
    }
    record_success(&app, &ctx);
    let _ = app.db.log(
        LogLevel::Info,
        "security",
        &format!("login from {}", ctx.ip),
    );
    rotate_into_authenticated(&app, &ctx, &headers, body.remember, json!({ "ok": true }))
}

/// TOTP code, or a single-use recovery code (consumed on success).
fn check_second_factor(app: &App, code: &str) -> ApiResult<bool> {
    let code = code.trim();
    if code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit()) {
        let secret_bytes = app
            .with_keyring(|kr| {
                app.db
                    .get_secret(kr, settings::SECRET_TOTP, settings::PURPOSE_TOTP)
            })
            .map_err(|_| ApiError::locked())?
            .map_err(ApiError::db)?
            .ok_or_else(|| ApiError::internal("2FA secret missing"))?;
        let ts = TotpSecret::from_bytes(&secret_bytes)
            .map_err(|_| ApiError::internal("bad 2FA secret"))?;
        let last = app
            .db
            .get_setting(settings::TOTP_LAST_STEP)
            .ok()
            .flatten()
            .and_then(|v| v.parse::<u64>().ok());
        return Ok(match totp::verify(&ts, code, now_unix(), last) {
            Some(step) => {
                app.db
                    .set_setting(settings::TOTP_LAST_STEP, &step.to_string())
                    .map_err(ApiError::db)?;
                true
            }
            None => false,
        });
    }
    let mut hashes = settings::recovery_hashes(&app.db);
    match recovery::find_match(code, &hashes) {
        Some(i) => {
            hashes.remove(i);
            let j = serde_json::to_string(&hashes).map_err(|_| ApiError::internal("json"))?;
            app.db
                .set_setting(settings::RECOVERY_HASHES, &j)
                .map_err(ApiError::db)?;
            let _ = app.db.log(
                LogLevel::Warn,
                "security",
                &format!("recovery code used; {} left", hashes.len()),
            );
            Ok(true)
        }
        None => Ok(false),
    }
}

pub async fn logout(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let s = ctx.check_csrf(&headers)?;
    app.db.delete_session(&s.token_hash).map_err(ApiError::db)?;
    Ok(with_cookie(json!({ "ok": true }), clear_cookie(&app)))
}

// ---- first-run password ----------------------------------------------------

#[derive(Deserialize)]
pub struct SetupPasswordBody {
    password: String,
    #[serde(default)]
    setup_code: Option<String>,
}

pub async fn setup_password(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
    Json(body): Json<SetupPasswordBody>,
) -> ApiResult<Response> {
    ctx.check_csrf(&headers)?;
    if settings::password_hash(&app.db).is_some() {
        return Err(ApiError::bad_request(
            "a password is already set; change it under Security",
        ));
    }
    // A reachable install needs the setup code from the log, except when the
    // request comes from this computer itself (whoever sits at it already
    // controls the app). `ctx.ip` is the real client: behind the TLS proxy it
    // comes from X-Forwarded-For, so proxied visitors still need the code.
    if app.cfg.is_exposed() && !ctx.ip.is_loopback() {
        throttle_check(&app, &ctx)?;
        let expected = app
            .setup_code
            .lock()
            .map_err(|_| ApiError::internal("lock"))?
            .clone();
        let ok = matches!((&expected, &body.setup_code), (Some(e), Some(g))
            if remotemusicrp_crypto::tokens::ct_eq_str(e, g.trim()));
        if !ok {
            record_failure(&app, &ctx, "setup code");
            return Err(ApiError::unauthorized(
                "wrong setup code (see the server log)",
            ));
        }
    }
    let pw = secret(body.password);
    password::check_policy(&pw).map_err(ApiError::bad_request)?;
    let hash = hash_password(&app, pw).await?;
    app.db
        .set_setting(settings::PASSWORD_HASH, &hash)
        .map_err(ApiError::db)?;
    let _ = settings::set_bool(&app.db, settings::PASSWORD_SKIPPED, false);
    *app.setup_code
        .lock()
        .map_err(|_| ApiError::internal("lock"))? = None;
    // All earlier (anonymous) sessions become invalid.
    app.db.delete_other_sessions(None).map_err(ApiError::db)?;
    let _ = app.db.log(LogLevel::Info, "security", "admin password set");
    let (_, cookie) = create_session(&app, true, false, &headers)?;
    Ok(with_cookie(json!({ "ok": true }), cookie))
}

pub async fn skip_password(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    if app.cfg.is_exposed() {
        return Err(ApiError::bad_request(
            "a password is required when the UI is reachable from other machines",
        ));
    }
    settings::set_bool(&app.db, settings::PASSWORD_SKIPPED, true).map_err(ApiError::db)?;
    let _ = app.db.log(
        LogLevel::Warn,
        "security",
        "admin password skipped (localhost only)",
    );
    Ok(Json(json!({ "ok": true })))
}

// ---- security page -----------------------------------------------------------

pub async fn security(State(app): State<Arc<App>>, ctx: Ctx) -> ApiResult<Json<serde_json::Value>> {
    let me = ctx.require_auth(&app)?;
    let sessions: Vec<_> = app
        .db
        .list_sessions()
        .map_err(ApiError::db)?
        .into_iter()
        .map(|s| {
            json!({
                "label": s.label,
                "created_at": s.created_at,
                "last_seen": s.last_seen,
                "remember": s.remember,
                "current": s.token_hash == me.token_hash,
            })
        })
        .collect();
    let key_store = app
        .key_store
        .lock()
        .ok()
        .and_then(|k| k.as_ref().map(|s| s.describe()))
        .unwrap_or("locked");
    Ok(Json(json!({
        "password_set": settings::password_hash(&app.db).is_some(),
        "password_skipped": settings::get_bool(&app.db, settings::PASSWORD_SKIPPED),
        "password_required": app.cfg.is_exposed(),
        "totp_enabled": settings::get_bool(&app.db, settings::TOTP_ENABLED),
        "recovery_remaining": settings::recovery_hashes(&app.db).len(),
        "sessions": sessions,
        "key_store": key_store,
        "exposed": app.cfg.is_exposed(),
    })))
}

#[derive(Deserialize)]
pub struct ChangePasswordBody {
    #[serde(default)]
    current: Option<String>,
    new: String,
}

pub async fn change_password(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
    Json(body): Json<ChangePasswordBody>,
) -> ApiResult<Response> {
    ctx.guard(&app, &headers)?;
    if settings::password_hash(&app.db).is_some() {
        throttle_check(&app, &ctx)?;
        if !verify_password(&app, secret(body.current.unwrap_or_default())).await {
            record_failure(&app, &ctx, "password change");
            return Err(ApiError::unauthorized("current password is incorrect"));
        }
    }
    let pw = secret(body.new);
    password::check_policy(&pw).map_err(ApiError::bad_request)?;
    let hash = hash_password(&app, pw).await?;
    app.db
        .set_setting(settings::PASSWORD_HASH, &hash)
        .map_err(ApiError::db)?;
    let _ = settings::set_bool(&app.db, settings::PASSWORD_SKIPPED, false);
    // Log out every other device.
    app.db.delete_other_sessions(None).map_err(ApiError::db)?;
    let _ = app.db.log(
        LogLevel::Info,
        "security",
        "admin password changed; other sessions logged out",
    );
    let (_, cookie) = create_session(&app, true, false, &headers)?;
    Ok(with_cookie(json!({ "ok": true }), cookie))
}

pub async fn totp_begin(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    if settings::password_hash(&app.db).is_none() {
        return Err(ApiError::bad_request("set a password before enabling 2FA"));
    }
    let ts = TotpSecret::generate().map_err(|_| ApiError::internal("rng"))?;
    app.with_keyring(|kr| {
        app.db.put_secret(
            kr,
            settings::SECRET_TOTP_PENDING,
            settings::PURPOSE_TOTP,
            ts.as_bytes(),
        )
    })
    .map_err(|_| ApiError::locked())?
    .map_err(ApiError::db)?;
    Ok(Json(json!({
        "secret": ts.to_base32(),
        "otpauth_uri": ts.otpauth_uri("admin"),
    })))
}

#[derive(Deserialize)]
pub struct CodeBody {
    code: String,
}

pub async fn totp_confirm(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
    Json(body): Json<CodeBody>,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    let pending = app
        .with_keyring(|kr| {
            app.db
                .get_secret(kr, settings::SECRET_TOTP_PENDING, settings::PURPOSE_TOTP)
        })
        .map_err(|_| ApiError::locked())?
        .map_err(ApiError::db)?
        .ok_or_else(|| ApiError::bad_request("start 2FA setup first"))?;
    let ts = TotpSecret::from_bytes(&pending).map_err(|_| ApiError::internal("bad secret"))?;
    let step = totp::verify(&ts, &body.code, now_unix(), None).ok_or_else(|| {
        ApiError::bad_request("that code is not correct; check your device's clock")
    })?;
    app.with_keyring(|kr| {
        app.db.put_secret(
            kr,
            settings::SECRET_TOTP,
            settings::PURPOSE_TOTP,
            ts.as_bytes(),
        )
    })
    .map_err(|_| ApiError::locked())?
    .map_err(ApiError::db)?;
    app.db
        .delete_secret(settings::SECRET_TOTP_PENDING)
        .map_err(ApiError::db)?;
    app.db
        .set_setting(settings::TOTP_LAST_STEP, &step.to_string())
        .map_err(ApiError::db)?;
    settings::set_bool(&app.db, settings::TOTP_ENABLED, true).map_err(ApiError::db)?;
    let codes = new_recovery_codes(&app)?;
    let _ = app.db.log(LogLevel::Info, "security", "2FA enabled");
    Ok(Json(json!({ "recovery_codes": codes })))
}

fn new_recovery_codes(app: &App) -> ApiResult<Vec<String>> {
    let (codes, hashes) =
        recovery::generate(recovery::DEFAULT_COUNT).map_err(|_| ApiError::internal("rng"))?;
    let j = serde_json::to_string(&hashes).map_err(|_| ApiError::internal("json"))?;
    app.db
        .set_setting(settings::RECOVERY_HASHES, &j)
        .map_err(ApiError::db)?;
    Ok(codes)
}

#[derive(Deserialize)]
pub struct PasswordBody {
    password: String,
}

pub(crate) async fn reauth(app: &App, ctx: &Ctx, pw: String, what: &str) -> ApiResult<()> {
    throttle_check(app, ctx)?;
    if verify_password(app, secret(pw)).await {
        Ok(())
    } else {
        record_failure(app, ctx, what);
        Err(ApiError::unauthorized("password is incorrect"))
    }
}

pub async fn totp_disable(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
    Json(body): Json<PasswordBody>,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    reauth(&app, &ctx, body.password, "2FA disable").await?;
    app.db
        .delete_secret(settings::SECRET_TOTP)
        .map_err(ApiError::db)?;
    app.db
        .delete_secret(settings::SECRET_TOTP_PENDING)
        .map_err(ApiError::db)?;
    for k in [
        settings::TOTP_ENABLED,
        settings::TOTP_LAST_STEP,
        settings::RECOVERY_HASHES,
    ] {
        app.db.delete_setting(k).map_err(ApiError::db)?;
    }
    let _ = app.db.log(LogLevel::Warn, "security", "2FA disabled");
    Ok(Json(json!({ "ok": true })))
}

pub async fn recovery_regenerate(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
    Json(body): Json<PasswordBody>,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    if !settings::get_bool(&app.db, settings::TOTP_ENABLED) {
        return Err(ApiError::bad_request("2FA is not enabled"));
    }
    reauth(&app, &ctx, body.password, "recovery-code regeneration").await?;
    let codes = new_recovery_codes(&app)?;
    let _ = app
        .db
        .log(LogLevel::Info, "security", "recovery codes regenerated");
    Ok(Json(json!({ "recovery_codes": codes })))
}

pub async fn logout_others(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let me = ctx.guard(&app, &headers)?;
    let n = app
        .db
        .delete_other_sessions(Some(&me.token_hash))
        .map_err(ApiError::db)?;
    let _ = app.db.log(
        LogLevel::Info,
        "security",
        &format!("logged out {n} other session(s)"),
    );
    Ok(Json(json!({ "logged_out": n })))
}

pub async fn rotate_keys(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    if matches!(app.cfg.key_store, KeyStoreConfig::ExternalFile(_)) {
        return Err(ApiError::bad_request(
            "the master key file is managed externally; rotate it there",
        ));
    }
    let store_guard = app
        .key_store
        .lock()
        .map_err(|_| ApiError::internal("lock"))?;
    let store: &dyn KeyStore = store_guard.as_deref().ok_or_else(ApiError::locked)?;
    let mut kr = app
        .keyring
        .read()
        .map_err(|_| ApiError::internal("lock"))?
        .clone()
        .ok_or_else(ApiError::locked)?;
    let new_id = kr.rotate().map_err(|e| ApiError::internal(e.to_string()))?;
    // Persist old+new before touching records so a crash never strands data.
    store
        .save(&kr)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    *app.keyring
        .write()
        .map_err(|_| ApiError::internal("lock"))? = Some(kr.clone());
    let n = app.db.rewrap_secrets(&kr).map_err(ApiError::db)?;
    if app
        .db
        .secret_key_ids()
        .map_err(ApiError::db)?
        .iter()
        .all(|id| *id == new_id)
    {
        kr.retire_old_keys();
        store
            .save(&kr)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        *app.keyring
            .write()
            .map_err(|_| ApiError::internal("lock"))? = Some(kr);
    }
    let _ = app.db.log(
        LogLevel::Info,
        "security",
        &format!("master key rotated to v{new_id}; {n} secrets re-wrapped"),
    );
    Ok(Json(json!({ "key_version": new_id, "rewrapped": n })))
}

#[derive(Deserialize)]
pub struct DeleteBody {
    confirm: String,
    #[serde(default)]
    password: Option<String>,
}

pub async fn delete_everything(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
    Json(body): Json<DeleteBody>,
) -> ApiResult<Response> {
    ctx.guard(&app, &headers)?;
    if body.confirm != "DELETE" {
        return Err(ApiError::bad_request("type DELETE to confirm"));
    }
    if settings::password_hash(&app.db).is_some() {
        reauth(
            &app,
            &ctx,
            body.password.unwrap_or_default(),
            "delete everything",
        )
        .await?;
    }
    app.db.wipe().map_err(ApiError::db)?;
    let external = matches!(app.cfg.key_store, KeyStoreConfig::ExternalFile(_));
    {
        let mut store = app
            .key_store
            .lock()
            .map_err(|_| ApiError::internal("lock"))?;
        if let Some(s) = store.as_ref()
            && !external
        {
            s.delete().map_err(|e| ApiError::internal(e.to_string()))?;
        }
        if matches!(app.cfg.key_store, KeyStoreConfig::Passphrase(_)) {
            // New passphrase on next unlock.
            *store = None;
            *app.keyring
                .write()
                .map_err(|_| ApiError::internal("lock"))? = None;
        } else if let Some(s) = store.as_ref() {
            let kr = if external {
                app.keyring
                    .read()
                    .map_err(|_| ApiError::internal("lock"))?
                    .clone()
            } else {
                Some(
                    keystore::load_or_create(s.as_ref())
                        .map_err(|e| ApiError::internal(e.to_string()))?,
                )
            };
            *app.keyring
                .write()
                .map_err(|_| ApiError::internal("lock"))? = kr;
        }
    }
    let _ = app
        .db
        .log(LogLevel::Warn, "security", "all data deleted by the owner");
    app.reload.notify_one();
    Ok(with_cookie(
        json!({ "ok": true, "external_key": external }),
        clear_cookie(&app),
    ))
}
