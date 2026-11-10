# RemoteMusicRP: project instructions

> This is the single source of truth for every agent and contributor. `AGENTS.md` only points here.
> Keep this file accurate. Whenever a decision changes, update the relevant section **and** add an entry to the Decisions log at the bottom.
> Details live in `docs/` (`docs/PHASE0.md`, `docs/SECURITY.md`, `docs/DATA.md`).

**Current status (2026-11-09):** Phase 1 is complete (see `docs/PHASE1.md`). **Phase 2 is functionally complete** (see `docs/PHASE2.md`). **Phase 3 started, features first** (owner choice): blocklist, quiet hours, activity stats, email alerts and export/import are built. **Server mode (Social SDK) is built** for one Discord account per instance and passed a live test in WSL (sign-in, presence, restart from the stored token, clean stop). **Phase 4 started:** release and Docker workflows, packaging (zip, tarball, AppImage, deb, rpm, PKGBUILD, Docker), Windows no-console launcher and install docs are written (see `docs/INSTALL.md`); the first real release run is pending. **Renamed to RemoteMusicRP** (2026-11-10). Linux tray icon and the bundled Inter font are done. i18n/Weblate later (owner choice).

---

## 1. What we're building

RemoteMusicRP is a single, free, open-source (**AGPL-3.0**), self-hosted app. It reads a user's "now playing" track from a scrobbling service and shows it as Discord Rich Presence. Last.fm comes first. Other services (for example ListenBrainz) come later, and only after they are verified to be possible.

Example: someone listens on their phone with any app that scrobbles to Last.fm, and the track appears on their Discord profile.

- There is **no** paid tier, **no** hosted service run by the maintainer, **no** platform accounts and **no** payments.
- Every user runs their own instance: a desktop exe, or Docker at home or on their own VPS.
- Every user supplies their **own** Discord developer app, their **own** Last.fm API key, and optionally their **own** email-sending credentials.
- The maintainer never sees anyone's credentials.

> **Phase 0 caveat:** the "no Discord client needed on a server" use case depends on the Social SDK test in Phase 1. See §4 and `docs/PHASE0.md`.

## 2. Absolute rules

- **Security is priority #1**, above features and convenience. **Resource use is priority #2**: very low RAM, CPU and network use.
- **Discord ToS compliance is mandatory.** Use only official mechanisms:
  - Discord local RPC (desktop client IPC).
  - Official OAuth2 / Social SDK paths that Discord documents for third-party apps.
  - **Never** use user tokens, self-bot techniques, gateway presence with user tokens, or undocumented endpoints. This includes `POST /users/@me/headless-sessions`; see §4.
  - If a feature cannot be built compliantly, drop it and tell the owner.
- **Last.fm API terms:** each user uses their own key, for personal and non-commercial use only.
  - Stay far below rate limits. No numeric limit is published, and Last.fm warns against "several calls per second". Our cap is ≤1 request/s per instance.
  - Send an identifiable User-Agent.
  - Cache according to the HTTP headers.
  - Keep stored Last.fm data under 100 MB.
  - Show the attribution: "powered by AudioScrobbler" plus links back to Last.fm pages (ToS 2.7 and 4.2.2).
- **Security-vs-functionality protocol:** whenever a decision trades security against functionality (session lifetimes, key storage, update behaviour, internet exposure, etc.), **STOP and ask the owner**. Give 2–3 options, your recommendation and the tradeoff. Never silently pick the weaker option.
- No artificial delays. Use constant-time comparisons and tuned hashing instead.
- **Store only what is necessary:** encrypted tokens/sessions, the admin password hash, and settings.
- **No telemetry.** The only outbound calls allowed are to:
  - Discord
  - the scrobbling provider(s)
  - the artwork lookup providers: the iTunes Search API (`itunes.apple.com`) and the MusicBrainz Cover Art Archive (`coverartarchive.org`, `musicbrainz.org`)
  - the user's chosen email provider
  - the update check (GitHub Releases)
- **No scraping of any website** and no borrowing of other services' tokens. Use only documented public APIs, under their terms.
- **Never commit secrets.** Only `.env.example`, with placeholders.
- Be honest about uncertainty. If something cannot be verified, say so instead of guessing.

## 3. Tech and architecture

- **Rust** for the backend and core. The Cargo workspace has these crates:
  - `core`: domain logic, presence building, templates, schedules, blocklist.
  - `crypto`: envelope encryption, key management, password and TOTP hashing helpers.
  - `providers`: a trait-based scrobble source. `LastFm` is implemented; others sit behind the same trait. This crate also holds an `ArtworkSource` trait (`AppleSearch`, `CoverArtArchive`).
  - `discord`: a `PresenceSink` trait with the `LocalRpc` mode, plus a future server mode if approved (§4).
  - `storage`: SQLite (WAL) behind a small API. It holds settings, envelope-encrypted secrets, the activity log and caches.
  - `app`: the binary. Modules: `service` (presence loop), `web` (axum API, sessions, auth, embedded UI), `updater`, `tray` (Windows/macOS), `tray_linux` (StatusNotifierItem via `ksni`), plus a small admin CLI.
