// SPDX-License-Identifier: AGPL-3.0-only
//! RemoteMusicRP: your Last.fm "now playing" as Discord Rich Presence.
//!
//! `run` (the default) starts the presence service and the web UI. A few
//! CLI commands exist for headless administration and recovery.
#![forbid(unsafe_code)]

mod app;
mod email;
mod sdk_mode;
mod service;
mod settings;
#[cfg(any(windows, target_os = "macos"))]
mod tray;
mod tray_art;
mod tray_common;
#[cfg(target_os = "linux")]
mod tray_linux;
mod updater;
mod web;

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use remotemusicrp_core::config::{self, Config, KeyStoreConfig};
use remotemusicrp_crypto::keystore::{KeyStore, PassphraseFileKeyStore, load_or_create};
use remotemusicrp_crypto::{Keyring, SecretString};
use remotemusicrp_storage::{Db, LogLevel};

const USAGE: &str = "\
RemoteMusicRP: your Last.fm now playing as Discord Rich Presence.

USAGE:
    remotemusicrp [--config <file>] <command>

COMMANDS:
    run [--no-tray]          Start presence and the web UI (default)
    status                   Show configuration status (no secrets)
    reset-password           Remove the admin password and 2FA (recovery)
    rollback-update          Restore the previous version after an update
    rotate-keys              Rotate the master key and re-wrap all secrets
    delete-everything --yes  Securely delete all data and the master key
    help                     Show this help
";

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match real_main(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn real_main(mut args: Vec<String>) -> Result<(), String> {
    let mut config_path: Option<PathBuf> =
        std::env::var_os("REMOTEMUSICRP_CONFIG").map(PathBuf::from);
    if let Some(i) = args.iter().position(|a| a == "--config") {
        if i + 1 >= args.len() {
            return Err("--config needs a file path".into());
        }
        config_path = Some(PathBuf::from(args.remove(i + 1)));
        args.remove(i);
    }
    let command = args.first().map(String::as_str).unwrap_or("run");
    if matches!(command, "help" | "-h" | "--help") {
        print!("{USAGE}");
        return Ok(());
    }
    if matches!(command, "version" | "-V" | "--version") {
        println!("remotemusicrp {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let toml_text = match &config_path {
        Some(p) => Some(
            std::fs::read_to_string(p)
                .map_err(|e| format!("cannot read config file: {}", e.kind()))?,
        ),
        None => None,
    };
    let cfg =
        config::load(toml_text.as_deref(), |k| std::env::var(k).ok()).map_err(|e| e.to_string())?;
    init_logging(&cfg.log_level);
    // All TLS uses rustls with the ring provider and the OS trust store.
    let _ = rustls::crypto::ring::default_provider().install_default();

    match command {
        "run" => run(cfg, &args),
        "status" => status(&cfg),
        "reset-password" => reset_password(&cfg),
        "rollback-update" => rollback_update(),
        "rotate-keys" => rotate_keys(&cfg),
        "delete-everything" => {
            if args.get(1).map(String::as_str) != Some("--yes") {
                return Err("this permanently deletes all data and keys; re-run with --yes".into());
            }
            delete_everything(&cfg)
        }
        other => Err(format!("unknown command '{other}'\n\n{USAGE}")),
    }
}

fn init_logging(level: &str) {
    let level = match level {
        "error" => tracing::Level::ERROR,
        "warn" => tracing::Level::WARN,
        "debug" => tracing::Level::DEBUG,
        _ => tracing::Level::INFO,
    };
    use std::io::IsTerminal;
    // Colours only on a terminal: not in journald, desktop.log or Docker logs.
    tracing_subscriber::fmt()
        .with_max_level(level)
        .with_target(false)
        .with_ansi(std::io::stdout().is_terminal())
        .init();
}

/// Resolves on SIGTERM (Unix); never on other platforms.
async fn sigterm() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut s) = signal(SignalKind::terminate()) {
            s.recv().await;
            return;
        }
    }
    std::future::pending::<()>().await;
}

fn open_db(cfg: &Config) -> Result<Db, String> {
    migrate_legacy_data(cfg);
    Db::open(cfg.db_path()).map_err(|e| e.to_string())
}

