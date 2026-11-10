// SPDX-License-Identifier: AGPL-3.0-only
//! Instance configuration (deployment-level; user preferences such as
//! presence customisation live in the database).
//!
//! Sources, lowest to highest precedence: built-in defaults, the TOML file
//! (`remotemusicrp.toml`), environment variables (`REMOTEMUSICRP_*`).

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::Deserialize;

pub const MIN_POLL_SECS: u64 = 3;
pub const MAX_POLL_SECS: u64 = 300;
pub const DEFAULT_POLL_SECS: u64 = 5;
pub const DEFAULT_BIND: &str = "127.0.0.1:8484";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("could not parse config file: {0}")]
    Parse(String),
    #[error("invalid value for {field}: {reason}")]
    Invalid { field: &'static str, reason: String },
    #[error("could not determine a data directory; set data_dir or REMOTEMUSICRP_DATA_DIR")]
    NoDataDir,
}

/// Where the master keyring is kept (CLAUDE.md §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyStoreConfig {
    /// OS keystore (desktop default).
    Os,
    /// App-managed 0600 file (must not be inside the data directory).
    File(PathBuf),
    /// Externally provisioned, read-only (Docker secret, systemd-creds).
    ExternalFile(PathBuf),
    /// Passphrase-encrypted file; passphrase needed after every restart.
    Passphrase(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtworkConfig {
    pub apple: bool,
    pub cover_art_archive: bool,
    pub apple_first: bool,
    /// iTunes Search API storefront (ISO 3166-1 alpha-2).
    pub country: String,
}

/// How presence reaches Discord (CLAUDE.md §4).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DiscordMode {
    /// The Discord desktop client on this machine (default).
    #[default]
    LocalRpc,
    /// EXPERIMENTAL: the official Social SDK, no Discord client needed.
    /// `library` is the owner-downloaded `libdiscord_partner_sdk.so`.
    SocialSdk { library: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub discord_mode: DiscordMode,
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub poll_interval_secs: u64,
    pub log_level: String,
    pub key_store: KeyStoreConfig,
    pub artwork: ArtworkConfig,
    pub log_retention_days: u32,
    pub update_check: bool,
    /// Public https origin when served behind a TLS reverse proxy
    /// (required for non-loopback binds unless `lan_access`), e.g.
    /// `https://sa.example.com`.
    pub public_origin: Option<String>,
    /// Plain-HTTP access from the local network (owner decision
    /// 2026-11-10): listens on all interfaces, but only answers clients
    /// with private/LAN addresses and Host names that are private IPs or
    /// local names. Traffic is NOT encrypted.
    pub lan_access: bool,
}

/// True for addresses that can only be on the same local network:
/// loopback, private IPv4 (RFC 1918), IPv4 link-local, IPv6 unique-local
/// (fc00::/7) and link-local (fe80::/10), including IPv4-mapped IPv6.
pub fn is_lan_ip(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_lan_ip(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

impl Config {
    /// The UI is reachable from other machines: a password and TLS (or a TLS
    /// reverse proxy) become mandatory.
    pub fn is_exposed(&self) -> bool {
        !self.bind.ip().is_loopback()
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("remotemusicrp.db")
    }
}

// ---- file format -----------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    bind: Option<String>,
    data_dir: Option<PathBuf>,
    poll_interval_secs: Option<u64>,
    log_level: Option<String>,
    key_store: Option<RawKeyStore>,
    artwork: Option<RawArtwork>,
    activity_log: Option<RawActivityLog>,
    updates: Option<RawUpdates>,
    public_origin: Option<String>,
    lan_access: Option<bool>,
    discord: Option<RawDiscord>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDiscord {
    mode: Option<String>,
    sdk_library: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawKeyStore {
    kind: String,
    path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawArtwork {
    apple: Option<bool>,
    cover_art_archive: Option<bool>,
    apple_first: Option<bool>,
    country: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawActivityLog {
    retention_days: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUpdates {
    check: Option<bool>,
}

/// Platform default data directory, from environment variables only.
pub fn default_data_dir(env: &impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let get = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        get("LOCALAPPDATA").map(|p| p.join("RemoteMusicRP"))
    } else if cfg!(target_os = "macos") {
        get("HOME").map(|h| {
            h.join("Library")
                .join("Application Support")
                .join("RemoteMusicRP")
        })
    } else {
        get("XDG_DATA_HOME")
            .map(|p| p.join("remotemusicrp"))
            .or_else(|| get("HOME").map(|h| h.join(".local").join("share").join("remotemusicrp")))
    }
}

fn invalid(field: &'static str, reason: impl Into<String>) -> ConfigError {
    ConfigError::Invalid {
        field,
        reason: reason.into(),
    }
}

fn parse_bool_env(field: &'static str, v: &str) -> Result<bool, ConfigError> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(invalid(field, "expected true/false")),
    }
}

/// Names the project had before (newest first), as (display name, Linux
/// lower-case name). Used for one-time migration of existing installs.
pub const LEGACY_NAMES: [(&str, &str); 2] = [
    ("RemoteScrobbleRP", "remotescrobblerp"),
    ("ScrobbleAway", "scrobbleaway"),
];

/// Data directories used under earlier project names (newest first).
pub fn legacy_data_dirs(env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let get = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    LEGACY_NAMES
        .iter()
        .filter_map(|(display, lower)| {
            if cfg!(windows) {
                get("LOCALAPPDATA").map(|p| p.join(display))
            } else if cfg!(target_os = "macos") {
                get("HOME").map(|h| h.join("Library").join("Application Support").join(display))
            } else {
                get("XDG_DATA_HOME")
                    .map(|p| p.join(lower))
                    .or_else(|| get("HOME").map(|h| h.join(".local").join("share").join(lower)))
            }
        })
        .collect()
}

/// Accept only `https://host[:port]` with no path, query or credentials.
fn validate_origin(o: &str) -> Result<String, ConfigError> {
    let bad = || invalid("public_origin", "expected https://host[:port] with no path");
    let rest = o.strip_prefix("https://").ok_or_else(bad)?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.is_empty()
        || rest.len() > 253
        || !rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
    {
        return Err(bad());
    }
    Ok(format!("https://{}", rest.to_ascii_lowercase()))
}

/// Build the configuration from optional TOML text plus an environment
/// lookup function (injected so tests don't touch the real environment).
pub fn load(
    toml_text: Option<&str>,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Config, ConfigError> {
    let raw: RawConfig = match toml_text {
        Some(t) => toml::from_str(t).map_err(|e| ConfigError::Parse(e.message().to_owned()))?,
        None => RawConfig::default(),
    };
    let env_var = |k: &str| env(k).filter(|v| !v.trim().is_empty());

    let bind_str = env_var("REMOTEMUSICRP_BIND")
        .or(raw.bind)
        .unwrap_or_else(|| DEFAULT_BIND.to_owned());
    let bind: SocketAddr = bind_str
        .trim()
        .parse()
        .map_err(|_| invalid("bind", "expected ip:port, e.g. 127.0.0.1:8484"))?;

    let data_dir = env_var("REMOTEMUSICRP_DATA_DIR")
        .map(PathBuf::from)
        .or(raw.data_dir)
        .or_else(|| default_data_dir(&env))
        .ok_or(ConfigError::NoDataDir)?;

    let poll_interval_secs = match env_var("REMOTEMUSICRP_POLL_INTERVAL_SECS") {
        Some(v) => v
            .trim()
            .parse()
            .map_err(|_| invalid("poll_interval_secs", "expected a number"))?,
        None => raw.poll_interval_secs.unwrap_or(DEFAULT_POLL_SECS),
    };
    if !(MIN_POLL_SECS..=MAX_POLL_SECS).contains(&poll_interval_secs) {
        return Err(invalid(
            "poll_interval_secs",
            format!("must be between {MIN_POLL_SECS} and {MAX_POLL_SECS} seconds"),
        ));
    }

    let log_level = env_var("REMOTEMUSICRP_LOG")
        .or(raw.log_level)
        .unwrap_or_else(|| "info".to_owned())
        .to_ascii_lowercase();
    if !matches!(log_level.as_str(), "error" | "warn" | "info" | "debug") {
        return Err(invalid("log_level", "expected error, warn, info or debug"));
    }

    let key_store = if let Some(p) = env_var("REMOTEMUSICRP_MASTER_KEY_FILE") {
        KeyStoreConfig::ExternalFile(PathBuf::from(p))
    } else {
        match raw.key_store {
            None => KeyStoreConfig::Os,
            Some(k) => {
                let need_path = || {
                    k.path
                        .clone()
                        .ok_or_else(|| invalid("key_store.path", "required for this kind"))
                };
                match k.kind.as_str() {
                    "os" => KeyStoreConfig::Os,
                    "file" => KeyStoreConfig::File(need_path()?),
                    "external-file" => KeyStoreConfig::ExternalFile(need_path()?),
                    "passphrase" => KeyStoreConfig::Passphrase(need_path()?),
                    _ => {
                        return Err(invalid(
                            "key_store.kind",
                            "expected os, file, external-file or passphrase",
                        ));
                    }
                }
            }
        }
    };
    // A plaintext key file must never sit next to the database (CLAUDE.md §5).
    if let KeyStoreConfig::File(p) | KeyStoreConfig::ExternalFile(p) = &key_store
        && p.starts_with(&data_dir)
    {
        return Err(invalid(
            "key_store.path",
            "must not be inside the data directory",
        ));
    }

    let art = raw.artwork;
    let artwork = ArtworkConfig {
        apple: art.as_ref().and_then(|a| a.apple).unwrap_or(true),
        cover_art_archive: art
            .as_ref()
            .and_then(|a| a.cover_art_archive)
            .unwrap_or(true),
        apple_first: art.as_ref().and_then(|a| a.apple_first).unwrap_or(true),
        country: art
            .as_ref()
            .and_then(|a| a.country.clone())
            .unwrap_or_else(|| "US".to_owned())
            .to_ascii_uppercase(),
    };
    if artwork.country.len() != 2 || !artwork.country.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(invalid(
            "artwork.country",
            "expected a 2-letter country code",
        ));
    }

    let log_retention_days = raw
        .activity_log
        .and_then(|a| a.retention_days)
        .unwrap_or(30);
    if !(1..=365).contains(&log_retention_days) {
        return Err(invalid(
            "activity_log.retention_days",
            "must be between 1 and 365",
        ));
    }

    let update_check = match env_var("REMOTEMUSICRP_UPDATE_CHECK") {
        Some(v) => parse_bool_env("REMOTEMUSICRP_UPDATE_CHECK", &v)?,
        None => raw.updates.and_then(|u| u.check).unwrap_or(true),
    };

    let public_origin = match env_var("REMOTEMUSICRP_PUBLIC_ORIGIN").or(raw.public_origin) {
        Some(o) => Some(validate_origin(o.trim())?),
        None => None,
    };
    let lan_access = match env_var("REMOTEMUSICRP_LAN_ACCESS") {
        Some(v) => parse_bool_env("REMOTEMUSICRP_LAN_ACCESS", &v)?,
        None => raw.lan_access.unwrap_or(false),
    };
    if lan_access && public_origin.is_some() {
        return Err(invalid(
            "lan_access",
            "can't be combined with public_origin: use one or the other",
        ));
    }
    // LAN access on the default local address: listen on every interface
    // (same port). The web layer then only answers LAN clients.
    let bind = if lan_access && bind.ip().is_loopback() {
        SocketAddr::new(std::net::Ipv4Addr::UNSPECIFIED.into(), bind.port())
    } else {
        bind
    };
    if !bind.ip().is_loopback() && public_origin.is_none() && !lan_access {
        return Err(invalid(
            "public_origin",
            "required when bind is not a loopback address: put RemoteMusicRP behind a TLS reverse proxy \
             and set public_origin = \"https://your.host\", or set lan_access = true for plain-HTTP \
             access from your local network only",
        ));
    }

    let raw_discord = raw.discord;
    let mode = env_var("REMOTEMUSICRP_DISCORD_MODE")
        .or_else(|| raw_discord.as_ref().and_then(|d| d.mode.clone()))
        .unwrap_or_else(|| "local-rpc".to_owned());
    let discord_mode = match mode.trim() {
        "local-rpc" => DiscordMode::LocalRpc,
        "social-sdk" => {
            let library = env_var("REMOTEMUSICRP_DISCORD_SDK_LIBRARY")
                .map(PathBuf::from)
                .or_else(|| raw_discord.and_then(|d| d.sdk_library))
                .ok_or_else(|| {
                    invalid(
                        "discord.sdk_library",
                        "required for mode = \"social-sdk\": the full path to the                          libdiscord_partner_sdk.so you downloaded",
                    )
                })?;
            if !library.is_absolute() {
                return Err(invalid("discord.sdk_library", "must be an absolute path"));
            }
            DiscordMode::SocialSdk { library }
        }
        _ => {
            return Err(invalid("discord.mode", "expected local-rpc or social-sdk"));
        }
    };

    Ok(Config {
        discord_mode,
        public_origin,
        lan_access,
        bind,
        data_dir,
        poll_interval_secs,
        log_level,
        key_store,
        artwork,
        log_retention_days,
        update_check,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn env_with(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    const BASE: &[(&str, &str)] = &[("REMOTEMUSICRP_DATA_DIR", "/data")];

    #[test]
    fn lan_access() {
        // Off by default.
        assert!(!load(None, env_with(BASE)).unwrap().lan_access);
        // On: listens on every interface, same port; counts as exposed (so
        // a password is required).
        let c = load(Some("data_dir = \"/d\"\nlan_access = true"), |_| None).unwrap();
        assert!(c.lan_access && c.is_exposed());
        assert_eq!(c.bind.to_string(), "0.0.0.0:8484");
        let c = load(
            Some("data_dir = \"/d\"\nbind = \"127.0.0.1:9000\""),
            env_with(&[("REMOTEMUSICRP_LAN_ACCESS", "1")]),
        )
        .unwrap();
        assert_eq!(c.bind.to_string(), "0.0.0.0:9000");
        // Not together with the TLS proxy setting.
        assert!(
            load(
                Some("data_dir = \"/d\"\nlan_access = true\npublic_origin = \"https://x.example\""),
                |_| None
            )
            .is_err()
        );
        // A non-loopback bind still needs one of the two.
        assert!(load(Some("data_dir = \"/d\"\nbind = \"0.0.0.0:8484\""), |_| None).is_err());
    }

    #[test]
    fn lan_ips() {
        let lan = |s: &str| is_lan_ip(s.parse().unwrap());
        for ok in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.9",
            "172.31.255.1",
            "192.168.1.20",
            "169.254.3.4",
            "::1",
            "fd12:3456::1",
            "fe80::1",
            "::ffff:192.168.1.5",
        ] {
            assert!(lan(ok), "{ok} should be LAN");
        }
        for bad in [
            "8.8.8.8",
            "172.32.0.1",
            "100.64.0.1",
            "1.1.1.1",
            "2001:db8::1",
            "2606:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            assert!(!lan(bad), "{bad} should not be LAN");
        }
    }

    #[test]
    fn defaults_are_safe() {
        let c = load(None, env_with(BASE)).unwrap();
        assert_eq!(c.bind.to_string(), DEFAULT_BIND);
        assert!(!c.is_exposed());
        assert!(!c.lan_access);
        assert_eq!(c.poll_interval_secs, 5);
        assert_eq!(c.key_store, KeyStoreConfig::Os);
        assert!(c.artwork.apple && c.artwork.cover_art_archive && c.artwork.apple_first);
        assert_eq!(c.log_retention_days, 30);
        assert!(c.update_check);
        assert_eq!(c.db_path(), PathBuf::from("/data").join("remotemusicrp.db"));
    }

    #[test]
    fn full_file() {
        let toml = r#"
            bind = "0.0.0.0:9000"
            public_origin = "https://SA.example.com/"
            data_dir = "/srv/sa"
            poll_interval_secs = 10
            log_level = "DEBUG"
            [key_store]
            kind = "passphrase"
            path = "/keys/keyring.enc"
            [artwork]
            apple_first = false
            country = "gb"
            [activity_log]
            retention_days = 7
            [updates]
            check = false
        "#;
        let c = load(Some(toml), |_| None).unwrap();
        assert!(c.is_exposed());
        assert_eq!(c.public_origin.as_deref(), Some("https://sa.example.com"));
        assert_eq!(c.data_dir, PathBuf::from("/srv/sa"));
        assert_eq!(c.poll_interval_secs, 10);
        assert_eq!(c.log_level, "debug");
        assert_eq!(
            c.key_store,
            KeyStoreConfig::Passphrase("/keys/keyring.enc".into())
        );
        assert!(!c.artwork.apple_first);
        assert_eq!(c.artwork.country, "GB");
        assert_eq!(c.log_retention_days, 7);
        assert!(!c.update_check);
    }

    #[test]
    fn env_overrides_file() {
        let toml = r#"bind = "0.0.0.0:9000"
public_origin = "https://sa.example.com"
poll_interval_secs = 10"#;
        let c = load(
            Some(toml),
            env_with(&[
                ("REMOTEMUSICRP_DATA_DIR", "/data"),
                ("REMOTEMUSICRP_BIND", "127.0.0.1:1234"),
                ("REMOTEMUSICRP_POLL_INTERVAL_SECS", "30"),
                ("REMOTEMUSICRP_MASTER_KEY_FILE", "/run/secrets/sa_key"),
                ("REMOTEMUSICRP_UPDATE_CHECK", "off"),
            ]),
        )
        .unwrap();
        assert_eq!(c.bind.port(), 1234);
        assert_eq!(c.poll_interval_secs, 30);
        assert_eq!(
            c.key_store,
            KeyStoreConfig::ExternalFile("/run/secrets/sa_key".into())
        );
        assert!(!c.update_check);
    }

    #[test]
    fn rejects_bad_values() {
        let bad = [
            "poll_interval_secs = 1",
            "poll_interval_secs = 1000",
            "bind = \"localhost\"",
            "log_level = \"trace\"",
            "unknown_key = 1",
            "[key_store]\nkind = \"file\"",
            "[key_store]\nkind = \"magic\"",
            "[artwork]\ncountry = \"USA\"",
            "[activity_log]\nretention_days = 0",
            "bind = ",
            "bind = \"0.0.0.0:9000\"",
            "public_origin = \"http://insecure.example\"",
            "public_origin = \"https://x.example/path\"",
            "public_origin = \"https://user@x.example\"",
        ];
        for t in bad {
            assert!(load(Some(t), env_with(BASE)).is_err(), "accepted: {t}");
        }
    }

    #[test]
    fn key_file_not_in_data_dir() {
        let toml =
            "data_dir = \"/srv/sa\"\n[key_store]\nkind = \"file\"\npath = \"/srv/sa/master.key\"";
        let err = load(Some(toml), |_| None).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid {
                field: "key_store.path",
                ..
            }
        ));
    }

    #[test]
    fn missing_data_dir() {
        assert_eq!(load(None, |_| None).unwrap_err(), ConfigError::NoDataDir);
    }
}
