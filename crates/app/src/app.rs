// SPDX-License-Identifier: AGPL-3.0-only
//! Shared application state.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use remotemusicrp_core::config::{Config, KeyStoreConfig};
use remotemusicrp_crypto::keystore::{FileKeyStore, KeyStore, OsKeyStore};
use remotemusicrp_crypto::{Keyring, keystore};
use remotemusicrp_storage::Db;
use serde::Serialize;
use tokio::sync::{Notify, watch};

/// What the presence service is doing (shown on the dashboard).
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct ServiceStatus {
    /// `starting`, `locked`, `needs_setup`, `running`, `paused`, `error`
    pub phase: &'static str,
    pub discord_connected: bool,
    pub discord_user: Option<String>,
    pub discord_display_name: Option<String>,
    pub discord_avatar_url: Option<String>,
    /// Online status (server mode only): `online`, `idle`, `dnd`, `invisible`.
    pub discord_status: Option<&'static str>,
    pub discord_error: Option<String>,
    pub lastfm_error: Option<String>,
    /// Why nothing is shown on Discord although the service runs:
    /// `quiet`, `blocked_artist`, `blocked_track`.
    pub hidden: Option<&'static str>,
    pub now_playing: Option<NowPlayingStatus>,
    /// The exact activity JSON last sent to Discord (live preview).
    pub activity: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NowPlayingStatus {
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub art_url: Option<String>,
    pub art_source: Option<&'static str>,
    /// "album", "ep" or "single": the release the cover belongs to.
    pub art_release: Option<&'static str>,
    pub track_url: Option<String>,
    pub started_unix: u64,
    pub duration_ms: Option<u64>,
}

/// Per-IP login throttling: after 5 failures, lock for 1 min, doubling up
/// to 1 h (owner decision). Never a permanent lockout.
#[derive(Debug, Default)]
pub struct Lockout {
    entries: HashMap<IpAddr, (u32, Option<Instant>)>,
}

const FREE_ATTEMPTS: u32 = 5;
const BASE_LOCK: Duration = Duration::from_secs(60);
const MAX_LOCK: Duration = Duration::from_secs(3600);

impl Lockout {
    /// Remaining lock time for `ip`, if locked.
    pub fn locked_for(&self, ip: IpAddr) -> Option<Duration> {
        let (_, until) = self.entries.get(&ip)?;
        let until = (*until)?;
        let now = Instant::now();
        (until > now).then(|| until - now)
    }

    pub fn record_failure(&mut self, ip: IpAddr) -> Option<Duration> {
        if self.entries.len() > 10_000 {
            // Bound memory under a distributed attack.
            let now = Instant::now();
            self.entries.retain(|_, (_, u)| u.is_some_and(|u| u > now));
        }
        let e = self.entries.entry(ip).or_insert((0, None));
        e.0 = e.0.saturating_add(1);
        if e.0 >= FREE_ATTEMPTS {
            let exp = (e.0 - FREE_ATTEMPTS).min(6);
            let lock = (BASE_LOCK * 2u32.pow(exp)).min(MAX_LOCK);
            e.1 = Some(Instant::now() + lock);
            return Some(lock);
        }
        None
    }

    pub fn record_success(&mut self, ip: IpAddr) {
        self.entries.remove(&ip);
    }
}

pub struct App {
    pub cfg: Config,
    pub db: Db,
    pub http: remotemusicrp_providers::reqwest::Client,
    /// `None` until unlocked (passphrase mode) or loaded.
    pub keyring: RwLock<Option<Keyring>>,
    pub key_store: Mutex<Option<Box<dyn KeyStore>>>,
    pub status: watch::Sender<ServiceStatus>,
    /// Wakes the presence service after settings/credentials change.
    pub reload: Notify,
    pub lockout: Mutex<Lockout>,
    /// One-time code required to set the first password on an exposed
    /// install (printed to the log at startup), so a stranger who reaches
    /// the UI first cannot claim it.
    pub setup_code: Mutex<Option<String>>,
    /// Argon2id uses 64 MiB per hash: cap concurrent hashes so a flood of
    /// login attempts cannot exhaust memory.
    pub hash_permits: tokio::sync::Semaphore,
    /// Latest update-check result.
    pub update: Mutex<crate::updater::UpdateInfo>,
    /// Asks the main loop to shut down (used by Quit and after updates).
    pub stop: watch::Sender<bool>,
    pub started_unix: u64,
    /// EXPERIMENTAL server mode (Social SDK), when configured.
    pub sdk: Option<crate::sdk_mode::SdkMode>,
    pub sdk_shared: crate::sdk_mode::Shared,
    /// Distinct tracks seen since start (activity stats).
    pub tracks_seen: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App").finish_non_exhaustive()
    }
}

