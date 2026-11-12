// SPDX-License-Identifier: AGPL-3.0-only
//! Start at login, on every desktop OS, using each one's standard per-user
//! mechanism (no admin rights):
//!
//! - **Windows:** a `RemoteMusicRP` value in
//!   `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. It starts
//!   `remotemusicrp-desktop.exe` (no console window), or `remotemusicrp.exe`
//!   (which relaunches hidden) when the launcher is missing or a `--config`
//!   file is in use.
//! - **macOS:** a LaunchAgent, `~/Library/LaunchAgents/<LABEL>.plist`, with
//!   `RunAtLoad` (GUI sessions only, so the menu-bar icon works).
//! - **Linux desktops:** an XDG autostart entry,
//!   `$XDG_CONFIG_HOME/autostart/remotemusicrp.desktop` (default
//!   `~/.config/autostart`). AppImages register the `.AppImage` file itself,
//!   since the program inside lives in a temporary mount.
//!
//! Package (systemd) and Docker installs have their own service managers, so
//! the option isn't offered there.
//!
//! We store nothing about it ourselves: the entry is the only record. The
//! one exception is a "default applied" flag (see [`apply_default`]), so the
//! default turns it on once and never again after the owner turns it off.
//!
//! People can also turn startup apps off outside the app: Task Manager /
//! Settings → Apps → Startup on Windows (`StartupApproved\Run`, not
//! documented by Microsoft: an odd first byte means "disabled"), or
//! `Hidden=true` / `X-GNOME-Autostart-enabled=false` on Linux. We read that
//! only to show the right state; turning it on here rewrites the entry.

use serde::Serialize;

use crate::app::App;
use crate::settings;

/// Set once the on-by-default choice has been applied.
const DEFAULT_APPLIED: &str = "autostart.default_applied";

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Status {
    /// Desktop installs only (not package, Docker or headless Linux).
    pub supported: bool,
    /// "windows", "macos" or "linux": picks the wording in the UI.
    pub os: &'static str,
    /// The system will start this program at login.
    pub enabled: bool,
    /// Registered, but turned off in the system's own startup settings.
    pub blocked_by_system: bool,
    /// Registered, but for a copy elsewhere (e.g. this one was moved).
    pub other_location: bool,
}

const OS: &str = if cfg!(windows) {
    "windows"
} else if cfg!(target_os = "macos") {
    "macos"
} else {
    "linux"
};

/// A registered entry, as read back from the system.
#[derive(Debug, PartialEq, Eq)]
struct Entry {
    /// What it starts, comparable with [`sys::expected`].
    signature: String,
    blocked: bool,
}

/// Only desktop installs: package (systemd) and Docker installs have their
/// own service managers, and a Linux machine without a desktop session has
/// no login to start with.
fn supported() -> bool {
    let kind_ok = matches!(crate::updater::install_kind(), "portable" | "appimage");
    let desktop = !cfg!(target_os = "linux")
        || ["XDG_CURRENT_DESKTOP", "WAYLAND_DISPLAY", "DISPLAY"]
            .iter()
            .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()));
    kind_ok && desktop && cfg!(any(windows, target_os = "macos", target_os = "linux"))
}

pub fn status() -> Status {
    let mut s = Status {
        os: OS,
        ..Status::default()
    };
    if !supported() {
        return s;
    }
    s.supported = true;
    if let Some(entry) = sys::read() {
        s.blocked_by_system = entry.blocked;
        s.enabled = !entry.blocked;
        s.other_location = sys::expected().is_ok_and(|want| !same(&want, &entry.signature));
    }
    s
}

pub fn set(enabled: bool) -> Result<(), String> {
    if !supported() {
        return Err("starting at login isn't available for this install".into());
    }
    if enabled { sys::write() } else { sys::remove() }
}

