// SPDX-License-Identifier: AGPL-3.0-only
//! Last.fm source.
//!
//! - Now playing: `user.getRecentTracks` (`limit=1`). The playing track has
//!   `@attr.nowplaying="true"`. Read-only methods need only an API key.
//! - Duration: `track.getInfo` (`duration`, milliseconds, `"0"` = unknown),
//!   cached because it never changes.
//! - Requests are spaced at least 1 s apart per instance (Last.fm publishes
//!   no number but warns against "several calls per second").

use std::sync::Mutex;
use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

use crate::cache::BoundedCache;
use crate::http::{RateLimiter, json_limited, redact, status_error};
use crate::{NowPlaying, ProviderError, Result, Scrobble, ScrobbleSource, Track, cap};

const DEFAULT_BASE: &str = "https://ws.audioscrobbler.com/2.0/";
const MAX_FIELD: usize = 256;
const DURATION_TTL: Duration = Duration::from_secs(30 * 24 * 3600);

#[derive(Debug)]
pub struct LastFm {
    client: reqwest::Client,
    base: String,
    username: String,
    api_key: SecretString,
    limiter: RateLimiter,
    durations: Mutex<BoundedCache<String, Option<u64>>>,
}

/// Validate a Last.fm username (2–15 chars, letters/digits/`_`/`-`, starts
/// with a letter) before ever sending it anywhere.
pub fn valid_username(name: &str) -> bool {
    let len = name.chars().count();
    (2..=15).contains(&len)
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Validate the shape of a Last.fm API key (32 hex chars).
pub fn valid_api_key(key: &str) -> bool {
    key.len() == 32 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

impl LastFm {
    pub fn new(client: reqwest::Client, username: &str, api_key: String) -> Result<Self> {
        Self::with_base(client, DEFAULT_BASE, username, api_key)
    }

    pub fn with_base(
        client: reqwest::Client,
        base: &str,
        username: &str,
        api_key: String,
    ) -> Result<Self> {
        if !valid_username(username) {
            return Err(ProviderError::Config("invalid Last.fm username".into()));
        }
        if !valid_api_key(&api_key) {
            return Err(ProviderError::Config(
                "invalid Last.fm API key format".into(),
            ));
        }
        Ok(Self {
            client,
            base: base.to_owned(),
            username: username.to_owned(),
            api_key: SecretString::from(api_key),
            limiter: RateLimiter::new(Duration::from_secs(1)),
            durations: Mutex::new(BoundedCache::new(2000)),
        })
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    async fn call<T: for<'de> Deserialize<'de>>(&self, params: &[(&str, &str)]) -> Result<T> {
        self.limiter.acquire().await;
        let resp = self
            .client
            .get(&self.base)
            .query(params)
            .query(&[
                ("api_key", self.api_key.expose_secret()),
                ("format", "json"),
            ])
            .send()
            .await
            .map_err(|e| ProviderError::Transient(redact(&e)))?;
        let status = resp.status();
        // Last.fm returns error JSON with 4xx/5xx codes; parse it when present.
        let body: serde_json::Value = match json_limited(resp).await {
            Ok(v) => v,
            Err(_) if !status.is_success() => return Err(status_error(status)),
            Err(e) => return Err(e),
        };
        if let Some(code) = body.get("error").and_then(serde_json::Value::as_u64) {
            return Err(map_api_error(code));
        }
        if !status.is_success() {
            return Err(status_error(status));
        }
        serde_json::from_value(body).map_err(|_| ProviderError::Parse)
    }
}

/// Map Last.fm API error codes (https://www.last.fm/api/errorcodes).
fn map_api_error(code: u64) -> ProviderError {
    match code {
        4 | 9 | 10 | 14 | 15 | 26 => ProviderError::Auth,
        6 | 7 | 17 => ProviderError::Config("Last.fm user not found or private".into()),
        29 => ProviderError::RateLimited,
        _ => ProviderError::Transient(format!("Last.fm error {code}")),
    }
}

impl ScrobbleSource for LastFm {
    fn name(&self) -> &'static str {
        "Last.fm"
    }

    async fn now_playing(&self) -> Result<NowPlaying> {
        let resp: RecentTracksResponse = self
            .call(&[
                ("method", "user.getrecenttracks"),
                ("user", &self.username),
                ("limit", "1"),
            ])
            .await?;
        parse_recent(resp)
    }

    async fn track_duration_ms(&self, track: &Track) -> Result<Option<u64>> {
        let key = track.cache_key();
        if let Ok(cache) = self.durations.lock()
            && let Some(v) = cache.get(&key, DURATION_TTL)
        {
            return Ok(v);
        }
        let resp: TrackInfoResponse = self
            .call(&[
                ("method", "track.getinfo"),
                ("artist", &track.artist),
                ("track", &track.title),
                ("autocorrect", "0"),
            ])
            .await
            .or_else(|e| match e {
                // Unknown track: cache "no duration" rather than retrying.
                ProviderError::Config(_) => Ok(TrackInfoResponse::default()),
                other => Err(other),
            })?;
        let duration = parse_duration(&resp);
        if let Ok(mut cache) = self.durations.lock() {
            cache.insert(key, duration);
        }
        Ok(duration)
    }
}

// ---- wire format -------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct RecentTracksResponse {
    recenttracks: RecentTracks,
}

#[derive(Debug, Deserialize)]
struct RecentTracks {
    #[serde(default)]
    track: OneOrMany<RawTrack>,
}

/// Last.fm returns an object for one result and an array for several.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OneOrMany<T> {
    Many(Vec<T>),
    One(T),
}

