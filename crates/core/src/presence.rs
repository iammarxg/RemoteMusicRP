// SPDX-License-Identifier: AGPL-3.0-only
//! Turn "what's playing" + user preferences into a Discord activity.
//!
//! Text comes from three templates (first line, second line, album line).
//! By default they are generated from simple layout options (swap, one line,
//! "by"/"on" prefixes); "custom templates" lets the user write them.

use remotemusicrp_discord::{Activity, ActivityKind, Button, StatusDisplay};
use remotemusicrp_providers::Track;
use remotemusicrp_providers::artwork::Artwork;

pub const PROJECT_URL: &str = "https://github.com/iammarxg/RemoteMusicRP";

/// Maximum template / custom text length (Discord text fields are ≤128).
pub const MAX_TEMPLATE_CHARS: usize = 128;
const MAX_BUTTON_LABEL: usize = 32;
const MAX_BUTTON_URL: usize = 512;
const MAX_IMAGE_URL: usize = 256;

/// "Listening to …" vs "Playing …" etc.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityType {
    #[default]
    Listening,
    Playing,
    Watching,
    Competing,
}

/// What follows "Listening to" on the profile and in the member list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameSource {
    /// The Discord application's own name.
    #[default]
    App,
    Artist,
    Title,
    Album,
    /// `custom_name`, which may use the template variables.
    Custom,
}

/// Which text the member list / DM list shows under your name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusLine {
    /// "Listening to <name>".
    Name,
    /// The first line (song title by default).
    #[default]
    FirstLine,
    /// The second line (artist by default).
    SecondLine,
}

/// Which link button (if any) to show. Discord allows at most two buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkButton {
    None,
    LastFm,
    AppleMusic,
}

/// What to show when no album art was found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingArt {
    #[default]
    None,
    /// `missing_art_url` (https image).
    Custom,
}

/// User presence preferences (stored in the database as JSON).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PresenceSettings {
    // -- general --
    pub activity_type: ActivityType,
    pub name_source: NameSource,
    pub custom_name: String,
    pub status_line: StatusLine,
    // -- song information --
    pub show_title: bool,
    pub show_artist: bool,
    pub show_album: bool,
    pub show_art: bool,
    pub show_progress: bool,
    /// Artist on the first line, title on the second.
    pub swap_title_artist: bool,
    /// Title and artist together on the first line; album on the second.
    pub one_line: bool,
    pub prefix_by: bool,
    pub prefix_on: bool,
    /// Use the three templates below instead of the layout options.
    pub custom_templates: bool,
    /// Comma-separated words; sections/brackets of the title and album
    /// containing them are hidden (e.g. "live, remastered, anniversary").
    pub strip_keywords: String,
    /// Variables: `{track}`, `{artist}`, `{album}`.
    pub details_template: String,
    pub state_template: String,
    pub album_template: String,
    // -- links --
    pub link_title: bool,
    pub link_artist: bool,
    pub link_album: bool,
    // -- buttons --
    pub link_button: LinkButton,
    pub custom_button: bool,
    pub custom_button_label: String,
    pub custom_button_url: String,
    pub get_status_button: bool,
    // -- misc --
    pub missing_art: MissingArt,
    pub missing_art_url: String,
}

impl Default for PresenceSettings {
    fn default() -> Self {
        Self {
            activity_type: ActivityType::Listening,
            name_source: NameSource::App,
            custom_name: String::new(),
            status_line: StatusLine::FirstLine,
            show_title: true,
            show_artist: true,
            show_album: true,
            show_art: true,
            show_progress: true,
            swap_title_artist: false,
            one_line: false,
            prefix_by: true,
            prefix_on: false,
            custom_templates: false,
            strip_keywords: String::new(),
            details_template: "{track}".into(),
            state_template: "by {artist}".into(),
            album_template: "{album}".into(),
            link_title: true,
            link_artist: true,
            link_album: true,
            link_button: LinkButton::LastFm,
            custom_button: false,
            custom_button_label: String::new(),
            custom_button_url: String::new(),
            get_status_button: true,
            missing_art: MissingArt::None,
            missing_art_url: String::new(),
        }
    }
}

