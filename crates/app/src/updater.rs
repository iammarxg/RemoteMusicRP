// SPDX-License-Identifier: AGPL-3.0-only
//! Auto-updater (portable desktop exe / tarball installs only).
//!
//! - Checks GitHub Releases about once a day (if enabled), HTTPS only.
//! - **Prompted by default**; silent auto-install is opt-in (owner decision).
//! - Before replacing anything, verifies BOTH:
//!   1. the binary's minisign (Ed25519) signature with the public key
//!      embedded in this build, including a signed trusted comment that names
//!      the exact version and target (blocks downgrade/cross-target replays);
//!   2. its SHA-256 against a `SHA256SUMS` file that is itself signed.
//! - Replaces the running executable atomically and keeps `<exe>.old`. If the
//!   new version fails to start three times in a row it is rolled back.
//! - Package-manager and Docker installs never self-update; they only show
//!   "update available".

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use minisign_verify::{PublicKey, Signature};
use remotemusicrp_storage::LogLevel;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use crate::app::App;
use crate::{now_unix, settings};

const RELEASES_API: &str = "https://api.github.com/repos/iammarxg/RemoteMusicRP/releases/latest";
const CHECK_EVERY: Duration = Duration::from_secs(24 * 3600);
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(90);
const MAX_BINARY: usize = 64 * 1024 * 1024;
const MAX_TEXT: usize = 256 * 1024;
const HEALTHY_AFTER: Duration = Duration::from_secs(60);
const MAX_START_ATTEMPTS: u32 = 3;

/// Release signing public key (minisign, base64). `None` until the owner
/// creates the release key (Phase 4); self-install is disabled without it.
pub const UPDATE_PUBLIC_KEY: Option<&str> = option_env!("REMOTEMUSICRP_UPDATE_PUBKEY");

/// How this copy was installed. Packagers/Docker set this at build time; an
/// AppImage is detected at runtime (the same binary also ships in the tarball).
///
/// One Linux binary ships in the tarball, the AppImage, the deb/rpm packages
/// and the Docker image, so the kind is also detected at runtime. Every
/// runtime signal can only turn self-update **off**, never on:
/// - `REMOTEMUSICRP_INSTALL_KIND=package|docker` (set by the systemd unit
///   and the Docker image);
/// - an executable under `/usr/` (but not `/usr/local/`), which belongs to
///   the package manager.
pub fn install_kind() -> &'static str {
    if let Some(k @ ("package" | "docker")) = option_env!("REMOTEMUSICRP_INSTALL_KIND") {
        return k;
    }
    match std::env::var("REMOTEMUSICRP_INSTALL_KIND").as_deref() {
        Ok("package") => return "package",
        Ok("docker") => return "docker",
        _ => {}
    }
    if cfg!(unix)
        && std::env::current_exe()
            .is_ok_and(|p| p.starts_with("/usr/") && !p.starts_with("/usr/local/"))
    {
        return "package";
    }
    if appimage_path().is_some() {
        "appimage"
    } else {
        "portable"
    }
}

/// The `.AppImage` file this process runs from, if any.
///
/// Inside an AppImage the program lives in a read-only mount, so an update
/// must replace the `.AppImage` file itself (named by the AppImage runtime in
/// `$APPIMAGE`). Only trusted when our own executable sits inside `$APPDIR`
/// and the path is an absolute, existing `*.AppImage` file.
fn appimage_path() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let file = PathBuf::from(std::env::var_os("APPIMAGE")?);
    let appdir = PathBuf::from(std::env::var_os("APPDIR")?);
    let exe = std::env::current_exe().ok()?;
    (file.is_absolute()
        && appdir.is_absolute()
        && exe.starts_with(&appdir)
        && file.extension().is_some_and(|e| e == "AppImage")
        && file.is_file())
    .then_some(file)
}

/// The file an update replaces (and restarts).
fn update_target() -> Result<PathBuf, String> {
    match appimage_path() {
        Some(p) => Ok(p),
        None => std::env::current_exe().map_err(|e| format!("current exe: {}", e.kind())),
    }
}

pub const TARGET: &str = env!("SA_TARGET");

fn exe_suffix() -> &'static str {
    if cfg!(windows) { ".exe" } else { "" }
}

/// Release asset name for this platform.
pub fn asset_name() -> String {
    if appimage_path().is_some() {
        format!("remotemusicrp-{TARGET}.AppImage")
    } else {
        format!("remotemusicrp-{TARGET}{}", exe_suffix())
    }
}