impl<T> Default for OneOrMany<T> {
    fn default() -> Self {
        Self::Many(Vec::new())
    }
}

impl<T> OneOrMany<T> {
    fn into_vec(self) -> Vec<T> {
        match self {
            Self::Many(v) => v,
            Self::One(t) => vec![t],
        }
    }
}

#[derive(Debug, Deserialize)]
struct TextField {
    #[serde(rename = "#text", default)]
    text: String,
    #[serde(default)]
    mbid: String,
}

#[derive(Debug, Deserialize)]
struct RawTrack {
    name: String,
    artist: TextField,
    album: Option<TextField>,
    url: Option<String>,
    #[serde(rename = "@attr")]
    attr: Option<TrackAttr>,
    date: Option<DateField>,
}

#[derive(Debug, Deserialize)]
struct TrackAttr {
    nowplaying: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DateField {
    uts: String,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct TrackInfoResponse {
    track: Option<TrackInfo>,
}

#[derive(Debug, Deserialize)]
struct TrackInfo {
    duration: Option<serde_json::Value>,
}

fn non_empty(s: &str, max: usize) -> Option<String> {
    let s = cap(s, max);
    (!s.is_empty()).then_some(s)
}

fn valid_mbid(s: &str) -> Option<String> {
    let s = s.trim();
    let ok = s.len() == 36
        && s.chars().enumerate().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        });
    ok.then(|| s.to_ascii_lowercase())
}

fn to_track(raw: &RawTrack) -> Option<Track> {
    let title = non_empty(&raw.name, MAX_FIELD)?;
    let artist = non_empty(&raw.artist.text, MAX_FIELD)?;
    let album = raw
        .album
        .as_ref()
        .and_then(|a| non_empty(&a.text, MAX_FIELD));
    let album_mbid = raw.album.as_ref().and_then(|a| valid_mbid(&a.mbid));
    let url = raw
        .url
        .as_deref()
        .and_then(|u| crate::safe_url(u, &["last.fm"], 512));
    Some(Track {
        artist,
        title,
        album,
        album_mbid,
        url,
    })
}

pub(crate) fn parse_recent(resp: RecentTracksResponse) -> Result<NowPlaying> {
    let tracks = resp.recenttracks.track.into_vec();
    let previous = tracks.iter().find(|t| t.date.is_some()).and_then(|t| {
        let started_unix = t.date.as_ref()?.uts.parse::<u64>().ok()?;
        Some(Scrobble {
            track: to_track(t)?,
            started_unix,
        })
    });
    if let Some(playing) = tracks
        .iter()
        .find(|t| t.attr.as_ref().and_then(|a| a.nowplaying.as_deref()) == Some("true"))
    {
        let track = to_track(playing).ok_or(ProviderError::Parse)?;
        return Ok(NowPlaying::Playing { track, previous });
    }
    match previous {
        Some(last) => Ok(NowPlaying::Stopped { last }),
        None if tracks.iter().any(|t| t.date.is_some()) => Err(ProviderError::Parse),
        None => Ok(NowPlaying::Nothing),
    }
}

