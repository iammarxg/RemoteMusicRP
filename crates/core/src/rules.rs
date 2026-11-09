// SPDX-License-Identifier: AGPL-3.0-only
//! When the presence is hidden: the artist/song blocklist and quiet hours.

use jiff::Zoned;
use jiff::civil::Weekday;
use jiff::tz::TimeZone;
use remotemusicrp_providers::Track;
use remotemusicrp_providers::cleanup::{base_artist, base_title};
use serde::{Deserialize, Serialize};

pub const MAX_BLOCK_ENTRIES: usize = 200;
pub const MAX_ENTRY_CHARS: usize = 200;
pub const MAX_WINDOWS: usize = 14;

/// A time window (local time) in which the presence is hidden.
/// `start == end` means the whole day; `start > end` runs past midnight
/// and belongs to the day it starts on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuietWindow {
    /// Monday first.
    pub days: [bool; 7],
    /// "HH:MM", 24-hour.
    pub start: String,
    pub end: String,
}

impl Default for QuietWindow {
    fn default() -> Self {
        Self {
            days: [true; 7],
            start: "23:00".into(),
            end: "07:00".into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rules {
    /// Hide the presence while any of these artists is playing.
    pub blocked_artists: Vec<String>,
    /// Song names ("Title" or "Artist - Title") to hide.
    pub blocked_tracks: Vec<String>,
    pub quiet_enabled: bool,
    /// IANA time zone, empty = this computer's time zone.
    pub time_zone: String,
    pub quiet: Vec<QuietWindow>,
}

/// Why the presence is hidden right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hidden {
    BlockedArtist,
    BlockedTrack,
}

impl Hidden {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BlockedArtist => "blocked_artist",
            Self::BlockedTrack => "blocked_track",
        }
    }
}

