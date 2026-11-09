// SPDX-License-Identifier: AGPL-3.0-only
//! Background presence service: Last.fm → artwork → Discord local RPC.
//!
//! Settings are re-read every poll (cheap), credential changes wake the
//! service through `App::reload`, and status is published for the UI.

use std::sync::Arc;
use std::time::Duration;

use remotemusicrp_core::presence::{NowPlayingView, build_activity};
use remotemusicrp_core::timing::{FAST_POLL_SECS, estimate_start, in_fast_poll_window};
use remotemusicrp_discord::rpc::ConnectedUser;
use remotemusicrp_discord::{Activity, DiscordError, LocalRpcSink, PresenceSink};
use remotemusicrp_providers::artwork::{
    AppleSearch, Artwork, ArtworkResolver, ArtworkSourceKind, CoverArtArchive,
};
use remotemusicrp_providers::lastfm::LastFm;
use remotemusicrp_providers::{NowPlaying, ProviderError, Scrobble, ScrobbleSource};
use remotemusicrp_storage::LogLevel;
use tokio::sync::watch;

use crate::app::{App, NowPlayingStatus};
use crate::{now_unix, settings};

const IDLE_AFTER: Duration = Duration::from_secs(10 * 60);
const IDLE_POLL: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(300);
const GAP_FAST_POLL_SECS: u64 = 20;
const REFINE_WINDOW_SECS: u64 = 60;
const HOUSEKEEPING_EVERY: Duration = Duration::from_secs(6 * 3600);

struct CurrentTrack {
    key: String,
    first_seen: u64,
    started: u64,
    art: Option<Artwork>,
    duration: Option<u64>,
    considered_prev: Option<u64>,
}

impl CurrentTrack {
    fn duration(&self) -> Option<u64> {
        self.duration
            .or_else(|| self.art.as_ref().and_then(|a| a.duration_ms))
    }
}

/// Spacing between presence updates in server mode (as for local RPC).
const SDK_MIN_UPDATE: Duration = Duration::from_secs(4);

/// Where presence goes: the local Discord client, or (EXPERIMENTAL) the
/// Social SDK in server mode.
enum Sink {
    Local(LocalRpcSink),
    Sdk {
        last_sent: Option<Option<Activity>>,
        last_update: Option<tokio::time::Instant>,
    },
}

impl Sink {
    fn new(app: &App, client_id: String) -> Self {
        if app.sdk.is_some() {
            Self::Sdk {
                last_sent: None,
                last_update: None,
            }
        } else {
            Self::Local(LocalRpcSink::new(client_id))
        }
    }

    async fn update(&mut self, app: &Arc<App>, a: Option<&Activity>) -> Result<(), DiscordError> {
        match self {
            Self::Local(s) => s.update(a).await,
            Self::Sdk {
                last_sent,
                last_update,
            } => {
                let Some(sdk) = app.sdk.as_ref() else {
                    return Err(DiscordError::Sdk("server mode is not loaded".into()));
                };
                sdk.ensure_connected(app).await.inspect_err(|_| {
                    *last_sent = None;
                })?;
                if last_sent.as_ref() == Some(&a.cloned()) {
                    return Ok(());
                }
                if let Some(t) = *last_update {
                    let wait = SDK_MIN_UPDATE.saturating_sub(t.elapsed());
                    if !wait.is_zero() {
                        tokio::time::sleep(wait).await;
                    }
                }
                *last_update = Some(tokio::time::Instant::now());
                sdk.set_activity(a).await?;
                *last_sent = Some(a.cloned());
                Ok(())
            }
        }
    }

    fn is_connected(&self, app: &App) -> bool {
        match self {
            Self::Local(s) => s.is_connected(),
            Self::Sdk { .. } => app.sdk.as_ref().is_some_and(|s| s.ready()),
        }
    }

    fn connected_user(&self, app: &App) -> Option<ConnectedUser> {
        match self {
            Self::Local(s) => s.connected_user().cloned(),
            Self::Sdk { .. } => app.sdk.as_ref().and_then(|s| s.user()),
        }
    }

    async fn shutdown(&mut self, app: &App) {
        match self {
            Self::Local(s) => s.shutdown().await,
            Self::Sdk { last_sent, .. } => {
                if let Some(sdk) = app.sdk.as_ref() {
                    sdk.shutdown().await;
                }
                *last_sent = None;
            }
        }
    }
}

