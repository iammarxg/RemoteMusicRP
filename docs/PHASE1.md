# Phase 1 notes (started and completed 2026-11-09)

## Status

| Step | Status |
|---|---|
| 1. Social SDK headless test + terms review | **Done.** Works technically (see Result below). The owner chose to build it as opt-in experimental in Phase 3. |
| 2. Live local-RPC checks (buttons, `status_display_type`, external image URLs, Flatpak/Snap) | **Verified on Windows, 2026-11-09**, with the owner's app and real Discord. Confirmed: "Listening to <app name>", details/state, external Apple art URL with album hover text, start/end progress bar, and both buttons (Last.fm + "Get this status") all render. Flatpak/Snap are still untested (no Linux desktop). |
| 3. Workspace, LICENSE, CONTRIBUTING, CI skeleton | Done. LICENSE is the AGPL-3.0 text from https://www.gnu.org/licenses/agpl-3.0.txt, downloaded 2026-11-09 with owner approval. |
| 4. `crypto` crate | Done (34 tests). |
| 5. Provider trait + Last.fm (+ artwork sources) | Done (22 tests). A live call to Last.fm over HTTPS was verified (a bad key is correctly reported as rejected credentials). |
| 6. Discord local RPC | Done (16 tests, including a fake Discord IPC server). Not yet tried against a real client. |
| 7. SQLite storage + config | Done (storage 10 tests, core 10 tests). |

## Social SDK Terms review (step 1, paper part)

Source: the Discord Social SDK Terms, effective 2025-03-17, read on 2026-11-09 via the in-app browser: https://support-dev.discord.com/hc/en-us/articles/30225844245271-Discord-Social-SDK-Terms

- **Licence (§2.a):** the licence allows a developer to "distribute the Discord Social SDK as integrated into your Application". It is not sublicensable.
  - In RemoteMusicRP's model, each user owns their own Discord application, so each user is "the developer".
  - The maintainer shipping the SDK binary in releases or Docker images is **not** covered: §2.b(ii) forbids redistribution outside the licence.
  - → Confirms the plan: **each user downloads the SDK themselves**, and we never bundle it.
- **Use must follow the Documentation (§2.b(iii)(A)):** running the SDK headless on a server with no Discord client is **not described** in the documentation.
  - This is the main compliance risk. A working test alone is not enough; the behaviour should also be documented, or confirmed by Discord.
- **Integration Requirements (§3.a):** Discord may review and test integrations and require changes.
- **Age (§3.b):** users must be 13 or older.
- **Automated access (§2.b(vi)):** no crawling or data-mining tools. This doesn't apply to presence updates.
- **Not games-only:** the text speaks of "your Application" throughout. The product pages are framed for games.
- **AGPL:** we would only load a library that the user downloaded themselves; we never convey it. That seems workable, but this is not legal advice.

**Paper verdict:** ⚠️ permitted only with the user-download model. Headless use sits outside the documented behaviour.

## Hands-on headless test plan (needs the owner)

Environment: WSL Ubuntu on the owner's PC (installed, stopped). Ubuntu in WSL has no Discord client and no GUI, which matches the target.

1. The owner enables the Social SDK on their Discord application. In the developer portal: Social SDK → Getting Started form → Submit.
2. The owner downloads the Linux SDK archive from the portal (Downloads). This means accepting the Social SDK Terms personally.
3. In WSL: build a tiny C++ test against the SDK. Its only job: `Client::UpdateToken(<OAuth token with openid sdk.social_layer_presence>)` → `Client::Connect` → `UpdateRichPresence` (type Listening, details/state, external image URL, timestamps, 2 buttons).
   - It lives in a throwaway `spikes/` folder and is not part of the Rust workspace, so it needs no `unsafe` Rust.
4. Get the token with the OAuth2 authorization-code + PKCE flow in a normal browser, then exchange it with curl. No Discord client is involved.
5. Check from another Discord account/device whether the presence appears, what renders (buttons, art, progress bar), and how long it stays after the process exits.
6. Record the result here and give the owner a go/no-go.

## Live findings (2026-11-09)