- `web/`: Svelte 5 + Vite UI, built to `web/dist` and embedded with `rust-embed`.
- Use `#![forbid(unsafe_code)]` in every crate. **The one exception (signed off by the owner on 2026-11-09):** a separate *optional* crate (`discord-sdk-ffi`) may contain the minimal, audited FFI to the Social SDK's C API. It is feature-gated, off by default, loads the user-supplied library at runtime, and every `unsafe` block carries a `// SAFETY:` comment.
- Secrets use `secrecy` and `zeroize`.
- Keep dependencies minimal and well-maintained, with pinned versions (`Cargo.lock` committed, exact versions where it matters).
- Run `cargo deny check all` in CI. Its `advisories` check uses the same RustSec database as `cargo audit`. Licenses, bans (no OpenSSL) and sources are enforced in `deny.toml`.
- TLS: rustls with the `ring` provider and the OS trust store. No OpenSSL anywhere.
- The toolchain is pinned in `rust-toolchain.toml` (1.99.1, the stable release at Phase 1 start). Edition 2024.
- **Database:** SQLite, a single file in WAL mode. Keep a storage trait clean enough that Postgres could be added later. **Do not build Postgres now.**
- **Web UI:** Svelte 5 + Vite (single-page app, no SvelteKit), so the HTML needs no inline bootstrap script and the CSP stays strict.
  - Built at release time and embedded in the Rust binary (e.g. `rust-embed`).
  - **No external CDN or third-party assets at runtime.** Fonts and icons are self-hosted.
  - It must feel very polished: smooth transitions, responsive layout, and good empty, loading and error states. It must also stay light.
  - Pages:
    - Setup wizard: Connect Discord, then Connect Last.fm, with step-by-step guides for creating a Discord app and a Last.fm API key.
    - Dashboard with a live presence preview.
    - Presence customisation.
    - Hide & schedule (blocklist and quiet hours).
    - Connections.
    - Settings.
    - Security.
    - Activity log.
    - About, with a GitHub link.
- **Instance access:** **one owner per instance** (no multi-user).
  - Bind to `127.0.0.1` by default.
  - The admin password is hashed with Argon2id. It is optional only on a localhost-only desktop install and **required** whenever the UI is reachable from another machine.
  - Optional TOTP 2FA with recovery codes. **No passkeys** (owner decision).
  - **Local password:** on a localhost-only install the wizard recommends setting a password but allows skipping it. Exposed installs always require one.
  - **Sessions:** a default session ends after 1 h idle or 12 h total. "Remember me" at login extends it to 30 days. Session tokens are stored only as hashes, can be revoked from the Security page, and are rotated at login.
  - **Brute force:** after 5 failed logins from one IP, that IP is locked for 1 min, doubling up to 1 h. Failures are logged. There is no permanent lockout.
  - **TLS:** reverse proxy only; there is no built-in TLS. A non-loopback bind is refused unless the config declares the TLS proxy (`public_origin = "https://…"`) or `lan_access = true`. Example Caddy and nginx configs ship with the app.
  - **LAN access (owner decision 2026-11-10, plain HTTP):** `lan_access = true` / `REMOTEMUSICRP_LAN_ACCESS=1` listens on all interfaces (same port) and answers only clients with private/link-local addresses, with a Host that is a private IP or a local name (`.local`, `.lan`, `.home.arpa`, `.internal`) and a same-origin `Origin` on writes. Password required. Not combinable with `public_origin`. Cookies are not `Secure` (HTTP). Not encrypted: documented and shown in the log and Settings.
  - **First password on a reachable install:** set on the computer itself (loopback client, real IP via XFF behind the proxy) with no code; other devices see "Finish setup on the other computer" and switch to the login once a password exists; the one-time setup code from the log remains for headless servers.
  - **Host and Origin checks** on every request, to block DNS rebinding and cross-site requests against the localhost UI.
- **Polling:** the default interval is 5 s and the user can change it. The enforced minimum is 3 s.
  - **Smart fast polling:** every 2 s from 10 s before the current track's expected end until 30 s after it (or until the next track appears). This catches track changes within about 2 s for a few extra calls per song.
  - Exponential backoff with jitter on errors.
  - Slower polling when idle (no track playing for N minutes).
  - Use conditional requests where the API supports them.
- Resource targets (verify and report each phase):
  - Idle RAM in the low tens of MB.
  - Near-zero idle CPU.
  - A small binary.

## 4. Discord modes (updated by Phase 0)

| Mode | Status | Notes |
|---|---|---|
| **Local RPC** (desktop client IPC) | ✅ **Default and only confirmed mode** | Officially supported. The Social SDK docs say Rich Presence works without auth when talking to a running client. Stores **zero** Discord secrets. Works only for the account logged into that client. Uses the user's own Discord application ID. |
| **HTTP "headless sessions"** | ❌ **Dropped** | `POST /users/@me/headless-sessions` appears only in unofficial community docs. It needs the `activities.write` scope, which Discord's official OAuth2 docs list as "not currently available for apps". Using it would not be compliant. |
| **Official Social SDK, headless** (server/Docker, multi-account) | 🧪 **Approved by owner as opt-in EXPERIMENTAL** (2026-11-09) | Hands-on test passed: presence set from WSL Ubuntu with no Discord client and no display (`docs/PHASE1.md`). Known risks, shown to users in the UI and docs: headless use is not described in Discord's documentation (SDK Terms §2.b(iii)(A)); Linux x86_64 only (no arm64); needs ALSA/PulseAudio/X11 client libraries; Public Client must be on; proprietary binary that **each user downloads themselves** (never bundled or redistributed by us, §2.a/§2.b(ii)). If Discord objects or breaks it, it gets removed. |

