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

impl ArtworkSourceKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Apple => "Apple",
            Self::CoverArtArchive => "Cover Art Archive",
        }
    }
}

/// The kind of release the artwork belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseKind {
    Album,
    Ep,
    Single,
}

impl ReleaseKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Album => "album",
            Self::Ep => "ep",
            Self::Single => "single",
        }
    }

    /// Apple marks singles and EPs in the collection name
    /// ("Red Bottoms - Single", "Love Songs - EP"); everything else is an album.
    fn from_apple(collection: &str) -> Self {
        let c = collection.trim_end().to_lowercase();
        if c.ends_with(" - single") {
            Self::Single
        } else if c.ends_with(" - ep") {
            Self::Ep
        } else {
            Self::Album
        }
    }

    /// MusicBrainz release-group primary type.
    fn from_mb(primary_type: &str) -> Option<Self> {
        match primary_type.to_ascii_lowercase().as_str() {
            "album" => Some(Self::Album),
            "ep" => Some(Self::Ep),
            "single" => Some(Self::Single),
            _ => None,
        }
    }
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
    /// Album, EP or single, when the source says.
    pub release: Option<ReleaseKind>,
}

/// What a lookup tried and why it didn't find art, for the activity log.
/// Holds only track, artist and album names and result counts: no URLs with
/// query strings, no keys.
#[derive(Debug, Default, Clone)]
pub struct Trace {
    notes: Vec<String>,
    /// Art was found, but for another release than the one being played.
    pub other_release: Option<String>,
}

impl Trace {
    const MAX_NOTES: usize = 16;
    const MAX_NOTE_CHARS: usize = 240;

    pub fn note(&mut self, note: impl Into<String>) {
        if self.notes.len() < Self::MAX_NOTES {
            self.notes
                .push(note.into().chars().take(Self::MAX_NOTE_CHARS).collect());
        }
    }

    pub fn notes(&self) -> &[String] {
        &self.notes
    }
}

