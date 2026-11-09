// SPDX-License-Identifier: AGPL-3.0-only
//! Scrobble sources (Last.fm; others behind the same trait) and album-art
//! lookups (Apple iTunes Search API, MusicBrainz Cover Art Archive).
//!
//! All provider data is treated as untrusted: responses are size-limited,
//! strings are length-capped, and only https URLs on expected hosts are
//! accepted for images and links.
#![forbid(unsafe_code)]

pub mod artwork;
pub mod cache;
pub mod cleanup;
pub mod http;
pub mod lastfm;

pub use reqwest;

use std::future::Future;

/// A track as reported by a scrobble source.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Track {
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    /// MusicBrainz release id of the album, when the source knows it.
    pub album_mbid: Option<String>,
    /// Canonical page for the track on the source (e.g. Last.fm track page).
    pub url: Option<String>,
}

impl Track {
    /// Stable cache key: lowercase artist + title.
    pub fn cache_key(&self) -> String {
        format!(
            "{}\u{1f}{}",
            self.artist.to_lowercase(),
            self.title.to_lowercase()
        )
    }
}

/// A completed scrobble. `started_unix` is when the track *started*
/// playing (Last.fm scrobble timestamps are start times).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scrobble {
    pub track: Track,
    pub started_unix: u64,
}

/// What the user is doing right now according to the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NowPlaying {
    /// A track is playing now. `previous` is the most recent completed
    /// scrobble, used to estimate when the current track started.
    Playing {
        track: Track,
        previous: Option<Scrobble>,
    },
    /// Nothing is playing; this was the most recent scrobble.
    Stopped { last: Scrobble },
    /// No listening history at all.
    Nothing,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// Credentials rejected or suspended: the user must re-enter them.
    #[error("credentials rejected by provider")]
    Auth,
    /// Misconfiguration the user must fix (e.g. unknown username).
    #[error("configuration problem: {0}")]
    Config(String),
    /// Provider asked us to slow down.
    #[error("rate limited by provider")]
    RateLimited,
    /// Temporary network/server problem; retry with backoff.
    #[error("temporary provider error: {0}")]
    Transient(String),
    /// Response did not match the expected format.
    #[error("unexpected response from provider")]
    Parse,
}

impl ProviderError {
    /// Whether the poller should back off and retry automatically.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimited | Self::Transient(_) | Self::Parse)
    }
}

pub type Result<T> = std::result::Result<T, ProviderError>;

/// A source of "now playing" information.
pub trait ScrobbleSource: Send + Sync {
    fn name(&self) -> &'static str;
    fn now_playing(&self) -> impl Future<Output = Result<NowPlaying>> + Send;
    /// Track length in milliseconds, if known.
    fn track_duration_ms(&self, track: &Track) -> impl Future<Output = Result<Option<u64>>> + Send;
}

/// Truncate to at most `max` chars (on a char boundary) and trim.
pub(crate) fn cap(s: &str, max: usize) -> String {
    s.trim()
        .chars()
        .take(max)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Accept only https URLs whose host is (a subdomain of) one of `hosts`.
pub(crate) fn safe_url(raw: &str, hosts: &[&str], max_len: usize) -> Option<String> {
    if raw.len() > max_len {
        return None;
    }
    let url = reqwest::Url::parse(raw).ok()?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    let ok = hosts
        .iter()
        .any(|h| host == *h || host.ends_with(&format!(".{h}")));
    ok.then(|| url.to_string()).filter(|s| s.len() <= max_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_url_rules() {
        let hosts = ["mzstatic.com"];
        assert!(safe_url("https://is1-ssl.mzstatic.com/a.jpg", &hosts, 256).is_some());
        assert!(safe_url("https://mzstatic.com/a.jpg", &hosts, 256).is_some());
        assert!(safe_url("http://is1.mzstatic.com/a.jpg", &hosts, 256).is_none());
        assert!(safe_url("https://evilmzstatic.com/a.jpg", &hosts, 256).is_none());
        assert!(safe_url("https://mzstatic.com.evil.io/a.jpg", &hosts, 256).is_none());
        assert!(safe_url("https://user:pw@mzstatic.com/a.jpg", &hosts, 256).is_none());
        assert!(safe_url("javascript:alert(1)", &hosts, 256).is_none());
        let long = format!("https://mzstatic.com/{}", "a".repeat(300));
        assert!(safe_url(&long, &hosts, 256).is_none());
    }

    #[test]
    fn cap_respects_chars() {
        assert_eq!(cap("  héllo wörld  ", 5), "héllo");
        assert_eq!(cap("ab", 10), "ab");
    }
}