/// The signed trusted comment every release file must carry: product,
/// version and exact asset name (blocks downgrade and cross-asset replays).
pub fn expected_comment(version: &str, asset: &str) -> String {
    format!("remotemusicrp {version} {asset}")
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: Option<String>,
    pub available: bool,
    pub security: bool,
    pub notes_url: Option<String>,
    pub checked_at: Option<u64>,
    pub error: Option<String>,
    pub can_install: bool,
    pub install_kind: &'static str,
    pub installing: bool,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    html_url: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize, Clone)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// Parse "v1.2.3" / "1.2.3" into a comparable tuple.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim().trim_start_matches('v');
    let core = v.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    let r = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some(r)
}

fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse_version(latest), parse_version(current)), (Some(l), Some(c)) if l > c)
}

pub fn can_self_install() -> bool {
    matches!(install_kind(), "portable" | "appimage") && UPDATE_PUBLIC_KEY.is_some()
}

fn base_info() -> UpdateInfo {
    UpdateInfo {
        current: env!("CARGO_PKG_VERSION").to_owned(),
        can_install: can_self_install(),
        install_kind: install_kind(),
        ..Default::default()
    }
}

/// Latest known state (for the API).
pub fn info(app: &App) -> UpdateInfo {
    app.update
        .lock()
        .map(|u| u.clone())
        .unwrap_or_else(|_| base_info())
}

fn set_info(app: &App, f: impl FnOnce(&mut UpdateInfo)) {
    if let Ok(mut u) = app.update.lock() {
        f(&mut u);
    }
}

pub fn initial_info() -> UpdateInfo {
    base_info()
}

async fn fetch_release(app: &App) -> Result<Release, String> {
    let resp = app
        .http
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| remotemusicrp_providers::http::redact(&e))?;
    if resp.status().as_u16() == 404 {
        return Err("no releases published yet".into());
    }
    if !resp.status().is_success() {
        return Err(format!("GitHub returned HTTP {}", resp.status().as_u16()));
    }
    remotemusicrp_providers::http::json_limited::<Release>(resp)
        .await
        .map_err(|e| e.to_string())
}

/// Check GitHub for a newer release and update `app.update`.
pub async fn check(app: &App) -> UpdateInfo {
    let result = fetch_release(app).await;
    set_info(app, |u| {
        u.checked_at = Some(now_unix());
        match result {
            Ok(r) if !r.draft && !r.prerelease => {
                let text = format!(
                    "{} {}",
                    r.name.as_deref().unwrap_or(""),
                    r.body.as_deref().unwrap_or("")
                )
                .to_ascii_lowercase();
                u.available = is_newer(&r.tag_name, &u.current);
                u.security = u.available && text.contains("[security]");
                u.latest = Some(
                    r.tag_name
                        .trim_start_matches('v')
                        .chars()
                        .take(32)
                        .collect(),
                );
                u.notes_url = r.html_url.filter(|h| h.starts_with("https://github.com/"));
                u.error = None;
            }
            Ok(_) => u.error = None,
            Err(e) => u.error = Some(e),
        }
    });
    info(app)
}