/// Windows paths are case-insensitive.
fn same(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// On by default (owner decision): turn it on once, the first time a desktop
/// install starts (also after updating from a version without the option).
/// Later choices are the owner's: once applied, this never runs again.
/// Not for internet-exposed installs, development builds (run from a Cargo
/// `target` folder) or with `REMOTEMUSICRP_NO_AUTOSTART=1`.
pub fn apply_default(app: &App) {
    if !supported() || settings::get_bool(&app.db, DEFAULT_APPLIED) {
        return;
    }
    let opted_out = std::env::var("REMOTEMUSICRP_NO_AUTOSTART").is_ok_and(|v| v == "1");
    if opted_out || is_dev_build() || (app.cfg.is_exposed() && !app.cfg.lan_access) {
        return;
    }
    let now = status();
    if !now.enabled && !now.blocked_by_system {
        match set(true) {
            Ok(()) => {
                let _ = app.db.log(
                    remotemusicrp_storage::LogLevel::Info,
                    "settings",
                    "Start at login turned on (the default). Turn it off in Settings → Startup.",
                );
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not turn on start at login");
                return;
            }
        }
    }
    let _ = settings::set_bool(&app.db, DEFAULT_APPLIED, true);
}

/// Running from `…/target/{debug,release}/` (cargo run, local builds).
fn is_dev_build() -> bool {
    std::env::current_exe().is_ok_and(|exe| {
        let parts: Vec<_> = exe.components().map(|c| c.as_os_str().to_owned()).collect();
        parts
            .windows(2)
            .any(|w| w[0] == "target" && (w[1] == "debug" || w[1] == "release"))
    })
}

/// The program to start at login and its arguments (`--config` passed on).
#[cfg_attr(windows, allow(dead_code))]
fn launch_args() -> Result<Vec<String>, String> {
    let mut args = vec!["run".to_owned()];
    if let Some(cfg) = crate::config_path() {
        args.push("--config".into());
        args.push(path_str(&cfg)?);
    }
    Ok(args)
}

fn path_str(p: &std::path::Path) -> Result<String, String> {
    p.to_str()
        .map(str::to_owned)
        .ok_or_else(|| "the program's path isn't valid Unicode".to_owned())
}

// ---- Windows ---------------------------------------------------------------

#[cfg(windows)]
mod sys {
    use super::{Entry, path_str};
    use windows_registry::CURRENT_USER;

    /// Where the two values live (a scratch key in tests).
    pub(super) struct Keys {
        pub run: &'static str,
        pub approved: &'static str,
    }
    const WINDOWS: Keys = Keys {
        run: r"Software\Microsoft\Windows\CurrentVersion\Run",
        approved: r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
    };
    pub(super) const NAME: &str = "RemoteMusicRP";

    /// The command line Windows runs at login. Windows paths can't contain
    /// `"`, so plain quoting is safe.
    pub fn expected() -> Result<String, String> {
        let exe = std::env::current_exe().map_err(|e| format!("program path: {}", e.kind()))?;
        let quote = |p: &std::path::Path| path_str(p).map(|s| format!("\"{s}\""));
        Ok(match crate::config_path() {
            // The launcher doesn't pass arguments on; the main program
            // relaunches itself hidden when given only `--config <file>`.
            Some(cfg) => format!("{} --config {}", quote(&exe)?, quote(&cfg)?),
            None => {
                let launcher = exe
                    .parent()
                    .map(|d| d.join("remotemusicrp-desktop.exe"))
                    .filter(|p| p.is_file());
                quote(launcher.as_deref().unwrap_or(&exe))?
            }
        })
    }

    pub fn read() -> Option<Entry> {
        read_in(&WINDOWS)
    }
    pub fn write() -> Result<(), String> {
        write_in(&WINDOWS, &expected()?)
    }
    pub fn remove() -> Result<(), String> {
        remove_in(&WINDOWS)
    }

    pub(super) fn read_in(k: &Keys) -> Option<Entry> {
        let signature = CURRENT_USER.open(k.run).ok()?.get_string(NAME).ok()?;
        let blocked = CURRENT_USER
            .open(k.approved)
            .and_then(|key| key.get_value(NAME))
            .is_ok_and(|v| v.first().is_some_and(|b| b & 1 == 1));
        Some(Entry { signature, blocked })
    }

    pub(super) fn write_in(k: &Keys, cmd: &str) -> Result<(), String> {
        CURRENT_USER
            .create(k.run)
            .and_then(|key| key.set_string(NAME, cmd))
            .map_err(|e| format!("registry: {}", e.message()))?;
        // Clears a "disabled" mark from Task Manager, so turning it on here
        // really turns it on.
        remove_value(k.approved)
    }

    pub(super) fn remove_in(k: &Keys) -> Result<(), String> {
        remove_value(k.run)?;
        remove_value(k.approved)
    }

    /// Removes our value, treating "not there" as success.
    fn remove_value(path: &str) -> Result<(), String> {
        let Ok(key) = CURRENT_USER.create(path) else {
            return Ok(());
        };
        match key.remove_value(NAME) {
            Ok(()) => Ok(()),
            // HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND)
            Err(e) if e.code().0 as u32 == 0x8007_0002 => Ok(()),
            Err(e) => Err(format!("registry: {}", e.message())),
        }
    }
}

// ---- macOS and Linux: a file in the user's folder ----------------------------

/// Builders and parsers for the macOS and Linux entries. Plain functions so
/// they are tested on every platform.
#[allow(dead_code)] // each OS uses part of it
mod files {
    use std::path::Path;

    use super::Entry;

    pub const LAUNCH_AGENT_LABEL: &str = "io.github.iammarxg.remotemusicrp";

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    /// A LaunchAgent that starts the program at login in GUI sessions.
    /// Output goes nowhere (launchd never trims log files); the activity
    /// log in the web UI has what matters.
    pub fn launch_agent(program: &str, args: &[String]) -> String {
        let items: String = std::iter::once(program)
            .chain(args.iter().map(String::as_str))
            .map(|a| format!("        <string>{}</string>\n", xml_escape(a)))
            .collect();
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LAUNCH_AGENT_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
{items}    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>LimitLoadToSessionType</key>
    <string>Aqua</string>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#
        )
    }

    /// One `Exec=` argument, quoted per the Desktop Entry spec: reserved
    /// characters need double quotes, inside which `"`, `` ` ``, `$` and `\`
    /// are backslash-escaped; then the string-value escaping doubles every
    /// backslash again, and `%` (field codes) is written `%%`.
    pub fn exec_arg(arg: &str) -> String {
        const RESERVED: &str = " \t\n\"'\\><~|&;$*?#()`";
        let quoted = if arg.is_empty() || arg.chars().any(|c| RESERVED.contains(c)) {
            let mut q = String::from("\"");
            for c in arg.chars() {
                if matches!(c, '"' | '`' | '$' | '\\') {
                    q.push('\\');
                }
                q.push(c);
            }
            q.push('"');
            q
        } else {
            arg.to_owned()
        };
        quoted.replace('\\', "\\\\").replace('%', "%%")
    }

    pub fn exec_line(program: &str, args: &[String]) -> String {
        std::iter::once(program)
            .chain(args.iter().map(String::as_str))
            .map(exec_arg)
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn desktop_entry(program: &str, args: &[String]) -> String {
        // The Exec value can't contain a line break (no valid path does,
        // and `exec_arg` quotes them anyway).
        format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=RemoteMusicRP\n\
             Comment=Your Last.fm now playing as Discord Rich Presence\n\
             Exec={}\n\
             Terminal=false\n\
             NoDisplay=true\n\
             X-GNOME-Autostart-enabled=true\n",
            exec_line(program, args).replace('\n', "\\n")
        )
    }

    /// Reads back an autostart entry: its `Exec=` line, and whether the
    /// desktop turned it off.
    pub fn parse_desktop(content: &str) -> Entry {
        let mut exec = String::new();
        let mut blocked = false;
        let mut in_main = false;
        for line in content.lines().map(str::trim) {
            if line.starts_with('[') {
                in_main = line == "[Desktop Entry]";
                continue;
            }
            if !in_main {
                continue;
            }
            match line.split_once('=').map(|(k, v)| (k.trim(), v.trim())) {
                Some(("Exec", v)) => exec = v.to_owned(),
                Some(("Hidden", v)) if v.eq_ignore_ascii_case("true") => blocked = true,
                Some(("X-GNOME-Autostart-enabled", v)) if v.eq_ignore_ascii_case("false") => {
                    blocked = true;
                }
                _ => {}
            }
        }
        Entry {
            signature: exec,
            blocked,
        }
    }

    /// Writes via a temporary file and a rename, so a half-written entry
    /// never exists. Not group/world-writable (launchd refuses those).
    pub fn write_file(path: &Path, content: &str) -> Result<(), String> {
        let dir = path.parent().ok_or("no parent folder")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("create folder: {}", e.kind()))?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, content).map_err(|e| format!("write: {}", e.kind()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644));
        }
        std::fs::rename(&tmp, path).map_err(|e| format!("write: {}", e.kind()))
    }

    pub fn remove_file(path: &Path) -> Result<(), String> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("remove: {}", e.kind())),
        }
    }
}

