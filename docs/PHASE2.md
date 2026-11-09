# Phase 2 notes (2026-11-09)

## What exists

| Area | Status |
|---|---|
| Web server (axum, single-threaded tokio) | Done. Embedded UI, JSON API, `/healthz`. |
| Hardening | Done: strict CSP (`script-src 'self'`, no inline scripts or styles, no third-party origins except album-art image hosts), Host allow-list (DNS rebinding), Origin check on every state change, per-session CSRF tokens, 16 KiB body limit, `no-store` on the API, HSTS behind HTTPS. |
| Sessions | 1 h idle / 12 h total, or 30 days with "remember me". The cookie is `HttpOnly`, `SameSite=Strict`, and `__Host-` + `Secure` behind HTTPS. Only SHA-256 token hashes are stored. Sessions rotate at login and password change, can be revoked from the Security page, and are capped at 200 anonymous rows. |
| Auth | Argon2id password (64 MiB, t=3, at most 2 hashes at once, so floods can't exhaust memory). Per-IP lockout after 5 failures: 1 min, doubling to 1 h. TOTP (RFC 6238) with replay protection. 10 single-use recovery codes. CLI `reset-password` for recovery. |
| First run | Wizard: password (skippable on localhost only), then Discord Application ID, then Last.fm username and key (validated live against Last.fm). An exposed install needs a one-time **setup code** printed to the server log before anyone can set the first password. |
| Passphrase mode | The UI shows an unlock screen. The passphrase is rate-limited like logins. |
| Presence service | A background task: settings are applied live, credential changes restart it, and its status feeds the dashboard. |
| Pages | Dashboard (live now-playing with blurred album-art backdrop and the exact Discord preview), Presence (toggles, templates, buttons, live preview), Connections, Settings (instance config, appearance, updates), Security (password, 2FA with QR, recovery codes, devices, key rotation, delete everything), Activity log, About. |
| Design | Monochrome black and white (owner request). Dark, light or system theme, remembered per browser. Inter/system font, line icons, no external assets. |
| i18n | All strings in `web/src/locales/en.json`. RTL switches automatically (logical CSS properties). |
| Tray | Windows/macOS: Open, Pause/Resume, Quit. Disable with `--no-tray` or `REMOTEMUSICRP_NO_TRAY=1`. Never shown on exposed installs. |
| Auto-updater | Daily check of GitHub Releases. Prompted by default; auto-install is opt-in. Verifies the minisign signature (with a trusted comment pinning the version and target) **and** a signed SHA256SUMS. Atomic self-replace, `<exe>.old` backup, automatic rollback after 3 failed starts, and a CLI `rollback-update`. Package/Docker builds only notify. |
| Rename | ScrobbleAway → RemoteMusicRP, with automatic migration of the data folder, database file and OS-keystore entry (verified on the owner's install). |

## Presence customisation (added 2026-11-09)

A live probe against the owner's Discord (local RPC, the owner's app) showed that Discord **accepts and stores**:
- a custom `name`
- `details_url`, `state_url` and `assets.large_url`
- a custom button
- `status_display_type`
- activity types 0, 2, 3 and 5

Settings (`PresenceSettings`, JSON in the DB; older saved settings still load):
- activity type
- name source (app / artist / title / album / custom)
- member-list line (name / first / second)
- show toggles
- layout (one line, swap, "by" and "on" prefixes) or custom templates
- link toggles
- song-link button, custom button, "Get this status"
- missing-art placeholder

The UI previews both the member-list line and the profile card live. Not yet confirmed by eye: that Discord *displays* the custom name. The probe only proves that Discord accepted it.

## Measured (Windows 11, release build, 2026-11-09)

- Binary: **5.1 MB**, with the UI, web server, tray and updater included.
- UI bundle: about 45 KB of JS and 4 KB of CSS, gzipped.
- Running (presence playing, Discord connected, tray, web server idle): **19.2 MB working set, 5.0 MB private memory**, 11 threads.
- Tests: 110 passing. The API security smoke test passed 28 of 28 checks: Host/Origin/CSRF enforcement, session rotation, lockout, validation and body limit.

## Not done / follow-ups

- **Update signing key:** the owner must create a minisign key pair (Phase 4). Until then `can_install` is false and the UI says updates must be downloaded manually.
- **Linux tray:** planned via the KSNI backend (no GTK).
- **Windows console window:** the exe is a console app, so a console window opens next to the tray. Phase 4 packaging should ship a GUI-subsystem build, or a launcher, for the desktop installer.
- The tray icon was not checked visually by the agent; the owner should confirm it appears.
- ~~Inter only if installed~~ Inter 4.1 is bundled since 2026-11-10 (owner OK).
