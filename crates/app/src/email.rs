// SPDX-License-Identifier: AGPL-3.0-only
//! Optional email alerts, sent with the owner's own Resend API key or SMTP
//! server. TLS is always required (implicit TLS or STARTTLS); the secret
//! (API key or SMTP password) is stored envelope-encrypted.

use std::sync::Arc;
use std::time::Duration;

use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use remotemusicrp_storage::LogLevel;
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::settings;

const RESEND_URL: &str = "https://api.resend.com/emails";
const SEND_TIMEOUT: Duration = Duration::from_secs(20);
/// At most one alert of each kind per day.
const ALERT_EVERY_SECS: u64 = 24 * 3600;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    #[default]
    None,
    Resend,
    Smtp,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmtpSecurity {
    /// Plain connection upgraded with STARTTLS (required, never skipped).
    #[default]
    Starttls,
    /// TLS from the first byte (usually port 465).
    Tls,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmailSettings {
    pub provider: Provider,
    pub from: String,
    pub to: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: SmtpSecurity,
    pub smtp_username: String,
    /// Alert when Last.fm stops accepting the saved credentials.
    pub alert_lastfm: bool,
}

impl Default for EmailSettings {
    fn default() -> Self {
        Self {
            provider: Provider::None,
            from: String::new(),
            to: String::new(),
            smtp_host: String::new(),
            smtp_port: 587,
            smtp_security: SmtpSecurity::Starttls,
            smtp_username: String::new(),
            alert_lastfm: true,
        }
    }
}

fn mailbox(s: &str, what: &'static str) -> Result<Mailbox, &'static str> {
    if s.len() > 320 || s.chars().any(char::is_control) {
        return Err(what);
    }
    s.trim().parse::<Mailbox>().map_err(|_| what)
}

fn valid_host(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && !h.starts_with(['.', '-'])
        && h.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// Whether `s` looks like a Resend API key (`re_…`).
pub fn valid_resend_key(s: &str) -> bool {
    s.starts_with("re_") && s.len() <= 128 && s.chars().all(|c| c.is_ascii_graphic())
}

impl EmailSettings {
    pub fn normalized(mut self) -> Self {
        for f in [
            &mut self.from,
            &mut self.to,
            &mut self.smtp_host,
            &mut self.smtp_username,
        ] {
            *f = f.trim().to_owned();
        }
        self.smtp_host.make_ascii_lowercase();
        self
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.provider == Provider::None {
            return Ok(());
        }
        mailbox(&self.from, "enter a valid sender address")?;
        mailbox(&self.to, "enter a valid recipient address")?;
        if self.provider == Provider::Smtp {
            if !valid_host(&self.smtp_host) {
                return Err("enter the SMTP server name, e.g. smtp.example.com");
            }
            if self.smtp_port == 0 {
                return Err("enter the SMTP port, e.g. 587 or 465");
            }
            if self.smtp_username.len() > 256 || self.smtp_username.chars().any(char::is_control) {
                return Err("the SMTP username is too long");
            }
        }
        Ok(())
    }
}

pub fn validate_secret(provider: Provider, secret: &str) -> Result<(), &'static str> {
    match provider {
        Provider::Resend if !valid_resend_key(secret) => Err("a Resend API key starts with re_"),
        Provider::Smtp if secret.len() > 256 || secret.chars().any(char::is_control) => {
            Err("the SMTP password is too long")
        }
        _ => Ok(()),
    }
}

pub fn load(app: &App) -> EmailSettings {
    app.db
        .get_setting(settings::EMAIL)
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

fn load_secret(app: &App) -> Result<Option<remotemusicrp_crypto::Zeroizing<Vec<u8>>>, String> {
    app.with_keyring(|kr| {
        app.db
            .get_secret(kr, settings::SECRET_EMAIL, settings::PURPOSE_EMAIL)
    })
    .map_err(str::to_owned)?
    .map_err(|e| e.to_string())
}

/// Send one email with the saved settings. Errors are user-facing and never
/// contain the secret.
pub async fn send(app: &App, subject: &str, text: &str) -> Result<(), String> {
    let cfg = load(app);
    if cfg.provider == Provider::None {
        return Err("email is not set up".into());
    }
    cfg.validate().map_err(str::to_owned)?;
    let secret = load_secret(app)?.ok_or("the email password or API key is missing")?;
    let secret = std::str::from_utf8(&secret).map_err(|_| "stored email secret is corrupt")?;
    match cfg.provider {
        Provider::None => Ok(()),
        Provider::Resend => send_resend(app, &cfg, secret, subject, text).await,
        Provider::Smtp => send_smtp(&cfg, secret, subject, text).await,
    }
}

async fn send_resend(
    app: &App,
    cfg: &EmailSettings,
    key: &str,
    subject: &str,
    text: &str,
) -> Result<(), String> {
    let resp = app
        .http
        .post(RESEND_URL)
        .bearer_auth(key)
        .timeout(SEND_TIMEOUT)
        .json(&serde_json::json!({
            "from": cfg.from,
            "to": [cfg.to],
            "subject": subject,
            "text": text,
        }))
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "Resend did not answer in time".to_owned()
            } else {
                "could not reach Resend".to_owned()
            }
        })?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    // Resend errors look like {"statusCode":…, "message":"…", "name":"…"}.
    let msg = resp
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_owned))
        .map(|m| {
            m.chars()
                .filter(|c| !c.is_control())
                .take(200)
                .collect::<String>()
        })
        .unwrap_or_default();
    Err(match status.as_u16() {
        401 | 403 => format!("Resend rejected the API key or sender. {msg}"),
        _ => format!("Resend returned an error ({status}). {msg}"),
    }
    .trim()
    .to_owned())
}