- **Server mode (built 2026-11-09):** `[discord] mode = "social-sdk"` + `sdk_library = "/abs/path/libdiscord_partner_sdk.so"` (or `REMOTEMUSICRP_DISCORD_MODE` / `REMOTEMUSICRP_DISCORD_SDK_LIBRARY`). Only in builds with the cargo feature `social-sdk`. The library path comes from config only (never the web UI), must be absolute, a regular file, and not group/world-writable. **One Discord account and one Last.fm account per instance** (owner decision; no multi-account). Sign-in: PKCE in the owner's browser, redirect to `<UI origin>/discord-callback` (shown on the Connections page to add in the portal), single-use `state` bound to the session, 10 min; paste-the-address fallback. Only the refresh token is stored (encrypted); access tokens stay in memory and are renewed a day before expiry; sign-out revokes the token. A failed refresh logs an error and sends a "discord" email alert. The SDK thread is stopped explicitly before the process exits (the SDK aborts with `Check failed: !shutdownCompleted` if its exit-time teardown runs while another thread is still calling it). **Live test passed 2026-11-09** (WSL Ubuntu, no Discord client): browser sign-in, now playing sent, restart reconnects from the stored refresh token, clean stop. The preview's name and avatar come from the SDK's cached current user (`GetCurrentUserV2`, no extra network call), validated like the local RPC READY user and kept in memory only. Measured on Linux: ~20 MB RSS (≈5 MB private heap; ≈15 MB is the mapped binary and SDK library), no measurable CPU when idle, 8 threads; binary 6.8 MB.
- The wizard detects the environment and picks the mode, explaining why. Today that means it always picks Local RPC. On a server without the Social SDK mode, the wizard explains that a Discord desktop client is required.
- Multiple Discord accounts depend on the Social SDK mode, so they are conditional too.
- Activity type: **Listening (2)**, which local RPC SET_ACTIVITY accepts.
- Unverified details to test live in Phase 1: buttons over local RPC, `status_display_type`, and Flatpak/Snap socket subpaths. Unix IPC path resolution: the first set of `XDG_RUNTIME_DIR`, `TMPDIR`, `TMP`, `TEMP`, else `/tmp`, then `discord-ipc-0..9`. We also try `app/com.discordapp.Discord/` and `snap.discord/` subdirectories (unverified).

## 5. Security design (multiple layers, no unnecessary latency)

Full threat model: `docs/SECURITY.md`. Data inventory: `docs/DATA.md`.

- **Encryption at rest (envelope encryption):**
  - Per-record data keys use XChaCha20-Poly1305, with AAD binding table, record id, purpose and key version.
  - Data keys are wrapped by a master key that has a key ID and version.
  - A background re-wrap job supports frequent automated key rotation.
  - The master key is **never** stored in the database or next to it.
- **Master key storage (decided):**
  - Desktop (Windows/macOS/Linux): the OS keystore (Windows Credential Manager/DPAPI, macOS Keychain, Linux Secret Service).
  - Docker: a Docker secret, or a `0600` file on a **separate** volume from the data volume.
  - deb/rpm/Arch server installs: a `0600` file in its own state folder outside the data directory (`/var/lib/remotemusicrp/keys`), so automatic key rotation keeps working. systemd-creds/TPM was considered and declined for now (owner decision 2026-11-10).
  - **Optional passphrase mode on every install type:** the key is derived with Argon2id and exists only in memory, so the app needs the passphrase after each restart.
- **Web hardening:**
  - TLS when exposed, and HSTS.
  - A strict CSP with no inline scripts and no third-party origins.
  - Cookies are `Secure`, `HttpOnly` and `SameSite=Strict`.
  - CSRF tokens on all state-changing requests.
  - Login rate limiting and lockout.
  - Strict input validation.
  - No secrets in logs or error messages.
  - Full security headers: `X-Content-Type-Options`, `Referrer-Policy`, `Permissions-Policy`, `frame-ancestors 'none'`, COOP/CORP.
  - When the bind address is not loopback, check the `Host` header.
- **Containers:** distroless or minimal base, non-root user, read-only root filesystem, all capabilities dropped, `no-new-privileges`, an SBOM, and images signed with cosign.
- **Supply chain:**
  - GitHub Actions pinned by commit SHA, with least-privilege `GITHUB_TOKEN`.
  - Releases signed with minisign/ed25519; the public key is embedded in the binary.
  - Checksums published, and reproducible builds where practical.
- **Data control:**
  - Disconnect Discord or Last.fm individually.
  - "Delete everything" securely wipes all data and keys, including the keystore entry.

## 6. Feature list (all in the one free version)

- Last.fm now-playing to Rich Presence: title, artist, album and album art.
- **Album art source** (Last.fm artwork is not used, because ToS 5.1.8 excludes it from the licence):
  1. **Apple first:** look the track up via the free, keyless iTunes Search API (`/search`, `entity=song`) and use `artworkUrl100`.
     - We ask for 1024×1024 by changing the size segment of the URL. This is undocumented, so fall back to the original URL if it fails.
     - Respect the documented limit of about 20 calls/min: cache per track and never look up the same track twice.
  2. **Fallback:** MusicBrainz Cover Art Archive, in this order: the release MBID from Last.fm; the **album's cover** (MusicBrainz release group found by album + artist name, then `coverartarchive.org/release-group/<id>`); releases containing the recording. CAA returns `http://` links, which are upgraded to `https://`. MusicBrainz allows 1 request/s and needs a User-Agent.
  3. Each lookup is retried once (1.5 s) on temporary errors. If nothing is found, a second pass uses the **simplified** track: bare title (before " - ", no brackets), main artist (no "feat."), bare album. Last resort: Apple album search for the album cover.
  4. Otherwise show the user's placeholder image (if set) or no image.
  - Discord fetches the image URL itself. We store only the URL and match result, never image bytes.
  - Optional "Listen on Apple Music" button using the result's `trackViewUrl`. Discord allows max 2 buttons, so the user picks which link button (Last.fm or Apple Music) sits next to "Get this status".
  - The source order and each source can be configured.
  - The Search API terms frame album art as promotional, shown near a store link. This is a grey area for a Discord status, accepted by the owner (2026-11-09). Re-check the terms before each release.
