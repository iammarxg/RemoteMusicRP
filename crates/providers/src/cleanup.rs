// SPDX-License-Identifier: AGPL-3.0-only
//! Title/album clean-up.
//!
//! - [`strip_keywords`]: drop " - " sections and (bracketed) parts that
//!   contain user keywords such as "live" or "remastered", then tidy the
//!   leftover separators: "NAME - live/remastered - SLAVE" → "NAME - SLAVE".
//!   Matching is whole-word and case-insensitive ("live" never matches
//!   "Alive"), and the first section (the song's own name) is never removed.
//! - [`base_title`] / [`base_artist`] / [`simplified`]: the bare names used
//!   as a fallback album-art query.

use crate::Track;

const MAX_KEYWORDS: usize = 32;
const MAX_KEYWORD_LEN: usize = 40;
const SEPARATORS: [&str; 3] = [" - ", " – ", " — "];

/// Parse a comma-separated keyword list (lowercased, trimmed, bounded).
pub fn parse_keywords(list: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for k in list.split(',') {
        let k = k.trim().to_lowercase();
        if k.is_empty() || k.chars().count() > MAX_KEYWORD_LEN || out.contains(&k) {
            continue;
        }
        out.push(k);
        if out.len() == MAX_KEYWORDS {
            break;
        }
    }
    out
}

/// Whole-word, case-insensitive containment (`hay` must be lowercase).
fn contains_word(hay: &str, kw: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = hay[start..].find(kw) {
        let i = start + pos;
        let before = hay[..i].chars().next_back();
        let after = hay[i + kw.len()..].chars().next();
        let boundary = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
        if boundary(before) && boundary(after) {
            return true;
        }
        start = i + kw.len().max(1);
        if start >= hay.len() {
            break;
        }
    }
    false
}

fn has_keyword(text: &str, keywords: &[String]) -> bool {
    let lower = text.to_lowercase();
    keywords.iter().any(|k| contains_word(&lower, k))
}

/// Remove `(…)` / `[…]` groups whose content matches `drop`.
fn remove_groups(s: &str, drop: impl Fn(&str) -> bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        let open = rest.find(['(', '[']);
        let Some(open) = open else {
            out.push_str(rest);
            break;
        };
        let close_ch = if rest[open..].starts_with('(') {
            ')'
        } else {
            ']'
        };
        let Some(close_rel) = rest[open..].find(close_ch) else {
            out.push_str(rest);
            break;
        };
        let close = open + close_rel;
        out.push_str(&rest[..open]);
        let inner = &rest[open + 1..close];
        if !drop(inner) {
            out.push_str(&rest[open..=close]);
        }
        rest = &rest[close + 1..];
    }
    out
}

fn split_sections(s: &str) -> (Vec<&str>, &'static str) {
    let sep = SEPARATORS
        .iter()
        .copied()
        .find(|sep| s.contains(sep))
        .unwrap_or(" - ");
    let mut parts = vec![s];
    for sep in SEPARATORS {
        parts = parts.into_iter().flat_map(|p| p.split(sep)).collect();
    }
    (parts, sep)
}

fn tidy(s: &str) -> String {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let collapsed = collapsed.replace("()", "").replace("[]", "");
    collapsed
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|c: char| {
            matches!(c, '-' | '–' | '—' | '·' | '|' | ',' | ':') || c.is_whitespace()
        })
        .to_owned()
}

