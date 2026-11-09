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

fn valid_link(s: &str) -> bool {
    s.len() <= MAX_LINK_URL && s.starts_with("https://") && !s.chars().any(char::is_whitespace)
}

fn valid_button(b: &Button) -> bool {
    let label_len = b.label.trim().chars().count();
    (1..=MAX_BUTTON_LABEL).contains(&label_len)
        && b.url.len() <= MAX_BUTTON_URL
        && b.url.starts_with("https://")
        && !b.url.chars().any(char::is_whitespace)
}

impl Activity {
    /// Serialize to the Discord activity object, dropping anything invalid
    /// rather than sending it.
    pub fn to_json(&self) -> Value {
        let mut obj = Map::new();
        obj.insert("type".into(), json!(self.kind as u8));
        if let Some(n) = self.name.as_deref().and_then(fit_text) {
            obj.insert("name".into(), json!(n));
        }
        if let Some(d) = self.details.as_deref().and_then(fit_text) {
            obj.insert("details".into(), json!(d));
            if let Some(u) = self.details_url.as_deref().filter(|u| valid_link(u)) {
                obj.insert("details_url".into(), json!(u));
            }
        }
        if let Some(s) = self.state.as_deref().and_then(fit_text) {
            obj.insert("state".into(), json!(s));
            if let Some(u) = self.state_url.as_deref().filter(|u| valid_link(u)) {
                obj.insert("state_url".into(), json!(u));
            }
        }

        let mut ts = Map::new();
        if let Some(start) = self.start_unix {
            ts.insert("start".into(), json!(start));
            if let Some(end) = self.end_unix.filter(|e| *e > start) {
                ts.insert("end".into(), json!(end));
            }
        }
        if !ts.is_empty() {
            obj.insert("timestamps".into(), Value::Object(ts));
        }

        let mut assets = Map::new();
        if let Some(img) = self.large_image.as_deref().filter(|s| valid_image(s)) {
            assets.insert("large_image".into(), json!(img));
            if let Some(t) = self.large_text.as_deref().and_then(fit_text) {
                assets.insert("large_text".into(), json!(t));
            }
            if let Some(u) = self.large_url.as_deref().filter(|u| valid_link(u)) {
                assets.insert("large_url".into(), json!(u));
            }
        }
        if let Some(img) = self.small_image.as_deref().filter(|s| valid_image(s)) {
            assets.insert("small_image".into(), json!(img));
            if let Some(t) = self.small_text.as_deref().and_then(fit_text) {
                assets.insert("small_text".into(), json!(t));
            }
        }
        if !assets.is_empty() {
            obj.insert("assets".into(), Value::Object(assets));
        }

        let buttons: Vec<Value> = self
            .buttons
            .iter()
            .filter(|b| valid_button(b))
            .take(MAX_BUTTONS)
            .map(|b| json!({ "label": b.label.trim(), "url": b.url }))
            .collect();
        if !buttons.is_empty() {
            obj.insert("buttons".into(), Value::Array(buttons));
        }

        if let Some(sd) = self.status_display {
            obj.insert("status_display_type".into(), json!(sd as u8));
        }
        Value::Object(obj)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

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
