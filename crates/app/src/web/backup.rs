// SPDX-License-Identifier: AGPL-3.0-only
//! Settings export/import.
//!
//! - "full" (default): settings **and** secrets (Last.fm API key, email
//!   password/API key), encrypted with a passphrase the owner chooses
//!   (Argon2id + XChaCha20-Poly1305, purpose-bound to `export`).
//! - "settings": settings only, never any secret, plain JSON.
//!
//! Login credentials (admin password, 2FA, sessions) are never exported:
//! importing must not change who can sign in to an instance.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use remotemusicrp_core::presence::PresenceSettings;
use remotemusicrp_core::rules::Rules;
use remotemusicrp_crypto::passphrase::{self, KdfParams};
use remotemusicrp_crypto::{ExposeSecret, SecretString, Zeroizing};
use remotemusicrp_providers::lastfm;
use remotemusicrp_storage::LogLevel;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::session::Ctx;
use super::{ApiError, ApiResult};
use crate::app::App;
use crate::email::{self, EmailSettings};
use crate::settings;

const FORMAT: &str = "remotemusicrp-export";
const VERSION: u32 = 1;
const PURPOSE: &str = "export";
const MIN_PASSPHRASE: usize = 10;
const MAX_PASSPHRASE: usize = 1024;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Contents {
    #[serde(skip_serializing_if = "Option::is_none")]
    presence: Option<PresenceSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rules: Option<Rules>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lastfm_username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    discord_client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<EmailSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updates_auto_install: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    secrets: Option<Secrets>,
}

/// A secret string that is wiped from memory when dropped.
struct Sec(String);

impl Sec {
    fn as_str(&self) -> &str {
        &self.0
    }
}

impl Drop for Sec {
    fn drop(&mut self) {
        drop(Zeroizing::new(std::mem::take(&mut self.0)));
    }
}

impl Serialize for Sec {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Sec {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Sec)
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Secrets {
    #[serde(skip_serializing_if = "Option::is_none")]
    lastfm_api_key: Option<Sec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    email_secret: Option<Sec>,
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secrets(..)")
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportFile {
    format: String,
    version: u32,
    kind: String,
    app_version: String,
    created_unix: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data: Option<Contents>,
    /// Base64 of the passphrase-encrypted JSON `Contents` (full exports).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    encrypted: Option<String>,
}

fn secret_string(app: &App, name: &str, purpose: &str) -> ApiResult<Option<String>> {
    let v = app
        .with_keyring(|kr| app.db.get_secret(kr, name, purpose))
        .map_err(|_| ApiError::locked())?
        .map_err(ApiError::db)?;
    v.map(|b| String::from_utf8(b.to_vec()).map_err(|_| ApiError::internal("corrupt secret")))
        .transpose()
}

fn collect(app: &App, with_secrets: bool) -> ApiResult<Contents> {
    let db = &app.db;
    let email = email::load(app);
    Ok(Contents {
        presence: Some(settings::presence(db)),
        rules: Some(settings::rules(db)),
        lastfm_username: db
            .get_setting(settings::LASTFM_USER)
            .map_err(ApiError::db)?,
        discord_client_id: db
            .get_setting(settings::DISCORD_CLIENT_ID)
            .map_err(ApiError::db)?,
        email: Some(email),
        updates_auto_install: Some(settings::get_bool(db, settings::UPDATES_AUTO_INSTALL)),
        secrets: if with_secrets {
            Some(Secrets {
                lastfm_api_key: secret_string(
                    app,
                    settings::SECRET_LASTFM_KEY,
                    settings::PURPOSE_LASTFM_KEY,
                )?
                .map(Sec),
                email_secret: secret_string(app, settings::SECRET_EMAIL, settings::PURPOSE_EMAIL)?
                    .map(Sec),
            })
        } else {
            None
        },
    })
}

async fn argon<T: Send + 'static>(
    app: &App,
    f: impl FnOnce() -> T + Send + 'static,
) -> ApiResult<T> {
    let _permit = app
        .hash_permits
        .acquire()
        .await
        .map_err(|_| ApiError::internal("semaphore"))?;
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| ApiError::internal("join"))
}

