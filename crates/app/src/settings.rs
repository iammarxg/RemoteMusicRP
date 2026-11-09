// SPDX-License-Identifier: AGPL-3.0-only
//! Keys and typed accessors for user settings stored in the database.

use remotemusicrp_core::presence::PresenceSettings;
use remotemusicrp_core::rules::Rules;
use remotemusicrp_storage::Db;

pub const LASTFM_USER: &str = "lastfm.username";
pub const DISCORD_CLIENT_ID: &str = "discord.client_id";
pub const PRESENCE: &str = "presence.settings";
pub const RULES: &str = "presence.rules";
pub const EMAIL: &str = "email.settings";
pub const EMAIL_LAST_ALERT_PREFIX: &str = "email.last_alert.";
pub const PAUSED: &str = "presence.paused";
pub const PASSWORD_HASH: &str = "auth.password_hash";
pub const PASSWORD_SKIPPED: &str = "auth.password_skipped";
pub const TOTP_ENABLED: &str = "auth.totp_enabled";
pub const TOTP_LAST_STEP: &str = "auth.totp_last_step";
pub const RECOVERY_HASHES: &str = "auth.recovery_hashes";
pub const SETUP_COMPLETE: &str = "setup.complete";
pub const UPDATES_AUTO_INSTALL: &str = "updates.auto_install";

pub const SECRET_LASTFM_KEY: &str = "lastfm.api_key";
pub const PURPOSE_LASTFM_KEY: &str = "lastfm_api_key";
pub const SECRET_TOTP: &str = "auth.totp";
pub const SECRET_TOTP_PENDING: &str = "auth.totp_pending";
pub const PURPOSE_TOTP: &str = "totp_secret";
/// Resend API key or SMTP password.
pub const SECRET_EMAIL: &str = "email.secret";
pub const PURPOSE_EMAIL: &str = "email_secret";
/// Server mode: the Discord OAuth2 refresh token (access tokens stay in memory).
pub const SECRET_DISCORD_REFRESH: &str = "discord.refresh_token";
#[cfg_attr(not(feature = "social-sdk"), allow(dead_code))]
pub const PURPOSE_DISCORD_REFRESH: &str = "discord_refresh_token";

pub fn get_bool(db: &Db, key: &str) -> bool {
    matches!(db.get_setting(key).ok().flatten().as_deref(), Some("true"))
}

pub fn set_bool(db: &Db, key: &str, value: bool) -> remotemusicrp_storage::Result<()> {
    db.set_setting(key, if value { "true" } else { "false" })
}

pub fn presence(db: &Db) -> PresenceSettings {
    db.get_setting(PRESENCE)
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

pub fn rules(db: &Db) -> Rules {
    db.get_setting(RULES)
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

pub fn password_hash(db: &Db) -> Option<String> {
    db.get_setting(PASSWORD_HASH)
        .ok()
        .flatten()
        .filter(|h| !h.is_empty())
}

pub fn recovery_hashes(db: &Db) -> Vec<String> {
    db.get_setting(RECOVERY_HASHES)
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}