- **Animated album art: skipped for now** (owner decision, 2026-11-09). It is only available through the paid Apple Music API, scraping is forbidden, and showing it would need a publicly exposed URL. Revisit only if Apple or Discord offers a free, official path.
- One Discord account and one Last.fm account per instance. **Multiple Discord accounts: dropped** (owner decision 2026-11-09), so per-account templates are dropped too.
- Presence customisation (done in Phase 2, modelled on Music Presence):
  - Activity type: Listening / Playing / Watching / Competing.
  - Text after "Listening to": the Discord app's name, artist, title, album, or custom text with variables. Uses the activity `name` field, which local RPC accepts (verified 2026-11-09).
  - Member list / DM line: the "Listening to" text, the first line (title) or the second line (artist), via `status_display_type`. A chosen line that starts with "by " or "on " is shown as "Listening to <artist/album>" instead (the unprefixed text goes into the activity `name`, shown with `status_display_type` = Name), so the member list never reads "Listening to by Artist". The profile keeps its "by" line; its header then names the artist instead of the app.
  - Song info toggles (title, artist, album, art, progress bar) and layout options: one line, swap title/artist, "by" and "on" prefixes. Optionally write your own templates with `{track}`, `{artist}`, `{album}` for the first line, second line and album line.
  - Clickable title/artist/album that open the Last.fm pages (`details_url`, `state_url`, `large_url`).
  - Buttons: song link (Last.fm or Apple Music), a custom button, and "Get this status" (max 2 shown, in that order).
  - Placeholder image when no album art is found (custom https URL).
  - **Hide words:** a user comma-separated list (e.g. "live, remastered"). " - " sections and (bracketed) parts of the title and album that contain a word are hidden, and leftover separators are tidied. Matching is whole-word; the song's own name (first section) is never cut.
  - ~~Per-Discord-account templates~~ dropped with multi-account.
- Buttons:
  - A Last.fm profile or track link.
  - A "Get this status" button pointing to `https://github.com/iammarxg/RemoteMusicRP`. The user can turn it off; it is **on by default**.
  - **No fallback:** if a mode doesn't support buttons, hide or disable the option and explain why.
- An estimated progress bar from the track length, using start/end timestamps and cached `track.getInfo` duration (in ms).
- A "Last listened" state after the music stops, and a pause toggle.
- An artist/track blocklist that hides matches from the presence (done, "Hide & schedule" page). One entry per line; artists match the full or main artist (feat. ignored); songs match "Title" or "Artist - Title", including live/remastered variants. Case-insensitive. Last.fm keeps being polled so the next song shows.
- Quiet hours (done): weekly periods (days + from/until, overnight allowed, start = end means all day) in the computer's or a chosen IANA time zone (`jiff`). Last.fm is polled every 30 s while quiet.
- Activity log and uptime stats (done): level filter; uptime, songs since start (memory only), warnings/errors in the last 24 h. No listening history is stored.
- Optional email alerts when a connection needs re-auth (done), using the user's own Resend API key or SMTP (`lettre`, rustls + ring, OS trust store). TLS is mandatory (TLS or STARTTLS). At most one alert per problem per day; a test-email button. Triggers: Last.fm rejecting the key or username, and (server mode) a failed Discord token renewal.
- Settings export/import (done, Settings → Backup; login data is never exported, and a full export requires the admin password when one is set):
  - **The default is a full export including secrets**, encrypted with a passphrase the user chooses (Argon2id + XChaCha20-Poly1305).
  - **Optional settings-only export** that never contains secrets.
- A health endpoint and an optional local metrics endpoint.
- A system tray icon on desktop installs: Windows and macOS (tray-icon + winit) and Linux (StatusNotifierItem over D-Bus via `ksni`; KDE, GNOME with the AppIndicator extension, Cinnamon, XFCE…). Menu: Open, Pause/Resume, Quit. Never on exposed, package or Docker installs. On Linux without a tray host the web UI opens in the browser instead.
- The web UI bundles the Inter 4.1 variable font (SIL OFL 1.1; `web/public/fonts/`, licence text shipped beside it and in THIRD-PARTY-LICENSES.html). Downloaded from the official GitHub release `Inter-4.1.zip` (SHA-256 `9883fdd4…b11e`; `InterVariable.woff2` SHA-256 `693b77d4…a8e3`).
- A GitHub link in the web UI.
- The auto-updater and i18n (see §7).
- **Online status (server mode only, 2026-11-09):** the previews show the account's online status (online / idle / do not disturb / invisible) as a monochrome badge on the avatar, read from the SDK's cached current user (`Discord_UserHandle_Status`, re-read every 5 s, memory only). Connections → Discord has a picker that changes it with the documented `Discord_Client_SetOnlineStatus` (CSRF-protected `POST /api/discord/sdk/online-status`, logged). This changes the status on Discord itself, so other devices see it too; it is never stored or re-applied by us. Verified live: dnd → idle → dnd. Local RPC mode cannot do either, so nothing is shown there. Mobile/console indicators remain dropped.

## 7. Updates and i18n

- **Auto-updater** (desktop exe and generic tarball only):
  - Signed releases (minisign/ed25519) with the public key embedded.
  - Verify the signature **and** the checksum before replacing anything.
  - HTTPS only.
  - Atomic replace with rollback.
  - Check about once a day. The user can disable it.
  - **Prompted by default.** Notify the user and install on one click; security releases are highlighted. Silent auto-install is **opt-in**.
  - Disabled for package-manager installs (deb/rpm/Arch) and Docker; the UI only shows "update available".
- **i18n:**
  - All UI strings are externalised from day one, with RTL/Arabic support ready (logical CSS properties, `dir` handling).
  - Community translation runs on **Hosted Weblate (Libre plan)**. Request it in Phase 3 and re-check eligibility then.