struct Creds {
    user: String,
    api_key: String,
    client_id: String,
}

fn load_creds(app: &App) -> Result<Option<Creds>, String> {
    let user = app
        .db
        .get_setting(settings::LASTFM_USER)
        .map_err(|e| e.to_string())?;
    let client_id = app
        .db
        .get_setting(settings::DISCORD_CLIENT_ID)
        .map_err(|e| e.to_string())?;
    let (Some(user), Some(client_id)) = (user, client_id) else {
        return Ok(None);
    };
    let key = app
        .with_keyring(|kr| {
            app.db.get_secret(
                kr,
                settings::SECRET_LASTFM_KEY,
                settings::PURPOSE_LASTFM_KEY,
            )
        })
        .map_err(str::to_owned)?
        .map_err(|e| e.to_string())?;
    let Some(key) = key else { return Ok(None) };
    let api_key =
        String::from_utf8(key.to_vec()).map_err(|_| "stored API key is corrupt".to_owned())?;
    Ok(Some(Creds {
        user,
        api_key,
        client_id,
    }))
}

/// Run until `shutdown` flips to true.
pub async fn run(app: Arc<App>, mut shutdown: watch::Receiver<bool>) {
    let mut last_housekeeping = tokio::time::Instant::now();
    loop {
        if *shutdown.borrow() {
            return;
        }
        if last_housekeeping.elapsed() >= HOUSEKEEPING_EVERY {
            let _ = app.db.prune(app.cfg.log_retention_days);
            let _ = app.db.prune_sessions(now_unix() as i64);
            last_housekeeping = tokio::time::Instant::now();
        }
        if app.is_locked() {
            app.set_status(|s| {
                *s = crate::app::ServiceStatus {
                    phase: "locked",
                    ..Default::default()
                }
            });
        } else {
            match load_creds(&app) {
                Ok(Some(creds)) => {
                    run_presence(&app, creds, &mut shutdown).await;
                    continue;
                }
                Ok(None) => app.set_status(|s| {
                    *s = crate::app::ServiceStatus {
                        phase: "needs_setup",
                        ..Default::default()
                    }
                }),
                Err(e) => app.set_status(|s| {
                    *s = crate::app::ServiceStatus {
                        phase: "error",
                        lastfm_error: Some(e),
                        ..Default::default()
                    }
                }),
            }
        }
        tokio::select! {
            () = app.reload.notified() => {}
            _ = shutdown.changed() => {}
            () = tokio::time::sleep(Duration::from_secs(30)) => {}
        }
    }
}

