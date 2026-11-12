# Data inventory (initial, 2026-11-09)

RemoteMusicRP stores only what it needs to work. There is **no telemetry**, and nothing is ever sent to the maintainer.

All data lives on the owner's own machine:
- in the SQLite database file (WAL mode) in the data directory
- plus the master key, which is stored separately (see below)

| Data | Why | Protection | Retention |
|---|---|---|---|
| Master key | Encrypts all secrets | OS keystore (desktop), Docker secret or 0600 file on a separate volume (Docker), 0600 file in `/var/lib/remotemusicrp/keys` (deb/rpm/Arch), or memory only (passphrase mode). **Never in the DB.** | Until "Delete everything"; rotated periodically |
| Wrapped data keys (with key ID/version) | Envelope encryption | Wrapped by the master key | Re-wrapped on rotation; deleted with their record |
| Last.fm username | To read now-playing | Plain (not secret; public on Last.fm) | Until Last.fm is disconnected |
| Last.fm API key (user's own) | API access | Encrypted (XChaCha20-Poly1305, AAD-bound) | Until Last.fm is disconnected |
| Discord application ID (user's own) | Local RPC handshake | Plain (public identifier) | Until Discord is disconnected |
| Discord OAuth2 refresh token (server mode only) | Renew the 7-day access token after restarts | Encrypted (envelope); the access token is kept in memory only | Until signed out (the token is also revoked at Discord) or Discord is disconnected |
| Discord account name, avatar hash and online status (server mode only) | Previews | Memory only, re-read from the SDK's local cache every 5 s; never written to disk. A status the owner picks is sent to Discord and not stored | Until the process stops |
| Pending server-mode sign-in (PKCE verifier, state, session hash) | Finish the OAuth2 sign-in | Memory only | 10 minutes or first use |
| Start-at-login entry (desktop installs; on by default, can be turned off) | Start the app at login | Windows: a `RemoteMusicRP` value in `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. macOS: `~/Library/LaunchAgents/io.github.iammarxg.remotemusicrp.plist`. Linux: `~/.config/autostart/remotemusicrp.desktop`. Each holds only this program's own path, `run` and `--config <file>` if one is used; never taken from a request. The DB keeps only a flag that the default was applied | Until switched off or "Delete everything" |
| Email credentials (Resend key or SMTP password, optional) | Re-auth alerts | Encrypted | Until removed or email is turned off |
| Email settings (provider, sender, recipient, SMTP server/port/username) and the time the last alert of each kind was sent | Alerts, max one per problem per day | Plain (no secrets) | Until removed |
| Admin password hash | Login | Argon2id | Until changed or deleted |
| TOTP secret (optional) | 2FA | Encrypted | Until 2FA is disabled |
| Recovery-code hashes | 2FA recovery | Argon2id (or a fast hash, since the codes are high-entropy) | Until used or regenerated |
| Sessions | Keep the owner logged in | Random token; only a hash stored server-side | Expire (lifetime to be confirmed with the owner) |
| Web UI sessions (token **hash**, CSRF token, timestamps, coarse browser label such as "Chrome on Windows") | Keep the owner logged in | Hash only; the raw token exists only in the browser cookie | Expire after 1 h idle / 12 h, or 30 days with "remember me"; pruned automatically |
| `update-pending` marker (data dir) | Rollback after a failed update | Plain counter | Deleted after the new version has run for 1 minute |
| Theme choice (browser `localStorage`, not the server) | Remember dark/light | n/a | Until cleared in the browser |
| Settings (presence toggles, templates, artist/song blocklist, quiet hours and time zone) | Configuration | Plain | Until changed |
| Activity log (connection events, presence changes, quiet hours, exports/imports, email results, errors; **no secrets and no listening history**) | Troubleshooting | Plain | Default **30 days**, configurable |
| Counter of songs seen since start | Activity stats | Memory only | Until restart |
| Artwork lookup cache (artist + track → Apple/Cover Art Archive image URL, Apple track link, or "no match"; **no image bytes**) | Avoid repeated lookups; stay under the iTunes Search API limit (~20/min) | Plain | LRU with size cap; "no match" entries expire after 7 days |
| `track.getInfo` cache (artist, track, duration) | Progress bar without repeated API calls | Plain | LRU, kept well under Last.fm's **100 MB** storage cap (ToS 4.3); obeys HTTP cache headers |

## User controls

- **Disconnect Discord** or **disconnect Last.fm** separately. This deletes that connection's secrets and wrapped keys.
- **Export (Settings → Backup):**
  - The default is a full export including secrets (Last.fm API key, email key/password), encrypted with a passphrase the user chooses (Argon2id 64 MiB/t=3 + XChaCha20-Poly1305, AAD-bound to the purpose `export`). Requires the admin password when one is set.
  - Alternatively, a settings-only export (plain JSON) that never contains secrets; importing a plain file that contains secrets is refused.
  - Login data (password hash, 2FA secret, recovery codes, sessions) is **never** exported, so importing cannot change who can log in.
- **Delete everything:**
  1. Deletes all rows.
  2. Removes the DB, WAL and SHM files, after overwriting them where the filesystem allows it.
  3. Removes the master key from the keystore or secret file.
  4. Removes the start-at-login entry, if any.
  5. Returns the app to the first-run wizard.
  - Note: secure overwrite cannot be guaranteed on SSDs or copy-on-write filesystems. Deleting the master key (crypto-shredding) is the real guarantee.

## Outbound connections

- Discord (local IPC, and the Social SDK mode if it ships)
- Last.fm (`ws.audioscrobbler.com`)
- Artwork lookups: iTunes Search API (`itunes.apple.com`), then MusicBrainz / Cover Art Archive. These send artist and track names only. Discord, not RemoteMusicRP, fetches the image itself.
- ListenBrainz, later and only if configured
- The user's email provider, if configured: Resend (`api.resend.com`) or the user's own SMTP server (TLS or STARTTLS required; plaintext SMTP is refused)
- GitHub Releases, for the update check (can be disabled)