fn norm(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// "HH:MM" → minutes since midnight.
fn parse_hm(s: &str) -> Option<u16> {
    let (h, m) = s.trim().split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 {
        return None;
    }
    let h: u16 = h.parse().ok()?;
    let m: u16 = m.parse().ok()?;
    (h < 24 && m < 60).then_some(h * 60 + m)
}

fn weekday_index(w: Weekday) -> usize {
    usize::try_from(w.to_monday_zero_offset()).unwrap_or(0)
}

impl Rules {
    pub fn validate(&self) -> Result<(), &'static str> {
        for list in [&self.blocked_artists, &self.blocked_tracks] {
            if list.len() > MAX_BLOCK_ENTRIES {
                return Err("a blocklist can have at most 200 entries");
            }
            if list
                .iter()
                .any(|e| e.chars().count() > MAX_ENTRY_CHARS || e.chars().any(char::is_control))
            {
                return Err("blocklist entries can be at most 200 characters");
            }
        }
        if self.quiet.len() > MAX_WINDOWS {
            return Err("you can add at most 14 quiet-hour periods");
        }
        for w in &self.quiet {
            if parse_hm(&w.start).is_none() || parse_hm(&w.end).is_none() {
                return Err("quiet hours need times like 23:00");
            }
            if !w.days.iter().any(|d| *d) {
                return Err("pick at least one day for each quiet-hour period");
            }
        }
        if !self.time_zone.is_empty()
            && (self.time_zone.len() > 64 || TimeZone::get(&self.time_zone).is_err())
        {
            return Err("unknown time zone");
        }
        Ok(())
    }

    /// Drop blank and duplicate entries and trim whitespace.
    pub fn normalized(mut self) -> Self {
        for list in [&mut self.blocked_artists, &mut self.blocked_tracks] {
            let mut seen = Vec::new();
            list.retain_mut(|e| {
                *e = e.trim().to_owned();
                let k = norm(e);
                if k.is_empty() || seen.contains(&k) {
                    return false;
                }
                seen.push(k);
                true
            });
        }
        self.time_zone = self.time_zone.trim().to_owned();
        self
    }

    /// Whether `track` is on the blocklist.
    pub fn blocked(&self, track: &Track) -> Option<Hidden> {
        let artist = norm(&track.artist);
        let main = norm(&base_artist(&track.artist));
        if self
            .blocked_artists
            .iter()
            .map(|a| norm(a))
            .any(|a| a == artist || a == main)
        {
            return Some(Hidden::BlockedArtist);
        }
        let title = norm(&track.title);
        let bare = norm(&base_title(&track.title));
        let hit = self.blocked_tracks.iter().map(|t| norm(t)).any(|t| {
            t == title
                || t == bare
                || [&artist, &main]
                    .iter()
                    .any(|a| t == format!("{a} - {title}") || t == format!("{a} - {bare}"))
        });
        hit.then_some(Hidden::BlockedTrack)
    }

    fn zone(&self) -> TimeZone {
        if self.time_zone.is_empty() {
            TimeZone::system()
        } else {
            TimeZone::get(&self.time_zone).unwrap_or_else(|_| TimeZone::system())
        }
    }

    /// Whether quiet hours are active at `unix` seconds.
    pub fn quiet_at(&self, unix: i64) -> bool {
        if !self.quiet_enabled || self.quiet.is_empty() {
            return false;
        }
        let Ok(ts) = jiff::Timestamp::from_second(unix) else {
            return false;
        };
        self.quiet_in(&ts.to_zoned(self.zone()))
    }

    fn quiet_in(&self, now: &Zoned) -> bool {
        let today = weekday_index(now.weekday());
        let yesterday = (today + 6) % 7;
        let minute =
            u16::try_from(now.hour()).unwrap_or(0) * 60 + u16::try_from(now.minute()).unwrap_or(0);
        self.quiet.iter().any(|w| {
            let (Some(s), Some(e)) = (parse_hm(&w.start), parse_hm(&w.end)) else {
                return false;
            };
            match s.cmp(&e) {
                std::cmp::Ordering::Equal => w.days[today],
                std::cmp::Ordering::Less => w.days[today] && minute >= s && minute < e,
                std::cmp::Ordering::Greater => {
                    (w.days[today] && minute >= s) || (w.days[yesterday] && minute < e)
                }
            }
        })
    }

    /// The time zone actually used (for display).
    pub fn zone_name(&self) -> String {
        self.zone().iana_name().unwrap_or("UTC").to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(artist: &str, title: &str) -> Track {
        Track {
            artist: artist.into(),
            title: title.into(),
            ..Default::default()
        }
    }

    #[test]
    fn blocks_artists_and_tracks() {
        let r = Rules {
            blocked_artists: vec!["  radiohead ".into()],
            blocked_tracks: vec!["Bad Guy".into(), "Drake - One Dance".into()],
            ..Default::default()
        };
        assert_eq!(
            r.blocked(&track("Radiohead", "Creep")),
            Some(Hidden::BlockedArtist)
        );
        assert_eq!(
            r.blocked(&track("Radiohead feat. X", "Creep")),
            Some(Hidden::BlockedArtist)
        );
        assert_eq!(
            r.blocked(&track("Billie Eilish", "bad guy - Live")),
            Some(Hidden::BlockedTrack)
        );
        assert_eq!(
            r.blocked(&track("Drake", "One Dance")),
            Some(Hidden::BlockedTrack)
        );
        assert_eq!(r.blocked(&track("Someone", "One Dance")), None);
        assert_eq!(r.blocked(&track("Radioheads", "Creep")), None);
    }

    fn at(r: &Rules, s: &str) -> bool {
        let z: Zoned = s.parse().unwrap_or_else(|_| panic!("bad time {s}"));
        r.quiet_in(&z)
    }

    #[test]
    fn quiet_windows() {
        let mut days = [false; 7];
        days[0] = true; // Monday
        let r = Rules {
            quiet_enabled: true,
            quiet: vec![QuietWindow {
                days,
                start: "23:00".into(),
                end: "07:00".into(),
            }],
            ..Default::default()
        };
        // 2026-11-09 is a Monday.
        assert!(at(&r, "2026-11-09T23:30[UTC]"));
        assert!(at(&r, "2026-11-10T06:59[UTC]")); // Tuesday morning, started Monday
        assert!(!at(&r, "2026-11-10T07:00[UTC]"));
        assert!(!at(&r, "2026-11-09T06:00[UTC]")); // Monday morning belongs to Sunday
        assert!(!at(&r, "2026-11-10T23:30[UTC]"));

        let day = Rules {
            quiet_enabled: true,
            quiet: vec![QuietWindow {
                days,
                start: "09:00".into(),
                end: "17:00".into(),
            }],
            ..Default::default()
        };
        assert!(at(&day, "2026-11-09T09:00[UTC]"));
        assert!(!at(&day, "2026-11-09T17:00[UTC]"));
        assert!(!day.quiet_at(0) || day.quiet_enabled);
    }

    #[test]
    fn validation() {
        let mut r = Rules::default();
        assert!(r.validate().is_ok());
        r.quiet.push(QuietWindow {
            start: "25:00".into(),
            ..Default::default()
        });
        assert!(r.validate().is_err());
        r.quiet[0].start = "7:30".into();
        assert!(r.validate().is_ok());
        r.time_zone = "Mars/Olympus".into();
        assert!(r.validate().is_err());
        r.time_zone = "Europe/Berlin".into();
        assert!(r.validate().is_ok());
        let n = Rules {
            blocked_artists: vec![" A ".into(), "a".into(), " ".into()],
            ..Default::default()
        }
        .normalized();
        assert_eq!(n.blocked_artists, vec!["A".to_owned()]);
    }
}
