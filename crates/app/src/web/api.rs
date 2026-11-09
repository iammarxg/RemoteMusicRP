// SPDX-License-Identifier: AGPL-3.0-only
//! Main JSON API.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use remotemusicrp_core::presence::PresenceSettings;
use remotemusicrp_core::rules::Rules;
use remotemusicrp_crypto::ExposeSecret;
use remotemusicrp_crypto::SecretString;
use remotemusicrp_providers::lastfm::{self, LastFm};
use remotemusicrp_providers::{ProviderError, ScrobbleSource};
use remotemusicrp_storage::LogLevel;
use serde::Deserialize;
use serde_json::json;

use super::session::{Ctx, auth_required, create_session};
use super::{ApiError, ApiResult};
use crate::app::App;
use crate::settings;

/// Cap on unauthenticated sessions so the table can't be flooded.
const MAX_ANON_SESSIONS: i64 = 200;

/// Bootstrap state for the UI. Creates a (not yet logged-in) session when
/// none exists, so a CSRF token is always available.
pub async fn state(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let (session, cookie) = match ctx.session.clone() {
        Some(s) => (s, None),
        None => {
            let _ = app.db.trim_anonymous_sessions(MAX_ANON_SESSIONS);
            let (s, c) = create_session(&app, false, false, &headers)?;
            (s, Some(c))
        }
    };
    let required = auth_required(&app);
    let authenticated = session.authenticated || !required;
    let locked = app.is_locked();
    let lastfm = app
        .db
        .get_setting(settings::LASTFM_USER)
        .map_err(ApiError::db)?;
    let body = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "csrf": session.csrf,
        "authenticated": authenticated,
        "auth_required": required,
        "locked": locked,
        "exposed": app.cfg.is_exposed(),
        "discord_mode": if app.sdk.is_some() { "social_sdk" } else { "local_rpc" },
        "needs_setup_code": app.cfg.is_exposed() && settings::password_hash(&app.db).is_none(),
        "setup": {
            "password_set": settings::password_hash(&app.db).is_some(),
            "password_skipped": settings::get_bool(&app.db, settings::PASSWORD_SKIPPED),
            "lastfm_connected": lastfm.is_some() && app.db.has_secret(settings::SECRET_LASTFM_KEY).unwrap_or(false),
            "discord_configured": app.db.get_setting(settings::DISCORD_CLIENT_ID).map_err(ApiError::db)?.is_some(),
            "complete": settings::get_bool(&app.db, settings::SETUP_COMPLETE),
        },
    });
    let mut resp = Json(body).into_response();
    if let Some(c) = cookie {
        resp.headers_mut().append(header::SET_COOKIE, c);
    }
    Ok(resp)
}