## 8. Build, release and distribution

- Only support platforms that still get vendor security updates. Re-check the matrix before each release.
- GitHub Actions builds:
  - Windows 11 x64.
  - macOS Apple Silicon.
  - Linux x86_64 and aarch64: `.deb`, `.rpm`, an Arch `PKGBUILD` and a generic tarball.
- **No 32-bit builds** (owner decision): no armhf and no i386.
- Docker: multi-arch **amd64 + arm64**, published on Docker Hub under `ammarx4`.
- A local script builds the Windows 11 x64 exe on the owner's machine for testing.
- Licensing: AGPL-3.0, with a LICENSE file, SPDX headers and a third-party license report (e.g. `cargo about`).
- CONTRIBUTING.md: **no DCO sign-off is required** (owner decision).

### Platform support matrix (as of 2026-11-09; sources in `docs/PHASE0.md`)

| Platform | Minimum supported |
|---|---|
| Windows | Windows 11 x64, versions still serviced: Home/Pro 25H2 and later (26H1 on new devices only); Enterprise 24H2 and later |
| macOS | macOS 15 Sequoia and later, Apple Silicon only (15, 26, 27 get security updates) |
| Linux (glibc) | Debian 12 LTS / 13, Ubuntu 22.04 / 24.04 / 26.04 / 26.10, Fedora 44 / 45, RHEL 8 / 9 / 10. Target build floor: **glibc 2.28** (RHEL 8). For the server tarball, also consider a fully static musl build (decide in Phase 4). |
| Docker | linux/amd64 and linux/arm64 |

## 9. Roadmap

- **Phase 0 (done):** verification spike. See `docs/PHASE0.md`.
- **Phase 1** (needs owner approval):
  1. **Social SDK headless test.** Run a short, time-boxed test on Linux x64 with no Discord client, plus a review of the Social SDK Terms. Report the go/no-go to the owner.
  2. Live local-RPC checks: buttons, `status_display_type`, external image URLs in `large_image`, and Flatpak/Snap paths.
  3. Cargo workspace, LICENSE, CONTRIBUTING.md, CI skeleton (fmt, clippy, test, deny).
  4. The `crypto` crate.
  5. The provider trait and Last.fm.
  6. Discord local RPC.
  7. SQLite storage and config.
- **Phase 2:** web UI and wizard, auth + TOTP, presence customisation, tray, auto-updater.
- **Phase 3:** experimental Social SDK server mode (opt-in; `discord-sdk-ffi` crate; device or PKCE sign-in from the web UI; encrypted refresh token; multi-account), then the remaining features (templates, buttons, schedules, activity log, email alerts, export/import), then i18n and Weblate setup.
- **Phase 4:** packaging, the GitHub Actions release pipeline, Docker images, signing and docs.

## 10. Working rules

- Testing:
  - Unit-test crypto, auth, config parsing and provider parsing thoroughly.
  - Elsewhere, use mocks plus a small smoke test.
  - No exhaustive UI tests. Keep CI fast.
- Keep commits small. Only commit or push when the owner asks.
- Update this file and the Decisions log whenever something changes.
- Be honest about uncertainty.
- Never commit secrets.

## 11. Open items

1. **Last.fm ToS 2.7:** public access to pages that use Last.fm web services needs Last.fm's prior written approval. Does a Discord presence count as such a page? This has not been resolved. The owner must decide (e.g. contact partners@last.fm).
2. ~~Last.fm ToS 5.1.8 (artwork excluded)~~ **Resolved 2026-11-09:** art comes from Apple (iTunes Search API), then the Cover Art Archive, not Last.fm. Still to verify live in Phase 1: Discord shows external image URLs over **local RPC** (documented for the gateway Activity object only), and the 1024px Apple URL trick works.
3. ~~Social SDK go/no-go~~ **Decided 2026-11-09:** the test passed; the owner approved building it as opt-in experimental despite the documentation/terms risk.
4. ~~Local RPC rendering~~ **Verified 2026-11-09** on Windows: buttons, external image URLs, progress bar and the Listening type all work. Flatpak/Snap paths are still untested.
5. ListenBrainz: the live response shape and its API terms are unverified (only relevant when a second provider is added).
8. ~~Update signing key~~ **Done 2026-11-09:** public key `RWQ0Snkh7EvjhsxdofSMvsiHaYLRSCY6wUHMQsrPAnNSQ2tXjnOqk+Ia` (key ID 86E34BEC21794A34) is committed as `remotemusicrp.pub` and embedded at release build time. The private key lives in the `release` GitHub environment secrets (owner decision); the owner keeps an offline backup.
9. ~~Linux tray (KSNI backend)~~ **Done 2026-11-10** (tested against a stand-in tray host on a private D-Bus; not yet on a real KDE/GNOME desktop). ~~Windows GUI-subsystem build~~ **Done 2026-11-09:** `remotemusicrp-desktop.exe` launcher.
10. ~~systemd-creds/TPM for package installs~~ **Decided 2026-11-10:** keep the 0600 key file (rotation keeps working). Revisit only if TPM binding becomes possible without losing rotation.
6. `deny.toml` ignores three "unmaintained" (not vulnerable) zbus 5 sub-crates that come in through `keyring` 4.2's Linux Secret Service store. Remove the ignores when keyring moves to zbus 6; re-review by 2027-02-01.
7. ~~LICENSE file~~ **Done 2026-11-09:** AGPL-3.0 text from gnu.org (SHA-256 `0d96a4ff…079abcb0`).

## 12. Decisions log