fn check_passphrase(p: &SecretString) -> ApiResult<()> {
    let n = p.expose_secret().chars().count();
    if n < MIN_PASSPHRASE {
        return Err(ApiError::bad_request(
            "the passphrase must be at least 10 characters",
        ));
    }
    if n > MAX_PASSPHRASE {
        return Err(ApiError::bad_request("the passphrase is too long"));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct ExportBody {
    kind: String,
    #[serde(default)]
    passphrase: Option<String>,
    #[serde(default)]
    password: Option<String>,
}

pub async fn export(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
    Json(body): Json<ExportBody>,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    let full = match body.kind.as_str() {
        "full" => true,
        "settings" => false,
        _ => return Err(ApiError::bad_request("unknown export type")),
    };
    let mut file = ExportFile {
        format: FORMAT.into(),
        version: VERSION,
        kind: body.kind.clone(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        created_unix: crate::now_unix(),
        data: None,
        encrypted: None,
    };
    if full {
        let pass = SecretString::from(body.passphrase.unwrap_or_default());
        check_passphrase(&pass)?;
        // Secrets leave the instance: confirm the admin password if one is set.
        if settings::password_hash(&app.db).is_some() {
            super::auth::reauth(&app, &ctx, body.password.unwrap_or_default(), "export").await?;
        }
        let contents = collect(&app, true)?;
        let plain =
            Zeroizing::new(serde_json::to_vec(&contents).map_err(|_| ApiError::internal("json"))?);
        drop(contents);
        let blob = argon(&app, move || {
            passphrase::seal(
                pass.expose_secret().as_bytes(),
                PURPOSE,
                &plain,
                KdfParams::DEFAULT,
            )
        })
        .await?
        .map_err(|_| ApiError::internal("encryption failed"))?;
        file.encrypted = Some(B64.encode(blob));
    } else {
        file.data = Some(collect(&app, false)?);
    }
    let _ = app.db.log(
        LogLevel::Info,
        "backup",
        if full {
            "full export created (encrypted, includes secrets)"
        } else {
            "settings-only export created"
        },
    );
    Ok(Json(
        serde_json::to_value(&file).map_err(|_| ApiError::internal("json"))?,
    ))
}

#[derive(Deserialize)]
pub struct ImportBody {
    file: serde_json::Value,
    #[serde(default)]
    passphrase: Option<String>,
}

fn validate(c: &mut Contents) -> ApiResult<()> {
    let bad = ApiError::bad_request;
    if let Some(p) = &c.presence {
        p.validate().map_err(|e| bad(format!("presence: {e}")))?;
    }
    if let Some(r) = c.rules.take() {
        let r = r.normalized();
        r.validate()
            .map_err(|e| bad(format!("hide & schedule: {e}")))?;
        c.rules = Some(r);
    }
    if let Some(u) = &c.lastfm_username
        && !lastfm::valid_username(u)
    {
        return Err(bad("the file contains an invalid Last.fm username".into()));
    }
    if let Some(id) = &c.discord_client_id
        && !remotemusicrp_discord::valid_client_id(id)
    {
        return Err(bad(
            "the file contains an invalid Discord Application ID".into()
        ));
    }
    if let Some(e) = c.email.take() {
        let e = e.normalized();
        e.validate().map_err(|m| bad(format!("email: {m}")))?;
        c.email = Some(e);
    }
    if let Some(s) = &c.secrets {
        if let Some(k) = &s.lastfm_api_key
            && !lastfm::valid_api_key(k.as_str())
        {
            return Err(bad("the file contains an invalid Last.fm API key".into()));
        }
        if let Some(es) = &s.email_secret {
            let provider = c.email.as_ref().map(|e| e.provider).unwrap_or_default();
            email::validate_secret(provider, es.as_str())
                .map_err(|m| bad(format!("email: {m}")))?;
        }
    }
    Ok(())
}

pub async fn import(
    State(app): State<Arc<App>>,
    ctx: Ctx,
    headers: HeaderMap,
    Json(body): Json<ImportBody>,
) -> ApiResult<Json<serde_json::Value>> {
    ctx.guard(&app, &headers)?;
    let file: ExportFile = serde_json::from_value(body.file)
        .map_err(|_| ApiError::bad_request("this is not a RemoteMusicRP export file"))?;
    if file.format != FORMAT {
        return Err(ApiError::bad_request(
            "this is not a RemoteMusicRP export file",
        ));
    }
    if file.version > VERSION {
        return Err(ApiError::bad_request(
            "this export was made by a newer version of RemoteMusicRP; update first",
        ));
    }
    let mut contents = match (file.data, file.encrypted) {
        (Some(d), None) => {
            if d.secrets.is_some() {
                return Err(ApiError::bad_request(
                    "an unencrypted export must not contain secrets",
                ));
            }
            d
        }
        (None, Some(enc)) => {
            let pass = SecretString::from(body.passphrase.unwrap_or_default());
            if pass.expose_secret().is_empty() {
                return Err(ApiError::bad_request(
                    "this export is encrypted; enter its passphrase",
                ));
            }
            let blob = B64
                .decode(enc.trim())
                .map_err(|_| ApiError::bad_request("the export file is damaged"))?;
            let plain = argon(&app, move || {
                passphrase::open(pass.expose_secret().as_bytes(), PURPOSE, &blob)
            })
            .await?
            .map_err(|_| ApiError::bad_request("wrong passphrase, or the file is damaged"))?;
            serde_json::from_slice::<Contents>(&plain)
                .map_err(|_| ApiError::bad_request("the export file is damaged"))?
        }
        _ => {
            return Err(ApiError::bad_request("the export file is damaged"));
        }
    };
    validate(&mut contents)?;
    if contents.secrets.is_some() && app.is_locked() {
        return Err(ApiError::locked());
    }

    // Everything is valid: apply.
    let db = &app.db;
    let json_err = |_| ApiError::internal("json");
    if let Some(p) = &contents.presence {
        db.set_setting(
            settings::PRESENCE,
            &serde_json::to_string(p).map_err(json_err)?,
        )
        .map_err(ApiError::db)?;
    }
    if let Some(r) = &contents.rules {
        db.set_setting(
            settings::RULES,
            &serde_json::to_string(r).map_err(json_err)?,
        )
        .map_err(ApiError::db)?;
    }
    if let Some(e) = &contents.email {
        db.set_setting(
            settings::EMAIL,
            &serde_json::to_string(e).map_err(json_err)?,
        )
        .map_err(ApiError::db)?;
    }
    if let Some(v) = contents.updates_auto_install {
        settings::set_bool(db, settings::UPDATES_AUTO_INSTALL, v).map_err(ApiError::db)?;
    }
    if let Some(id) = &contents.discord_client_id {
        db.set_setting(settings::DISCORD_CLIENT_ID, id)
            .map_err(ApiError::db)?;
    }
    let mut imported_secrets = 0;
    if let Some(s) = &contents.secrets {
        for (value, name, purpose) in [
            (
                &s.lastfm_api_key,
                settings::SECRET_LASTFM_KEY,
                settings::PURPOSE_LASTFM_KEY,
            ),
            (
                &s.email_secret,
                settings::SECRET_EMAIL,
                settings::PURPOSE_EMAIL,
            ),
        ] {
            if let Some(v) = value {
                app.with_keyring(|kr| db.put_secret(kr, name, purpose, v.as_str().as_bytes()))
                    .map_err(|_| ApiError::locked())?
                    .map_err(ApiError::db)?;
                imported_secrets += 1;
            }
        }
    }
    // Only switch the Last.fm user when its key is known to match: either
    // the key came with it, or there was no user before.
    if let Some(u) = &contents.lastfm_username {
        let has_key = contents
            .secrets
            .as_ref()
            .is_some_and(|s| s.lastfm_api_key.is_some());
        let current = db
            .get_setting(settings::LASTFM_USER)
            .map_err(ApiError::db)?;
        if has_key || current.is_none() || current.as_deref() == Some(u) {
            db.set_setting(settings::LASTFM_USER, u)
                .map_err(ApiError::db)?;
        }
    }
    let _ = db.log(
        LogLevel::Info,
        "backup",
        &format!(
            "settings imported ({} export, {imported_secrets} secret(s))",
            if file.kind == "full" {
                "full"
            } else {
                "settings-only"
            }
        ),
    );
    email::reset_alert(&app, "lastfm");
    app.reload.notify_one();
    Ok(Json(json!({ "ok": true, "secrets": imported_secrets })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contents_roundtrip_and_rejects_unknown_fields() {
        let c = Contents {
            lastfm_username: Some("someone".into()),
            secrets: Some(Secrets {
                lastfm_api_key: Some(Sec("0123456789abcdef0123456789abcdef".into())),
                email_secret: None,
            }),
            ..Default::default()
        };
        let j = serde_json::to_string(&c).unwrap_or_default();
        let mut back: Contents = serde_json::from_str(&j).unwrap_or_default();
        assert!(validate(&mut back).is_ok());
        assert!(serde_json::from_str::<Contents>(r#"{"password_hash":"x"}"#).is_err());
        let mut bad: Contents =
            serde_json::from_str(r#"{"discord_client_id":"abc"}"#).unwrap_or_default();
        assert!(validate(&mut bad).is_err());
    }

    #[test]
    fn debug_hides_secrets() {
        let s = Secrets {
            lastfm_api_key: Some(Sec("supersecret".into())),
            email_secret: None,
        };
        assert!(!format!("{s:?}").contains("supersecret"));
    }
}