#[cfg(not(windows))]
fn home() -> Result<std::path::PathBuf, String> {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| "HOME isn't set".to_owned())
}

#[cfg(target_os = "macos")]
mod sys {
    use super::{Entry, files, launch_args, path_str};

    fn path() -> Result<std::path::PathBuf, String> {
        Ok(super::home()?
            .join("Library/LaunchAgents")
            .join(format!("{}.plist", files::LAUNCH_AGENT_LABEL)))
    }

    /// The whole file we would write (compared as-is).
    pub fn expected() -> Result<String, String> {
        let exe = std::env::current_exe().map_err(|e| format!("program path: {}", e.kind()))?;
        Ok(files::launch_agent(&path_str(&exe)?, &launch_args()?))
    }

    pub fn read() -> Option<Entry> {
        // Turning it off in System Settings → Login Items is kept in a
        // private macOS database we can't read, so it isn't shown here.
        let signature = std::fs::read_to_string(path().ok()?).ok()?;
        Some(Entry {
            signature,
            blocked: false,
        })
    }

    pub fn write() -> Result<(), String> {
        files::write_file(&path()?, &expected()?)
    }

    pub fn remove() -> Result<(), String> {
        files::remove_file(&path()?)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod sys {
    use super::{Entry, files, launch_args, path_str};

    fn path() -> Result<std::path::PathBuf, String> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_absolute())
            .map_or_else(|| super::home().map(|h| h.join(".config")), Ok)?;
        Ok(config.join("autostart").join("remotemusicrp.desktop"))
    }