| Date | Decision | Reason / notes |
|---|---|---|
| 2026-11-09 | Project name: **ScrobbleAway** (superseded the same day, see below) | Owner choice; avoids Discord/Last.fm trademarks |
| 2026-11-09 | Repo and "Get this status" URL: `https://github.com/iammarxg/RemoteMusicRP` | Owner choice; no separate website yet |
| 2026-11-09 | HTTP headless-sessions mode **dropped** | Needs `activities.write`, officially "not currently available for apps"; documented only unofficially |
| 2026-11-09 | Local RPC first; Social SDK headless mode conditional on a Phase 1 test and a terms review | Only official no-client path; experimental on Linux, terms unverified |
| 2026-11-09 | Single owner per instance | Smaller attack surface |
| 2026-11-09 | Master key: OS keystore (desktop), Docker secret or 0600 file on a separate volume (Docker), systemd-creds/TPM or 0600 file (deb/rpm); passphrase mode optional everywhere | Unattended restarts by default, stronger mode available |
| 2026-11-09 | Updates prompted by default, silent auto-install opt-in | Limits blast radius of a compromised release pipeline |
| 2026-11-09 | No passkeys | Owner choice; password + optional TOTP |
| 2026-11-09 | Web UI: Svelte 5 + SvelteKit static, embedded | Small bundles, built-in transitions, good i18n |
| 2026-11-09 | Export: encrypted full export by default plus settings-only export without secrets | Owner choice |
| 2026-11-09 | No 32-bit Linux builds | Owner choice; fewer CI targets |
| 2026-11-09 | Translation: Hosted Weblate Libre | Crowdin OSS requires ≥3 months of activity and an active community |
| 2026-11-09 | No DCO sign-off | Owner choice; lowest contributor friction |
| 2026-11-09 | Last.fm request cap ≤1 req/s per instance; poll minimum 3 s | "5 req/s" is not an official figure; official text warns against several calls per second |
| 2026-11-09 | Album art: iTunes Search API first, MusicBrainz Cover Art Archive fallback; no Last.fm artwork | Free and keyless; Last.fm ToS 5.1.8 excludes artwork; Apple's promotional-use terms are a grey area, accepted by the owner |
| 2026-11-09 | Animated album art skipped for now | Only via the paid Apple Music API ($99/yr developer token); scraping music.apple.com breaks Apple's site and Media Services terms; showing it needs a public URL (internet exposure) |
| 2026-11-09 | No scraping of any website, ever | Compliance; use documented public APIs only |
| 2026-11-09 | Phase 1 approved and started | Owner approval |
| 2026-11-09 | Added a `storage` crate to the workspace | Keeps the SQLite layer separate and reusable |
| 2026-11-09 | rustls + `ring` provider; reqwest 0.13 with `rustls-no-provider` | Avoids aws-lc build tooling (cmake/NASM) on Windows and cross builds; no OpenSSL |
| 2026-11-09 | OS keystore via `keyring` 4.2 (`v1` stores) | Maintained; pure-Rust Secret Service (zbus) on Linux |
| 2026-11-09 | `cargo deny` advisories instead of a separate `cargo audit` job | Same RustSec database, one tool, faster CI |
| 2026-11-09 | Argon2id defaults: 64 MiB, t=3, p=1 for password, passphrase and export; minimum 19 MiB/t=2 accepted when reading | RFC 9106; memory is only used transiently at login, unlock or export |
| 2026-11-09 | Smart polling: 2 s only around expected track ends, 5 s otherwise | Owner choice; tighter timer without continuous fast polling (Last.fm warns against continuous rapid calls) |
| 2026-11-09 | Track start estimated from the previous scrobble's start + length (max 15 s correction); progress bar hidden once a track is 10 s past its length | Last.fm exposes neither playback position nor pause state |
| 2026-11-09 | Social SDK headless test passed; the owner chose to build server mode as opt-in experimental (Phase 3) despite the risk | Owner decision. Recommended alternative (ask Discord first) declined; risks are documented and shown to users |
| 2026-11-09 | Phase 2 approved and started | Owner approval |
| 2026-11-09 | **Renamed to RemoteScrobbleRP** (repo `ammarx4/RemoteScrobbleRP`, binary `remotescrobblerp`, env prefix `REMOTESCROBBLERP_`; superseded 2026-11-10, see below) | Owner choice. Existing installs migrate automatically (data folder, database file, OS keystore entry). The crypto domain strings `scrobbleaway/…` are kept unchanged on purpose: existing encrypted data is bound to them |
| 2026-11-09 | License stays AGPL-3.0 (MIT and Apache-2.0 were considered) | Owner choice; keeps forks and hosted versions open |
| 2026-11-09 | UI redesign requested: monochrome black and white, dark and light themes | Owner feedback on the first design |
| 2026-11-09 | Monochrome UI shipped: black/white, hairline borders, album art as the only colour; System/Dark/Light switch stored per browser | Implements the owner's request |
| 2026-11-09 | First password on an exposed install requires a one-time setup code from the server log | Stops a stranger who reaches the UI first from claiming it |
| 2026-11-09 | Concurrent Argon2 hashing capped at 2 | 64 MiB per hash; prevents memory exhaustion from login floods |
| 2026-11-09 | Updater verifies the minisign signature (trusted comment pins version and target) **and** a signed SHA256SUMS; automatic rollback after 3 failed starts | Defence in depth against bad or replayed releases |
| 2026-11-09 | Tray on Windows/macOS via tray-icon + winit; never on exposed installs | Desktop convenience; servers stay headless |
| 2026-11-09 | **No paid signing.** Only free signing is used: minisign for update/release signatures (key generated locally, no cost), and Sigstore/cosign keyless for Docker images. No paid Windows Authenticode certificate and no paid Apple Developer ID / notarization | Owner decision (no costs). Consequence: Windows SmartScreen shows an "unknown publisher" warning and macOS Gatekeeper blocks the first launch until the user allows it; the install docs must explain this |
| 2026-11-09 | Phase 3 on hold until the current version is production-ready | Owner request |
| 2026-11-09 | Expanded presence options: activity type, custom "Listening to" name, member-list line, layout toggles, clickable links, custom button, missing-art placeholder | Owner request (Music Presence parity). Local RPC accepted `name`, `*_url`, custom buttons and `status_display_type` in a live probe |
| 2026-11-09 | Previews show the owner's Discord avatar and display name (from the local RPC READY payload; avatar hash validated; `cdn.discordapp.com` added to the CSP `img-src`) | Owner request |
| 2026-11-09 | "by"/"on" can't differ between the member list and the profile: Discord renders the same field in both. Workaround documented in the UI: put the artist in the "Listening to" name and show that in the member list | Discord limitation |
| 2026-11-09 | "Hide words" keyword filter for titles/albums; artwork retry + simplified-name pass + album-cover fallback | Owner request (live/remastered variants, missing covers) |
| 2026-11-09 | Fixed: Cover Art Archive results were always rejected (CAA returns `http://` links; now upgraded to https). Added album (release-group) cover lookup. Deezer evaluated and skipped (no results for test artist; would need its own terms review) | Album art missing for many tracks (e.g. Radiohead is absent from Apple's US catalogue) |
| 2026-11-09 | Sessions: 12 h total / 1 h idle by default; "remember me" gives 30 days | Owner choice (convenience); mitigated by hashed, revocable sessions and rotation at login |
| 2026-11-09 | Login lockout: escalating per-IP delays (1 min, doubling to 1 h) after 5 failures | Owner choice; avoids an attacker locking the owner out permanently |
| 2026-11-09 | Localhost desktop: the wizard recommends a password but allows skipping | Owner choice; exposed installs always require one |
| 2026-11-09 | TLS via reverse proxy only | Owner choice; less code, proven TLS, automatic certificates (Caddy) |
| 2026-11-09 | Svelte 5 + Vite instead of SvelteKit | SvelteKit's static output needs an inline bootstrap script, which would weaken the strict CSP |
| 2026-11-09 | Owner signed off the `unsafe` FFI exception for the optional `discord-sdk-ffi` crate | Needed to call the Social SDK C API |
| 2026-11-09 | Social SDK Terms reviewed: the SDK may not be redistributed by us, so users must download it; headless use is not described in the documentation | Terms §2.a and §2.b(ii)/(iii); see `docs/PHASE1.md` |
| 2026-11-09 | Last.fm artwork stays off (not even opt-in) | Owner decision after the Cover Art Archive fix; ToS 5.1.8 |
| 2026-11-09 | Phase 3 started, features first; Social SDK server mode after | Owner choice; lower-risk features ship before the experimental mode |
| 2026-11-09 | Quiet hours use `jiff` for time zones; email uses `lettre` (no default features; rustls + ring + platform verifier) | Maintained crates; reuse the existing TLS stack (3 new crates for email). Binary 5.2 → 6.0 MB |
| 2026-11-09 | Stronger defaults chosen without asking (no functionality lost): exports never contain login data; full export requires the admin password if set; SMTP without TLS is refused | Security protocol: only weaker options need the owner's sign-off |
| 2026-11-09 | Server mode built: one Discord + one Last.fm account per instance, no multi-account | Owner decision. FFI in `discord-sdk-ffi` (libloading 0.8, ~45 C functions, one SDK thread, every `unsafe` with `// SAFETY:`); app feature `social-sdk`, off by default |
| 2026-11-09 | Server-mode sign-in via PKCE + redirect to the instance itself (not the SDK device flow) | The SDK's device flow shows its own on-screen UI, impossible headless; the PKCE flow was proven in the Phase 1 test |
| 2026-11-09 | SDK library path only from config/env, with permission checks | Loading a library = running code in our process; the web UI must never control it |
| 2026-11-09 | Online status in server mode: show it in the previews and let the owner change it (online/idle/dnd/invisible) | Owner choice ("Both"), reversing the earlier drop for server mode only. Uses only documented SDK functions with the existing `sdk.social_layer_presence` scope (worked live, no new permission). The chosen status is not stored |
| 2026-11-09 | Release signing key stored as GitHub secrets (environment `release`), signing automatic in CI | Owner decision. Recommended alternative (offline signing) declined. Tradeoff: a compromised GitHub account or workflow could sign updates. Mitigations: secrets only in the final job of a dedicated environment, releases created as drafts the owner publishes, the updater ignores drafts |
| 2026-11-09 | Trusted comment format: `remotemusicrp <version> <asset file name>` on every release file and SHA256SUMS | Binds each signature to one version and one file (no downgrade or cross-asset replay); tested with a real-signature fixture from a throwaway key |
| 2026-11-09 | AppImage added (x86_64 + aarch64), updater-aware (replaces `$APPIMAGE` only when our exe is inside `$APPDIR`). AppImage runtime taken from the hash-pinned appimagetool 1.9.1 instead of the unpinned "continuous" runtime | Owner request; supply-chain pinning |
| 2026-11-09 | Linux builds link against glibc 2.28 via cargo-zigbuild (zig wheel hash-pinned); one Linux binary serves tarball, AppImage, deb, rpm and Docker. Install kind detected at runtime, and runtime signals can only disable self-update | Matches the platform floor (RHEL 8) with no per-format rebuilds |
| 2026-11-09 | Windows: separate tiny GUI-subsystem launcher (`remotemusicrp-desktop.exe`) instead of making the main exe GUI-subsystem | Keeps admin CLI output working, needs no `unsafe` (AttachConsole would), and updates keep working |
| 2026-11-09 | Docker: images built only from the signed release binaries after publishing; `-sdk` variant (Debian slim + ALSA/Pulse/X11 libs, amd64) for server mode and a distroless default (amd64+arm64). The SDK is mounted by the user, never shipped. UI stays on 127.0.0.1 inside the container; access via host networking or a Caddy container sharing the app's network namespace | Keeps the loopback/XFF trust rules intact; SDK Terms §2.b(ii) |
| 2026-11-09 | Packages: systemd unit with `DynamicUser`, strict sandboxing, installed but not enabled; master key as a 0600 file in its own state subfolder | See open item 10 for systemd-creds |
| 2026-11-09 | Linux x86_64 release builds include the `social-sdk` feature (inert unless configured) | One build for desktop and server; the SDK is still never bundled |
| 2026-11-10 | **Renamed to RemoteMusicRP** (repo `iammarxg/RemoteMusicRP`, binary `remotemusicrp`, env prefix `REMOTEMUSICRP_`, Docker `ammarx4/remotemusicrp`, packages/paths `remotemusicrp`) | Owner choice (the recommendation was to keep RemoteScrobbleRP). Done before the first release. Installs under either old name (ScrobbleAway, RemoteScrobbleRP) migrate automatically: data folder, database file, OS keystore entry. Old `REMOTESCROBBLERP_*` environment variables are not read (nothing was released with them). Crypto domain strings stay `scrobbleaway/…`. The signing key pair is unchanged (`remotemusicrp.pub`); trusted comments now start with `remotemusicrp` |
| 2026-11-10 | Update check falls back to GitHub's documented `/releases/latest` redirect (HEAD request, tag taken from the final URL, strictly validated) when the API fails, e.g. its 60-requests-per-hour anonymous limit; installs build download URLs from the tag instead of the API asset list. A failed check now says "Couldn't check" instead of "up to date" | Owner report: "You're up to date" was shown next to "GitHub returned HTTP 403" (rate limit). Only the `[security]` highlight needs the API; the fallback can't see release notes |
| 2026-11-10 | **LAN access over plain HTTP** (`lan_access`), opt-in | Owner choice. Options offered: built-in HTTPS with a self-made certificate (recommended), plain HTTP, or a documented proxy setup. Tradeoff accepted: on the LAN the password and session cookie are not encrypted. Mitigations: LAN-only clients and Host names, same-origin writes, password required, warnings |
| 2026-11-10 | First password: set on the computer itself without a code; other devices are told to finish setup there; setup code only as a fallback for headless servers | Owner feedback: entering a code from a log file was unintuitive. Tradeoff: any local program or user on that machine can set the first password, consistent with the existing trust in the local machine. Proxied (internet) and LAN visitors still need the code |
| 2026-11-10 | Fixed: a reachable install without a password showed a login nobody could pass (the setup screen needed a login first) | Found in the LAN test; affected public_origin installs too |
| 2026-11-10 | Windows: `remotemusicrp.exe` started with no arguments (double-click) relaunches itself hidden with `run` (tray, output to `desktop.log`) and exits; `run` and admin commands stay in the terminal; `REMOTEMUSICRP_FOREGROUND=1` disables the relaunch | Owner report: the raw `.exe` from the release page opened a console window. A brief console flash remains; avoiding it entirely would need `unsafe` Win32 calls |
| 2026-11-10 | v0.1.0 published (GitHub release, signed; Docker `0.1.0`/`latest` amd64+arm64 and `0.1.0-sdk`/`latest-sdk` amd64) | First release. Downloads verified independently with minisign after publishing |
| 2026-11-10 | Fixed: server mode sent activities to the Social SDK without Discord's field limits (only local RPC applied them), so one bad field made Discord reject the whole status. Both paths now use `Activity::sanitized()`. Link/button URL length is measured after percent-encoding (non-ASCII bytes count as 3) | Found live: a title in an unusual script gave a Last.fm link under 256 bytes raw but over 256 encoded |
| 2026-11-10 | GitHub repo is **`iammarxg/RemoteMusicRP`** (public, created 2026-11-10); Docker Hub stays **`ammarx4/remotemusicrp`** (public, created 2026-11-10) | Owner choice: `ammarx4` does not exist on GitHub; the owner's GitHub account is `iammarxg`. Updater download check, "Get this status" button, cosign identity and docs all point at `iammarxg` |
| 2026-11-10 | Linux tray via `ksni` 0.3.7 (Unlicense; reuses zbus 5 from the keystore), only with a desktop session bus; falls back to opening the browser. AppImage desktop entry no longer opens a terminal | Owner request; no GTK dependency |
| 2026-11-10 | Inter 4.1 bundled (upright variable font only, 344 KB; the UI uses no italics). Binary 6.0 → 6.4 MB | Owner request; same look on every system |
| 2026-11-10 | Rename migration: a data folder that exists without a database (the Windows launcher creates it for its log) counts as "not set up", and the old folder's contents are moved into it | Bug found in the live rename test |
| 2026-11-10 | Member-list line with a "by"/"on" prefix is shown as "Listening to <artist/album>" automatically (replaces the documented manual workaround) | Owner request: "Listening to by Artist" confused people. The "Text after Listening to" setting is overridden while this applies, and the UI says so |
| 2026-11-10 | Package installs keep the master key in a 0600 file (own state folder, outside the data dir); systemd-creds/TPM not used | Owner decision. A TPM-bound credential can't be rewritten by the unprivileged service, which would stop automatic key rotation; the alternative (a root helper at service start) was declined as extra code for now. Tradeoff: the key is protected by file permissions, not bound to the machine's TPM |
| 2026-11-09 | `0BSD` added to the licence allow-list | `quoted_printable` (via lettre); public-domain-equivalent, AGPL-compatible |