/// One-time move of data from an earlier project name's folder/file name
/// (ScrobbleAway, then RemoteScrobbleRP; newest found wins).
fn migrate_legacy_data(cfg: &Config) {
    let env = |k: &str| std::env::var(k).ok();
    let default = config::default_data_dir(&env);
    // "Not set up yet" means no database under any name. The folder itself
    // may already exist: the Windows launcher creates it for its log file.
    let has_db = |dir: &Path| {
        std::iter::once("remotemusicrp")
            .chain(config::LEGACY_NAMES.iter().map(|(_, lower)| *lower))
            .any(|n| dir.join(format!("{n}.db")).exists())
    };
    if default.as_deref() == Some(cfg.data_dir.as_path())
        && !has_db(&cfg.data_dir)
        && let Some(old) = config::legacy_data_dirs(&env)
            .into_iter()
            .find(|p| p.is_dir() && has_db(p))
    {
        match move_dir_contents(&old, &cfg.data_dir) {
            Ok(()) => eprintln!("Moved existing data to {}", cfg.data_dir.display()),
            Err(e) => eprintln!("warning: could not move old data folder: {}", e.kind()),
        }
    }
    let new_db = cfg.db_path();
    let Some(old_db) = config::LEGACY_NAMES
        .iter()
        .map(|(_, lower)| cfg.data_dir.join(format!("{lower}.db")))
        .find(|p| p.exists())
    else {
        return;
    };
    if !new_db.exists() {
        for suffix in ["", "-wal", "-shm"] {
            let mut from = old_db.clone().into_os_string();
            from.push(suffix);
            let mut to = new_db.clone().into_os_string();
            to.push(suffix);
            let from = PathBuf::from(from);
            if from.exists() {
                let _ = std::fs::rename(&from, PathBuf::from(to));
            }
        }
    }
}

/// Move `from` to `to`: a plain rename when `to` doesn't exist yet,
/// otherwise each entry that `to` doesn't already have (same volume, so
/// each move is a rename), then the emptied `from` is removed.
fn move_dir_contents(from: &Path, to: &Path) -> std::io::Result<()> {
    if !to.exists() {
        return std::fs::rename(from, to);
    }
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if !target.exists() {
            std::fs::rename(entry.path(), target)?;
        }
    }
    // Only succeeds if empty; leftovers (e.g. an old log) stay put.
    let _ = std::fs::remove_dir(from);
    Ok(())
}

/// Key store for CLI commands (passphrase read from stdin).
fn cli_key_store(cfg: &Config) -> Result<Box<dyn KeyStore>, String> {
    match &cfg.key_store {
        KeyStoreConfig::Passphrase(p) => {
            eprint!("Passphrase: ");
            let mut s = String::new();
            std::io::stdin()
                .lock()
                .read_line(&mut s)
                .map_err(|e| format!("stdin: {}", e.kind()))?;
            Ok(Box::new(PassphraseFileKeyStore::new(
                p,
                SecretString::from(s.trim().to_owned()),
            )))
        }
        other => app::open_key_store(other),
    }
}

fn run(cfg: Config, args: &[String]) -> Result<(), String> {
    let db = open_db(&cfg)?;
    let http = remotemusicrp_providers::http::client().map_err(|e| e.to_string())?;
    let app = Arc::new(app::App::new(cfg, db, http)?);

    if app.cfg.is_exposed() && settings::password_hash(&app.db).is_none() {
        let code = remotemusicrp_crypto::tokens::random_token().map_err(|e| e.to_string())?;
        let code: String = code
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(12)
            .collect();
        // Shown only on the server's own console/log, never in the UI.
        eprintln!("\n  First-time setup code (needed to set the admin password): {code}\n");
        *app.setup_code.lock().map_err(|_| "lock")? = Some(code);
    }
    updater::startup_rollback_check(&app)?;
    if app.is_locked() {
        tracing::info!("instance is locked: enter the passphrase in the web UI");
    }
    let _ = app.db.log(
        LogLevel::Info,
        "service",
        &format!("RemoteMusicRP {} started", env!("CARGO_PKG_VERSION")),
    );

    let tray = tray_common::wanted(&app, args);
    #[cfg(any(windows, target_os = "macos"))]
    if tray {
        return tray::run(app, |a| serve(a, false));
    }
    serve(app, tray)
}

/// Run the presence service, web UI and updater until stopped. `linux_tray`
/// adds the Linux tray icon (other platforms run their tray around this).
fn serve(app: Arc<app::App>, linux_tray: bool) -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("runtime: {}", e.kind()))?;
    rt.block_on(async move {
        let stop_tx = app.stop.clone();
        let mut stop_rx = stop_tx.subscribe();
        let svc = tokio::spawn(service::run(app.clone(), stop_tx.subscribe()));
        let web = tokio::spawn(web::serve(app.clone(), stop_tx.subscribe()));
        tokio::spawn(updater::run(
            app.clone(),
            stop_tx.clone(),
            stop_tx.subscribe(),
        ));
        #[cfg(target_os = "linux")]
        if linux_tray {
            tokio::spawn(tray_linux::run(app.clone()));
        }
        #[cfg(not(target_os = "linux"))]
        let _ = linux_tray;

        let result = tokio::select! {
            _ = tokio::signal::ctrl_c() => Ok(()),
            // systemd and Docker stop services with SIGTERM.
            () = sigterm() => Ok(()),
            _ = stop_rx.changed() => Ok(()),
            r = web => r.map_err(|e| e.to_string()).and_then(|r| r),
        };
        let _ = stop_tx.send(true);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), svc).await;
        // The Social SDK must be stopped before exit, or it aborts.
        if let Some(sdk) = app.sdk.as_ref() {
            sdk.shutdown().await;
        }
        let _ = app.db.prune(app.cfg.log_retention_days);
        result
    })
}

