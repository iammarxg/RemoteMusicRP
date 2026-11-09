// SPDX-License-Identifier: AGPL-3.0-only
//! Track timing estimates.
//!
//! Last.fm only says *that* a track is playing, not where playback is. We
//! estimate the start time instead:
//!
//! 1. By default, the moment we first saw the track as "now playing". This
//!    lags the true start by up to one poll interval plus the player's
//!    reporting delay.
//! 2. Better for back-to-back listening: the previous scrobble's start time
//!    (Last.fm scrobble timestamps are start times) plus its length. We use
//!    this only when it lands shortly *before* we first saw the new track,
//!    so pauses between songs can't move the timer far off.
//!
//! Last.fm has no pause signal either. When a track stays "now playing"
//! well past its length, it is most likely paused or on repeat, and we stop
//! showing a progress bar ([`is_overrun`]).

/// Largest backwards correction we trust from the previous scrobble.
pub const MAX_START_CORRECTION_SECS: u64 = 15;
/// How long past the expected end we wait before treating the track as
/// paused/overrun.
pub const OVERRUN_GRACE_SECS: u64 = 10;

/// Poll interval used around an expected track change.
pub const FAST_POLL_SECS: u64 = 2;
/// Start fast polling this long before the expected end...
pub const FAST_POLL_BEFORE_END_SECS: u64 = 10;
/// ...and keep it up to this long after (then the track counts as overrun).
pub const FAST_POLL_AFTER_END_SECS: u64 = 30;

/// True when a track change is likely soon, so polling should speed up.
/// This catches the next track within ~2 s for only a few extra calls per
/// song, instead of polling fast all the time.
pub fn in_fast_poll_window(started: u64, duration_ms: Option<u64>, now: u64) -> bool {
    duration_ms.is_some_and(|ms| {
        let end = started.saturating_add(ms.div_ceil(1000));
        now.saturating_add(FAST_POLL_BEFORE_END_SECS) >= end
            && now <= end.saturating_add(FAST_POLL_AFTER_END_SECS)
    })
}

/// Estimate when the current track started.
///
/// `first_seen` is when we first saw it as now playing. `previous` is the
/// last completed scrobble's `(start, duration_ms)` if both are known.
pub fn estimate_start(first_seen: u64, previous: Option<(u64, u64)>) -> u64 {
    let Some((prev_start, prev_ms)) = previous else {
        return first_seen;
    };
    let candidate = prev_start.saturating_add(prev_ms.div_ceil(1000));
    if candidate <= first_seen && first_seen - candidate <= MAX_START_CORRECTION_SECS {
        candidate
    } else {
        first_seen
    }
}

/// True when `now` is past the expected end plus a grace period.
pub fn is_overrun(started: u64, duration_ms: Option<u64>, now: u64) -> bool {
    duration_ms.is_some_and(|ms| {
        now > started
            .saturating_add(ms.div_ceil(1000))
            .saturating_add(OVERRUN_GRACE_SECS)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_first_seen_without_previous() {
        assert_eq!(estimate_start(1000, None), 1000);
    }

    #[test]
    fn back_to_back_correction() {
        // Previous started at 800 and lasted 196.2 s -> ended at 997.
        assert_eq!(estimate_start(1000, Some((800, 196_200))), 997);
    }

    #[test]
    fn ignores_estimates_after_first_seen() {
        // Previous track was skipped early: estimate is in the future.
        assert_eq!(estimate_start(1000, Some((900, 200_000))), 1000);
    }

    #[test]
    fn ignores_large_gaps() {
        // A pause between tracks: estimate is too far back.
        assert_eq!(estimate_start(1000, Some((500, 200_000))), 1000);
    }

    #[test]
    fn fast_poll_window() {
        // Track 1000..1200.
        let d = Some(200_000);
        assert!(!in_fast_poll_window(1000, d, 1100));
        assert!(!in_fast_poll_window(1000, d, 1189));
        assert!(in_fast_poll_window(1000, d, 1190));
        assert!(in_fast_poll_window(1000, d, 1230));
        assert!(!in_fast_poll_window(1000, d, 1231));
        assert!(!in_fast_poll_window(1000, None, 1200));
    }

    #[test]
    fn overrun_detection() {
        assert!(!is_overrun(1000, None, 99_999));
        assert!(!is_overrun(1000, Some(200_000), 1200));
        assert!(!is_overrun(1000, Some(200_000), 1210));
        assert!(is_overrun(1000, Some(200_000), 1211));
    }
}