/// Inner loop for one set of credentials. Returns on reload or shutdown.
async fn run_presence(app: &Arc<App>, creds: Creds, shutdown: &mut watch::Receiver<bool>) {
    let source = match LastFm::new(app.http.clone(), &creds.user, creds.api_key) {
        Ok(s) => s,
        Err(e) => {
            app.set_status(|s| {
                *s = crate::app::ServiceStatus {
                    phase: "error",
                    lastfm_error: Some(e.to_string()),
                    ..Default::default()
                }
            });
            wait_reload(app, shutdown, Duration::from_secs(3600)).await;
            return;
        }
    };
    let art_cfg = &app.cfg.artwork;
    let resolver = ArtworkResolver::new(
        art_cfg
            .apple
            .then(|| AppleSearch::new(app.http.clone(), &art_cfg.country)),
        art_cfg
            .cover_art_archive
            .then(|| CoverArtArchive::new(app.http.clone())),
        art_cfg.apple_first,
    );
    let mut sink = Sink::new(app, creds.client_id);
    let interval = Duration::from_secs(app.cfg.poll_interval_secs);
    let mut backoff = interval;
    let mut current: Option<CurrentTrack> = None;
    let mut fast_until: Option<u64> = None;
    let mut idle_since: Option<tokio::time::Instant> = None;
    let mut discord_warned = false;
    let mut was_quiet = false;
    let _ = app
        .db
        .log(LogLevel::Info, "service", "presence service started");

    loop {
        let mut next = interval;
        let presence = settings::presence(&app.db);
        let paused = settings::get_bool(&app.db, settings::PAUSED);
        let rules = settings::rules(&app.db);
        let quiet = rules.quiet_at(i64::try_from(now_unix()).unwrap_or(0));
        if quiet != was_quiet {
            was_quiet = quiet;
            let msg = if quiet {
                "quiet hours started"
            } else {
                "quiet hours ended"
            };
            tracing::info!("{msg}");
            let _ = app.db.log(LogLevel::Info, "schedule", msg);
        }

        if paused || quiet {
            current = None;
            let res = sink.update(app, None).await;
            report_sink(app, &sink, res, &mut discord_warned);
            app.set_status(|s| {
                s.phase = if paused { "paused" } else { "running" };
                s.hidden = (!paused).then_some("quiet");
                s.now_playing = None;
                s.activity = None;
            });
            next = IDLE_POLL.max(interval);
        } else {
            match source.now_playing().await {
                Ok(NowPlaying::Playing { track, previous }) => {
                    backoff = interval;
                    idle_since = None;
                    fast_until = None;
                    let key = track.cache_key();
                    if current.as_ref().map(|c| &c.key) != Some(&key) {
                        let first_seen = now_unix();
                        let art = resolver.resolve(&track).await;
                        let duration = source.track_duration_ms(&track).await.unwrap_or_else(|e| {
                            tracing::debug!(error = %e, "duration lookup failed");
                            None
                        });
                        tracing::info!(artist = %track.artist, title = %track.title, art = art.is_some(), "now playing");
                        app.tracks_seen
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        current = Some(CurrentTrack {
                            key,
                            first_seen,
                            started: first_seen,
                            art,
                            duration,
                            considered_prev: None,
                        });
                    }
                    if let Some(cur) = current.as_mut() {
                        refine_start(cur, previous.as_ref(), &source).await;
                        let view = NowPlayingView {
                            track: &track,
                            artwork: cur.art.as_ref(),
                            duration_ms: cur.duration,
                            started_unix: cur.started,
                            now_unix: now_unix(),
                        };
                        let hidden = rules.blocked(&track).map(|h| h.as_str());
                        let activity = build_activity(&view, &presence);
                        let res = sink
                            .update(
                                app,
                                if hidden.is_some() {
                                    None
                                } else {
                                    Some(&activity)
                                },
                            )
                            .await;
                        report_sink(app, &sink, res, &mut discord_warned);
                        let np = NowPlayingStatus {
                            artist: track.artist.clone(),
                            title: track.title.clone(),
                            album: track.album.clone(),
                            art_url: cur.art.as_ref().map(|a| a.image_url.clone()),
                            art_source: cur.art.as_ref().map(|a| match a.source {
                                ArtworkSourceKind::Apple => "apple",
                                ArtworkSourceKind::CoverArtArchive => "cover_art_archive",
                            }),
                            track_url: track.url.clone(),
                            started_unix: cur.started,
                            duration_ms: cur.duration(),
                        };
                        let json = hidden.is_none().then(|| activity.to_json());
                        app.set_status(|s| {
                            s.phase = "running";
                            s.lastfm_error = None;
                            s.hidden = hidden;
                            s.now_playing = Some(np);
                            s.activity = json;
                        });
                        if in_fast_poll_window(cur.started, cur.duration(), now_unix()) {
                            next = next.min(Duration::from_secs(FAST_POLL_SECS));
                        }
                    }
                }
                Ok(NowPlaying::Stopped { .. } | NowPlaying::Nothing) => {
                    backoff = interval;
                    if let Some(prev) = current.take() {
                        tracing::info!("playback stopped");
                        if in_fast_poll_window(prev.started, prev.duration(), now_unix()) {
                            fast_until = Some(now_unix() + GAP_FAST_POLL_SECS);
                        }
                    }
                    if fast_until.is_some_and(|t| now_unix() <= t) {
                        next = next.min(Duration::from_secs(FAST_POLL_SECS));
                    } else {
                        fast_until = None;
                    }
                    let since = *idle_since.get_or_insert_with(tokio::time::Instant::now);
                    if since.elapsed() >= IDLE_AFTER {
                        next = IDLE_POLL.max(interval);
                    }
                    let res = sink.update(app, None).await;
                    report_sink(app, &sink, res, &mut discord_warned);
                    app.set_status(|s| {
                        s.phase = "running";
                        s.lastfm_error = None;
                        s.hidden = None;
                        s.now_playing = None;
                        s.activity = None;
                    });
                }
                Err(e) if e.is_retryable() => {
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    next = jitter(backoff);
                    tracing::warn!(error = %e, retry_in = ?next, "Last.fm request failed");
                    app.set_status(|s| s.lastfm_error = Some(e.to_string()));
                }
                Err(e) => {
                    let msg = match e {
                        ProviderError::Auth => {
                            "Last.fm rejected the API key. Reconnect Last.fm.".to_owned()
                        }
                        other => other.to_string(),
                    };
                    let _ = app.db.log(LogLevel::Error, "lastfm", &msg);
                    crate::email::alert(
                        app,
                        "lastfm",
                        "RemoteMusicRP: Last.fm needs your attention".into(),
                        format!(
                            "Your Discord status is no longer updated because Last.fm reported a problem:

                             {msg}

Open your RemoteMusicRP page and go to Connections to fix it.

                             You get this email at most once a day while the problem lasts."
                        ),
                    );
                    sink.shutdown(app).await;
                    app.set_status(|s| {
                        *s = crate::app::ServiceStatus {
                            phase: "error",
                            lastfm_error: Some(msg),
                            ..Default::default()
                        }
                    });
                    // Wait for the user to fix the connection.
                    wait_reload(app, shutdown, Duration::from_secs(3600)).await;
                    return;
                }
            }
        }

        tokio::select! {
            () = tokio::time::sleep(next) => {}
            () = app.reload.notified() => {
                sink.shutdown(app).await;
                return;
            }
            _ = shutdown.changed() => {
                tracing::info!("stopping; clearing presence");
                sink.shutdown(app).await;
                let _ = app.db.log(LogLevel::Info, "service", "presence service stopped");
                return;
            }
        }
    }
}

