// SPDX-License-Identifier: AGPL-3.0-only
//! Shared tray helpers for every platform.

use crate::app::App;

/// Whether to show a tray: desktop installs (localhost or LAN access), unless
/// disabled with `--no-tray` / `REMOTEMUSICRP_NO_TRAY=1`. Never for package
/// or Docker installs (those are services without a desktop).
pub fn wanted(app: &App, args: &[String]) -> bool {
    // LAN access is still a desktop install; internet exposure isn't.
    (!app.cfg.is_exposed() || app.cfg.lan_access)
        && !args.iter().any(|a| a == "--no-tray")
        && std::env::var("REMOTEMUSICRP_NO_TRAY")
            .map(|v| v != "1")
            .unwrap_or(true)
        && !matches!(crate::updater::install_kind(), "package" | "docker")
}

/// The local web UI address the tray opens.
pub fn ui_url(app: &App) -> String {
    format!("http://127.0.0.1:{}", app.cfg.bind.port())
}

/// Open `url` in the default browser. `url` is always our own
/// `http://127.0.0.1:<port>` address, never user input.
pub fn open_url(url: &str) {
    #[cfg(windows)]
    let r = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match r {
        // Reap the helper so it doesn't linger as a zombie process.
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => tracing::warn!(error = %e.kind(), "could not open the browser"),
    }
}