async fn download(app: &App, url: &str, limit: usize) -> Result<Vec<u8>, String> {
    if !url.starts_with("https://github.com/iammarxg/RemoteMusicRP/releases/download/") {
        return Err("unexpected download location".into());
    }
    let mut resp = app
        .http
        .get(url)
        .send()
        .await
        .map_err(|e| remotemusicrp_providers::http::redact(&e))?;
    if !resp.status().is_success() {
        return Err(format!("download failed: HTTP {}", resp.status().as_u16()));
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| remotemusicrp_providers::http::redact(&e))?
    {
        if buf.len() + chunk.len() > limit {
            return Err("download too large".into());
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

fn find_hash(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hash, file) = l.trim().split_once(char::is_whitespace)?;
        let file = file.trim().trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

/// Verify a downloaded update (`asset` is the release file name it was
/// downloaded as). Pure function for testing.
pub fn verify_update(
    pubkey_b64: &str,
    version: &str,
    asset: &str,
    binary: &[u8],
    binary_sig: &str,
    sums: &[u8],
    sums_sig: &str,
) -> Result<(), String> {
    let pk = PublicKey::from_base64(pubkey_b64).map_err(|_| "invalid embedded public key")?;
    let sig = Signature::decode(binary_sig).map_err(|_| "malformed binary signature")?;
    pk.verify(binary, &sig, false)
        .map_err(|_| "binary signature is NOT valid")?;
    if sig.trusted_comment().trim() != expected_comment(version, asset) {
        return Err("signature is for a different version or platform".into());
    }
    let ssig = Signature::decode(sums_sig).map_err(|_| "malformed checksum signature")?;
    pk.verify(sums, &ssig, false)
        .map_err(|_| "checksum file signature is NOT valid")?;
    let sums = std::str::from_utf8(sums).map_err(|_| "checksum file is not text")?;
    let expected = find_hash(sums, asset).ok_or("binary missing from checksum file")?;
    let actual = data_encoding_hex(&Sha256::digest(binary));
    if expected != actual {
        return Err("checksum mismatch".into());
    }
    Ok(())
}

fn data_encoding_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn marker_path(app: &App) -> PathBuf {
    app.cfg.data_dir.join("update-pending")
}

fn backup_path(exe: &Path) -> PathBuf {
    let mut p = exe.as_os_str().to_owned();
    p.push(".old");
    PathBuf::from(p)
}

/// Download, verify, replace, and restart. Returns an error message on
/// failure; nothing is replaced unless every check passes.
pub async fn install(app: &Arc<App>, stop: &watch::Sender<bool>) -> Result<String, String> {
    let pubkey = UPDATE_PUBLIC_KEY
        .ok_or("this build has no update signing key; download updates manually")?;
    if !matches!(install_kind(), "portable" | "appimage") {
        return Err("installed via a package manager or Docker; update it there".into());
    }
    set_info(app, |u| u.installing = true);
    let result = async {
        let release = fetch_release(app).await?;
        let version = release.tag_name.trim_start_matches('v').to_owned();
        if !is_newer(&version, env!("CARGO_PKG_VERSION")) {
            return Err("already up to date".to_owned());
        }
        let asset = |name: &str| {
            release
                .assets
                .iter()
                .find(|a| a.name == name)
                .cloned()
                .ok_or_else(|| format!("release is missing {name}"))
        };
        let name = asset_name();
        let bin = download(app, &asset(&name)?.browser_download_url, MAX_BINARY).await?;
        let bin_sig = download(
            app,
            &asset(&format!("{name}.minisig"))?.browser_download_url,
            MAX_TEXT,
        )
        .await?;
        let sums = download(app, &asset("SHA256SUMS")?.browser_download_url, MAX_TEXT).await?;
        let sums_sig = download(
            app,
            &asset("SHA256SUMS.minisig")?.browser_download_url,
            MAX_TEXT,
        )
        .await?;
        verify_update(
            pubkey,
            &version,
            &name,
            &bin,
            std::str::from_utf8(&bin_sig).map_err(|_| "bad signature file")?,
            &sums,
            std::str::from_utf8(&sums_sig).map_err(|_| "bad signature file")?,
        )?;

        let exe = update_target()?;
        let dir = exe.parent().ok_or("executable has no directory")?;
        let tmp = dir.join(format!(".remotemusicrp-update-{version}.tmp"));
        std::fs::write(&tmp, &bin).map_err(|e| format!("write update: {}", e.kind()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("chmod: {}", e.kind()))?;
        }
        std::fs::copy(&exe, backup_path(&exe)).map_err(|e| format!("backup: {}", e.kind()))?;
        replace_with(&exe, &tmp).map_err(|e| format!("replace: {e}"))?;
        let _ = std::fs::remove_file(&tmp);
        std::fs::write(marker_path(app), "0").map_err(|e| format!("marker: {}", e.kind()))?;
        Ok(version)
    }
    .await;
    set_info(app, |u| u.installing = false);
    let version = result?;
    let _ = app.db.log(
        LogLevel::Info,
        "update",
        &format!("updated to {version}; restarting"),
    );
    restart(stop);
    Ok(version)
}

/// Atomically put `new` in place of `target`. The running executable needs
/// `self_replace` (Windows can't overwrite a running exe); an `.AppImage`
/// file is not the running image, so a same-folder rename is enough.
fn replace_with(target: &Path, new: &Path) -> Result<(), String> {
    if appimage_path().is_some() {
        std::fs::rename(new, target).map_err(|e| e.kind().to_string())
    } else {
        self_replace::self_replace(new).map_err(|e| e.kind().to_string())
    }
}

/// Start the (new) executable with the same arguments, then shut down.
fn restart(stop: &watch::Sender<bool>) {
    if let Ok(exe) = update_target() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if let Err(e) = std::process::Command::new(exe).args(args).spawn() {
            tracing::error!(error = %e.kind(), "could not restart after update; start RemoteMusicRP again manually");
        }
    }
    let _ = stop.send(true);
}

/// Called at startup. Rolls back a freshly installed update that keeps
/// failing to start. Returns `Err` if a rollback happened (caller exits).
pub fn startup_rollback_check(app: &App) -> Result<(), String> {
    let marker = marker_path(app);
    let Ok(text) = std::fs::read_to_string(&marker) else {
        return Ok(());
    };
    let attempts: u32 = text.trim().parse().unwrap_or(0) + 1;
    if attempts >= MAX_START_ATTEMPTS {
        let exe = update_target()?;
        let old = backup_path(&exe);
        let _ = std::fs::remove_file(&marker);
        if old.exists() {
            replace_with(&exe, &old).map_err(|e| format!("rollback failed: {e}"))?;
            let _ = app.db.log(
                LogLevel::Error,
                "update",
                "update failed to start repeatedly; rolled back",
            );
            return Err(
                "the last update failed to start and was rolled back; start RemoteMusicRP again"
                    .into(),
            );
        }
        return Ok(());
    }
    let _ = std::fs::write(&marker, attempts.to_string());
    Ok(())
}

/// Background task: daily check, optional auto-install, and clearing the
/// rollback marker once the new version has run for a minute.
pub async fn run(app: Arc<App>, stop_tx: watch::Sender<bool>, mut stop: watch::Receiver<bool>) {
    let marker = marker_path(&app);
    tokio::select! {
        () = tokio::time::sleep(HEALTHY_AFTER.min(FIRST_CHECK_AFTER)) => {}
        _ = stop.changed() => return,
    }
    if marker.exists() {
        let _ = std::fs::remove_file(&marker);
        let _ = app.db.log(
            LogLevel::Info,
            "update",
            &format!("now running {}", env!("CARGO_PKG_VERSION")),
        );
    }
    if !app.cfg.update_check {
        return;
    }
    loop {
        let info = check(&app).await;
        if info.available
            && info.can_install
            && settings::get_bool(&app.db, settings::UPDATES_AUTO_INSTALL)
            && let Err(e) = install(&app, &stop_tx).await
        {
            let _ = app.db.log(
                LogLevel::Warn,
                "update",
                &format!("automatic update failed: {e}"),
            );
        }
        tokio::select! {
            () = tokio::time::sleep(CHECK_EVERY) => {}
            _ = stop.changed() => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.10.0-rc1"), Some((0, 10, 0)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version("x"), None);
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn checksum_lines() {
        let name = asset_name();
        let h = "a".repeat(64);
        let sums = format!("{} other-file\n{h}  {name}\n", "b".repeat(64));
        assert_eq!(find_hash(&sums, &name), Some(h.clone()));
        assert_eq!(find_hash(&format!("{h} *{name}"), &name), Some(h));
        assert_eq!(find_hash("short  x", "x"), None);
    }

    #[test]
    fn rejects_bad_keys_and_signatures() {
        assert!(verify_update("not-a-key", "1.0.0", "a", b"x", "", b"", "").is_err());
        // Well-formed public key (from the minisign docs), garbage signature.
        let pk = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
        assert!(
            verify_update(
                pk,
                "1.0.0",
                "a",
                b"x",
                "untrusted comment: x\nnope",
                b"",
                ""
            )
            .is_err()
        );
    }

    #[test]
    fn install_kind_default() {
        assert_eq!(install_kind(), "portable");
    }

    #[test]
    fn comment_names_version_and_asset() {
        assert_eq!(
            expected_comment("1.2.3", "remotemusicrp-x86_64-unknown-linux-gnu.AppImage"),
            "remotemusicrp 1.2.3 remotemusicrp-x86_64-unknown-linux-gnu.AppImage"
        );
    }

    /// Positive path against a real minisign signature. The fixture comes
    /// from a throwaway key made only for this test (never a release key);
    /// regenerate with `scripts/make-update-fixture.sh`.
    #[test]
    fn accepts_a_genuine_signed_update() {
        let fx = include_str!("../tests/fixtures/update/fixture.txt");
        let get = |k: &str| {
            fx.lines()
                .find_map(|l| l.strip_prefix(k).and_then(|r| r.strip_prefix('=')))
                .unwrap_or_default()
                .replace("\\n", "\n")
        };
        let pk = get("pubkey");
        let asset = get("asset");
        let bin = get("binary");
        let ok = verify_update(
            &pk,
            "9.9.9",
            &asset,
            bin.as_bytes(),
            &get("binary_sig"),
            get("sums").as_bytes(),
            &get("sums_sig"),
        );
        assert_eq!(ok, Ok(()));
        // Same files, wrong version: refused (downgrade/replay protection).
        assert!(
            verify_update(
                &pk,
                "9.9.8",
                &asset,
                bin.as_bytes(),
                &get("binary_sig"),
                get("sums").as_bytes(),
                &get("sums_sig"),
            )
            .is_err()
        );
        // Same files presented as a different asset: refused.
        assert!(
            verify_update(
                &pk,
                "9.9.9",
                "remotemusicrp-other-target",
                bin.as_bytes(),
                &get("binary_sig"),
                get("sums").as_bytes(),
                &get("sums_sig"),
            )
            .is_err()
        );
        // Tampered binary: refused.
        assert!(
            verify_update(
                &pk,
                "9.9.9",
                &asset,
                b"tampered",
                &get("binary_sig"),
                get("sums").as_bytes(),
                &get("sums_sig"),
            )
            .is_err()
        );
    }
}