/// Up to three Apple results, for a log note.
fn describe_apple(resp: &AppleResponse, songs: bool) -> String {
    let shown: Vec<String> = resp
        .results
        .iter()
        .take(3)
        .map(|r| {
            let artist = r.artist_name.as_deref().unwrap_or("?");
            let album = r.collection_name.as_deref().unwrap_or("?");
            if songs {
                format!(
                    "\"{}\" by {artist} on \"{album}\"",
                    r.track_name.as_deref().unwrap_or("?")
                )
            } else {
                format!("\"{album}\" by {artist}")
            }
        })
        .collect();
    match resp.results.len() {
        0 => "no results".to_owned(),
        n => format!("{n} results, e.g. {}", shown.join("; ")),
    }
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

/// Normalize an album name: like [`normalize`], and also ignore a release
/// type suffix ("X - Single", "X - EP", "X EP", "X (Single)"), so Last.fm's
/// and Apple's spellings of the same release compare equal.
pub fn normalize_album(s: &str) -> String {
    let n = normalize(s);
    let lowered = s.trim().to_lowercase();
    let bare = lowered.split(" - ").next().unwrap_or(&lowered).trim_end();
    for suffix in [" ep", " single"] {
        if let Some(stem) = bare.strip_suffix(suffix) {
            let stem = normalize(stem);
            if !stem.is_empty() {
                return stem;
            }
        }
    }
    n
}

/// Stricter album key: keeps bracketed parts ("Red Bottoms (Remixes)" is
/// not "Red Bottoms") but still ignores the release type suffix.
fn exact_album(s: &str) -> String {
    let lowered = s.trim().to_lowercase();
    let mut bare = lowered.as_str();
    for suffix in [
        " - single",
        " - ep",
        " ep",
        " single",
        " (single)",
        " [single]",
    ] {
        if let Some(stem) = bare.strip_suffix(suffix)
            && stem.chars().any(char::is_alphanumeric)
        {
            bare = stem;
            break;
        }
    }
    bare.chars().filter(|c| c.is_alphanumeric()).collect()
}

/// The individual artists in a credit ("A, B & C feat. D" → a, b, c, d).
/// Band names with "&" split too, but the same way on both sides.
fn artist_set(credit: &str) -> Vec<String> {
    const SEPARATORS: &[&str] = &[
        ", ",
        " & ",
        " and ",
        " x ",
        " × ",
        " feat. ",
        " feat ",
        " ft. ",
        " featuring ",
        " with ",
        " / ",
        "; ",
        " + ",
    ];
    let mut parts = vec![credit.to_lowercase()];
    for sep in SEPARATORS {
        parts = parts
            .iter()
            .flat_map(|p| p.split(sep).map(str::to_owned).collect::<Vec<_>>())
            .collect();
    }
    let mut out: Vec<String> = parts
        .iter()
        .map(|p| normalize(p))
        .filter(|p| !p.is_empty())
        .collect();
    out.dedup();
    out
}

/// Same artist credit, allowing for different order ("A, B & C" vs
/// "A, C & B"), extra or missing featured artists, and abbreviations.
fn artist_matches(wanted: &str, got: &str) -> bool {
    let (w, g) = (normalize(wanted), normalize(got));
    if w.is_empty() || g.is_empty() {
        return false;
    }
    if w == g || g.contains(&w) || w.contains(&g) {
        return true;
    }
    let (ws, gs) = (artist_set(wanted), artist_set(got));
    let subset = |a: &[String], b: &[String]| !a.is_empty() && a.iter().all(|x| b.contains(x));
    subset(&ws, &gs) || subset(&gs, &ws) || ws.first().is_some_and(|m| gs.first() == Some(m))
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
    /// The album's own artist ("Various Artists" for compilations); song
    /// results carry it next to the track's artist.
    collection_artist_name: Option<String>,
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

/// The best Apple song match, and whether it comes from the album Last.fm
/// reported (so a single's cover isn't shown for an album track).
#[derive(Debug)]
pub(crate) struct ApplePick {
    pub art: Artwork,
    pub album_matched: bool,
}

fn apple_image(r: &AppleResult) -> Option<String> {
    let raw = r.artwork_url100.as_deref()?;
    crate::safe_url(&upscale_apple(raw), &["mzstatic.com"], MAX_IMAGE_URL)
        .or_else(|| crate::safe_url(raw, &["mzstatic.com"], MAX_IMAGE_URL))
}

fn apple_link(u: Option<&str>) -> Option<String> {
    u.and_then(|u| crate::safe_url(u, &["music.apple.com", "itunes.apple.com"], MAX_LINK_URL))
}

/// Pick a song result: title and artist must match. Among those, the one
/// from the album being played wins (Apple often lists the single first);
/// without an album match, the first result is kept and flagged.
pub(crate) fn pick_apple(track: &Track, resp: AppleResponse) -> Option<ApplePick> {
    let wanted_title = normalize(&track.title);
    let wanted_album = track
        .album
        .as_deref()
        .map(normalize_album)
        .filter(|a| !a.is_empty());
    let candidates: Vec<(AppleResult, String)> = resp
        .results
        .into_iter()
        .filter(|r| {
            r.track_name
                .as_deref()
                .is_some_and(|t| normalize(t) == wanted_title)
                && r.artist_name
                    .as_deref()
                    .is_some_and(|a| artist_matches(&track.artist, a))
        })
        .filter_map(|r| {
            let img = apple_image(&r)?;
            Some((r, img))
        })
        .collect();
    // An exact name (brackets included) beats a loose one, so "X (Remixes)
    // EP" doesn't pick the "X - Single" that only matches loosely.
    let exact = track.album.as_deref().map(exact_album);
    let matched = wanted_album.as_ref().and_then(|want| {
        let collection = |(r, _): &(AppleResult, String)| r.collection_name.clone();
        candidates
            .iter()
            .position(|c| collection(c).map(|n| exact_album(&n)) == exact)
            .or_else(|| {
                candidates
                    .iter()
                    .position(|c| collection(c).map(|n| normalize_album(&n)).as_ref() == Some(want))
            })
    });
    let (r, image_url) = candidates.into_iter().nth(matched.unwrap_or(0))?;
    Some(ApplePick {
        album_matched: matched.is_some(),
        art: Artwork {
            source: ArtworkSourceKind::Apple,
            image_url,
            link_url: apple_link(r.track_view_url.as_deref()),
            duration_ms: r
                .track_time_millis
                .filter(|ms| (1..=3 * 3600 * 1000).contains(ms)),
            release: r.collection_name.as_deref().map(ReleaseKind::from_apple),
        },
    })
}

/// Pick an album (collection) result: album name and artist must match.
pub(crate) fn pick_apple_album(artist: &str, album: &str, resp: AppleResponse) -> Option<Artwork> {
    let wanted = normalize_album(album);
    if wanted.is_empty() {
        return None;
    }
    resp.results.into_iter().find_map(|r| {
        let album_ok = r
            .collection_name
            .as_deref()
            .is_some_and(|c| normalize_album(c) == wanted);
        // Compilations are credited to "Various Artists": accept those when
        // the album name matches exactly (brackets included).
        let compilation = |a: &str| {
            normalize(a) == "variousartists"
                && r.collection_name.as_deref().map(exact_album) == Some(exact_album(album))
        };
        let artist_ok = [
            r.artist_name.as_deref(),
            r.collection_artist_name.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|a| artist_matches(artist, a) || compilation(a));
        if !(album_ok && artist_ok) {
            return None;
        }
        Some(Artwork {
            source: ArtworkSourceKind::Apple,
            image_url: apple_image(&r)?,
            // The album page: song results link "album?i=<that song>", which
            // may be another track than the one playing.
            link_url: apple_link(
                r.collection_view_url
                    .as_deref()
                    .map(|u| u.split_once('?').map_or(u, |(page, _)| page)),
            ),
            duration_ms: None,
            release: r.collection_name.as_deref().map(ReleaseKind::from_apple),
        })
    })
}

impl AppleSearch {
    /// One Search API call. `attribute` limits which field `term` is
    /// matched against (e.g. `albumTerm`: only album names).
    async fn search(
        &self,
        term: &str,
        entity: &str,
        limit: &str,
        attribute: Option<&str>,
    ) -> Result<AppleResponse> {
        self.limiter.acquire().await;
        let mut query = vec![
            ("term", term),
            ("media", "music"),
            ("entity", entity),
            ("limit", limit),
            ("country", self.country.as_str()),
        ];
        if let Some(a) = attribute {
            query.push(("attribute", a));
        }
        let resp = self
            .client
            .get(&self.base)
            .query(&query)
            .send()
            .await
            .map_err(|e| ProviderError::Transient(redact(&e)))?;
        if !resp.status().is_success() {
            return Err(status_error(resp.status()));
        }
        json_limited(resp).await
    }

    /// Last-resort lookup: the album's cover by album + artist name.
    pub async fn lookup_album(
        &self,
        artist: &str,
        album: &str,
        trace: &mut Trace,
    ) -> Result<Option<Artwork>> {
        // 1. Songs on an album with this name. Finds albums the album search
        //    doesn't list (e.g. "Grand Theft Auto VI: The Album"); every
        //    track carries the album's cover.
        let body = self.search(album, "song", "50", Some("albumTerm")).await?;
        let seen = describe_apple(&body, true);
        if let Some(art) = pick_apple_album(artist, album, body) {
            return Ok(Some(art));
        }
        trace.note(format!(
            "Apple ({}) album-name search: no songs on \"{album}\" by {artist} ({seen})",
            self.country
        ));
        // 2. The album search.
        let body = self
            .search(&format!("{artist} {album}"), "album", "10", None)
            .await?;
        let seen = describe_apple(&body, false);
        let art = pick_apple_album(artist, album, body);
        if art.is_none() {
            trace.note(format!(
                "Apple ({}) album search: no album \"{album}\" by {artist} ({seen})",
                self.country
            ));
        }
        Ok(art)
    }

    /// The song search, with notes on what was found and why it didn't match.
    pub async fn lookup_traced(&self, track: &Track, trace: &mut Trace) -> Result<Option<Artwork>> {
        // Room for the album version when singles are listed first.
        let body = self
            .search(
                &format!("{} {}", track.artist, track.title),
                "song",
                "25",
                None,
            )
            .await?;
        let seen = describe_apple(&body, true);
        let Some(pick) = pick_apple(track, body) else {
            trace.note(format!(
                "Apple ({}) song search: nothing titled \"{}\" by {} ({seen})",
                self.country, track.title, track.artist
            ));
            return Ok(None);
        };
        if pick.album_matched || track.album.is_none() {
            return Ok(Some(pick.art));
        }
        // The song only matched another release (usually the single): use
        // the cover of the album being played if Apple has it, keeping the
        // song's length.
        let album = track.album.as_deref().unwrap_or_default();
        if let Some(mut art) = self.lookup_album(&track.artist, album, trace).await? {
            art.duration_ms = pick.art.duration_ms;
            return Ok(Some(art));
        }
        trace.other_release = Some(format!(
            "Apple has \"{}\" only on other releases, so the cover of one of them is shown \
             instead of \"{album}\" ({seen})",
            track.title
        ));
        Ok(Some(pick.art))
    }
}

impl ArtworkSource for AppleSearch {
    fn kind(&self) -> ArtworkSourceKind {
        ArtworkSourceKind::Apple
    }

    async fn lookup(&self, track: &Track) -> Result<Option<Artwork>> {
        self.lookup_traced(track, &mut Trace::default()).await
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
    async fn search_release_group(
        &self,
        artist: &str,
        album: &str,
    ) -> Result<Option<(String, Option<ReleaseKind>)>> {
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
    #[serde(default, rename = "primary-type")]
    primary_type: Option<String>,
    #[serde(default, rename = "artist-credit")]
    artist_credit: Vec<MbArtistCredit>,
}

fn valid_mbid(id: &str) -> bool {
    id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

pub(crate) fn pick_release_group(
    artist: &str,
    album: &str,
    resp: MbGroupSearch,
) -> Option<(String, Option<ReleaseKind>)> {
    let wanted = normalize_album(album);
    if wanted.is_empty() {
        return None;
    }
    resp.release_groups
        .into_iter()
        .filter(|g| g.score >= 90)
        .filter(|g| {
            g.title
                .as_deref()
                .is_some_and(|t| normalize_album(t) == wanted)
        })
        .filter(|g| {
            g.artist_credit
                .iter()
                .any(|a| a.name.as_deref().is_some_and(|n| artist_matches(artist, n)))
        })
        .find(|g| valid_mbid(&g.id))
        .map(|g| {
            let kind = g.primary_type.as_deref().and_then(ReleaseKind::from_mb);
            (g.id, kind)
        })
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

impl CoverArtArchive {
    /// The lookup, with notes on each step that found nothing.
    pub async fn lookup_traced(&self, track: &Track, trace: &mut Trace) -> Result<Option<Artwork>> {
        let found = |image_url, release| Artwork {
            source: ArtworkSourceKind::CoverArtArchive,
            image_url,
            link_url: None,
            duration_ms: None,
            release,
        };
        // 1. The exact release Last.fm told us about.
        if let Some(id) = &track.album_mbid {
            match self.release_art(id).await? {
                Some(url) => return Ok(Some(found(url, None))),
                None => trace.note(format!(
                    "Cover Art Archive: the release Last.fm named ({id}) has no front cover"
                )),
            }
        }
        // 2. The album's cover (release group), found by album + artist name.
        if let Some(album) = track.album.as_deref() {
            match self.search_release_group(&track.artist, album).await? {
                Some((group, kind)) => match self.release_group_art(&group).await? {
                    Some(url) => return Ok(Some(found(url, kind))),
                    None => trace.note(format!(
                        "Cover Art Archive: album \"{album}\" ({group}) has no front cover"
                    )),
                },
                None => trace.note(format!(
                    "MusicBrainz: no album \"{album}\" by {}",
                    track.artist
                )),
            }
        }
        // 3. Releases containing this recording.
        let ids = self.search_release_ids(track).await?;
        if ids.is_empty() {
            trace.note(format!(
                "MusicBrainz: no recording \"{}\" by {}",
                track.title, track.artist
            ));
        }
        for id in &ids {
            if let Some(url) = self.release_art(id).await? {
                return Ok(Some(found(url, None)));
            }
        }
        if !ids.is_empty() {
            trace.note(format!(
                "Cover Art Archive: none of the {} releases with \"{}\" has a front cover",
                ids.len(),
                track.title
            ));
        }
        Ok(None)
    }
}

impl ArtworkSource for CoverArtArchive {
    fn kind(&self) -> ArtworkSourceKind {
        ArtworkSourceKind::CoverArtArchive
    }

    async fn lookup(&self, track: &Track) -> Result<Option<Artwork>> {
        self.lookup_traced(track, &mut Trace::default()).await
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
        trace: &mut Trace,
    ) -> Option<Result<Option<Artwork>>> {
        let mut attempt = 0;
        loop {
            let res = match kind {
                ArtworkSourceKind::Apple => self.apple.as_ref()?.lookup_traced(track, trace).await,
                ArtworkSourceKind::CoverArtArchive => {
                    self.caa.as_ref()?.lookup_traced(track, trace).await
                }
            };
            match res {
                Err(e) if e.is_retryable() && attempt == 0 => {
                    attempt += 1;
                    trace.note(format!("{}: {e}; retrying once", kind.name()));
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
        self.resolve_explained(track).await.0
    }

    /// Like [`Self::resolve`], plus what was tried (`None` when the answer
    /// came from the cache, so each miss is reported once).
    pub async fn resolve_explained(&self, track: &Track) -> (Option<Artwork>, Option<Trace>) {
        // The same song on an album and as a single has different covers.
        let key = format!(
            "{}\u{1f}{}",
            track.cache_key(),
            track
                .album
                .as_deref()
                .map(normalize_album)
                .unwrap_or_default()
        );
        if let Ok(cache) = self.cache.lock()
            && let Some(hit) = cache.get(&key, POSITIVE_TTL)
            && (hit.is_some() || cache.get(&key, NEGATIVE_TTL).is_some())
        {
            return (hit, None);
        }
        let mut trace = Trace::default();
        if self.apple.is_none() && self.caa.is_none() {
            trace.note("All album art sources are turned off in the configuration");
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
            if pass > 0 {
                trace.note(format!(
                    "Retrying with simplified names: \"{}\" by {}{}",
                    t.title,
                    t.artist,
                    t.album
                        .as_deref()
                        .map(|a| format!(" on \"{a}\""))
                        .unwrap_or_default()
                ));
            }
            for kind in order {
                match self.lookup_once(kind, t, &mut trace).await {
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
                        trace.note(format!("{}: {e}", kind.name()));
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
                match apple.lookup_album(&t.artist, album, &mut trace).await {
                    Ok(art) => found = art,
                    Err(e) => {
                        had_error = true;
                        trace.note(format!("Apple album search: {e}"));
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
        if had_error && found.is_none() {
            trace.note("Not remembered as \"no art\": it will be tried again next time");
        }
        (found, Some(trace))
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
        .unwrap()
        .art;
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
        assert_eq!(art.release, Some(ReleaseKind::Album));
    }

    /// Real iTunes order for "ADÉLA Red Bottoms": the single comes first.
    const RED_BOTTOMS: &str = r#"{"results":[
        {"artistName":"ADÉLA","trackName":"Red Bottoms","collectionName":"Red Bottoms - Single",
         "artworkUrl100":"https://is1-ssl.mzstatic.com/image/thumb/Music/single/100x100bb.jpg",
         "trackViewUrl":"https://music.apple.com/us/album/red-bottoms/1?i=1","trackTimeMillis":187000},
        {"artistName":"ADÉLA","trackName":"Red Bottoms","collectionName":"PRIMA",
         "artworkUrl100":"https://is1-ssl.mzstatic.com/image/thumb/Music/prima/100x100bb.jpg",
         "trackViewUrl":"https://music.apple.com/us/album/red-bottoms/2?i=2","trackTimeMillis":187000},
        {"artistName":"ADÉLA","trackName":"Red Bottoms","collectionName":"Red Bottoms (Remixes) - EP",
         "artworkUrl100":"https://is1-ssl.mzstatic.com/image/thumb/Music/ep/100x100bb.jpg"}]}"#;

    fn on(album: Option<&str>) -> Track {
        Track {
            album: album.map(Into::into),
            ..track("ADÉLA", "Red Bottoms")
        }
    }

    #[test]
    fn apple_prefers_the_album_being_played() {
        let pick =
            |album| pick_apple(&on(album), serde_json::from_str(RED_BOTTOMS).unwrap()).unwrap();

        let p = pick(Some("PRIMA"));
        assert!(p.album_matched);
        assert!(p.art.image_url.contains("/prima/"), "{}", p.art.image_url);
        assert!(p.art.link_url.unwrap().ends_with("i=2"));
        assert_eq!(p.art.release, Some(ReleaseKind::Album));
        // Deluxe and other bracketed editions still count as the same album.
        assert!(pick(Some("PRIMA (Deluxe)")).album_matched);

        let p = pick(Some("Red Bottoms"));
        assert!(p.album_matched && p.art.image_url.contains("/single/"));
        assert_eq!(p.art.release, Some(ReleaseKind::Single));
        assert!(
            pick(Some("Red Bottoms - Single"))
                .art
                .image_url
                .contains("/single/")
        );

        let p = pick(Some("Red Bottoms (Remixes) EP"));
        assert!(p.album_matched && p.art.image_url.contains("/ep/"));
        assert_eq!(p.art.release, Some(ReleaseKind::Ep));

        // Album unknown to Apple, or not reported: first match, flagged.
        let p = pick(Some("Some Compilation"));
        assert!(!p.album_matched && p.art.image_url.contains("/single/"));
        assert!(!pick(None).album_matched);
    }

    #[test]
    fn artists_match_in_any_order() {
        let lastfm = "Justice, King Gizzard & The Lizard Wizard, Tame Impala & SZA";
        let apple = "Justice, Tame Impala, SZA & King Gizzard & The Lizard Wizard";
        assert!(artist_matches(lastfm, apple));
        assert!(artist_matches(
            "Yung Lean feat. Future",
            "Yung Lean, Future & Metro Boomin"
        ));
        assert!(artist_matches("Daft Punk", "Daft Punk"));
        // A cover by someone else is not the same credit.
        assert!(!artist_matches(lastfm, "Alala"));
        assert!(!artist_matches("SZA", "Justice, Tame Impala"));
        assert!(!artist_matches("", "Justice"));
    }

    /// Real iTunes results (US) for the GTA VI song; the album Last.fm
    /// reports ("The Album") isn't in Apple's search at all.
    const HURRICANE: &str = r#"{"results":[
        {"artistName":"Justice, Tame Impala, SZA & King Gizzard & The Lizard Wizard",
         "trackName":"Flower In A Hurricane [From GTAVI: Original Score]",
         "collectionName":"Flower In A Hurricane [From GTAVI: Original Score] - Single",
         "artworkUrl100":"https://is1-ssl.mzstatic.com/image/thumb/Music/single/100x100bb.jpg"},
        {"artistName":"Alala","trackName":"Flower in a Hurricane (from \"Grand Theft Auto VI\") [Epic Cover Version]",
         "collectionName":"Flower in a Hurricane - Single",
         "artworkUrl100":"https://is1-ssl.mzstatic.com/image/thumb/Music/cover/100x100bb.jpg"},
        {"artistName":"Justice, Tame Impala, SZA & King Gizzard & The Lizard Wizard",
         "trackName":"Flower In A Hurricane [From GTAVI: Original Score]",
         "collectionName":"Grand Theft Auto VI: Original Score",
         "artworkUrl100":"https://is1-ssl.mzstatic.com/image/thumb/Music/score/100x100bb.jpg"}]}"#;

    #[test]
    fn credit_order_and_unknown_album() {
        let t = Track {
            album: Some("Grand Theft Auto VI: The Album".into()),
            ..track(
                "Justice, King Gizzard & The Lizard Wizard, Tame Impala & SZA",
                "Flower In A Hurricane",
            )
        };
        let p = pick_apple(&t, serde_json::from_str(HURRICANE).unwrap()).unwrap();
        // Not the album being played, so the caller tries the album search
        // next and falls back to this cover.
        assert!(!p.album_matched);
        assert!(p.art.image_url.contains("/single/"), "{}", p.art.image_url);
        assert_eq!(p.art.release, Some(ReleaseKind::Single));
    }

    #[test]
    fn compilation_albums_by_various_artists() {
        let json = r#"{"results":[
            {"artistName":"Various Artists","collectionName":"Grand Theft Auto VI: The Album",
             "artworkUrl100":"https://is1-ssl.mzstatic.com/image/thumb/Music/va/100x100bb.jpg"},
            {"artistName":"Various Artists","collectionName":"Grand Theft Auto IV (Remixes)",
             "artworkUrl100":"https://is1-ssl.mzstatic.com/image/thumb/Music/x/100x100bb.jpg"}]}"#;
        let pick = |album| {
            pick_apple_album(
                "Justice, King Gizzard & The Lizard Wizard, Tame Impala & SZA",
                album,
                serde_json::from_str(json).unwrap(),
            )
        };
        assert!(
            pick("Grand Theft Auto VI: The Album")
                .unwrap()
                .image_url
                .contains("/va/")
        );
        // "Various Artists" only counts with the exact album name.
        assert!(pick("Grand Theft Auto IV").is_none());
    }

    /// Real shape of an `attribute=albumTerm` song result: the track's own
    /// artist differs, but the album is a "Various Artists" compilation.
    #[test]
    fn album_found_through_its_songs() {
        let json = r#"{"results":[
            {"artistName":"Morgan Wallen","collectionArtistName":"Various Artists",
             "trackName":"Last Thing You Need","collectionName":"Grand Theft Auto VI: The Album",
             "artworkUrl100":"https://is1-ssl.mzstatic.com/image/thumb/Music/album/100x100bb.jpg",
             "collectionViewUrl":"https://music.apple.com/us/album/last-thing-you-need/6812476961?i=6812477290&uo=4"},
            {"artistName":"Drew Jacobs","trackName":"Last Thing You Need",
             "collectionName":"Last Thing You Need (from GTAVI: The Album) - Single",
             "artworkUrl100":"https://is1-ssl.mzstatic.com/image/thumb/Music/single/100x100bb.jpg"}]}"#;
        let art = pick_apple_album(
            "Justice, King Gizzard & The Lizard Wizard, Tame Impala & SZA",
            "Grand Theft Auto VI: The Album",
            serde_json::from_str(json).unwrap(),
        )
        .unwrap();
        assert!(art.image_url.contains("/album/"), "{}", art.image_url);
        assert_eq!(art.release, Some(ReleaseKind::Album));
        assert_eq!(
            art.link_url.as_deref(),
            Some("https://music.apple.com/us/album/last-thing-you-need/6812476961")
        );
        // Another album with a similar name isn't taken.
        assert!(
            pick_apple_album(
                "Justice",
                "Grand Theft Auto VI",
                serde_json::from_str(json).unwrap()
            )
            .is_none()
        );
    }

    #[test]
    fn trace_is_bounded_and_describes_results() {
        let mut t = Trace::default();
        for i in 0..40 {
            t.note(format!("{i} {}", "x".repeat(500)));
        }
        assert_eq!(t.notes().len(), Trace::MAX_NOTES);
        assert!(
            t.notes()
                .iter()
                .all(|n| n.chars().count() <= Trace::MAX_NOTE_CHARS)
        );

        let resp: AppleResponse = serde_json::from_str(HURRICANE).unwrap();
        let d = describe_apple(&resp, true);
        assert!(
            d.starts_with("3 results, e.g. \"Flower In A Hurricane"),
            "{d}"
        );
        assert!(d.contains("by Alala on"), "{d}");
        let empty: AppleResponse = serde_json::from_str(r#"{"results":[]}"#).unwrap();
        assert_eq!(describe_apple(&empty, false), "no results");
    }

    #[test]
    fn album_names_ignore_release_type() {
        assert_eq!(normalize_album("Red Bottoms - Single"), "redbottoms");
        assert_eq!(normalize_album("Love Songs - EP"), "lovesongs");
        assert_eq!(normalize_album("Love Songs EP"), "lovesongs");
        assert_eq!(normalize_album("PRIMA (Deluxe)"), "prima");
        // A name that is only "EP" stays as it is.
        assert_eq!(normalize_album("EP"), "ep");
        assert_eq!(ReleaseKind::from_apple("X - Single"), ReleaseKind::Single);
        assert_eq!(ReleaseKind::from_apple("X - EP"), ReleaseKind::Ep);
        assert_eq!(ReleaseKind::from_apple("Xep"), ReleaseKind::Album);
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
            {"id":"5c14fd50-a2f1-3672-9537-b0dad91bea2f","score":100,"title":"Hail to the Thief","primary-type":"Album","artist-credit":[{"name":"Radiohead"}]}]}"#;
        assert_eq!(
            pick_release_group(
                "Radiohead",
                "Hail to the Thief",
                serde_json::from_str(json).unwrap()
            ),
            Some((
                "5c14fd50-a2f1-3672-9537-b0dad91bea2f".to_owned(),
                Some(ReleaseKind::Album)
            ))
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

    #[tokio::test]
    #[ignore = "uses the real Apple / MusicBrainz / Cover Art Archive services"]
    async fn credit_order_live_and_trace() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = crate::http::client().unwrap();
        let resolver = ArtworkResolver::new(
            Some(AppleSearch::new(client.clone(), "US")),
            Some(CoverArtArchive::new(client)),
            true,
        );
        let track = Track {
            artist: "Justice, King Gizzard & The Lizard Wizard, Tame Impala & SZA".into(),
            title: "Flower In A Hurricane".into(),
            album: Some("Grand Theft Auto VI: The Album".into()),
            album_mbid: None,
            url: None,
        };
        let (art, trace) = resolver.resolve_explained(&track).await;
        let trace = trace.unwrap();
        eprintln!(
            "art: {art:?}\nnotes: {:#?}\nother: {:?}",
            trace.notes(),
            trace.other_release
        );
        let art = art.unwrap();
        assert_eq!(art.release, Some(ReleaseKind::Album));
        assert!(art.link_url.unwrap().contains("/6812476961"));
        assert!(trace.other_release.is_none());
        // Second time: from the cache, nothing to report.
        assert!(resolver.resolve_explained(&track).await.1.is_none());

        let missing = Track {
            artist: "Nobody Anybody Zzqx".into(),
            title: "No Such Song Zzqx".into(),
            album: Some("No Such Album Zzqx".into()),
            album_mbid: None,
            url: None,
        };
        let (art, trace) = resolver.resolve_explained(&missing).await;
        let trace = trace.unwrap();
        eprintln!("missing notes: {:#?}", trace.notes());
        assert!(art.is_none());
        assert!(
            trace
                .notes()
                .iter()
                .any(|n| n.starts_with("Apple (US) song search"))
        );
        assert!(trace.notes().iter().any(|n| n.starts_with("MusicBrainz")));
    }

    #[tokio::test]
    #[ignore = "uses the real Apple service"]
    async fn album_track_gets_album_cover_not_single() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let apple = AppleSearch::new(crate::http::client().unwrap(), "US");
        let mut track = Track {
            artist: "ADÉLA".into(),
            title: "Red Bottoms".into(),
            album: Some("PRIMA".into()),
            album_mbid: None,
            url: None,
        };
        let album = apple.lookup(&track).await.unwrap().unwrap();
        eprintln!("album: {album:?}");
        assert_eq!(album.release, Some(ReleaseKind::Album));
        track.album = Some("Red Bottoms - Single".into());
        let single = apple.lookup(&track).await.unwrap().unwrap();
        eprintln!("single: {single:?}");
        assert_eq!(single.release, Some(ReleaseKind::Single));
        assert_ne!(album.image_url, single.image_url);
    }
}