impl App {
    pub fn new(
        cfg: Config,
        db: Db,
        http: remotemusicrp_providers::reqwest::Client,
    ) -> Result<Self, String> {
        let cfg_mode = cfg.discord_mode.clone();
        let (status, _) = watch::channel(ServiceStatus {
            phase: "starting",
            ..Default::default()
        });
        let app = Self {
            cfg,
            db,
            http,
            keyring: RwLock::new(None),
            key_store: Mutex::new(None),
            status,
            reload: Notify::new(),
            lockout: Mutex::new(Lockout::default()),
            setup_code: Mutex::new(None),
            hash_permits: tokio::sync::Semaphore::new(2),
            update: Mutex::new(crate::updater::initial_info()),
            stop: watch::channel(false).0,
            started_unix: crate::now_unix(),
            tracks_seen: std::sync::atomic::AtomicU64::new(0),
            sdk: match &cfg_mode {
                remotemusicrp_core::config::DiscordMode::LocalRpc => None,
                remotemusicrp_core::config::DiscordMode::SocialSdk { library } => {
                    Some(crate::sdk_mode::SdkMode::load(library)?)
                }
            },
            sdk_shared: crate::sdk_mode::Shared::default(),
        };
        // Non-passphrase stores can be opened right away.
        if !matches!(app.cfg.key_store, KeyStoreConfig::Passphrase(_)) {
            let store = open_key_store(&app.cfg.key_store)?;
            let kr = keystore::load_or_create(store.as_ref())
                .map_err(|e| format!("master key ({}): {e}", store.describe()))?;
            *app.keyring.write().map_err(|_| "lock poisoned")? = Some(kr);
            *app.key_store.lock().map_err(|_| "lock poisoned")? = Some(store);
        }
        Ok(app)
    }

    pub fn is_locked(&self) -> bool {
        self.keyring.read().map(|k| k.is_none()).unwrap_or(true)
    }

    /// Run `f` with the keyring, or fail if locked.
    pub fn with_keyring<T>(&self, f: impl FnOnce(&Keyring) -> T) -> Result<T, &'static str> {
        let guard = self.keyring.read().map_err(|_| "internal error")?;
        let kr = guard.as_ref().ok_or("instance is locked")?;
        Ok(f(kr))
    }

    pub fn set_status(&self, f: impl FnOnce(&mut ServiceStatus)) {
        self.status.send_if_modified(|s| {
            let before = s.clone();
            f(s);
            *s != before
        });
    }
}

pub fn open_key_store(k: &KeyStoreConfig) -> Result<Box<dyn KeyStore>, String> {
    Ok(match k {
        KeyStoreConfig::Os => {
            if !OsKeyStore::available() {
                return Err(
                    "no OS keystore available here; configure [key_store] kind = \"file\", \
                            \"external-file\" or \"passphrase\""
                        .into(),
                );
            }
            Box::new(OsKeyStore::new("default"))
        }
        KeyStoreConfig::File(p) => Box::new(FileKeyStore::new(p)),
        KeyStoreConfig::ExternalFile(p) => Box::new(FileKeyStore::read_only(p)),
        KeyStoreConfig::Passphrase(_) => return Err("passphrase mode must be unlocked".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lockout_escalates_and_resets() {
        let mut l = Lockout::default();
        let ip: IpAddr = "192.0.2.1".parse().unwrap_or(IpAddr::from([0, 0, 0, 0]));
        for _ in 0..4 {
            assert!(l.record_failure(ip).is_none());
        }
        assert!(l.locked_for(ip).is_none());
        assert_eq!(l.record_failure(ip), Some(Duration::from_secs(60)));
        assert!(l.locked_for(ip).is_some());
        assert_eq!(l.record_failure(ip), Some(Duration::from_secs(120)));
        for _ in 0..20 {
            l.record_failure(ip);
        }
        assert_eq!(l.record_failure(ip), Some(MAX_LOCK));
        l.record_success(ip);
        assert!(l.locked_for(ip).is_none());
    }
}
