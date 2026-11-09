// SPDX-License-Identifier: AGPL-3.0-only
//! Album-art lookup. Order (configurable): Apple iTunes Search API, then the
//! MusicBrainz Cover Art Archive. We only resolve *URLs*; Discord fetches the
//! image itself, so no image bytes are downloaded or stored.
//!
//! Last.fm artwork is deliberately not used (Last.fm API ToS 5.1.8 excludes
//! artwork from its licence). No website scraping, ever.

use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;

use crate::cache::BoundedCache;
use crate::http::{RateLimiter, json_limited, redact, status_error};
use crate::{ProviderError, Result, Track};

/// Discord asset URL limit we enforce for images.
const MAX_IMAGE_URL: usize = 256;
/// Discord button URL limit.
const MAX_LINK_URL: usize = 512;

const POSITIVE_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
const RETRY_DELAY: Duration = Duration::from_millis(1500);
const NEGATIVE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtworkSourceKind {
    Apple,
    CoverArtArchive,
}

/// A resolved artwork result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artwork {
    pub source: ArtworkSourceKind,
    /// https image URL for Discord's `large_image`.
    pub image_url: String,
    /// Optional link to the track/release on the source (e.g. Apple Music).
    pub link_url: Option<String>,
    /// Track length reported by the source, if any (Apple provides it).
    pub duration_ms: Option<u64>,
}

pub trait ArtworkSource: Send + Sync {
    fn kind(&self) -> ArtworkSourceKind;
    fn lookup(&self, track: &Track) -> impl Future<Output = Result<Option<Artwork>>> + Send;
}

/// Normalize titles/artists for matching: lowercase, drop bracketed suffixes
/// like "(Remastered 2011)" / "[feat. X]" / " - Radio Edit", keep alphanumerics.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0u32;
    let lowered = s.to_lowercase();
    let base = lowered
        .split(" - ")
        .next()
        .unwrap_or(&lowered)
        .split(" feat.")
        .next()
        .unwrap_or("")
        .split(" ft.")
        .next()
        .unwrap_or("");
    for c in base.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            c if depth == 0 && c.is_alphanumeric() => out.push(c),
            _ => {}
        }
    }
    out
}

fn artist_matches(wanted: &str, got: &str) -> bool {
    let (w, g) = (normalize(wanted), normalize(got));
    !w.is_empty() && (w == g || g.contains(&w) || w.contains(&g) && !g.is_empty())
}

// ---- Apple iTunes Search API -------------------------------------------

/// Apple's public, keyless iTunes Search API
/// (<https://performance-partners.apple.com/search-api>), limited to about
/// 20 calls/minute; we space calls at least 3 s apart.
#[derive(Debug)]
pub struct AppleSearch {
    client: reqwest::Client,
    base: String,
    country: String,
    limiter: RateLimiter,
}

impl AppleSearch {
    pub fn new(client: reqwest::Client, country: &str) -> Self {
        Self::with_base(client, "https://itunes.apple.com/search", country)
    }

