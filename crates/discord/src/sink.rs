// SPDX-License-Identifier: AGPL-3.0-only
//! Presence sinks. [`LocalRpcSink`] keeps one lazy connection to the local
//! Discord client, reconnects after failures, skips duplicate updates and
//! spaces updates to respect Discord's presence rate limit.

use std::future::Future;
use std::time::Duration;

use tokio::time::Instant;

use crate::activity::Activity;
use crate::rpc::{ConnectedUser, RpcClient};
use crate::{DiscordError, Result};

/// Discord limits presence updates; one per 4 s stays well inside it.
const MIN_UPDATE_INTERVAL: Duration = Duration::from_secs(4);
/// Don't hammer the pipe/socket while Discord is closed.
const RECONNECT_INTERVAL: Duration = Duration::from_secs(15);

pub trait PresenceSink: Send {
    /// Show `activity`, or clear presence with `None`.
    fn update(&mut self, activity: Option<&Activity>) -> impl Future<Output = Result<()>> + Send;
    /// Whether the sink currently has a live connection.
    fn is_connected(&self) -> bool;
}

#[derive(Debug)]
pub struct LocalRpcSink {
    client_id: String,
    client: Option<RpcClient>,
    last_sent: Option<Option<Activity>>,
    last_update: Option<Instant>,
    last_connect_attempt: Option<Instant>,
}

impl LocalRpcSink {
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            client: None,
            last_sent: None,
            last_update: None,
            last_connect_attempt: None,
        }
    }

    pub fn connected_user(&self) -> Option<&ConnectedUser> {
        self.client.as_ref().and_then(RpcClient::user)
    }

    async fn ensure_connected(&mut self) -> Result<&mut RpcClient> {
        if self.client.is_none() {
            if let Some(t) = self.last_connect_attempt
                && t.elapsed() < RECONNECT_INTERVAL
            {
                return Err(DiscordError::NotRunning);
            }
            self.last_connect_attempt = Some(Instant::now());
            let client = RpcClient::connect(&self.client_id).await?;
            tracing::info!("connected to Discord desktop client");
            self.client = Some(client);
            // New connection has no presence: force a resend.
            self.last_sent = None;
        }
        self.client.as_mut().ok_or(DiscordError::NotRunning)
    }

    /// Clear presence and disconnect (e.g. on shutdown or pause).
    pub async fn shutdown(&mut self) {
        if let Some(mut c) = self.client.take() {
            let _ = c.set_activity(None).await;
            c.close().await;
        }
        self.last_sent = None;
    }
}

impl PresenceSink for LocalRpcSink {
    async fn update(&mut self, activity: Option<&Activity>) -> Result<()> {
        if self.client.is_some() && self.last_sent.as_ref() == Some(&activity.cloned()) {
            return Ok(());
        }
        if let Some(t) = self.last_update {
            let wait = MIN_UPDATE_INTERVAL.saturating_sub(t.elapsed());
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
        }
        let result = match self.ensure_connected().await {
            Ok(client) => client.set_activity(activity).await,
            Err(e) => Err(e),
        };
        self.last_update = Some(Instant::now());
        match result {
            Ok(()) => {
                self.last_sent = Some(activity.cloned());
                Ok(())
            }
            Err(e) => {
                // Rejected commands keep the connection; transport errors drop it.
                if !matches!(e, DiscordError::Rejected { .. }) && self.client.take().is_some() {
                    tracing::warn!(error = %e, "lost connection to Discord");
                }
                Err(e)
            }
        }
    }

    fn is_connected(&self) -> bool {
        self.client.is_some()
    }
}