    /// The program: the `.AppImage` file for AppImages (the program inside
    /// lives in a temporary mount), otherwise this executable.
    fn program() -> Result<String, String> {
        let exe = match crate::updater::appimage_path() {
            Some(p) => p,
            None => std::env::current_exe().map_err(|e| format!("program path: {}", e.kind()))?,
        };
        path_str(&exe)
    }

    /// The `Exec=` value (compared with what's in the file).
    pub fn expected() -> Result<String, String> {
        Ok(files::exec_line(&program()?, &launch_args()?))
    }

    pub fn read() -> Option<Entry> {
        let content = std::fs::read_to_string(path().ok()?).ok()?;
        Some(files::parse_desktop(&content))
    }

    pub fn write() -> Result<(), String> {
        files::write_file(
            &path()?,
            &files::desktop_entry(&program()?, &launch_args()?),
        )
    }

    pub fn remove() -> Result<(), String> {
        files::remove_file(&path()?)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::files::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn desktop_exec_quoting() {
        assert_eq!(exec_arg("/opt/app/remotemusicrp"), "/opt/app/remotemusicrp");
        assert_eq!(exec_arg("/home/a b/x"), "\"/home/a b/x\"");
        // `$` and `"` are escaped inside quotes, then backslashes doubled.
        assert_eq!(exec_arg("/a$b"), "\"/a\\\\$b\"");
        assert_eq!(exec_arg("/a\"b"), "\"/a\\\\\"b\"");
        assert_eq!(exec_arg("/100%/x"), "/100%%/x");
        assert_eq!(
            exec_line("/apps/Remote Music.AppImage", &args(&["run"])),
            "\"/apps/Remote Music.AppImage\" run"
        );
    }

    #[test]
    fn desktop_entry_round_trip_and_blocking() {
        let file = desktop_entry("/apps/r.AppImage", &args(&["run", "--config", "/c/x.toml"]));
        let e = parse_desktop(&file);
        assert_eq!(e.signature, "/apps/r.AppImage run --config /c/x.toml");
        assert!(!e.blocked);
        assert!(parse_desktop(&file.replace("enabled=true", "enabled=false")).blocked);
        assert!(parse_desktop(&format!("{file}Hidden=true\n")).blocked);
        // Keys in other groups (e.g. desktop actions) don't count.
        assert!(!parse_desktop(&format!("{file}[Desktop Action x]\nHidden=true\n")).blocked);
    }

    #[test]
    fn launch_agent_escapes_paths() {
        let plist = launch_agent("/Apps/R&D <1>/remotemusicrp", &args(&["run"]));
        assert!(plist.contains("<string>/Apps/R&amp;D &lt;1&gt;/remotemusicrp</string>"));
        assert!(plist.contains("<string>run</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>\n    <true/>"));
        assert!(plist.contains(LAUNCH_AGENT_LABEL));
    }

    #[test]
    fn file_write_and_remove() {
        let dir = std::env::temp_dir().join(format!("rmrp-autostart-{}", std::process::id()));
        let path = dir.join("autostart").join("remotemusicrp.desktop");
        write_file(&path, "x").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "x");
        write_file(&path, "y").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "y");
        remove_file(&path).unwrap();
        remove_file(&path).unwrap();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn windows_registry_on_off_blocked_and_moved() {
        use super::sys::*;
        use windows_registry::CURRENT_USER;

        // Same logic against a scratch key (not a startup key), so tests
        // never make anything start at login.
        const ROOT: &str = r"Software\RemoteMusicRP-test-autostart";
        const TEST: Keys = Keys {
            run: r"Software\RemoteMusicRP-test-autostart\Run",
            approved: r"Software\RemoteMusicRP-test-autostart\Approved",
        };
        let _ = CURRENT_USER.remove_tree(ROOT);
        assert!(read_in(&TEST).is_none());
        remove_in(&TEST).unwrap();

        let cmd = expected().unwrap();
        assert!(cmd.starts_with('"') && cmd.contains(".exe\""), "{cmd}");
        write_in(&TEST, &cmd).unwrap();
        let e = read_in(&TEST).unwrap();
        assert_eq!(e.signature, cmd);
        assert!(!e.blocked);

        // Turned off in Task Manager: odd first byte.
        let approved = CURRENT_USER.create(TEST.approved).unwrap();
        approved
            .set_bytes(
                NAME,
                windows_registry::Type::Bytes,
                &[3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            )
            .unwrap();
        assert!(read_in(&TEST).unwrap().blocked);
        // Turning it on here clears that mark.
        write_in(&TEST, &cmd).unwrap();
        assert!(!read_in(&TEST).unwrap().blocked);
        assert!(approved.get_value(NAME).is_err());

        remove_in(&TEST).unwrap();
        assert!(read_in(&TEST).is_none());
        let _ = CURRENT_USER.remove_tree(ROOT);
    }
}