    pub fn with_base(client: reqwest::Client, base: &str, country: &str) -> Self {
        let country = if country.len() == 2 && country.bytes().all(|b| b.is_ascii_alphabetic()) {
            country.to_ascii_uppercase()
        } else {
            "US".to_owned()
        };
        Self {
            client,
            base: base.to_owned(),
            country,
            limiter: RateLimiter::new(Duration::from_secs(3)),
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct AppleResponse {
    #[serde(default)]
    results: Vec<AppleResult>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppleResult {
    artist_name: Option<String>,
    track_name: Option<String>,
    artwork_url100: Option<String>,
    track_view_url: Option<String>,
    track_time_millis: Option<u64>,
    collection_name: Option<String>,
    collection_view_url: Option<String>,
}

/// Ask for 1024×1024 by rewriting the size segment (undocumented, so the
/// original URL is kept if the pattern is not found).
fn upscale_apple(url: &str) -> String {
    for small in ["100x100bb", "100x100-75", "60x60bb"] {
        if url.contains(small) {
            return url.replacen(small, "1024x1024bb", 1);
        }
    }
    url.to_owned()
}

pub(crate) fn pick_apple(track: &Track, resp: AppleResponse) -> Option<Artwork> {
    let wanted_title = normalize(&track.title);
    resp.results.into_iter().find_map(|r| {
        let title_ok = r
            .track_name
            .as_deref()
            .is_some_and(|t| normalize(t) == wanted_title);
        let artist_ok = r
            .artist_name
            .as_deref()
            .is_some_and(|a| artist_matches(&track.artist, a));
        if !(title_ok && artist_ok) {
            return None;
        }
        let raw = r.artwork_url100.as_deref()?;
        let image_url = crate::safe_url(&upscale_apple(raw), &["mzstatic.com"], MAX_IMAGE_URL)
            .or_else(|| crate::safe_url(raw, &["mzstatic.com"], MAX_IMAGE_URL))?;
        let link_url = r.track_view_url.as_deref().and_then(|u| {
            crate::safe_url(u, &["music.apple.com", "itunes.apple.com"], MAX_LINK_URL)
        });
        let duration_ms = r
            .track_time_millis
            .filter(|ms| (1..=3 * 3600 * 1000).contains(ms));
        Some(Artwork {
            source: ArtworkSourceKind::Apple,
            image_url,
            link_url,
            duration_ms,
        })
    })
}

/// Pick an album (collection) result: album name and artist must match.
pub(crate) fn pick_apple_album(artist: &str, album: &str, resp: AppleResponse) -> Option<Artwork> {
    let wanted = normalize(album);
    if wanted.is_empty() {
        return None;
    }
    resp.results.into_iter().find_map(|r| {
        let album_ok = r
            .collection_name
            .as_deref()
            .is_some_and(|c| normalize(c) == wanted);
        let artist_ok = r
            .artist_name
            .as_deref()
            .is_some_and(|a| artist_matches(artist, a));
        if !(album_ok && artist_ok) {
            return None;
        }
        let raw = r.artwork_url100.as_deref()?;
        let image_url = crate::safe_url(&upscale_apple(raw), &["mzstatic.com"], MAX_IMAGE_URL)
            .or_else(|| crate::safe_url(raw, &["mzstatic.com"], MAX_IMAGE_URL))?;
        let link_url = r.collection_view_url.as_deref().and_then(|u| {
            crate::safe_url(u, &["music.apple.com", "itunes.apple.com"], MAX_LINK_URL)
        });
        Some(Artwork {
            source: ArtworkSourceKind::Apple,
            image_url,
            link_url,
            duration_ms: None,
        })
    })
}

impl AppleSearch {
    /// Last-resort lookup: the album's cover by album + artist name.
    pub async fn lookup_album(&self, artist: &str, album: &str) -> Result<Option<Artwork>> {
        self.limiter.acquire().await;
        let term = format!("{artist} {album}");
        let resp = self
            .client
            .get(&self.base)
            .query(&[
                ("term", term.as_str()),
                ("media", "music"),
                ("entity", "album"),
                ("limit", "10"),
                ("country", self.country.as_str()),
            ])
            .send()
            .await
            .map_err(|e| ProviderError::Transient(redact(&e)))?;
        if !resp.status().is_success() {
            return Err(status_error(resp.status()));
        }
        let body: AppleResponse = json_limited(resp).await?;
        Ok(pick_apple_album(artist, album, body))
    }
}

impl ArtworkSource for AppleSearch {
    fn kind(&self) -> ArtworkSourceKind {
        ArtworkSourceKind::Apple
    }

    async fn lookup(&self, track: &Track) -> Result<Option<Artwork>> {
        self.limiter.acquire().await;
        let term = format!("{} {}", track.artist, track.title);
        let resp = self
            .client
            .get(&self.base)
            .query(&[
                ("term", term.as_str()),
                ("media", "music"),
                ("entity", "song"),
                ("limit", "10"),
                ("country", self.country.as_str()),
            ])
            .send()
            .await
            .map_err(|e| ProviderError::Transient(redact(&e)))?;
        if !resp.status().is_success() {
            return Err(status_error(resp.status()));
        }
        let body: AppleResponse = json_limited(resp).await?;
        Ok(pick_apple(track, body))
    }
}

// ---- MusicBrainz Cover Art Archive --------------------------------------

/// Cover Art Archive (<https://coverartarchive.org>) via the release MBID
/// from the scrobble source, or a MusicBrainz recording search when absent.
/// MusicBrainz allows 1 request/s with a meaningful User-Agent.
#[derive(Debug)]
pub struct CoverArtArchive {
    client: reqwest::Client,
    mb_base: String,
    caa_base: String,
    limiter: RateLimiter,
}

impl CoverArtArchive {
    pub fn new(client: reqwest::Client) -> Self {
        Self::with_bases(
            client,
            "https://musicbrainz.org/ws/2",
            "https://coverartarchive.org",
        )
    }

    pub fn with_bases(client: reqwest::Client, mb_base: &str, caa_base: &str) -> Self {
        Self {
            client,
            mb_base: mb_base.trim_end_matches('/').to_owned(),
            caa_base: caa_base.trim_end_matches('/').to_owned(),
            limiter: RateLimiter::new(Duration::from_millis(1100)),
        }
    }

    async fn release_art(&self, mbid: &str) -> Result<Option<String>> {
        self.limiter.acquire().await;
        let resp = self
            .client
            .get(format!("{}/release/{mbid}", self.caa_base))
            .send()
            .await
            .map_err(|e| ProviderError::Transient(redact(&e)))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(status_error(resp.status()));
        }
        let body: CaaResponse = json_limited(resp).await?;
        Ok(pick_caa(body))
    }

    async fn release_group_art(&self, mbid: &str) -> Result<Option<String>> {
        self.limiter.acquire().await;
        let resp = self
            .client
            .get(format!("{}/release-group/{mbid}", self.caa_base))
            .send()
            .await
            .map_err(|e| ProviderError::Transient(redact(&e)))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(status_error(resp.status()));
        }
        let body: CaaResponse = json_limited(resp).await?;
        Ok(pick_caa(body))
    }

    /// Find the album (MusicBrainz release group) by album + artist name.
    async fn search_release_group(&self, artist: &str, album: &str) -> Result<Option<String>> {
        self.limiter.acquire().await;
        let query = format!(
            "releasegroup:\"{}\" AND artist:\"{}\"",
            lucene_escape(album),
            lucene_escape(artist)
        );
        let resp = self
            .client
            .get(format!("{}/release-group", self.mb_base))
            .query(&[("query", query.as_str()), ("fmt", "json"), ("limit", "5")])
            .send()
            .await
            .map_err(|e| ProviderError::Transient(redact(&e)))?;
        if !resp.status().is_success() {
            return Err(status_error(resp.status()));
        }
        let body: MbGroupSearch = json_limited(resp).await?;
        Ok(pick_release_group(artist, album, body))
    }

    async fn search_release_ids(&self, track: &Track) -> Result<Vec<String>> {
        self.limiter.acquire().await;
        let query = format!(
            "recording:\"{}\" AND artist:\"{}\"",
            lucene_escape(&track.title),
            lucene_escape(&track.artist)
        );
        let resp = self
            .client
            .get(format!("{}/recording", self.mb_base))
            .query(&[("query", query.as_str()), ("fmt", "json"), ("limit", "5")])
            .send()
            .await
            .map_err(|e| ProviderError::Transient(redact(&e)))?;
        if !resp.status().is_success() {
            return Err(status_error(resp.status()));
        }
        let body: MbSearch = json_limited(resp).await?;
        Ok(pick_release_ids(track, body))
    }
}

/// Escape Lucene special characters inside a quoted MusicBrainz query.
fn lucene_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().take(200) {
        if "\\\"+-!(){}[]^~*?:/&|".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[derive(Debug, Deserialize)]
pub(crate) struct CaaResponse {
    #[serde(default)]
    images: Vec<CaaImage>,
}

#[derive(Debug, Deserialize)]
struct CaaImage {
    #[serde(default)]
    front: bool,
    image: Option<String>,
    #[serde(default)]
    thumbnails: std::collections::HashMap<String, String>,
}

const CAA_HOSTS: &[&str] = &["coverartarchive.org", "archive.org"];

/// The Cover Art Archive returns `http://` links; the same files are served
/// over https, and we only ever accept https.
fn caa_https(u: &str) -> String {
    match u.strip_prefix("http://") {
        Some(rest) => format!("https://{rest}"),
        None => u.to_owned(),
    }
}

pub(crate) fn pick_caa(resp: CaaResponse) -> Option<String> {
    let img = resp.images.iter().find(|i| i.front)?;
    ["1200", "large", "500"]
        .iter()
        .filter_map(|k| img.thumbnails.get(*k))
        .chain(img.image.iter())
        .find_map(|u| crate::safe_url(&caa_https(u), CAA_HOSTS, MAX_IMAGE_URL))
}

#[derive(Debug, Deserialize)]
pub(crate) struct MbGroupSearch {
    #[serde(default, rename = "release-groups")]
    release_groups: Vec<MbGroup>,
}

#[derive(Debug, Deserialize)]
struct MbGroup {
    id: String,
    #[serde(default)]
    score: u32,
    title: Option<String>,
    #[serde(default, rename = "artist-credit")]
    artist_credit: Vec<MbArtistCredit>,
}

fn valid_mbid(id: &str) -> bool {
    id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

pub(crate) fn pick_release_group(artist: &str, album: &str, resp: MbGroupSearch) -> Option<String> {
    let wanted = normalize(album);
    if wanted.is_empty() {
        return None;
    }
    resp.release_groups
        .into_iter()
        .filter(|g| g.score >= 90)
        .filter(|g| g.title.as_deref().is_some_and(|t| normalize(t) == wanted))
        .filter(|g| {
            g.artist_credit
                .iter()
                .any(|a| a.name.as_deref().is_some_and(|n| artist_matches(artist, n)))
        })
        .map(|g| g.id)
        .find(|id| valid_mbid(id))
}

#[derive(Debug, Deserialize)]
pub(crate) struct MbSearch {
    #[serde(default)]
    recordings: Vec<MbRecording>,
}

#[derive(Debug, Deserialize)]
struct MbRecording {
    #[serde(default)]
    score: u32,
    title: Option<String>,
    #[serde(default, rename = "artist-credit")]
    artist_credit: Vec<MbArtistCredit>,
    #[serde(default)]
    releases: Vec<MbRelease>,
}

#[derive(Debug, Deserialize)]
struct MbArtistCredit {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MbRelease {
    id: String,
}

pub(crate) fn pick_release_ids(track: &Track, resp: MbSearch) -> Vec<String> {
    let wanted = normalize(&track.title);
    resp.recordings
        .into_iter()
        .filter(|r| r.score >= 80)
        .filter(|r| r.title.as_deref().is_some_and(|t| normalize(t) == wanted))
        .filter(|r| {
            r.artist_credit.iter().any(|a| {
                a.name
                    .as_deref()
                    .is_some_and(|n| artist_matches(&track.artist, n))
            })
        })
        .flat_map(|r| r.releases.into_iter().map(|rel| rel.id))
        .filter(|id| id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
        .take(3)
        .collect()
}

impl ArtworkSource for CoverArtArchive {
    fn kind(&self) -> ArtworkSourceKind {
        ArtworkSourceKind::CoverArtArchive
    }

    async fn lookup(&self, track: &Track) -> Result<Option<Artwork>> {
        let found = |image_url| Artwork {
            source: ArtworkSourceKind::CoverArtArchive,
            image_url,
            link_url: None,
            duration_ms: None,
        };
        // 1. The exact release Last.fm told us about.
        if let Some(id) = &track.album_mbid
            && let Some(url) = self.release_art(id).await?
        {
            return Ok(Some(found(url)));
        }
        // 2. The album's cover (release group), found by album + artist name.
        if let Some(album) = track.album.as_deref()
            && let Some(group) = self.search_release_group(&track.artist, album).await?
            && let Some(url) = self.release_group_art(&group).await?
        {
            return Ok(Some(found(url)));
        }
        // 3. Releases containing this recording.
        for id in self.search_release_ids(track).await? {
            if let Some(url) = self.release_art(&id).await? {
                return Ok(Some(found(url)));
            }
        }
        Ok(None)
    }
}

// ---- resolver ------------------------------------------------------------

/// Tries enabled sources in order and caches results (including "no art").
#[derive(Debug)]
pub struct ArtworkResolver {
    apple: Option<AppleSearch>,
    caa: Option<CoverArtArchive>,
    apple_first: bool,
    cache: Mutex<BoundedCache<String, Option<Artwork>>>,
}

impl ArtworkResolver {
    pub fn new(
        apple: Option<AppleSearch>,
        caa: Option<CoverArtArchive>,
        apple_first: bool,
    ) -> Self {
        Self {
            apple,
            caa,
            apple_first,
            cache: Mutex::new(BoundedCache::new(1000)),
        }
    }

    /// One lookup with a single retry after a short pause on temporary
    /// errors (network blips, rate limiting).
    async fn lookup_once(
        &self,
        kind: ArtworkSourceKind,
        track: &Track,
    ) -> Option<Result<Option<Artwork>>> {
        let mut attempt = 0;
        loop {
            let res = match kind {
                ArtworkSourceKind::Apple => self.apple.as_ref()?.lookup(track).await,
                ArtworkSourceKind::CoverArtArchive => self.caa.as_ref()?.lookup(track).await,
            };
            match res {
                Err(e) if e.is_retryable() && attempt == 0 => {
                    attempt += 1;
                    tracing::debug!(source = ?kind, error = %e, "artwork lookup failed; retrying once");
                    tokio::time::sleep(RETRY_DELAY).await;
                }
                other => return Some(other),
            }
        }
    }

    /// Resolve artwork:
    /// 1. the track as reported (each source retried once on errors);
    /// 2. the simplified track: bare title, main artist, bare album;
    /// 3. the album cover by album name (Apple).
    ///
    /// Errors are logged and treated as "no art" (not cached), so presence
    /// never fails because of art.
    pub async fn resolve(&self, track: &Track) -> Option<Artwork> {
        let key = track.cache_key();
        if let Ok(cache) = self.cache.lock()
            && let Some(hit) = cache.get(&key, POSITIVE_TTL)
            && (hit.is_some() || cache.get(&key, NEGATIVE_TTL).is_some())
        {
            return hit;
        }
        let order = if self.apple_first {
            [ArtworkSourceKind::Apple, ArtworkSourceKind::CoverArtArchive]
        } else {
            [ArtworkSourceKind::CoverArtArchive, ArtworkSourceKind::Apple]
        };
        let simple = crate::cleanup::simplified(track);
        let passes: Vec<&Track> = std::iter::once(track).chain(simple.as_ref()).collect();

        let mut had_error = false;
        let mut found = None;
        'outer: for (pass, t) in passes.iter().enumerate() {
            for kind in order {
                match self.lookup_once(kind, t).await {
                    Some(Ok(Some(art))) => {
                        if pass > 0 {
                            tracing::debug!(title = %t.title, "artwork found with simplified names");
                        }
                        found = Some(art);
                        break 'outer;
                    }
                    Some(Ok(None)) | None => {}
                    Some(Err(e)) => {
                        had_error = true;
                        tracing::debug!(source = ?kind, error = %e, "artwork lookup failed");
                    }
                }
            }
        }
        if found.is_none()
            && let Some(apple) = &self.apple
        {
            let t = simple.as_ref().unwrap_or(track);
            if let Some(album) = t.album.as_deref() {
                match apple.lookup_album(&t.artist, album).await {
                    Ok(art) => found = art,
                    Err(e) => {
                        had_error = true;
                        tracing::debug!(error = %e, "album artwork lookup failed");
                    }
                }
            }
        }
        if (found.is_some() || !had_error)
            && let Ok(mut cache) = self.cache.lock()
        {
            cache.insert(key, found.clone());
        }
        found
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn track(artist: &str, title: &str) -> Track {
        Track {
            artist: artist.into(),
            title: title.into(),
            album: None,
            album_mbid: None,
            url: None,
        }
    }

    #[test]
    fn normalization() {
        assert_eq!(normalize("Let It Be (Remastered 2009)"), "letitbe");
        assert_eq!(normalize("Song [feat. Someone]"), "song");
        assert_eq!(normalize("Song - Radio Edit"), "song");
        assert_eq!(normalize("Song feat. X"), "song");
        assert_eq!(normalize("Beyoncé"), "beyoncé");
    }

    const APPLE: &str = r#"{"resultCount":2,"results":[
        {"wrapperType":"track","kind":"song","artistName":"Other Artist","trackName":"One More Time",
         "artworkUrl100":"https://is1-ssl.mzstatic.com/image/thumb/Music/x/100x100bb.jpg"},
        {"wrapperType":"track","kind":"song","artistName":"Daft Punk","trackName":"One More Time",
         "collectionName":"Discovery",
         "artworkUrl100":"https://is1-ssl.mzstatic.com/image/thumb/Music/y/100x100bb.jpg",
         "trackViewUrl":"https://music.apple.com/us/album/one-more-time/697194953?i=697195462&uo=4",
         "trackTimeMillis":320357}]}"#;

    #[test]
    fn apple_picks_matching_artist_and_upscales() {
        let art = pick_apple(
            &track("Daft Punk", "One More Time"),
            serde_json::from_str(APPLE).unwrap(),
        )
        .unwrap();
        assert_eq!(
            art.image_url,
            "https://is1-ssl.mzstatic.com/image/thumb/Music/y/1024x1024bb.jpg"
        );
        assert!(
            art.link_url
                .unwrap()
                .starts_with("https://music.apple.com/")
        );
        assert_eq!(art.duration_ms, Some(320_357));
    }

    #[test]
    fn apple_album_fallback_pick() {
        let json = r#"{"results":[
            {"wrapperType":"collection","artistName":"Radiohead","collectionName":"Hail to the Thief",
             "artworkUrl100":"https://is1-ssl.mzstatic.com/image/thumb/a/100x100bb.jpg",
             "collectionViewUrl":"https://music.apple.com/us/album/hail-to-the-thief/1"}]}"#;
        let art = pick_apple_album(
            "Radiohead",
            "Hail to the Thief",
            serde_json::from_str(json).unwrap(),
        )
        .unwrap();
        assert!(art.image_url.ends_with("1024x1024bb.jpg"));
        assert!(
            art.link_url
                .unwrap()
                .starts_with("https://music.apple.com/")
        );
        assert!(
            pick_apple_album(
                "Other",
                "Hail to the Thief",
                serde_json::from_str(json).unwrap()
            )
            .is_none()
        );
    }

    #[test]
    fn caa_http_links_upgraded() {
        let json = r#"{"images":[{"front":true,"image":"http://coverartarchive.org/release/x/1.jpg",
            "thumbnails":{"1200":"http://coverartarchive.org/release/x/1-1200.jpg"}}]}"#;
        assert_eq!(
            pick_caa(serde_json::from_str(json).unwrap()).as_deref(),
            Some("https://coverartarchive.org/release/x/1-1200.jpg")
        );
    }

    #[test]
    fn release_group_pick() {
        let json = r#"{"release-groups":[
            {"id":"7fe26c12-b0a2-47c1-a9fc-3ea11b29225b","score":87,"title":"Hail to the Thief (B-Sides)","artist-credit":[{"name":"Radiohead"}]},
            {"id":"5c14fd50-a2f1-3672-9537-b0dad91bea2f","score":100,"title":"Hail to the Thief","artist-credit":[{"name":"Radiohead"}]}]}"#;
        assert_eq!(
            pick_release_group(
                "Radiohead",
                "Hail to the Thief",
                serde_json::from_str(json).unwrap()
            )
            .as_deref(),
            Some("5c14fd50-a2f1-3672-9537-b0dad91bea2f")
        );
        assert!(
            pick_release_group(
                "Other",
                "Hail to the Thief",
                serde_json::from_str(json).unwrap()
            )
            .is_none()
        );
    }

    #[test]
    fn apple_no_match() {
        assert!(
            pick_apple(
                &track("Nobody", "Nothing"),
                serde_json::from_str(APPLE).unwrap()
            )
            .is_none()
        );
    }

    #[test]
    fn apple_rejects_foreign_hosts() {
        let json = r#"{"results":[{"artistName":"A","trackName":"T",
            "artworkUrl100":"https://evil.example/100x100bb.jpg","trackViewUrl":"https://evil.example/"}]}"#;
        assert!(pick_apple(&track("A", "T"), serde_json::from_str(json).unwrap()).is_none());
    }

    #[test]
    fn caa_prefers_large_front() {
        let json = r#"{"images":[
            {"front":false,"image":"https://coverartarchive.org/release/x/back.jpg","thumbnails":{}},
            {"front":true,"image":"https://coverartarchive.org/release/x/1.jpg",
             "thumbnails":{"500":"https://coverartarchive.org/release/x/1-500.jpg",
                           "1200":"https://coverartarchive.org/release/x/1-1200.jpg"}}]}"#;
        assert_eq!(
            pick_caa(serde_json::from_str(json).unwrap()).unwrap(),
            "https://coverartarchive.org/release/x/1-1200.jpg"
        );
        assert!(pick_caa(serde_json::from_str(r#"{"images":[]}"#).unwrap()).is_none());
    }

    #[test]
    fn mb_search_filters() {
        let json = r#"{"recordings":[
            {"score":100,"title":"One More Time","artist-credit":[{"name":"Daft Punk"}],
             "releases":[{"id":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"},{"id":"bad"}]},
            {"score":50,"title":"One More Time","artist-credit":[{"name":"Daft Punk"}],
             "releases":[{"id":"ffffffff-bbbb-cccc-dddd-eeeeeeeeeeee"}]},
            {"score":100,"title":"One More Time","artist-credit":[{"name":"Cover Band"}],
             "releases":[{"id":"11111111-bbbb-cccc-dddd-eeeeeeeeeeee"}]}]}"#;
        let ids = pick_release_ids(
            &track("Daft Punk", "One More Time"),
            serde_json::from_str(json).unwrap(),
        );
        assert_eq!(ids, vec!["aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"]);
    }

    #[test]
    fn lucene_escaping() {
        assert_eq!(lucene_escape(r#"AC/DC "Live""#), r#"AC\/DC \"Live\""#);
    }
}

/// Live network check (run manually: `cargo test -p remotemusicrp-providers -- --ignored`).
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod live {
    use super::*;

    #[tokio::test]
    #[ignore = "uses the real Apple / MusicBrainz / Cover Art Archive services"]
    async fn radiohead_live_track_finds_studio_cover() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = crate::http::client().unwrap();
        let resolver = ArtworkResolver::new(
            Some(AppleSearch::new(client.clone(), "US")),
            Some(CoverArtArchive::new(client)),
            true,
        );
        let track = Track {
            artist: "Radiohead".into(),
            title: "Myxomatosis - live".into(),
            album: Some("Hail to the Thief (Live Recordings 2003-2009)".into()),
            album_mbid: None,
            url: None,
        };
        let art = resolver.resolve(&track).await;
        eprintln!("resolved: {art:?}");
        assert!(art.is_some_and(|a| a.image_url.starts_with("https://")));
    }
}