async fn wait_reload(app: &App, shutdown: &mut watch::Receiver<bool>, max: Duration) {
    tokio::select! {
        () = app.reload.notified() => {}
        _ = shutdown.changed() => {}
        () = tokio::time::sleep(max) => {}
    }
}

async fn refine_start(cur: &mut CurrentTrack, previous: Option<&Scrobble>, source: &LastFm) {
    let Some(prev) = previous else { return };
    if cur.considered_prev == Some(prev.started_unix)
        || prev.started_unix >= cur.first_seen
        || now_unix().saturating_sub(cur.first_seen) > REFINE_WINDOW_SECS
    {
        return;
    }
    cur.considered_prev = Some(prev.started_unix);
    let Ok(Some(prev_ms)) = source.track_duration_ms(&prev.track).await else {
        return;
    };
    let estimate = estimate_start(cur.first_seen, Some((prev.started_unix, prev_ms)));
    if estimate != cur.started {
        tracing::debug!(
            corrected_by_secs = cur.first_seen - estimate,
            "track start refined from previous scrobble"
        );
        cur.started = estimate;
    }
}

fn report_sink(app: &App, sink: &Sink, res: Result<(), DiscordError>, warned: &mut bool) {
    let cu = sink.connected_user(app);
    let avatar = cu.as_ref().and_then(ConnectedUser::avatar_url);
    let user = cu.as_ref().map(|u| u.username.clone());
    let display = cu.and_then(|u| u.display_name);
    let online = app.sdk.as_ref().and_then(|s| s.online_status());
    match res {
        Ok(()) => {
            *warned = false;
            app.set_status(|s| {
                s.discord_connected = true;
                s.discord_user = user;
                s.discord_display_name = display;
                s.discord_avatar_url = avatar;
                s.discord_status = online;
                s.discord_error = None;
            });
        }
        Err(e) => {
            if !*warned {
                tracing::warn!(error = %e, "could not update Discord presence");
                let _ = app.db.log(LogLevel::Warn, "discord", &e.to_string());
                *warned = true;
            }
            let msg = match e {
                DiscordError::NotRunning => "Discord desktop app is not running".to_owned(),
                other => other.to_string(),
            };
            app.set_status(|s| {
                s.discord_connected = sink.is_connected(app);
                s.discord_user = user;
                s.discord_status = online;
                s.discord_error = Some(msg);
            });
        }
    }
}

/// ±20% jitter so many instances don't retry in lockstep.
fn jitter(d: Duration) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|t| t.subsec_nanos())
        .unwrap_or(0);
    d.mul_f64(0.8 + f64::from(nanos % 400) / 1000.0)
}