fn check_text(t: &str, what: &'static str) -> Result<(), &'static str> {
    if t.chars().count() > MAX_TEMPLATE_CHARS {
        return Err(what);
    }
    if t.chars().any(char::is_control) {
        return Err("text cannot contain control characters");
    }
    Ok(())
}

fn https_url(u: &str, max: usize) -> bool {
    u.len() <= max
        && u.starts_with("https://")
        && u.len() > 8
        && !u.chars().any(|c| c.is_whitespace() || c.is_control())
}

impl PresenceSettings {
    /// Validate user input. Returns a user-facing reason on failure.
    pub fn validate(&self) -> Result<(), &'static str> {
        for t in [
            &self.details_template,
            &self.state_template,
            &self.album_template,
        ] {
            check_text(t, "templates can be at most 128 characters")?;
        }
        check_text(
            &self.custom_name,
            "the custom name can be at most 128 characters",
        )?;
        if self.strip_keywords.chars().count() > 500
            || self.strip_keywords.chars().any(char::is_control)
        {
            return Err("the hidden words list is too long (max 500 characters)");
        }
        if self.name_source == NameSource::Custom && self.custom_name.trim().is_empty() {
            return Err("enter a custom name, or pick another option");
        }
        if self.custom_button {
            let n = self.custom_button_label.trim().chars().count();
            if n == 0 || n > MAX_BUTTON_LABEL {
                return Err("the custom button text must be 1–32 characters");
            }
            check_text(
                &self.custom_button_label,
                "the custom button text is too long",
            )?;
            if !https_url(self.custom_button_url.trim(), MAX_BUTTON_URL) {
                return Err("the custom button link must start with https://");
            }
        }
        if self.missing_art == MissingArt::Custom
            && !https_url(self.missing_art_url.trim(), MAX_IMAGE_URL)
        {
            return Err("the placeholder image must be an https:// link (max 256 characters)");
        }
        Ok(())
    }

    /// The three templates actually used: (first line, second line, album).
    pub fn effective_templates(&self) -> (String, String, String) {
        if self.custom_templates {
            return (
                self.details_template.clone(),
                self.state_template.clone(),
                self.album_template.clone(),
            );
        }
        let artist = if self.prefix_by {
            "by {artist}"
        } else {
            "{artist}"
        };
        let album = if self.prefix_on {
            "on {album}"
        } else {
            "{album}"
        };
        if self.one_line {
            let first = if self.swap_title_artist {
                "{artist} – {track}"
            } else {
                "{track} – {artist}"
            };
            // The album already sits on the second line: no separate
            // album line, or Discord shows it twice.
            (first.into(), album.into(), String::new())
        } else if self.swap_title_artist {
            ("{artist}".into(), "{track}".into(), album.into())
        } else {
            ("{track}".into(), artist.into(), album.into())
        }
    }
}

/// Everything known about the current track.
#[derive(Debug, Clone)]
pub struct NowPlayingView<'a> {
    pub track: &'a Track,
    pub artwork: Option<&'a Artwork>,
    pub duration_ms: Option<u64>,
    /// When we first saw this track playing (unix seconds).
    pub started_unix: u64,
    /// Current time (unix seconds), used to detect overrun.
    pub now_unix: u64,
}

/// Expand `{track}`, `{artist}`, `{album}`. Unknown placeholders are kept
/// literally; values are inserted verbatim (no recursive expansion), so
/// track metadata can never inject further placeholders.
pub fn render_template(
    template: &str,
    track: Option<&str>,
    artist: Option<&str>,
    album: Option<&str>,
) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        let Some(close) = after.find('}') else {
            out.push_str(after);
            return tidy(&out);
        };
        let value = match &after[1..close] {
            "track" => Some(track.unwrap_or("")),
            "artist" => Some(artist.unwrap_or("")),
            "album" => Some(album.unwrap_or("")),
            _ => None,
        };
        match value {
            Some(v) => out.push_str(v),
            None => out.push_str(&after[..=close]),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    tidy(&out)
}

