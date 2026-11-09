// SPDX-License-Identifier: AGPL-3.0-only
//! Shared HTTP plumbing: client construction, per-host rate limiting and
//! size-limited JSON bodies.

use std::time::Duration;

use serde::de::DeserializeOwned;
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::{ProviderError, Result};

/// Identifiable User-Agent, required by Last.fm, ListenBrainz and MusicBrainz.
pub const USER_AGENT: &str = concat!(
    "RemoteMusicRP/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/iammarxg/RemoteMusicRP)"
);

/// Largest response body we will read (provider JSON is a few KiB).
pub const MAX_BODY: usize = 512 * 1024;

/// Build the HTTP client used for all provider calls. HTTPS only, no
/// redirects to plain HTTP, conservative timeouts.
///
/// A rustls crypto provider must be installed by the binary first.
pub fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::limited(3))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(1)
        .build()
        .map_err(|e| ProviderError::Transient(format!("http client: {e}")))
}

/// Enforces a minimum interval between requests to one provider.
#[derive(Debug)]
pub struct RateLimiter {
    min_interval: Duration,
    next: Mutex<Option<Instant>>,
}

impl RateLimiter {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            next: Mutex::new(None),
        }
    }

    /// Wait until a request is allowed, then reserve the slot.
    pub async fn acquire(&self) {
        let mut next = self.next.lock().await;
        let now = Instant::now();
        if let Some(t) = *next
            && t > now
        {
            tokio::time::sleep_until(t).await;
        }
        *next = Some(Instant::now() + self.min_interval);
    }
}

/// Read a response body as JSON, refusing anything larger than [`MAX_BODY`].
pub async fn json_limited<T: DeserializeOwned>(mut resp: reqwest::Response) -> Result<T> {
    if resp.content_length().is_some_and(|l| l > MAX_BODY as u64) {
        return Err(ProviderError::Parse);
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| ProviderError::Transient(redact(&e)))?
    {
        if buf.len() + chunk.len() > MAX_BODY {
            return Err(ProviderError::Parse);
        }
        buf.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&buf).map_err(|_| ProviderError::Parse)
}

/// Describe a reqwest error without the URL (which may contain API keys).
pub fn redact(e: &reqwest::Error) -> String {
    let kind = if e.is_timeout() {
        "timeout"
    } else if e.is_connect() {
        "connection failed"
    } else if e.is_request() {
        "request failed"
    } else if e.is_body() || e.is_decode() {
        "bad response body"
    } else {
        "http error"
    };
    match e.status() {
        Some(s) => format!("{kind} (HTTP {})", s.as_u16()),
        None => kind.to_owned(),
    }
}

/// Map an HTTP status that indicates failure to a provider error.
pub fn status_error(status: reqwest::StatusCode) -> ProviderError {
    match status.as_u16() {
        429 => ProviderError::RateLimited,
        401 | 403 => ProviderError::Auth,
        500..=599 => ProviderError::Transient(format!("HTTP {}", status.as_u16())),
        code => ProviderError::Transient(format!("HTTP {code}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn rate_limiter_spaces_requests() {
        let rl = RateLimiter::new(Duration::from_secs(1));
        let start = Instant::now();
        rl.acquire().await;
        rl.acquire().await;
        rl.acquire().await;
        assert!(start.elapsed() >= Duration::from_secs(2));
    }

    #[test]
    fn user_agent_is_identifiable() {
        assert!(USER_AGENT.starts_with("RemoteMusicRP/"));
        assert!(USER_AGENT.contains("github.com/iammarxg/RemoteMusicRP"));
    }
}
