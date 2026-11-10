// SPDX-License-Identifier: AGPL-3.0-only
//! Rich Presence activity model, validated against Discord's documented
//! field limits before anything is sent.

use serde_json::{Map, Value, json};

/// Discord string fields must be 2–128 characters.
const MIN_TEXT: usize = 2;
const MAX_TEXT: usize = 128;
const MAX_ASSET: usize = 256;
const MAX_BUTTON_LABEL: usize = 32;
const MAX_BUTTON_URL: usize = 512;
const MAX_BUTTONS: usize = 2;
/// Max length Discord accepts for the clickable `*_url` fields.
const MAX_LINK_URL: usize = 256;

/// Activity types allowed over local RPC (verified live 2026-11-09).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ActivityKind {
    Playing = 0,
    #[default]
    Listening = 2,
    Watching = 3,
    Competing = 5,
}

/// Which field Discord shows in the member-list status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusDisplay {
    Name = 0,
    State = 1,
    Details = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    pub label: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Activity {
    pub kind: ActivityKind,
    /// Overrides the "Listening to <name>" text. `None` = the Discord
    /// application's name. Accepted over local RPC (verified 2026-11-09).
    pub name: Option<String>,
    /// First line (e.g. track title).
    pub details: Option<String>,
    /// Makes the first line clickable (https only).
    pub details_url: Option<String>,
    /// Second line (e.g. artist).
    pub state: Option<String>,
    /// Makes the second line clickable (https only).
    pub state_url: Option<String>,
    /// Asset key or https URL.
    pub large_image: Option<String>,
    /// Hover text for the large image (e.g. album).
    pub large_text: Option<String>,
    /// Makes the album art / album line clickable (https only).
    pub large_url: Option<String>,
    pub small_image: Option<String>,
    pub small_text: Option<String>,
    /// Unix seconds; with `end_unix` Discord shows a progress bar.
    pub start_unix: Option<u64>,
    pub end_unix: Option<u64>,
    pub buttons: Vec<Button>,
    pub status_display: Option<StatusDisplay>,
}

/// Fit text into Discord's 2–128 char limits. Too-long text is cut with an
/// ellipsis; 1-char text is padded with an invisible braille blank.
fn fit_text(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let count = s.chars().count();
    let out = if count > MAX_TEXT {
        let mut t: String = s.chars().take(MAX_TEXT - 1).collect();
        t.push('…');
        t
    } else if count < MIN_TEXT {
        format!("{s}\u{2800}")
    } else {
        s.to_owned()
    };
    Some(out)
}