/// Remove sections/brackets containing any keyword. Returns the input
/// unchanged if no keywords are set or nothing would be left.
pub fn strip_keywords(s: &str, keywords: &[String]) -> String {
    if keywords.is_empty() || s.trim().is_empty() {
        return s.to_owned();
    }
    let without_groups = remove_groups(s, |inner| has_keyword(inner, keywords));
    let (sections, sep) = split_sections(&without_groups);
    let kept: Vec<&str> = sections
        .iter()
        .enumerate()
        .filter(|(i, part)| *i == 0 || !has_keyword(part, keywords))
        .map(|(_, p)| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    let out = tidy(&kept.join(sep));
    if out.is_empty() { s.to_owned() } else { out }
}

/// The bare song/album name: text before the first " - " with every
/// bracketed part removed. "Myxomatosis - live" → "Myxomatosis".
pub fn base_title(s: &str) -> String {
    let no_groups = remove_groups(s, |_| true);
    let (sections, _) = split_sections(&no_groups);
    let out = tidy(sections.first().copied().unwrap_or(""));
    if out.is_empty() { tidy(s) } else { out }
}

/// The main artist: drops "feat./ft./featuring …".
pub fn base_artist(s: &str) -> String {
    let lower = s.to_lowercase();
    let cut = [" feat. ", " feat ", " ft. ", " featuring ", " (feat"]
        .iter()
        .filter_map(|m| lower.find(m))
        .min()
        .unwrap_or(s.len());
    tidy(&s[..cut])
}

/// A copy of `track` reduced to bare title, main artist and bare album, or
/// `None` if that changes nothing.
pub fn simplified(track: &Track) -> Option<Track> {
    let t = Track {
        artist: base_artist(&track.artist),
        title: base_title(&track.title),
        album: track
            .album
            .as_deref()
            .map(base_title)
            .filter(|a| !a.is_empty()),
        album_mbid: None,
        url: track.url.clone(),
    };
    (t.artist != track.artist || t.title != track.title || t.album != track.album).then_some(t)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn kw(s: &str) -> Vec<String> {
        parse_keywords(s)
    }

    #[test]
    fn strips_sections_and_tidies_dashes() {
        let k = kw("live, remastered");
        assert_eq!(
            strip_keywords("NAME - live/remastered - SLAVE", &k),
            "NAME - SLAVE"
        );
        assert_eq!(
            strip_keywords("Climbing Up the Walls - Remastered", &k),
            "Climbing Up the Walls"
        );
        assert_eq!(strip_keywords("Myxomatosis - live", &k), "Myxomatosis");
        assert_eq!(strip_keywords("Song – Live – Edit", &k), "Song – Edit");
    }

    #[test]
    fn strips_brackets() {
        let k = kw("live, anniversary");
        assert_eq!(
            strip_keywords("Hail to the Thief (Live Recordings 2003-2009)", &k),
            "Hail to the Thief"
        );
        assert_eq!(
            strip_keywords("Album [25th Anniversary Edition] (Deluxe)", &k),
            "Album (Deluxe)"
        );
    }

    #[test]
    fn whole_words_only_and_first_section_kept() {
        let k = kw("live");
        assert_eq!(
            strip_keywords("Alive - Oliver Mix", &k),
            "Alive - Oliver Mix"
        );
        assert_eq!(strip_keywords("Live Forever", &k), "Live Forever");
        assert_eq!(strip_keywords("Live - Something", &k), "Live - Something");
    }

    #[test]
    fn no_keywords_no_change() {
        assert_eq!(strip_keywords("A - live", &[]), "A - live");
        assert_eq!(
            parse_keywords(" , ,LIVE, live ,remastered"),
            vec!["live", "remastered"]
        );
    }

    #[test]
    fn simplification() {
        assert_eq!(
            base_title("Climbing Up the Walls - Remastered"),
            "Climbing Up the Walls"
        );
        assert_eq!(base_title("Song (feat. X) [Live]"), "Song");
        assert_eq!(base_artist("Rihanna feat. Future"), "Rihanna");
        assert_eq!(base_artist("Chris Brown & Tyga"), "Chris Brown & Tyga");
        let t = Track {
            artist: "Radiohead".into(),
            title: "Myxomatosis - live".into(),
            album: Some("Hail to the Thief (Live Recordings 2003-2009)".into()),
            album_mbid: Some("x".into()),
            url: None,
        };
        let s = simplified(&t).unwrap();
        assert_eq!(s.title, "Myxomatosis");
        assert_eq!(s.album.as_deref(), Some("Hail to the Thief"));
        assert!(s.album_mbid.is_none());
        let plain = Track {
            artist: "A".into(),
            title: "T".into(),
            album: None,
            album_mbid: None,
            url: None,
        };
        assert!(simplified(&plain).is_none());
    }
}