- Detection lag measured on one track: "now playing" was first seen about **2 s** after the true start, within one 5 s poll. Another track lagged 12 s or more (the player's report reached Last.fm late).
- **Timing accuracy:** Last.fm reports no playback position. The start time is now estimated from the previous scrobble (scrobble timestamps are start times) plus its length, when that lands at most 15 s before we first saw the new track; otherwise first-seen time is used (`core::timing`).
- **Pause:** Last.fm has no pause signal, so a paused track keeps "playing" until the player or Last.fm drops it. Mitigation: once a track is 10 s past its length and still "now playing", the progress bar is hidden while the track info stays visible. Real pause detection is **not possible** through the Last.fm API.

## Social SDK hands-on test: progress

- SDK 1.10.19337 (2026-09-01). The C++ archive contains the Linux x86_64 `libdiscord_partner_sdk.so` (14 MB release build) and the C/C++ headers. No Linux arm64 build is included.
- The test program is `spikes/social-sdk-headless/` (C++20, built in WSL Ubuntu 24.04 with g++ 13). It does a PKCE sign-in in the owner's own browser, then `GetToken` → `UpdateToken` → `Connect` → `UpdateRichPresence`. The token stays in memory only.
- **Finding:** the Linux SDK library links against `libasound.so.2`, `libpulse.so.0` and `libX11.so.6` (for voice and overlay features), even when only presence is used. A server or Docker deployment would have to ship audio and X11 libraries, which works against the minimal/distroless image goal.
- The owner installed those libraries in WSL (`libasound2t64`, `libpulse0`), added the `http://127.0.0.1/callback` redirect and turned on **Public Client**, which `GetToken` without a client secret requires.

### Result (2026-11-09): technically works ✅

Run in WSL Ubuntu with no Discord client, `DISPLAY`/`WAYLAND_DISPLAY` unset, and the owner's Discord app:

```
[RESULT] GetToken OK (expires_in=604800s, scopes=openid sdk.social_layer_presence)
[STATUS] Connecting -> Connected -> Ready
(rpc_manager) RPC Connect error: -2      <- no local Discord client, as intended
[RESULT] UpdateRichPresence: SUCCESS
```

- The owner's Discord profile showed "Listening to ScrobbleAwayRP" with details, state, the external image URL with hover text, a start/end progress bar and both buttons. All the fields we use work through the SDK's own connection.
- Access token: 7 days. Refresh token: per the SDK header, "do not currently expire". Server mode would store the refresh token encrypted and refresh the access token before expiry.
- Resources: **15.8 MB RSS**, 7 threads, ~0% CPU while connected (similar to our whole local-RPC app).
- Sign-in uses PKCE in the owner's own browser; the redirect page failing to load is expected. A server deployment would need either this copy-paste step or a device-code flow in the web UI.

### Caveats that remain

1. **Documentation/terms (main risk):** headless server use is not described in Discord's docs, and §2.b(iii)(A) requires use "in accordance with the Documentation". Working is not the same as permitted.
2. **Distribution:** proprietary binary; users must download it themselves (§2.a/§2.b(ii)).
3. **Platforms:** Linux x86_64 only (no arm64 build); Linux support is labelled "Experimental".
4. **Dependencies:** the Linux library needs ALSA, PulseAudio and X11 client libraries, so no distroless image.
5. **Public Client** must stay on for this flow.
6. **FFI:** a Rust integration needs `unsafe` in a separate opt-in crate (owner sign-off per CLAUDE.md §3).

## Implementation notes

- **TLS:** rustls with the `ring` provider plus the OS trust store (`rustls-platform-verifier`). No OpenSSL, enforced by cargo-deny.
- **HTTP:** reqwest 0.13 (`rustls-no-provider`, json, query, gzip). HTTPS only, 20 s timeout, at most 3 redirects, at most 512 KiB response bodies. Error messages never include URLs (the Last.fm key is in the query string).
- **Rate limits:** Last.fm ≥1 s between calls, Apple Search ≥3 s, MusicBrainz ≥1.1 s.
- **OS keystore:** `keyring` 4.2, `v1` default stores: Windows Credential Manager, macOS Keychain, Secret Service over zbus (pure Rust).
- **Runtime:** single-threaded tokio runtime to keep RAM low.
- **Measured (Windows 11, release build, 2026-11-09):**
  - Binary: **4.21 MB**.
  - Idle while polling Last.fm every 5 s with the Discord IPC connected: **14.0 MB working set, 4.1 MB private memory**, 0.09 s CPU over the first ~15 s.
  - The HTTPS connection to Last.fm is reused between polls (keep-alive).
- **Phase 1 CLI** (until the Phase 2 web UI exists):
  - `connect-lastfm`, `disconnect-lastfm`, `set-discord-app`, `status`, `rotate-keys`, `delete-everything --yes`, `run`
  - Credentials are typed on stdin, and the input is **visible** while typing. The web UI replaces this in Phase 2.
- **Crates:** a separate `storage` crate was added, because the SQLite layer is reused by the app and future backends.

## Server mode live test (2026-11-09)

The built server mode (`social-sdk` feature) was tested end to end in WSL Ubuntu, with no Discord client:

- Browser sign-in (PKCE, redirect to `http://127.0.0.1:8485/discord-callback`): status went connecting → connected → ready in about 2 s.
- Now playing was sent to Discord from Last.fm.
- After a restart, the instance renewed the access token from the stored, encrypted refresh token and reached ready on its own.
- **Bug found and fixed:** stopping the app made the SDK abort (`Check failed: !shutdownCompleted`, `cdiscord.cpp` line 103). Our SDK thread was still pumping `RunCallbacks` while the SDK's exit-time teardown ran. The thread is now stopped explicitly (clear presence, disconnect, drop the client) before the process exits. Stopping is now clean.
- The dashboard preview now shows the account's display name and avatar in server mode too, read from the SDK's cached current user once the client is ready.
- Resource use (WSL, server mode, steady state): about 20 MB RSS, of which about 5 MB is private memory and 15 MB is the mapped binary and SDK library (file-backed, shared). CPU over 30 s: 0 ticks. 8 threads. Over about an hour of playback (around 15 songs), the presence updated without a Discord error.