async fn send_smtp(
    cfg: &EmailSettings,
    password: &str,
    subject: &str,
    text: &str,
) -> Result<(), String> {
    let from = mailbox(&cfg.from, "invalid sender address")?;
    let to = mailbox(&cfg.to, "invalid recipient address")?;
    let msg = Message::builder()
        .from(from)
        .to(to)
        .subject(subject)
        .user_agent(format!("RemoteMusicRP/{}", env!("CARGO_PKG_VERSION")))
        .body(text.to_owned())
        .map_err(|_| "could not build the email".to_owned())?;
    let builder = match cfg.smtp_security {
        SmtpSecurity::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.smtp_host),
        SmtpSecurity::Starttls => {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.smtp_host)
        }
    }
    .map_err(|_| "invalid SMTP server name".to_owned())?;
    let mut builder = builder.port(cfg.smtp_port).timeout(Some(SEND_TIMEOUT));
    if !cfg.smtp_username.is_empty() {
        builder = builder.credentials(Credentials::new(
            cfg.smtp_username.clone(),
            password.to_owned(),
        ));
    }
    let transport = builder.build();
    transport.send(msg).await.map(|_| ()).map_err(|e| {
        // lettre errors describe the stage (connection, TLS, auth, response)
        // and never include the password.
        let s = e.to_string();
        let s: String = s.chars().filter(|c| !c.is_control()).take(200).collect();
        if e.is_permanent() {
            format!("the SMTP server refused the email: {s}")
        } else {
            format!("SMTP error: {s}")
        }
    })
}

/// Fire-and-forget alert, at most once a day per `kind`. The result is
/// written to the activity log.
pub fn alert(app: &Arc<App>, kind: &'static str, subject: String, text: String) {
    let cfg = load(app);
    let wanted = match kind {
        "lastfm" => cfg.alert_lastfm,
        _ => true,
    };
    if cfg.provider == Provider::None || !wanted {
        return;
    }
    let key = format!("{}{kind}", settings::EMAIL_LAST_ALERT_PREFIX);
    let now = crate::now_unix();
    let last = app
        .db
        .get_setting(&key)
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    if now.saturating_sub(last) < ALERT_EVERY_SECS {
        return;
    }
    let _ = app.db.set_setting(&key, &now.to_string());
    let app = app.clone();
    tokio::spawn(async move {
        match send(&app, &subject, &text).await {
            Ok(()) => {
                let _ = app.db.log(LogLevel::Info, "email", "alert email sent");
            }
            Err(e) => {
                tracing::warn!(error = %e, "alert email failed");
                let _ = app
                    .db
                    .log(LogLevel::Warn, "email", &format!("alert email failed: {e}"));
            }
        }
    });
}

/// Forget that an alert was sent (after the problem was fixed), so the next
/// failure alerts again.
pub fn reset_alert(app: &App, kind: &str) {
    let _ = app
        .db
        .delete_setting(&format!("{}{kind}", settings::EMAIL_LAST_ALERT_PREFIX));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation() {
        let mut s = EmailSettings {
            provider: Provider::Smtp,
            from: "RemoteMusicRP <me@example.com>".into(),
            to: "me@example.com".into(),
            smtp_host: "smtp.example.com".into(),
            ..Default::default()
        };
        assert!(s.validate().is_ok());
        s.to = "me@example.com\r\nBcc: x@y.z".into();
        assert!(s.validate().is_err());
        s.to = "me@example.com".into();
        s.smtp_host = "smtp example".into();
        assert!(s.validate().is_err());
        s.smtp_host = "smtp.example.com".into();
        s.smtp_port = 0;
        assert!(s.validate().is_err());
        assert!(EmailSettings::default().validate().is_ok());
    }

    #[test]
    fn secrets() {
        assert!(validate_secret(Provider::Resend, "re_abc123").is_ok());
        assert!(validate_secret(Provider::Resend, "abc").is_err());
        assert!(validate_secret(Provider::Smtp, "pa ss").is_ok());
        assert!(validate_secret(Provider::Smtp, "a\nb").is_err());
    }
}