/// Collapse whitespace and drop dangling separators left by hidden fields
/// (e.g. "by " when the artist is hidden).
fn tidy(s: &str) -> String {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed
        .trim_matches(|c: char| matches!(c, '-' | '–' | '—' | '·' | '|') || c.is_whitespace());
    match trimmed {
        "by" | "on" | "from" => String::new(),
        t => t.to_owned(),
    }
}

/// Last.fm-style path segment: spaces become '+', reserved characters are
/// percent-encoded.
fn lastfm_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b' ' => out.push('+'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Artist and album page URLs derived from the Last.fm track URL
/// (`https://www.last.fm/music/<Artist>/_/<Track>`).
fn lastfm_pages(track: &Track) -> (Option<String>, Option<String>) {
    let Some(url) = track.url.as_deref() else {
        return (None, None);
    };
    let Some((artist_url, _)) = url.split_once("/_/") else {
        return (None, None);
    };
    let album_url = track
        .album
        .as_deref()
        .map(|a| format!("{artist_url}/{}", lastfm_segment(a)))
        .filter(|u| u.len() <= MAX_IMAGE_URL);
    (Some(artist_url.to_owned()), album_url)
}

/// The template without a leading "by " / "on " prefix, if it has one.
fn unprefixed(tpl: &str) -> Option<&str> {
    let t = tpl.trim_start();
    ["by ", "on "]
        .into_iter()
        .find_map(|p| t.strip_prefix(p))
        .map(str::trim_start)
        .filter(|rest| !rest.is_empty())
}

/// True when the member-list line replaces the "Listening to" name (see
/// `build_activity`), so the UI can say the name setting is overridden.
pub fn member_line_uses_name(s: &PresenceSettings) -> bool {
    let (d, st, _) = s.effective_templates();
    match s.status_line {
        StatusLine::Name => false,
        StatusLine::FirstLine => unprefixed(&d).is_some(),
        StatusLine::SecondLine => unprefixed(&st).is_some(),
    }
}

pub fn build_activity(view: &NowPlayingView<'_>, s: &PresenceSettings) -> Activity {
    let t = view.track;
    // Hide user keywords ("live", "remastered", …) from title and album.
    let keywords = remotemusicrp_providers::cleanup::parse_keywords(&s.strip_keywords);
    let clean_title = remotemusicrp_providers::cleanup::strip_keywords(&t.title, &keywords);
    let clean_album = t
        .album
        .as_deref()
        .map(|a| remotemusicrp_providers::cleanup::strip_keywords(a, &keywords));
    let track = s.show_title.then_some(clean_title.as_str());
    let artist = s.show_artist.then_some(t.artist.as_str());
    let album = if s.show_album {
        clean_album.as_deref()
    } else {
        None
    };

    let (tpl_details, tpl_state, tpl_album) = s.effective_templates();
    let details = render_template(&tpl_details, track, artist, album);
    let state = render_template(&tpl_state, track, artist, album);
    let album_line = render_template(&tpl_album, track, artist, album);

    let image = match (s.show_art, view.artwork, s.missing_art) {
        (true, Some(art), _) => Some(art.image_url.clone()),
        (true, None, MissingArt::Custom) => Some(s.missing_art_url.trim().to_owned()),
        _ => None,
    };
    // Discord shows `large_text` as a visible third line, so never repeat
    // text that is already on the first or second line.
    let album_line_dup = album_line == details || album_line == state;
    let large_text = image
        .as_ref()
        .and((!album_line.is_empty() && !album_line_dup).then(|| album_line.clone()));

    let duration_ms = view
        .duration_ms
        .or_else(|| view.artwork.and_then(|a| a.duration_ms));
    let overrun = crate::timing::is_overrun(view.started_unix, duration_ms, view.now_unix);
    let (start_unix, end_unix) = if s.show_progress && !overrun {
        let end = duration_ms.map(|ms| view.started_unix + ms.div_ceil(1000));
        (Some(view.started_unix), end)
    } else {
        // Past the track's length while Last.fm still says "now playing":
        // most likely paused (Last.fm has no pause signal) or on repeat.
        (None, None)
    };

    // Clickable text: each line links to what it shows.
    let (artist_page, album_page) = lastfm_pages(t);
    let link_for = |tpl: &str| -> Option<String> {
        let first_var = ["{track}", "{artist}", "{album}"]
            .into_iter()
            .filter_map(|v| tpl.find(v).map(|i| (i, v)))
            .min_by_key(|(i, _)| *i)
            .map(|(_, v)| v)?;
        match first_var {
            "{track}" if s.link_title => t.url.clone(),
            "{artist}" if s.link_artist => artist_page.clone(),
            "{album}" if s.link_album => album_page.clone(),
            _ => None,
        }
    };
    let details_url = link_for(&tpl_details);
    let state_url = link_for(&tpl_state);
    let large_url = if s.link_album {
        album_page.clone()
    } else {
        None
    };

    let mut buttons = Vec::new();
    match s.link_button {
        LinkButton::LastFm => {
            if let Some(url) = &t.url {
                buttons.push(Button {
                    label: "View on Last.fm".into(),
                    url: url.clone(),
                });
            }
        }
        LinkButton::AppleMusic => {
            if let Some(url) = view.artwork.and_then(|a| a.link_url.as_ref()) {
                buttons.push(Button {
                    label: "Listen on Apple Music".into(),
                    url: url.clone(),
                });
            }
        }
        LinkButton::None => {}
    }
    if s.custom_button {
        buttons.push(Button {
            label: s.custom_button_label.trim().into(),
            url: s.custom_button_url.trim().into(),
        });
    }
    if s.get_status_button {
        buttons.push(Button {
            label: "Get this status".into(),
            url: PROJECT_URL.into(),
        });
    }
    // Discord shows at most two: keep the first two in the order above,
    // but never drop "Get this status" in favour of nothing.
    buttons.truncate(2);

    let name = match s.name_source {
        NameSource::App => None,
        NameSource::Artist => Some(t.artist.clone()),
        NameSource::Title => Some(clean_title.clone()),
        NameSource::Album => clean_album.clone(),
        NameSource::Custom => Some(render_template(
            &s.custom_name,
            Some(&clean_title),
            Some(&t.artist),
            clean_album.as_deref(),
        )),
    }
    .filter(|n| !n.trim().is_empty());

    let kind = match s.activity_type {
        ActivityType::Listening => ActivityKind::Listening,
        ActivityType::Playing => ActivityKind::Playing,
        ActivityType::Watching => ActivityKind::Watching,
        ActivityType::Competing => ActivityKind::Competing,
    };
    let mut name = name;
    let status_display = match s.status_line {
        StatusLine::Name => StatusDisplay::Name,
        StatusLine::FirstLine | StatusLine::SecondLine => {
            let (tpl, shown) = if s.status_line == StatusLine::FirstLine {
                (&tpl_details, StatusDisplay::Details)
            } else {
                (&tpl_state, StatusDisplay::State)
            };
            // Discord shows the chosen line verbatim in the member list, so
            // "by Artist" would read "Listening to by Artist". For a
            // prefixed line, show it as "Listening to Artist" instead: put
            // the unprefixed text in the activity name and show the name.
            // The profile keeps "by Artist"; only its header changes from
            // the app name to the artist.
            match unprefixed(tpl) {
                Some(bare) => {
                    let text = render_template(bare, track, artist, album);
                    if text.is_empty() {
                        shown
                    } else {
                        name = Some(text);
                        StatusDisplay::Name
                    }
                }
                None => shown,
            }
        }
    };

    Activity {
        kind,
        name,
        details: (!details.is_empty()).then_some(details),
        details_url,
        state: (!state.is_empty()).then_some(state),
        state_url,
        large_image: image,
        large_text,
        large_url,
        small_image: None,
        small_text: None,
        start_unix,
        end_unix,
        buttons,
        status_display: Some(status_display),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use remotemusicrp_providers::artwork::ArtworkSourceKind;

    fn track() -> Track {
        Track {
            artist: "Daft Punk".into(),
            title: "One More Time".into(),
            album: Some("Discovery".into()),
            album_mbid: None,
            url: Some("https://www.last.fm/music/Daft+Punk/_/One+More+Time".into()),
        }
    }

    fn art() -> Artwork {
        Artwork {
            source: ArtworkSourceKind::Apple,
            image_url: "https://is1-ssl.mzstatic.com/x/1024x1024bb.jpg".into(),
            link_url: Some("https://music.apple.com/us/album/x".into()),
            duration_ms: Some(320_357),
        }
    }

    fn view<'a>(t: &'a Track, a: Option<&'a Artwork>) -> NowPlayingView<'a> {
        NowPlayingView {
            track: t,
            artwork: a,
            duration_ms: None,
            started_unix: 1000,
            now_unix: 1000,
        }
    }

    #[test]
    fn templates() {
        assert_eq!(
            render_template("{artist} - {track}", Some("T"), Some("A"), None),
            "A - T"
        );
        assert_eq!(
            render_template("{unknown} {track}", Some("T"), None, None),
            "{unknown} T"
        );
        assert_eq!(render_template("by {artist}", Some("T"), None, None), "");
        assert_eq!(
            render_template("{artist} – {track}", Some("T"), None, None),
            "T"
        );
        assert_eq!(
            render_template("unclosed {track", Some("T"), None, None),
            "unclosed {track"
        );
        // Metadata containing placeholders is not expanded again.
        assert_eq!(
            render_template("{track}", Some("{artist}"), Some("A"), None),
            "{artist}"
        );
    }

    #[test]
    fn default_activity() {
        let t = track();
        let a = art();
        let act = build_activity(&view(&t, Some(&a)), &PresenceSettings::default());
        assert_eq!(act.kind, ActivityKind::Listening);
        assert_eq!(act.name, None);
        assert_eq!(act.details.as_deref(), Some("One More Time"));
        assert_eq!(act.details_url.as_deref(), t.url.as_deref());
        assert_eq!(act.state.as_deref(), Some("by Daft Punk"));
        assert_eq!(
            act.state_url.as_deref(),
            Some("https://www.last.fm/music/Daft+Punk")
        );
        assert_eq!(act.large_text.as_deref(), Some("Discovery"));
        assert_eq!(
            act.large_url.as_deref(),
            Some("https://www.last.fm/music/Daft+Punk/Discovery")
        );
        assert_eq!(act.end_unix, Some(1321));
        assert_eq!(act.buttons.len(), 2);
        assert_eq!(act.status_display, Some(StatusDisplay::Details));
    }

    #[test]
    fn layouts() {
        let s = |f: fn(&mut PresenceSettings)| {
            let mut s = PresenceSettings::default();
            f(&mut s);
            s.effective_templates()
        };
        assert_eq!(s(|_| {}).0, "{track}");
        assert_eq!(
            s(|x| x.swap_title_artist = true),
            ("{artist}".into(), "{track}".into(), "{album}".into())
        );
        assert_eq!(s(|x| x.one_line = true).0, "{track} – {artist}");
        assert_eq!(s(|x| x.prefix_on = true).2, "on {album}");
        assert_eq!(s(|x| x.prefix_by = false).1, "{artist}");
        let custom = s(|x| {
            x.custom_templates = true;
            x.details_template = "♪ {track}".into();
        });
        assert_eq!(custom.0, "♪ {track}");
    }

    #[test]
    fn keywords_hidden_everywhere() {
        let t = Track {
            artist: "Radiohead".into(),
            title: "Climbing Up the Walls - Remastered".into(),
            album: Some("Hail to the Thief (Live Recordings 2003-2009)".into()),
            album_mbid: None,
            url: Some("https://www.last.fm/music/Radiohead/_/x".into()),
        };
        let s = PresenceSettings {
            strip_keywords: "live, remastered".into(),
            one_line: true,
            name_source: NameSource::Title,
            ..Default::default()
        };
        let act = build_activity(&view(&t, None), &s);
        assert_eq!(
            act.details.as_deref(),
            Some("Climbing Up the Walls – Radiohead")
        );
        assert_eq!(act.state.as_deref(), Some("Hail to the Thief"));
        assert_eq!(act.name.as_deref(), Some("Climbing Up the Walls"));
        let long = PresenceSettings {
            strip_keywords: "x".repeat(600),
            ..Default::default()
        };
        assert!(long.validate().is_err());
    }

    #[test]
    fn one_line_does_not_repeat_album() {
        let t = track();
        let a = art();
        let s = PresenceSettings {
            one_line: true,
            ..Default::default()
        };
        let act = build_activity(&view(&t, Some(&a)), &s);
        assert_eq!(act.details.as_deref(), Some("One More Time – Daft Punk"));
        assert_eq!(act.state.as_deref(), Some("Discovery"));
        assert_eq!(act.large_text, None);
        // Custom templates that repeat a line are also de-duplicated.
        let c = PresenceSettings {
            custom_templates: true,
            details_template: "{track}".into(),
            state_template: "{album}".into(),
            album_template: "{album}".into(),
            ..Default::default()
        };
        assert_eq!(build_activity(&view(&t, Some(&a)), &c).large_text, None);
    }

    #[test]
    fn prefixed_member_line_reads_listening_to_artist() {
        let t = track();
        // Default layout has "by {artist}" on the second line.
        let s = PresenceSettings {
            status_line: StatusLine::SecondLine,
            ..Default::default()
        };
        assert!(member_line_uses_name(&s));
        let act = build_activity(&view(&t, None), &s);
        // Member list: "Listening to Daft Punk"; profile keeps "by Daft Punk".
        assert_eq!(act.status_display, Some(StatusDisplay::Name));
        assert_eq!(act.name.as_deref(), Some("Daft Punk"));
        assert_eq!(act.state.as_deref(), Some("by Daft Punk"));
        // One-line layout: "on {album}" works the same way.
        let one = PresenceSettings {
            status_line: StatusLine::SecondLine,
            one_line: true,
            prefix_on: true,
            ..Default::default()
        };
        let act = build_activity(&view(&t, None), &one);
        assert_eq!(act.status_display, Some(StatusDisplay::Name));
        assert_eq!(act.name.as_deref(), Some("Discovery"));
        // Unprefixed lines and the first line keep the old behaviour.
        let first = PresenceSettings::default();
        assert!(!member_line_uses_name(&first));
        let act = build_activity(&view(&t, None), &first);
        assert_eq!(act.status_display, Some(StatusDisplay::Details));
        assert_eq!(act.name, None);
        // Artist hidden: nothing to show, fall back to the (empty) line.
        let hidden = PresenceSettings {
            status_line: StatusLine::SecondLine,
            show_artist: false,
            ..Default::default()
        };
        let act = build_activity(&view(&t, None), &hidden);
        assert_eq!(act.status_display, Some(StatusDisplay::State));
    }

    #[test]
    fn artist_in_member_list_and_custom_name() {
        let t = track();
        let s = PresenceSettings {
            status_line: StatusLine::SecondLine,
            prefix_by: false,
            name_source: NameSource::Custom,
            custom_name: "music by {artist}".into(),
            activity_type: ActivityType::Playing,
            ..Default::default()
        };
        let act = build_activity(&view(&t, None), &s);
        assert_eq!(act.status_display, Some(StatusDisplay::State));
        assert_eq!(act.state.as_deref(), Some("Daft Punk"));
        assert_eq!(act.name.as_deref(), Some("music by Daft Punk"));
        assert_eq!(act.kind, ActivityKind::Playing);
        let by_artist = PresenceSettings {
            name_source: NameSource::Artist,
            ..Default::default()
        };
        assert_eq!(
            build_activity(&view(&t, None), &by_artist).name.as_deref(),
            Some("Daft Punk")
        );
    }

    #[test]
    fn buttons_cap_and_custom() {
        let t = track();
        let s = PresenceSettings {
            custom_button: true,
            custom_button_label: "My site".into(),
            custom_button_url: "https://example.com".into(),
            ..Default::default()
        };
        let act = build_activity(&view(&t, None), &s);
        assert_eq!(act.buttons.len(), 2);
        assert_eq!(act.buttons[1].label, "My site");
    }

    #[test]
    fn missing_art_placeholder_and_links_off() {
        let t = track();
        let s = PresenceSettings {
            missing_art: MissingArt::Custom,
            missing_art_url: "https://example.com/p.png".into(),
            link_title: false,
            link_artist: false,
            link_album: false,
            ..Default::default()
        };
        let act = build_activity(&view(&t, None), &s);
        assert_eq!(
            act.large_image.as_deref(),
            Some("https://example.com/p.png")
        );
        assert!(act.details_url.is_none() && act.state_url.is_none() && act.large_url.is_none());
    }

    #[test]
    fn validation() {
        let ok = PresenceSettings::default();
        assert!(ok.validate().is_ok());
        let bad = [
            PresenceSettings {
                details_template: "x".repeat(200),
                ..Default::default()
            },
            PresenceSettings {
                name_source: NameSource::Custom,
                custom_name: " ".into(),
                ..Default::default()
            },
            PresenceSettings {
                custom_button: true,
                custom_button_label: "x".into(),
                custom_button_url: "http://a".into(),
                ..Default::default()
            },
            PresenceSettings {
                custom_button: true,
                custom_button_label: "x".repeat(40),
                custom_button_url: "https://a.b".into(),
                ..Default::default()
            },
            PresenceSettings {
                missing_art: MissingArt::Custom,
                missing_art_url: "javascript:x".into(),
                ..Default::default()
            },
            PresenceSettings {
                state_template: "a\u{0}b".into(),
                ..Default::default()
            },
        ];
        for b in bad {
            assert!(b.validate().is_err(), "{b:?}");
        }
    }

    #[test]
    fn json_compat_with_old_settings() {
        // Settings saved by the previous version still load.
        let old = r#"{"show_title":true,"show_artist":true,"show_album":true,"show_art":false,
            "details_template":"{track}","state_template":"by {artist}","link_button":"last_fm",
            "get_status_button":true,"show_progress":true}"#;
        let s: PresenceSettings = serde_json::from_str(old).unwrap();
        assert!(!s.show_art);
        assert_eq!(s.name_source, NameSource::App);
        assert!(serde_json::from_str::<PresenceSettings>(r#"{"evil":1}"#).is_err());
    }

    #[test]
    fn lastfm_page_urls() {
        let mut t = track();
        t.album = Some("Random Access/Memories & More".into());
        let (a, b) = lastfm_pages(&t);
        assert_eq!(a.as_deref(), Some("https://www.last.fm/music/Daft+Punk"));
        assert_eq!(
            b.as_deref(),
            Some("https://www.last.fm/music/Daft+Punk/Random+Access%2FMemories+%26+More")
        );
    }

    #[test]
    fn overrun_hides_progress() {
        let t = track();
        let v = NowPlayingView {
            track: &t,
            artwork: None,
            duration_ms: Some(200_000),
            started_unix: 1000,
            now_unix: 1230,
        };
        let act = build_activity(&v, &PresenceSettings::default());
        assert_eq!(act.start_unix, None);
        assert_eq!(act.details.as_deref(), Some("One More Time"));
    }
}
