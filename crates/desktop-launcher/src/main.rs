// SPDX-License-Identifier: AGPL-3.0-only
//! Windows desktop launcher.
//!
//! `remotemusicrp.exe` is a console program so its admin commands
//! (`status`, `reset-password`, …) print in a terminal. Double-clicking it
//! would also open a console window next to the tray icon. This tiny
//! launcher is a windowed (GUI-subsystem) program: it starts
//! `remotemusicrp.exe run` from its own folder with no console window,
//! sends that program's output to a log file, and exits.
//!
//! - No `unsafe`: it only uses `std`'s `CREATE_NO_WINDOW` process flag.
//! - The updater replaces only `remotemusicrp.exe`; a restarted copy
//!   inherits the hidden console, so no window appears after an update.
//! - Log: `%LOCALAPPDATA%\RemoteMusicRP\desktop.log`, replaced on each
//!   start. It holds only what the app prints, which never includes secrets.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, ExitCode, Stdio};

    /// `CREATE_NO_WINDOW` from the Win32 process creation flags.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let Some(exe) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("remotemusicrp.exe")))
    else {
        return ExitCode::FAILURE;
    };

    let log = std::env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map(|d| std::path::PathBuf::from(d).join("RemoteMusicRP"))
        .and_then(|dir| {
            std::fs::create_dir_all(&dir).ok()?;
            std::fs::File::create(dir.join("desktop.log")).ok()
        });
    let (out, err) = match log.and_then(|f| Some((f.try_clone().ok()?, f))) {
        Some((a, b)) => (Stdio::from(a), Stdio::from(b)),
        None => (Stdio::null(), Stdio::null()),
    };

    match Command::new(exe)
        .arg("run")
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    {
        Ok(_) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("remotemusicrp-desktop is the Windows launcher; run `remotemusicrp run` instead.");
}
