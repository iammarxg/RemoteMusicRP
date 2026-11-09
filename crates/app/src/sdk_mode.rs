// SPDX-License-Identifier: AGPL-3.0-only
//! EXPERIMENTAL server mode (Discord Social SDK, CLAUDE.md §4): sign-in,
//! token lifecycle and the presence sink. Safe code only; the FFI lives in
//! `remotemusicrp-discord-sdk-ffi` (cargo feature `social-sdk`).
//!
//! - Sign-in: OAuth2 authorization code + PKCE (public client) in the
//!   owner's own browser. Discord redirects to `<UI origin>/discord-callback`,
//!   the page posts `code` + `state` back. `state` is single-use, bound to
//!   the session that started it and valid for 10 minutes.
//! - Only the **refresh token** is stored (envelope-encrypted). Access tokens
//!   (7 days) live in memory and are renewed a day before they expire.
//! - One Discord account per instance (owner decision 2026-11-09).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use remotemusicrp_discord::{Activity, DiscordError};

use crate::app::App;

pub const SCOPES: &str = "openid sdk.social_layer_presence";
const SIGN_IN_TTL: Duration = Duration::from_secs(600);

/// A sign-in started from the web UI and not finished yet.
#[derive(Debug)]
#[cfg_attr(not(feature = "social-sdk"), allow(dead_code))]
pub struct PendingSignIn {
    pub state: String,
    pub verifier: remotemusicrp_crypto::Zeroizing<String>,
    pub redirect_uri: String,
    pub session_hash: String,
    pub created: Instant,
}

/// Whether this build contains server mode.
pub const COMPILED: bool = cfg!(feature = "social-sdk");

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The Discord authorize URL for a PKCE sign-in.
pub fn authorize_url(client_id: &str, redirect_uri: &str, challenge: &str, state: &str) -> String {
    format!(
        "https://discord.com/oauth2/authorize?response_type=code&client_id={}&redirect_uri={}\
         &scope={}&code_challenge={}&code_challenge_method=S256&state={}",
        encode(client_id),
        encode(redirect_uri),
        encode(SCOPES),
        encode(challenge),
        encode(state),
    )
}

#[derive(Debug, Default)]
pub struct Shared {
    pub pending: Mutex<Option<PendingSignIn>>,
}

impl PendingSignIn {
    pub fn expired(&self) -> bool {
        self.created.elapsed() > SIGN_IN_TTL
    }
}

#[cfg(feature = "social-sdk")]
pub use enabled::*;

#[cfg(not(feature = "social-sdk"))]
pub use disabled::*;

#[cfg(not(feature = "social-sdk"))]
mod disabled {
    use super::*;

    /// Placeholder: this build has no server mode.
    #[derive(Debug)]
    pub struct SdkMode;

    impl SdkMode {
        pub fn load(_library: &std::path::Path) -> Result<Self, String> {
            Err(
                "this build of RemoteMusicRP was made without Social SDK support \
                 (cargo feature `social-sdk`); use mode = \"local-rpc\" or a build with it"
                    .into(),
            )
        }
        pub fn status(&self) -> &'static str {
            "unavailable"
        }
        pub fn ready(&self) -> bool {
            false
        }
        pub fn user(&self) -> Option<remotemusicrp_discord::rpc::ConnectedUser> {
            None
        }
        pub fn online_status(&self) -> Option<&'static str> {
            None
        }
        pub async fn set_online_status(&self, _app: &App, _s: &str) -> Result<(), String> {
            Err("server mode is not available in this build".into())
        }
        pub async fn finish_sign_in(
            &self,
            _app: &App,
            _p: PendingSignIn,
            _code: String,
        ) -> Result<(), String> {
            Err("server mode is not available in this build".into())
        }
        pub async fn ensure_connected(
            &self,
            _app: &std::sync::Arc<App>,
        ) -> Result<(), DiscordError> {
            Err(DiscordError::Sdk("server mode is not available".into()))
        }
        pub async fn sign_out(&self, _app: &App) {}
        pub async fn set_activity(&self, _a: Option<&Activity>) -> Result<(), DiscordError> {
            Err(DiscordError::Sdk("server mode is not available".into()))
        }
        pub async fn shutdown(&self) {}
    }
}

#[cfg(feature = "social-sdk")]
mod enabled {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use remotemusicrp_crypto::Zeroizing;
    use remotemusicrp_discord::rpc::ConnectedUser;
    use remotemusicrp_discord_sdk_ffi::{OnlineStatus, Sdk, Status, Tokens};
    use remotemusicrp_storage::LogLevel;

    use super::*;
    use crate::settings;