fn valid_image(s: &str) -> bool {
    if s.is_empty() || s.len() > MAX_ASSET {
        return false;
    }
    if s.starts_with("https://") {
        return !s.chars().any(char::is_whitespace);
    }
    // Uploaded asset keys: lowercase letters, digits, '_' and '-'.
    s.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// Length of a URL as Discord sees it: anything that isn't printable ASCII
/// is percent-encoded (each UTF-8 byte becomes `%XX`, 3 characters). A
/// Last.fm link to a title in an unusual script can be under 256 bytes raw
/// but far over 256 once encoded, and Discord then rejects the whole status.
fn encoded_len(s: &str) -> usize {
    s.bytes()
        .map(|b| if b.is_ascii_graphic() { 1 } else { 3 })
        .sum()
}

fn valid_url(s: &str, max: usize) -> bool {
    s.starts_with("https://") && !s.chars().any(char::is_whitespace) && encoded_len(s) <= max
}

fn valid_link(s: &str) -> bool {
    valid_url(s, MAX_LINK_URL)
}

fn valid_button(b: &Button) -> bool {
    let label_len = b.label.trim().chars().count();
    (1..=MAX_BUTTON_LABEL).contains(&label_len) && valid_url(&b.url, MAX_BUTTON_URL)
}

impl Activity {
    /// The activity with every field made to fit Discord's limits: text is
    /// cut or padded, and links, images and buttons that Discord would
    /// reject are left out (rather than letting Discord refuse the whole
    /// status). Both the local RPC and the Social SDK paths send this.
    pub fn sanitized(&self) -> Activity {
        let details = self.details.as_deref().and_then(fit_text);
        let state = self.state.as_deref().and_then(fit_text);
        let large_image = self
            .large_image
            .as_deref()
            .filter(|s| valid_image(s))
            .map(str::to_owned);
        let small_image = self
            .small_image
            .as_deref()
            .filter(|s| valid_image(s))
            .map(str::to_owned);
        let link = |present: bool, u: &Option<String>| {
            u.as_deref()
                .filter(|u| present && valid_link(u))
                .map(str::to_owned)
        };
        Activity {
            kind: self.kind,
            name: self.name.as_deref().and_then(fit_text),
            details_url: link(details.is_some(), &self.details_url),
            state_url: link(state.is_some(), &self.state_url),
            large_text: large_image
                .as_ref()
                .and(self.large_text.as_deref().and_then(fit_text)),
            large_url: link(large_image.is_some(), &self.large_url),
            small_text: small_image
                .as_ref()
                .and(self.small_text.as_deref().and_then(fit_text)),
            details,
            state,
            large_image,
            small_image,
            start_unix: self.start_unix,
            end_unix: self
                .end_unix
                .filter(|e| self.start_unix.is_some_and(|s| *e > s)),
            buttons: self
                .buttons
                .iter()
                .filter(|b| valid_button(b))
                .take(MAX_BUTTONS)
                .map(|b| Button {
                    label: b.label.trim().to_owned(),
                    url: b.url.clone(),
                })
                .collect(),
            status_display: self.status_display,
        }
    }

    /// Serialize to the Discord activity object (local RPC), after
    /// [`sanitized`](Self::sanitized).
    pub fn to_json(&self) -> Value {
        let a = self.sanitized();
        let mut obj = Map::new();
        obj.insert("type".into(), json!(a.kind as u8));
        fn put(m: &mut Map<String, Value>, k: &str, v: &Option<String>) {
            if let Some(v) = v {
                m.insert(k.into(), json!(v));
            }
        }
        put(&mut obj, "name", &a.name);
        put(&mut obj, "details", &a.details);
        put(&mut obj, "details_url", &a.details_url);
        put(&mut obj, "state", &a.state);
        put(&mut obj, "state_url", &a.state_url);

        let mut ts = Map::new();
        if let Some(start) = a.start_unix {
            ts.insert("start".into(), json!(start));
        }
        if let Some(end) = a.end_unix {
            ts.insert("end".into(), json!(end));
        }
        if !ts.is_empty() {
            obj.insert("timestamps".into(), Value::Object(ts));
        }

        let mut assets = Map::new();
        put(&mut assets, "large_image", &a.large_image);
        put(&mut assets, "large_text", &a.large_text);
        put(&mut assets, "large_url", &a.large_url);
        put(&mut assets, "small_image", &a.small_image);
        put(&mut assets, "small_text", &a.small_text);
        if !assets.is_empty() {
            obj.insert("assets".into(), Value::Object(assets));
        }

        if !a.buttons.is_empty() {
            let buttons: Vec<Value> = a
                .buttons
                .iter()
                .map(|b| json!({ "label": b.label, "url": b.url }))
                .collect();
            obj.insert("buttons".into(), Value::Array(buttons));
        }

        if let Some(sd) = a.status_display {
            obj.insert("status_display_type".into(), json!(sd as u8));
        }
        Value::Object(obj)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn link_too_long_once_encoded_is_dropped_not_fatal() {
        // The failing case from the live log: a title in an unusual script.
        // Its Last.fm link is under 256 bytes raw but well over 256
        // characters once Discord percent-encodes it.
        let title = "⣎⡇ꉺლ༽இ•̛)ྀ◞ ༎ຶ ༽ৣৢ؞ৢ؞ؖ ꉺლ";
        let url = format!("https://www.last.fm/music/x/_/{title}{title}");
        assert!(url.len() <= 256, "raw bytes fit: {}", url.len());
        assert!(encoded_len(&url) > 256);
        let a = Activity {
            details: Some(title.into()),
            details_url: Some(url),
            state: Some("Artist".into()),
            state_url: Some("https://www.last.fm/music/Artist".into()),
            ..Default::default()
        };
        let s = a.sanitized();
        // The bad link is dropped; everything else still goes out.
        assert_eq!(s.details.as_deref(), Some(title));
        assert_eq!(s.details_url, None);
        assert_eq!(
            s.state_url.as_deref(),
            Some("https://www.last.fm/music/Artist")
        );
        assert!(a.to_json().get("details_url").is_none());
        assert_eq!(a.to_json()["details"], title);
    }

    #[test]
    fn sanitized_matches_json_limits() {
        // Long text is cut to 128, a link without its line is dropped, and
        // the end time must follow the start.
        let a = Activity {
            name: Some("n".repeat(300)),
            state_url: Some("https://example.com/orphan".into()),
            start_unix: Some(50),
            end_unix: Some(10),
            ..full()
        };
        let s = Activity {
            state: None,
            ..a.clone()
        }
        .sanitized();
        assert_eq!(s.name.as_deref().map(|n| n.chars().count()), Some(128));
        assert_eq!(s.state_url, None);
        assert_eq!(s.end_unix, None);
        assert_eq!(s.buttons.len(), 2);
    }

    fn full() -> Activity {
        Activity {
            details: Some("One More Time".into()),
            state: Some("Daft Punk".into()),
            large_image: Some("https://is1-ssl.mzstatic.com/x/1024x1024bb.jpg".into()),
            large_text: Some("Discovery".into()),
            start_unix: Some(1000),
            end_unix: Some(1320),
            buttons: vec![
                Button {
                    label: "View on Last.fm".into(),
                    url: "https://www.last.fm/music/x".into(),
                },
                Button {
                    label: "Get this status".into(),
                    url: "https://github.com/iammarxg/RemoteMusicRP".into(),
                },
                Button {
                    label: "Third".into(),
                    url: "https://example.com".into(),
                },
            ],
            status_display: Some(StatusDisplay::Details),
            ..Default::default()
        }
    }

    #[test]
    fn serializes_listening_activity() {
        let v = full().to_json();
        assert_eq!(v["type"], 2);
        assert_eq!(v["details"], "One More Time");
        assert_eq!(v["state"], "Daft Punk");
        assert_eq!(v["timestamps"]["start"], 1000);
        assert_eq!(v["timestamps"]["end"], 1320);
        assert_eq!(v["assets"]["large_text"], "Discovery");
        assert_eq!(v["buttons"].as_array().unwrap().len(), 2);
        assert_eq!(v["status_display_type"], 2);
    }

    #[test]
    fn enforces_text_limits() {
        let a = Activity {
            details: Some("x".repeat(500)),
            state: Some("y".into()),
            ..Default::default()
        };
        let v = a.to_json();
        assert_eq!(v["details"].as_str().unwrap().chars().count(), 128);
        assert!(v["details"].as_str().unwrap().ends_with('…'));
        assert_eq!(v["state"].as_str().unwrap().chars().count(), 2);
    }

    #[test]
    fn drops_invalid_fields() {
        let a = Activity {
            details: Some("   ".into()),
            large_image: Some("http://insecure.example/a.png".into()),
            large_text: Some("orphan text".into()),
            start_unix: Some(100),
            end_unix: Some(50),
            buttons: vec![
                Button {
                    label: "".into(),
                    url: "https://ok.example".into(),
                },
                Button {
                    label: "x".repeat(40),
                    url: "https://ok.example".into(),
                },
                Button {
                    label: "bad".into(),
                    url: "javascript:alert(1)".into(),
                },
            ],
            ..Default::default()
        };
        let v = a.to_json();
        assert!(v.get("details").is_none());
        assert!(v.get("assets").is_none());
        assert!(v.get("buttons").is_none());
        assert!(v["timestamps"].get("end").is_none());
    }

    #[test]
    fn name_kind_and_links() {
        let a = Activity {
            kind: ActivityKind::Playing,
            name: Some("music".into()),
            details: Some("Song".into()),
            details_url: Some("https://www.last.fm/music/A/_/Song".into()),
            state: Some("Artist".into()),
            state_url: Some("javascript:alert(1)".into()),
            large_image: Some("https://x.example/a.jpg".into()),
            large_url: Some("https://www.last.fm/music/A/Album".into()),
            ..Default::default()
        };
        let v = a.to_json();
        assert_eq!(v["type"], 0);
        assert_eq!(v["name"], "music");
        assert_eq!(v["details_url"], "https://www.last.fm/music/A/_/Song");
        assert!(v.get("state_url").is_none());
        assert_eq!(
            v["assets"]["large_url"],
            "https://www.last.fm/music/A/Album"
        );
        // A link without its text is dropped.
        let b = Activity {
            details_url: Some("https://x.example".into()),
            ..Default::default()
        };
        assert!(b.to_json().get("details_url").is_none());
        assert_eq!(b.to_json()["type"], 2);
    }

    #[test]
    fn asset_keys_allowed() {
        let a = Activity {
            large_image: Some("remotemusicrp_logo".into()),
            ..Default::default()
        };
        assert_eq!(a.to_json()["assets"]["large_image"], "remotemusicrp_logo");
        let bad = Activity {
            large_image: Some("Bad Key!".into()),
            ..Default::default()
        };
        assert!(bad.to_json().get("assets").is_none());
    }
}
