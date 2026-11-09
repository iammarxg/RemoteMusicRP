// SPDX-License-Identifier: AGPL-3.0-only
//! Where to find the Discord IPC endpoint.
//!
//! - Windows: named pipes `\\.\pipe\discord-ipc-{0..9}` (documented).
//! - Unix: the first set of `XDG_RUNTIME_DIR`, `TMPDIR`, `TMP`, `TEMP`
//!   (else `/tmp`), then `discord-ipc-{0..9}` (documented). Flatpak
//!   (`app/com.discordapp.Discord/`) and Snap (`snap.discord/`) subdirectories
//!   are probed as fallbacks; those two are community-known, not documented.

use std::path::PathBuf;

pub const MAX_INDEX: u8 = 10;

/// Windows pipe names in probe order.
pub fn windows_pipes() -> Vec<String> {
    (0..MAX_INDEX)
        .map(|i| format!(r"\\.\pipe\discord-ipc-{i}"))
        .collect()
}

/// Unix socket candidates in probe order. `env` is injected for testing.
pub fn unix_candidates(env: impl Fn(&str) -> Option<String>) -> (PathBuf, Vec<PathBuf>) {
    let base = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .iter()
        .find_map(|k| env(k).filter(|v| !v.is_empty()))
        .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    let dirs = [
        base.clone(),
        base.join("app").join("com.discordapp.Discord"),
        base.join("snap.discord"),
    ];
    let mut out = Vec::with_capacity(dirs.len() * MAX_INDEX as usize);
    for dir in &dirs {
        for i in 0..MAX_INDEX {
            out.push(dir.join(format!("discord-ipc-{i}")));
        }
    }
    (base, out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn unix_prefers_xdg_runtime_dir() {
        let env = |k: &str| match k {
            "XDG_RUNTIME_DIR" => Some("/run/user/1000".to_owned()),
            "TMPDIR" => Some("/var/tmp".to_owned()),
            _ => None,
        };
        let (base, c) = unix_candidates(env);
        assert_eq!(base, PathBuf::from("/run/user/1000"));
        assert_eq!(c[0], PathBuf::from("/run/user/1000/discord-ipc-0"));
        assert_eq!(
            c[10],
            PathBuf::from("/run/user/1000/app/com.discordapp.Discord/discord-ipc-0")
        );
        assert_eq!(
            c[20],
            PathBuf::from("/run/user/1000/snap.discord/discord-ipc-0")
        );
        assert_eq!(c.len(), 30);
    }

    #[test]
    fn unix_falls_back_to_tmp() {
        let (base, c) = unix_candidates(|_| None);
        assert_eq!(base, PathBuf::from("/tmp"));
        assert_eq!(c[9], PathBuf::from("/tmp/discord-ipc-9"));
    }

    #[test]
    fn windows_names() {
        let p = windows_pipes();
        assert_eq!(p[0], r"\\.\pipe\discord-ipc-0");
        assert_eq!(p.len(), 10);
    }
}