fn status(cfg: &Config) -> Result<(), String> {
    let db = open_db(cfg)?;
    let user = db
        .get_setting(settings::LASTFM_USER)
        .map_err(|e| e.to_string())?;
    let client = db
        .get_setting(settings::DISCORD_CLIENT_ID)
        .map_err(|e| e.to_string())?;
    let has_key = db
        .has_secret(settings::SECRET_LASTFM_KEY)
        .map_err(|e| e.to_string())?;
    println!("Data directory:   {}", cfg.data_dir.display());
    println!(
        "Web UI:           http://{}{}",
        cfg.bind,
        if cfg.is_exposed() {
            " (EXPOSED via TLS proxy)"
        } else {
            ""
        }
    );
    println!("Poll interval:    {} s", cfg.poll_interval_secs);
    println!(
        "Last.fm user:     {}",
        user.as_deref().unwrap_or("(not set)")
    );
    println!(
        "Last.fm API key:  {}",
        if has_key {
            "stored (encrypted)"
        } else {
            "(not set)"
        }
    );
    println!(
        "Discord app ID:   {}",
        client.as_deref().unwrap_or("(not set)")
    );
    println!(
        "Admin password:   {}",
        if settings::password_hash(&db).is_some() {
            "set"
        } else {
            "not set"
        }
    );
    println!(
        "2FA:              {}",
        if settings::get_bool(&db, settings::TOTP_ENABLED) {
            "on"
        } else {
            "off"
        }
    );
    Ok(())
}

/// Recovery path for a forgotten password: requires shell access to the
/// machine (which already implies full access to the data).
fn reset_password(cfg: &Config) -> Result<(), String> {
    let db = open_db(cfg)?;
    for k in [
        settings::PASSWORD_HASH,
        settings::TOTP_ENABLED,
        settings::TOTP_LAST_STEP,
        settings::RECOVERY_HASHES,
    ] {
        db.delete_setting(k).map_err(|e| e.to_string())?;
    }
    db.delete_secret(settings::SECRET_TOTP)
        .map_err(|e| e.to_string())?;
    db.delete_secret(settings::SECRET_TOTP_PENDING)
        .map_err(|e| e.to_string())?;
    db.delete_other_sessions(None).map_err(|e| e.to_string())?;
    let _ = db.log(
        LogLevel::Warn,
        "security",
        "admin password and 2FA reset from the command line",
    );
    println!(
        "Password and 2FA removed; all sessions logged out. Set a new password in the web UI."
    );
    Ok(())
}

/// Restore `<exe>.old` (kept by the updater) over the current executable.
fn rollback_update() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.kind().to_string())?;
    let mut old = exe.as_os_str().to_owned();
    old.push(".old");
    let old = PathBuf::from(old);
    if !old.exists() {
        return Err("no previous version is available to roll back to".into());
    }
    self_replace::self_replace(&old).map_err(|e| format!("rollback failed: {}", e.kind()))?;
    println!("Rolled back to the previous version. Start the app again.");
    Ok(())
}

fn rotate_keys(cfg: &Config) -> Result<(), String> {
    if matches!(cfg.key_store, KeyStoreConfig::ExternalFile(_)) {
        return Err("the master key file is managed externally".into());
    }
    let store = cli_key_store(cfg)?;
    let mut kr: Keyring = load_or_create(store.as_ref()).map_err(|e| e.to_string())?;
    let db = open_db(cfg)?;
    let new_id = kr.rotate().map_err(|e| e.to_string())?;
    store.save(&kr).map_err(|e| e.to_string())?;
    let n = db.rewrap_secrets(&kr).map_err(|e| e.to_string())?;
    if db
        .secret_key_ids()
        .map_err(|e| e.to_string())?
        .iter()
        .all(|id| *id == new_id)
    {
        kr.retire_old_keys();
        store.save(&kr).map_err(|e| e.to_string())?;
    }
    let _ = db.log(
        LogLevel::Info,
        "security",
        &format!("master key rotated to v{new_id}; {n} secrets re-wrapped"),
    );
    println!("Rotated to master key v{new_id}; re-wrapped {n} secrets.");
    Ok(())
}

fn delete_everything(cfg: &Config) -> Result<(), String> {
    let db = open_db(cfg)?;
    db.destroy().map_err(|e| e.to_string())?;
    match &cfg.key_store {
        KeyStoreConfig::ExternalFile(_) => {
            println!(
                "Data deleted. The master key file is managed externally; delete it yourself."
            );
        }
        KeyStoreConfig::Passphrase(p) => {
            match std::fs::remove_file(p) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("could not delete the key file: {}", e.kind())),
            }
            println!("All data and the master key have been deleted.");
        }
        other => {
            app::open_key_store(other)?
                .delete()
                .map_err(|e| e.to_string())?;
            println!("All data and the master key have been deleted.");
        }
    }
    Ok(())
}
