# Threat model (initial, 2026-11-09)

This is a living document. Update it as the design is implemented. The vulnerability-reporting policy is in `/SECURITY.md`.

## 1. System overview

There is one owner per instance. A single Rust binary contains:

- the poller (provider → presence)
- the presence sink (Discord local RPC; possibly the Social SDK later)
- an embedded web UI and API
- a SQLite database
- the updater, on desktop and tarball installs

Outbound connections go only to Discord, the scrobble provider, the user's email provider and GitHub Releases.

## 2. Assets

| Asset | Sensitivity | Where |
|---|---|---|
| Master key | Critical: it decrypts everything | OS keystore (desktop) / 0600 file on a separate volume (Docker) or in its own state folder (deb/rpm/Arch) / memory only (passphrase mode). Never in the DB. |
| Wrapped data keys + encrypted secrets (Last.fm API key, Discord OAuth tokens if the SDK mode ships, SMTP/Resend credentials, TOTP secret) | High | SQLite |
| Admin password hash (Argon2id), recovery-code hashes | High | SQLite |
| Session cookies / CSRF tokens | High (short-lived) | Browser + memory/DB |
| Release signing key (minisign) | Critical (supply chain) | Maintainer, offline/CI secret; never in the repo |
| Settings, activity log, track cache | Low–medium (listening history is personal) | SQLite |

## 3. Adversaries and mitigations

| # | Adversary | Example attack | Mitigations |
|---|---|---|---|
| A1 | Remote network attacker | Reaching an exposed UI: brute force, CSRF, XSS, session theft | Bind to 127.0.0.1 by default; admin password required when not loopback; TLS/HSTS required when exposed; Argon2id; rate limiting and lockout; optional TOTP; strict CSP; Secure/HttpOnly/SameSite=Strict cookies; CSRF tokens; Host-header check; input validation |
| A2 | LAN / on-path attacker | Sniffing or tampering with traffic | TLS for exposed UIs; HTTPS-only outbound (rustls, cert validation on); no plaintext fallback. **Exception (owner decision 2026-11-10): optional `lan_access` serves the UI over plain HTTP to the local network.** A LAN attacker can then read the password and session cookie in transit and take over the UI. Mitigations: off by default; only private/link-local client addresses accepted; Host must be a private IP or a local name (keeps DNS-rebinding protection); same-origin + CSRF on writes; password required; the warning is shown in the log and the UI. Docker bridge networking defeats the client-address check (all clients appear as the bridge gateway), so the docs require host networking for LAN access. |
| A3 | Local unprivileged malware on the desktop | Reading the DB, impersonating Discord IPC, hitting the localhost UI | Secrets encrypted at rest with the master key in the OS keystore; localhost UI still uses session auth + CSRF when a password is set; **residual risk:** any local process can talk to the Discord IPC socket and to our loopback port. Same-user malware is mostly out of scope (it could read keystore entries it is allowed to). |
| A4 | Stolen disk / backup / Docker volume | Offline extraction of tokens | Envelope encryption; the master key lives elsewhere (keystore, separate volume or separate 0600 folder; a disk or full-system backup that includes that folder exposes it, so use passphrase mode where that matters); optional passphrase mode; "Delete everything" wipes keys |
| A5 | Compromised update or release pipeline | Malicious binary pushed to users | minisign signature with the embedded public key **and** checksum verification before replacement; HTTPS; atomic replace with rollback; **prompted** updates by default; Actions pinned by SHA; least-privilege tokens; cosign-signed images + SBOM |
| A6 | Malicious or vulnerable dependency | Supply-chain compromise | Minimal dependencies; `cargo audit` + `cargo deny` (advisories, licenses, sources) in CI; `Cargo.lock` committed; review on update; no runtime CDN assets |
| A7 | Malicious provider data | Crafted track names (XSS in the UI, template injection, oversized fields) | Treat all provider data as untrusted; escape in the UI; template engine with only whitelisted variables, no code execution; length limits matching Discord field limits |
| A8 | Container escape / lateral movement | Exploiting the app inside Docker | Distroless, non-root, read-only rootfs, all capabilities dropped, `no-new-privileges`, only the data and key volumes writable |
| A9 | Log leakage | Secrets in logs or error pages | `secrecy` types (redacted Debug); no secrets in errors; tokens never logged; generic error messages to clients |

## 4. Trust boundaries

1. Browser ↔ web server (auth, CSRF, CSP)
2. App ↔ OS keystore / secret file
3. App ↔ SQLite file
4. App ↔ Discord IPC socket (local, unauthenticated by design)
4b. App ↔ Discord Social SDK (server mode, opt-in): the proprietary library runs inside our process, so it is loaded only from a path in the config file/env (never the web UI), which must be absolute and not group/world-writable. All SDK calls happen on one thread; the FFI is confined to the `discord-sdk-ffi` crate. OAuth2 uses PKCE with a single-use `state` bound to the session; only the refresh token is stored, encrypted.
5. App ↔ internet (Last.fm, ListenBrainz, email, GitHub): TLS, response size limits, timeouts. SMTP always uses TLS (implicit or mandatory STARTTLS) with OS-trust-store verification; addresses are parsed strictly (no header injection); email errors never contain the password or key
6. Release pipeline ↔ users (signatures)

## 5. Web UI controls (Phase 2)

- **DNS rebinding:** requests whose `Host` is not a loopback address on our port (or the configured public origin) get 421.
- **CSRF:** state-changing requests need a matching `Origin` **and** the session's CSRF token in `X-CSRF-Token`. Cookies are `SameSite=Strict`.
- **Sessions:** 256-bit random tokens, stored only as SHA-256 hashes. 1 h idle / 12 h total, or 30 days with "remember me" (owner decision). They rotate at login and password change and can be revoked. Anonymous sessions are capped.
- **Brute force:** per-IP lockout (5 free attempts, then 1 min doubling to 1 h). At most 2 Argon2 hashes run at once.
- **First password on an exposed instance:** requires a one-time setup code from the server log.
- **Second factor:** TOTP with replay protection, plus single-use 80-bit recovery codes stored as SHA-256 hashes.
- **Updates:** minisign signature with a version/target-pinned trusted comment, plus a signed SHA256SUMS. Downloads come only from the project's GitHub releases path. Atomic replace, backup and automatic rollback. Prompted by default.
- **CSP:** no inline scripts or styles. Images are allowed only from self, `data:` and the album-art hosts.

## 6. Open risks / TODO

- Server mode can change the owner's Discord online status (`POST /api/discord/sdk/online-status`): owner session + CSRF required, only `online|idle|dnd|invisible` accepted, unknown JSON fields refused, each change written to the activity log. Someone who gains the owner's session could change the status, a low-impact action compared with what a session already allows.
- The UI loads album-art images straight from Apple/Archive.org, and the owner's Discord avatar from `cdn.discordapp.com`, in the owner's browser. Those hosts see the owner's IP, the same exposure Discord clients already have.
- Until the release signing key exists, self-update is disabled.

- The Discord IPC socket and the loopback UI port are reachable by any local process (inherent to local RPC).
- The Social SDK mode (if approved) would add a proprietary native library and an `unsafe` FFI boundary. It needs its own review.
- Passwordless localhost desktop mode trusts every local user on the machine. Show a warning on multi-user OSes.
- Key rotation job: define the cadence (proposal: monthly master-key rotation, re-wrap of data keys). Rotation-frequency tradeoffs go to the owner.