pub async fn setup_complete(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    settings::set_bool(&app.db, settings::SETUP_COMPLETE, true).map_err(ApiError::db)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn status(State(app): State<Arc<App>>, ctx: Ctx) -> ApiResult<Json<serde_json::Value>> {
    ctx.require_auth(&app)?;
    let s = app.status.borrow().clone();
    Ok(Json(json!({
        "service": s,
        "paused": settings::get_bool(&app.db, settings::PAUSED),
        "uptime_secs": crate::now_unix().saturating_sub(app.started_unix),
        "now": crate::now_unix(),
    })))
}

pub async fn connections(
    State(app): State<Arc<App>>,
    ctx: Ctx,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.require_auth(&app)?;
    let user = app
        .db
        .get_setting(settings::LASTFM_USER)
        .map_err(ApiError::db)?;
    let has_key = app
        .db
        .has_secret(settings::SECRET_LASTFM_KEY)
        .map_err(ApiError::db)?;
    let client_id = app
        .db
        .get_setting(settings::DISCORD_CLIENT_ID)
        .map_err(ApiError::db)?;
    let s = app.status.borrow().clone();
    Ok(Json(json!({
        "lastfm": { "connected": user.is_some() && has_key, "username": user },
        "discord": {
            "mode": if app.sdk.is_some() { "social_sdk" } else { "local_rpc" },
            "client_id": client_id,
            "connected": s.discord_connected,
            "user": s.discord_user,
            "error": s.discord_error,
        },
    })))
}

#[derive(Deserialize)]
pub struct LastFmBody {
    username: String,
    api_key: String,
}

pub async fn connect_lastfm(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
    Json(body): Json<LastFmBody>,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    let user = body.username.trim().to_owned();
    let key = SecretString::from(body.api_key.trim().to_ascii_lowercase());
    if !lastfm::valid_username(&user) {
        return Err(ApiError::bad_request(
            "that is not a valid Last.fm username",
        ));
    }
    if !lastfm::valid_api_key(key.expose_secret()) {
        return Err(ApiError::bad_request(
            "a Last.fm API key is 32 characters (0-9, a-f)",
        ));
    }
    // Check the credentials before saving them.
    let probe = LastFm::new(app.http.clone(), &user, key.expose_secret().to_owned())
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    match probe.now_playing().await {
        Ok(_) => {}
        Err(ProviderError::Auth) => {
            return Err(ApiError::bad_request("Last.fm rejected this API key"));
        }
        Err(ProviderError::Config(_)) => {
            return Err(ApiError::bad_request("Last.fm user not found"));
        }
        Err(e) => {
            return Err(ApiError::bad_request(format!(
                "could not reach Last.fm: {e}"
            )));
        }
    }
    app.with_keyring(|kr| {
        app.db.put_secret(
            kr,
            settings::SECRET_LASTFM_KEY,
            settings::PURPOSE_LASTFM_KEY,
            key.expose_secret().as_bytes(),
        )
    })
    .map_err(|_| ApiError::locked())?
    .map_err(ApiError::db)?;
    app.db
        .set_setting(settings::LASTFM_USER, &user)
        .map_err(ApiError::db)?;
    let _ = app.db.log(
        LogLevel::Info,
        "lastfm",
        &format!("Last.fm connected as {user}"),
    );
    crate::email::reset_alert(&app, "lastfm");
    app.reload.notify_one();
    Ok(Json(json!({ "ok": true, "username": user })))
}

pub async fn disconnect_lastfm(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    app.db
        .delete_secret(settings::SECRET_LASTFM_KEY)
        .map_err(ApiError::db)?;
    app.db
        .delete_setting(settings::LASTFM_USER)
        .map_err(ApiError::db)?;
    let _ = app.db.log(LogLevel::Info, "lastfm", "Last.fm disconnected");
    app.reload.notify_one();
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct DiscordBody {
    client_id: String,
}

pub async fn connect_discord(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
    Json(body): Json<DiscordBody>,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    let id = body.client_id.trim();
    if !remotemusicrp_discord::valid_client_id(id) {
        return Err(ApiError::bad_request(
            "a Discord Application ID is 17–20 digits",
        ));
    }
    app.db
        .set_setting(settings::DISCORD_CLIENT_ID, id)
        .map_err(ApiError::db)?;
    let _ = app
        .db
        .log(LogLevel::Info, "discord", "Discord application ID saved");
    app.reload.notify_one();
    Ok(Json(json!({ "ok": true })))
}

pub async fn disconnect_discord(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    if let Some(sdk) = app.sdk.as_ref() {
        sdk.sign_out(&app).await;
    }
    app.db
        .delete_setting(settings::DISCORD_CLIENT_ID)
        .map_err(ApiError::db)?;
    let _ = app
        .db
        .log(LogLevel::Info, "discord", "Discord disconnected");
    app.reload.notify_one();
    Ok(Json(json!({ "ok": true })))
}

pub async fn get_presence(
    State(app): State<Arc<App>>,
    ctx: Ctx,
) -> ApiResult<Json<PresenceSettings>> {
    ctx.require_auth(&app)?;
    Ok(Json(settings::presence(&app.db)))
}

pub async fn put_presence(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
    Json(body): Json<PresenceSettings>,
) -> ApiResult<Json<PresenceSettings>> {
    ctx.guard(&app, &headers)?;
    body.validate().map_err(ApiError::bad_request)?;
    let j = serde_json::to_string(&body).map_err(|_| ApiError::internal("json"))?;
    app.db
        .set_setting(settings::PRESENCE, &j)
        .map_err(ApiError::db)?;
    Ok(Json(body))
}

#[derive(Deserialize)]
pub struct PauseBody {
    paused: bool,
}

pub async fn pause(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
    Json(body): Json<PauseBody>,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    settings::set_bool(&app.db, settings::PAUSED, body.paused).map_err(ApiError::db)?;
    let _ = app.db.log(
        LogLevel::Info,
        "presence",
        if body.paused {
            "presence paused"
        } else {
            "presence resumed"
        },
    );
    app.reload.notify_one();
    Ok(Json(json!({ "paused": body.paused })))
}

#[derive(Deserialize)]
pub struct ActivityQuery {
    #[serde(default)]
    limit: Option<u32>,
}

pub async fn activity(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    Query(q): Query<ActivityQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.require_auth(&app)?;
    let entries: Vec<_> = app
        .db
        .recent_log(q.limit.unwrap_or(200).min(1000))
        .map_err(ApiError::db)?
        .into_iter()
        .map(|e| json!({ "at": e.at, "level": e.level, "kind": e.kind, "message": e.message }))
        .collect();
    let now = crate::now_unix();
    let day_ago = i64::try_from(now.saturating_sub(86_400)).unwrap_or(0);
    let (info, warn, error) = app.db.log_counts_since(day_ago).map_err(ApiError::db)?;
    Ok(Json(json!({
        "entries": entries,
        "stats": {
            "started_unix": app.started_unix,
            "uptime_secs": now.saturating_sub(app.started_unix),
            "tracks_seen": app.tracks_seen.load(std::sync::atomic::Ordering::Relaxed),
            "last_day": { "info": info, "warn": warn, "error": error },
        },
    })))
}

fn rules_body(r: Rules) -> serde_json::Value {
    let now = i64::try_from(crate::now_unix()).unwrap_or(0);
    json!({
        "quiet_now": r.quiet_at(now),
        "zone_in_use": r.zone_name(),
        "system_zone": Rules::default().zone_name(),
        "rules": r,
    })
}

pub async fn get_rules(
    State(app): State<Arc<App>>,
    ctx: Ctx,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.require_auth(&app)?;
    Ok(Json(rules_body(settings::rules(&app.db))))
}

pub async fn put_rules(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
    Json(body): Json<Rules>,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    let body = body.normalized();
    body.validate().map_err(ApiError::bad_request)?;
    let j = serde_json::to_string(&body).map_err(|_| ApiError::internal("json"))?;
    app.db
        .set_setting(settings::RULES, &j)
        .map_err(ApiError::db)?;
    // The service re-reads the rules on every poll (no reconnect needed).
    Ok(Json(rules_body(body)))
}

fn email_body(app: &App) -> ApiResult<serde_json::Value> {
    Ok(json!({
        "settings": crate::email::load(app),
        "has_secret": app.db.has_secret(settings::SECRET_EMAIL).map_err(ApiError::db)?,
    }))
}

pub async fn get_email(
    State(app): State<Arc<App>>,
    ctx: Ctx,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.require_auth(&app)?;
    Ok(Json(email_body(&app)?))
}

#[derive(Deserialize)]
pub struct EmailBody {
    settings: crate::email::EmailSettings,
    /// New Resend API key or SMTP password; empty keeps the saved one.
    #[serde(default)]
    secret: Option<String>,
}

pub async fn put_email(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
    Json(body): Json<EmailBody>,
) -> ApiResult<Json<serde_json::Value>> {
    use crate::email::Provider;
    ctx.guard(&app, &headers)?;
    let new = body.settings.normalized();
    new.validate().map_err(ApiError::bad_request)?;
    let secret = SecretString::from(body.secret.unwrap_or_default().trim().to_owned());
    let old = crate::email::load(&app);
    let has_secret = app
        .db
        .has_secret(settings::SECRET_EMAIL)
        .map_err(ApiError::db)?;
    if new.provider == Provider::None {
        app.db
            .delete_secret(settings::SECRET_EMAIL)
            .map_err(ApiError::db)?;
    } else if !secret.expose_secret().is_empty() {
        crate::email::validate_secret(new.provider, secret.expose_secret())
            .map_err(ApiError::bad_request)?;
        app.with_keyring(|kr| {
            app.db.put_secret(
                kr,
                settings::SECRET_EMAIL,
                settings::PURPOSE_EMAIL,
                secret.expose_secret().as_bytes(),
            )
        })
        .map_err(|_| ApiError::locked())?
        .map_err(ApiError::db)?;
    } else if new.provider == Provider::Resend && (old.provider != Provider::Resend || !has_secret)
    {
        return Err(ApiError::bad_request("enter your Resend API key"));
    } else if new.provider == Provider::Smtp
        && !new.smtp_username.is_empty()
        && (old.provider != Provider::Smtp || !has_secret)
    {
        return Err(ApiError::bad_request("enter the SMTP password"));
    }
    let j = serde_json::to_string(&new).map_err(|_| ApiError::internal("json"))?;
    app.db
        .set_setting(settings::EMAIL, &j)
        .map_err(ApiError::db)?;
    let _ = app.db.log(
        LogLevel::Info,
        "email",
        if new.provider == Provider::None {
            "email alerts turned off"
        } else {
            "email settings saved"
        },
    );
    Ok(Json(email_body(&app)?))
}

pub async fn delete_email(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    app.db
        .delete_secret(settings::SECRET_EMAIL)
        .map_err(ApiError::db)?;
    app.db
        .delete_setting(settings::EMAIL)
        .map_err(ApiError::db)?;
    let _ = app
        .db
        .log(LogLevel::Info, "email", "email settings removed");
    Ok(Json(email_body(&app)?))
}

pub async fn test_email(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    crate::email::send(
        &app,
        "RemoteMusicRP test email",
        "This is a test email from your RemoteMusicRP instance.

         If you received it, email alerts are set up correctly. You will get an email          when a connection needs your attention (at most once a day per problem).",
    )
    .await
    .map_err(ApiError::bad_request)?;
    let _ = app.db.log(LogLevel::Info, "email", "test email sent");
    Ok(Json(json!({ "ok": true })))
}

pub async fn instance_settings(
    State(app): State<Arc<App>>,
    ctx: Ctx,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.require_auth(&app)?;
    let c = &app.cfg;
    Ok(Json(json!({
        "bind": c.bind.to_string(),
        "public_origin": c.public_origin,
        "exposed": c.is_exposed(),
        "poll_interval_secs": c.poll_interval_secs,
        "log_retention_days": c.log_retention_days,
        "update_check": c.update_check,
        "artwork": {
            "apple": c.artwork.apple,
            "cover_art_archive": c.artwork.cover_art_archive,
            "apple_first": c.artwork.apple_first,
            "country": c.artwork.country,
        },
        "data_dir": c.data_dir.display().to_string(),
    })))
}

pub async fn updates(State(app): State<Arc<App>>, ctx: Ctx) -> ApiResult<Json<serde_json::Value>> {
    ctx.require_auth(&app)?;
    Ok(Json(json!({
        "info": crate::updater::info(&app),
        "check_enabled": app.cfg.update_check,
        "auto_install": settings::get_bool(&app.db, settings::UPDATES_AUTO_INSTALL),
    })))
}

pub async fn updates_check(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
) -> ApiResult<Json<crate::updater::UpdateInfo>> {
    ctx.guard(&app, &headers)?;
    Ok(Json(crate::updater::check(&app).await))
}

pub async fn updates_install(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    let stop = app.stop.clone();
    match crate::updater::install(&app, &stop).await {
        Ok(v) => Ok(Json(json!({ "installed": v, "restarting": true }))),
        Err(e) => Err(ApiError::bad_request(e)),
    }
}

#[derive(Deserialize)]
pub struct UpdateSettingsBody {
    auto_install: bool,
}

pub async fn updates_settings(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
    Json(body): Json<UpdateSettingsBody>,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    settings::set_bool(&app.db, settings::UPDATES_AUTO_INSTALL, body.auto_install)
        .map_err(ApiError::db)?;
    Ok(Json(json!({ "auto_install": body.auto_install })))
}

pub async fn about() -> Json<serde_json::Value> {
    Json(json!({
        "name": "RemoteMusicRP",
        "version": env!("CARGO_PKG_VERSION"),
        "license": "AGPL-3.0-only",
        "repository": "https://github.com/iammarxg/RemoteMusicRP",
        "attribution": [
            { "name": "Last.fm", "text": "Powered by AudioScrobbler", "url": "https://www.last.fm/" },
            { "name": "Apple", "text": "Album art via the iTunes Search API", "url": "https://music.apple.com/" },
            { "name": "MusicBrainz", "text": "Album art from the Cover Art Archive", "url": "https://coverartarchive.org/" },
        ],
    }))
}