    /// Renew the access token this long before it expires.
    const RENEW_BEFORE_SECS: u64 = 24 * 3600;
    /// Wait for the SDK to become Ready after connecting.
    const READY_TIMEOUT: Duration = Duration::from_secs(30);

    #[derive(Debug)]
    pub struct SdkMode {
        sdk: Sdk,
        /// Unix time the in-memory access token expires (0 = none).
        expires_at: AtomicU64,
        /// Serialises sign-in/refresh so two can't race.
        busy: tokio::sync::Mutex<()>,
        warned: AtomicBool,
    }

    fn app_id(app: &App) -> Result<u64, String> {
        app.db
            .get_setting(settings::DISCORD_CLIENT_ID)
            .ok()
            .flatten()
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| "enter your Discord Application ID first".to_owned())
    }

    impl SdkMode {
        pub fn load(library: &std::path::Path) -> Result<Self, String> {
            let sdk = Sdk::load(library)?;
            tracing::warn!(
                "EXPERIMENTAL server mode: Discord Social SDK loaded from {}",
                library.display()
            );
            Ok(Self {
                sdk,
                expires_at: AtomicU64::new(0),
                busy: tokio::sync::Mutex::new(()),
                warned: AtomicBool::new(false),
            })
        }

        pub fn status(&self) -> &'static str {
            self.sdk.state().status.as_str()
        }

        pub fn ready(&self) -> bool {
            self.sdk.state().status == Status::Ready
        }

        /// The account's online status (`online`, `idle`, `dnd`,
        /// `invisible`, …), once connected.
        pub fn online_status(&self) -> Option<&'static str> {
            let s = self.sdk.state();
            (s.status == Status::Ready)
                .then_some(s.user?.online)
                .filter(|o| *o != OnlineStatus::Unknown)
                .map(OnlineStatus::as_str)
        }

        /// Change the account's online status.
        pub async fn set_online_status(&self, app: &App, status: &str) -> Result<(), String> {
            let s = OnlineStatus::parse_settable(status)
                .ok_or_else(|| "choose online, idle, dnd or invisible".to_owned())?;
            if !self.ready() {
                return Err("not connected to Discord yet".into());
            }
            self.sdk.set_online_status(s).await.map_err(|e| {
                tracing::warn!(error = %e, "could not change the Discord status");
                "Discord did not accept the status change".to_owned()
            })?;
            let _ = app.db.log(
                LogLevel::Info,
                "discord",
                &format!("Discord status set to {}", s.as_str()),
            );
            Ok(())
        }

        /// The signed-in account for the preview, validated like the local
        /// RPC one.
        pub fn user(&self) -> Option<ConnectedUser> {
            let u = self.sdk.state().user?;
            ConnectedUser::from_parts(
                &u.id.to_string(),
                &u.username,
                u.global_name.as_deref(),
                u.avatar.as_deref(),
            )
        }

        fn store_refresh(&self, app: &App, t: &Tokens) -> Result<(), String> {
            if t.refresh.is_empty() {
                return Err("Discord returned no refresh token".into());
            }
            app.with_keyring(|kr| {
                app.db.put_secret(
                    kr,
                    settings::SECRET_DISCORD_REFRESH,
                    settings::PURPOSE_DISCORD_REFRESH,
                    t.refresh.as_bytes(),
                )
            })
            .map_err(str::to_owned)?
            .map_err(|e| e.to_string())
        }

        fn load_refresh(app: &App) -> Result<Option<Zeroizing<String>>, String> {
            let v = app
                .with_keyring(|kr| {
                    app.db.get_secret(
                        kr,
                        settings::SECRET_DISCORD_REFRESH,
                        settings::PURPOSE_DISCORD_REFRESH,
                    )
                })
                .map_err(str::to_owned)?
                .map_err(|e| e.to_string())?;
            Ok(v.map(|b| Zeroizing::new(String::from_utf8_lossy(&b).into_owned())))
        }

        async fn use_tokens(&self, app: &App, t: Tokens) -> Result<(), String> {
            self.store_refresh(app, &t)?;
            let now = crate::now_unix();
            self.expires_at
                .store(now + t.expires_in_secs.max(60), Ordering::Relaxed);
            if !t
                .scopes
                .split(' ')
                .any(|s| s == "sdk.social_layer_presence")
            {
                tracing::warn!(scopes = %t.scopes, "Discord token lacks the presence scope");
            }
            self.sdk.disconnect();
            self.sdk.connect(t.access.clone()).await?;
            let mut state = self.sdk.clone();
            let _ = tokio::time::timeout(READY_TIMEOUT, async {
                while state.state().status != Status::Ready {
                    if !state.changed().await {
                        break;
                    }
                }
            })
            .await;
            Ok(())
        }

        pub async fn finish_sign_in(
            &self,
            app: &App,
            p: PendingSignIn,
            code: String,
        ) -> Result<(), String> {
            let _guard = self.busy.lock().await;
            let id = app_id(app)?;
            let tokens = self
                .sdk
                .get_token(id, Zeroizing::new(code), p.verifier, p.redirect_uri)
                .await
                .map_err(|e| format!("Discord sign-in failed: {e}"))?;
            self.use_tokens(app, tokens).await?;
            self.warned.store(false, Ordering::Relaxed);
            let _ = app.db.log(
                LogLevel::Info,
                "discord",
                "signed in to Discord (server mode)",
            );
            crate::email::reset_alert(app, "discord");
            app.reload.notify_one();
            Ok(())
        }

        /// Make sure the SDK is connected with a valid access token,
        /// refreshing it from the stored refresh token when needed.
        pub async fn ensure_connected(
            &self,
            app: &std::sync::Arc<App>,
        ) -> Result<(), DiscordError> {
            let now = crate::now_unix();
            let exp = self.expires_at.load(Ordering::Relaxed);
            let fresh = exp > now + RENEW_BEFORE_SECS;
            if fresh && self.ready() {
                return Ok(());
            }
            if fresh {
                // Token is fine; the SDK is (re)connecting by itself.
                return Err(DiscordError::Sdk(format!(
                    "connecting to Discord ({})",
                    self.status()
                )));
            }
            let _guard = self.busy.lock().await;
            if self.expires_at.load(Ordering::Relaxed) > now + RENEW_BEFORE_SECS {
                return Ok(()); // refreshed by someone else meanwhile
            }
            let refresh = Self::load_refresh(app)
                .map_err(DiscordError::Sdk)?
                .ok_or_else(|| {
                    DiscordError::Sdk("sign in with Discord on the Connections page".into())
                })?;
            let id = app_id(app).map_err(DiscordError::Sdk)?;
            match self.sdk.refresh_token(id, refresh).await {
                Ok(t) => {
                    self.use_tokens(app, t).await.map_err(DiscordError::Sdk)?;
                    tracing::info!("Discord access token renewed");
                    Ok(())
                }
                Err(e) => {
                    let msg =
                        format!("Discord sign-in expired or was revoked ({e}); sign in again");
                    if !self.warned.swap(true, Ordering::Relaxed) {
                        let _ = app.db.log(LogLevel::Error, "discord", &msg);
                        crate::email::alert(
                            app,
                            "discord",
                            "RemoteMusicRP: sign in to Discord again".into(),
                            format!(
                                "Your Discord status is no longer updated because the Discord                                  sign-in stopped working:

{msg}

Open your RemoteMusicRP                                  page, go to Connections and sign in with Discord again.

                                 You get this email at most once a day while the problem lasts."
                            ),
                        );
                    }
                    Err(DiscordError::Sdk(msg))
                }
            }
        }

        pub async fn sign_out(&self, app: &App) {
            let _guard = self.busy.lock().await;
            if let (Ok(Some(refresh)), Ok(id)) = (Self::load_refresh(app), app_id(app))
                && let Err(e) = self.sdk.revoke_token(id, refresh).await
            {
                tracing::warn!(error = %e, "could not revoke the Discord token");
            }
            let _ = app.db.delete_secret(settings::SECRET_DISCORD_REFRESH);
            self.expires_at.store(0, Ordering::Relaxed);
            self.sdk.clear_activity();
            self.sdk.disconnect();
            let _ = app.db.log(
                LogLevel::Info,
                "discord",
                "signed out of Discord (server mode)",
            );
        }

        pub async fn set_activity(&self, a: Option<&Activity>) -> Result<(), DiscordError> {
            match a {
                Some(a) => self.sdk.set_activity(a).await.map_err(DiscordError::Sdk),
                None => {
                    self.sdk.clear_activity();
                    Ok(())
                }
            }
        }

        /// Stop the SDK thread before the process exits.
        pub async fn shutdown(&self) {
            self.sdk.shutdown().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_is_encoded() {
        let u = authorize_url(
            "123456789012345678",
            "https://rp.example.com/discord-callback",
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            "abc",
        );
        assert!(u.starts_with("https://discord.com/oauth2/authorize?response_type=code"));
        assert!(u.contains("redirect_uri=https%3A%2F%2Frp.example.com%2Fdiscord-callback"));
        assert!(u.contains("scope=openid%20sdk.social_layer_presence"));
        assert!(u.contains("code_challenge_method=S256&state=abc"));
    }
}
