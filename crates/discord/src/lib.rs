// SPDX-License-Identifier: AGPL-3.0-only
//! Discord Rich Presence sinks.
//!
//! Only official mechanisms are used. Today that is **local RPC**: the
//! documented IPC socket of a running Discord desktop client
//! (<https://docs.discord.com/developers/topics/rpc>). No user tokens, no
//! gateway connections, no undocumented endpoints (see CLAUDE.md §4).
#![forbid(unsafe_code)]

pub mod activity;
pub mod ipc;
pub mod paths;
pub mod rpc;
pub mod sink;

pub use activity::{Activity, ActivityKind, Button, StatusDisplay};
pub use sink::{LocalRpcSink, PresenceSink};

#[derive(Debug, thiserror::Error)]
pub enum DiscordError {
    /// No Discord desktop client socket was found.
    #[error("Discord desktop client not running or not reachable")]
    NotRunning,
    #[error("Discord IPC I/O error: {0:?}")]
    Io(std::io::ErrorKind),
    #[error("Discord IPC protocol error: {0}")]
    Protocol(String),
    /// Discord rejected a command (code and message from Discord).
    #[error("Discord returned error {code}: {message}")]
    Rejected { code: i64, message: String },
    #[error("Discord closed the connection: {0}")]
    Closed(String),
    #[error("timed out waiting for Discord")]
    Timeout,
    #[error("invalid Discord application ID")]
    InvalidClientId,
    /// Server mode (Social SDK) is not ready: not signed in, connecting, or
    /// an SDK error. The message is user-facing.
    #[error("{0}")]
    Sdk(String),
}

impl From<std::io::Error> for DiscordError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.kind())
    }
}

pub type Result<T> = std::result::Result<T, DiscordError>;

/// Discord application IDs are snowflakes: 17–20 decimal digits.
pub fn valid_client_id(id: &str) -> bool {
    (17..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #[test]
    fn client_id_validation() {
        assert!(super::valid_client_id("1234567890123456789"));
        assert!(!super::valid_client_id("123"));
        assert!(!super::valid_client_id("12345678901234567a9"));
    }
}