pub(crate) fn parse_duration(resp: &TrackInfoResponse) -> Option<u64> {
    let v = resp.track.as_ref()?.duration.as_ref()?;
    let ms = match v {
        serde_json::Value::String(s) => s.parse::<u64>().ok()?,
        serde_json::Value::Number(n) => n.as_u64()?,
        _ => return None,
    };
    // 0 means unknown; cap at 3 h to ignore nonsense values.
    (1..=3 * 3600 * 1000).contains(&ms).then_some(ms)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn recent(json: &str) -> Result<NowPlaying> {
        parse_recent(serde_json::from_str(json).unwrap())
    }

    const PLAYING: &str = r##"{"recenttracks":{"track":[
        {"artist":{"mbid":"","#text":"Daft Punk"},"streamable":"0","image":[],
         "mbid":"","album":{"mbid":"B8C2A6D4-1111-2222-3333-444455556666","#text":"Discovery"},
         "name":"One More Time","@attr":{"nowplaying":"true"},
         "url":"https://www.last.fm/music/Daft+Punk/_/One+More+Time"},
        {"artist":{"mbid":"","#text":"Daft Punk"},"album":{"mbid":"","#text":"Discovery"},
         "name":"Aerodynamic","url":"https://www.last.fm/music/Daft+Punk/_/Aerodynamic",
         "date":{"uts":"1762650000","#text":"09 Nov 2026, 01:00"}}
    ],"@attr":{"user":"someone","page":"1","perPage":"1","totalPages":"100","total":"100"}}}"##;

    #[test]
    fn parses_now_playing() {
        let NowPlaying::Playing { track: t, previous } = recent(PLAYING).unwrap() else {
            panic!()
        };
        let prev = previous.unwrap();
        assert_eq!(prev.track.title, "Aerodynamic");
        assert_eq!(prev.started_unix, 1_762_650_000);
        assert_eq!(t.artist, "Daft Punk");
        assert_eq!(t.title, "One More Time");
        assert_eq!(t.album.as_deref(), Some("Discovery"));
        assert_eq!(
            t.album_mbid.as_deref(),
            Some("b8c2a6d4-1111-2222-3333-444455556666")
        );
        assert!(t.url.unwrap().starts_with("https://www.last.fm/"));
    }

    #[test]
    fn parses_stopped_single_object() {
        let json = r##"{"recenttracks":{"track":
            {"artist":{"mbid":"","#text":"A"},"album":{"mbid":"","#text":""},"name":"T",
             "url":"http://www.last.fm/music/A/_/T","date":{"uts":"1700000000"}},
            "@attr":{"user":"u"}}}"##;
        let NowPlaying::Stopped { last } = recent(json).unwrap() else {
            panic!()
        };
        assert_eq!(last.track.title, "T");
        assert_eq!(last.track.album, None);
        // http (not https) URLs are dropped.
        assert_eq!(last.track.url, None);
        assert_eq!(last.started_unix, 1_700_000_000);
    }

    #[test]
    fn parses_empty_history() {
        let json = r#"{"recenttracks":{"track":[],"@attr":{"user":"u"}}}"#;
        assert_eq!(recent(json).unwrap(), NowPlaying::Nothing);
    }

    #[test]
    fn rejects_blank_track() {
        let json = r##"{"recenttracks":{"track":[{"artist":{"#text":""},"name":"x","@attr":{"nowplaying":"true"}}]}}"##;
        assert!(matches!(recent(json), Err(ProviderError::Parse)));
    }

    #[test]
    fn caps_hostile_lengths() {
        let long = "x".repeat(10_000);
        let json = format!(
            r##"{{"recenttracks":{{"track":[{{"artist":{{"#text":"{long}"}},"name":"{long}","@attr":{{"nowplaying":"true"}}}}]}}}}"##
        );
        let NowPlaying::Playing { track: t, previous } = recent(&json).unwrap() else {
            panic!()
        };
        assert!(previous.is_none());
        assert_eq!(t.title.chars().count(), MAX_FIELD);
    }

    #[test]
    fn duration_parsing() {
        let p = |j: &str| parse_duration(&serde_json::from_str(j).unwrap());
        assert_eq!(p(r#"{"track":{"duration":"240000"}}"#), Some(240_000));
        assert_eq!(p(r#"{"track":{"duration":240000}}"#), Some(240_000));
        assert_eq!(p(r#"{"track":{"duration":"0"}}"#), None);
        assert_eq!(p(r#"{"track":{"duration":"abc"}}"#), None);
        assert_eq!(p(r#"{"track":{}}"#), None);
        assert_eq!(p(r#"{}"#), None);
    }

    #[test]
    fn error_codes() {
        assert!(matches!(map_api_error(10), ProviderError::Auth));
        assert!(matches!(map_api_error(26), ProviderError::Auth));
        assert!(matches!(map_api_error(6), ProviderError::Config(_)));
        assert!(matches!(map_api_error(29), ProviderError::RateLimited));
        assert!(map_api_error(11).is_retryable());
    }

    #[test]
    fn input_validation() {
        assert!(valid_username("rj"));
        assert!(valid_username("Some_User-1"));
        assert!(!valid_username("1abc"));
        assert!(!valid_username("a"));
        assert!(!valid_username("bad name"));
        assert!(!valid_username("a&method=x"));
        assert!(valid_api_key("0123456789abcdef0123456789ABCDEF"));
        assert!(!valid_api_key("short"));
        assert!(!valid_api_key("g123456789abcdef0123456789abcdef"));
    }

    #[test]
    fn debug_hides_api_key() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let lf = LastFm::new(
            crate::http::client().unwrap(),
            "someone",
            "0123456789abcdef0123456789abcdef".into(),
        )
        .unwrap();
        assert!(!format!("{lf:?}").contains("0123456789abcdef"));
    }
}
