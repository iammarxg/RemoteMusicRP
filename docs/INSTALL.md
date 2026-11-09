# Installing RemoteMusicRP

RemoteMusicRP shows what you're playing on Last.fm as your Discord status.
You run your own copy. You need:

- your own **Discord application** (free, made in the
  [Discord Developer Portal](https://discord.com/developers/applications));
- your own **Last.fm API key** (free, from
  [last.fm/api/account/create](https://www.last.fm/api/account/create)).

The setup wizard in the web UI walks you through both.

**Contents:**
- [Download and check the files](#1-download-and-check-the-files)
- [Windows 11](#2-windows-11)
- [macOS](#3-macos-apple-silicon)
- [Linux desktop (AppImage)](#4-linux-desktop-appimage)
- [Linux server (deb, rpm, Arch, tarball)](#5-linux-server-deb-rpm-arch-tarball)
- [Docker](#6-docker)
- [Server mode: no Discord app needed](#7-server-mode-no-discord-app-needed-experimental)
- [Opening the web UI from another computer](#8-opening-the-web-ui-from-another-computer)
- [Updates](#9-updates)

## 1. Download and check the files

Downloads are on the
[releases page](https://github.com/iammarxg/RemoteMusicRP/releases). Every
file is signed. To check a download, install
[minisign](https://jedisct1.github.io/minisign/), download the matching
`.minisig` file next to it, and run:

```
minisign -Vm <file> -P RWQ0Snkh7EvjhsxdofSMvsiHaYLRSCY6wUHMQsrPAnNSQ2tXjnOqk+Ia
```

It should say `Signature and comment signature verified`.

## 2. Windows 11

1. Download `remotemusicrp-<version>-x86_64-pc-windows-msvc.zip` and
   extract it to a folder you keep, for example
   `C:\Users\<you>\Apps\RemoteMusicRP`.
2. Start **`remotemusicrp-desktop.exe`**. It runs in the background with a
   tray icon and no console window. Click the tray icon to open the web UI.
3. **"Windows protected your PC" warning:** the app isn't signed with a paid
   certificate, so SmartScreen doesn't know it. Click **More info**, then
   **Run anyway**. You only need to do this once per version.
4. Keep the **Discord desktop app** running: RemoteMusicRP talks to it
   directly.

To start it with Windows, put a shortcut to `remotemusicrp-desktop.exe` in
`shell:startup` (press Win+R and type that).

Your data is in `%LOCALAPPDATA%\RemoteMusicRP`. The encryption key is in
Windows Credential Manager. If the app doesn't start, look at
`%LOCALAPPDATA%\RemoteMusicRP\desktop.log`.

Admin commands such as `remotemusicrp.exe status` or
`remotemusicrp.exe reset-password` run from a terminal in that folder.

## 3. macOS (Apple Silicon)

1. Download `remotemusicrp-<version>-aarch64-apple-darwin.tar.gz`, and
   double-click it to extract.
2. Open Terminal in that folder and run `./remotemusicrp`.
3. **"cannot be opened because the developer cannot be verified":** the app
   isn't notarized (that needs a paid Apple account). Open **System Settings →
   Privacy & Security**, scroll down and click **Open Anyway** next to
   RemoteMusicRP, then run it again. Or, in Terminal:
   `xattr -d com.apple.quarantine remotemusicrp`.
4. Click the menu-bar icon to open the web UI. Keep the Discord app running.

## 4. Linux desktop (AppImage)

1. Download `remotemusicrp-x86_64-unknown-linux-gnu.AppImage` (or the
   `aarch64` one for ARM).
2. Make it executable and start it:
   ```
   chmod +x remotemusicrp-*.AppImage
   ./remotemusicrp-*.AppImage
   ```
3. A tray icon appears. Click it to open the web UI. Keep the Discord app
   running.

The tray icon works on KDE Plasma, Cinnamon, XFCE, MATE and others. On
GNOME it needs the **AppIndicator and KStatusNotifierItem Support**
extension (Ubuntu has it already). Without a tray, RemoteMusicRP opens the
web UI in your browser when it starts.

The AppImage updates itself (with your OK). If it won't start because FUSE is
missing, run it with `--appimage-extract-and-run`, or use the tarball.

## 5. Linux server (deb, rpm, Arch, tarball)

These install RemoteMusicRP as a hardened system service. It runs as a
throwaway user, can only write to its own folder, and listens on
`127.0.0.1:8484`. Your package manager handles updates.

**Debian / Ubuntu:**
```
sudo apt install ./remotemusicrp_<version>-1_amd64.deb
```
**Fedora / RHEL:**
```
sudo dnf install ./remotemusicrp-<version>-1.x86_64.rpm
```
**Arch Linux:** download `PKGBUILD` into an empty folder and run
`makepkg -si`.

Then:

1. Look at `/etc/remotemusicrp/remotemusicrp.toml`. The defaults are
   fine for most people.
2. Start the service and make it start at boot:
   ```
   sudo systemctl enable --now remotemusicrp
   ```
3. Open the web UI. On the server itself: `http://127.0.0.1:8484`. From
   your own computer, use an SSH tunnel and then open the same address
   locally:
   ```
   ssh -L 8484:127.0.0.1:8484 you@your-server
   ```
4. A server has no Discord app, so you need [server mode](#7-server-mode-no-discord-app-needed-experimental).

Logs: `journalctl -u remotemusicrp`. Data is in
`/var/lib/remotemusicrp/data`; the encryption key is in
`/var/lib/remotemusicrp/keys` (0600).

**Tarball** (`remotemusicrp-<version>-<target>.tar.gz`): the same program
plus the service file and config in `packaging/`. Install them by hand in the
same places as above (binary in `/usr/local/bin`, and change `ExecStart` to
match).

## 6. Docker

Images on Docker Hub, signed with Sigstore cosign:

| Image | What |
|---|---|
| `ammarx4/remotemusicrp:latest-sdk` | Server mode (experimental), amd64 only |
| `ammarx4/remotemusicrp:latest` | Minimal image, amd64 and arm64 |

The container runs as a non-root user with a read-only filesystem. It keeps
data in the `/data` volume and the encryption key in a separate `/keys`
volume.

1. Download [`compose.yaml`](../packaging/docker/compose.yaml) (and
   [`Caddyfile`](../packaging/examples/Caddyfile) if you want HTTPS access
   from other computers).
2. Put your own copy of `libdiscord_partner_sdk.so` next to `compose.yaml`
   (see [server mode](#7-server-mode-no-discord-app-needed-experimental)).
3. Choose in `compose.yaml` how you open the web UI:
   - **Option A (local only, Linux):** host networking. The UI is at
     `http://127.0.0.1:8484` on the Docker machine only.
   - **Option B (from anywhere):** Caddy in front with automatic HTTPS. Set
     your domain in `Caddyfile` and in `REMOTEMUSICRP_PUBLIC_ORIGIN`.
4. Start it with `docker compose up -d`, then follow the setup wizard.

**Docker Desktop (Windows or macOS):** files shared from your computer always
look writable by everyone inside the container, so RemoteMusicRP refuses
to load the SDK from them. Copy it into a Docker volume once instead:
```
docker volume create remotemusicrp_sdk
docker run --rm --user 0 --entrypoint install -v remotemusicrp_sdk:/sdk -v "$PWD:/src:ro" ammarx4/remotemusicrp:latest-sdk -m 0644 -o 0 -g 0 /src/libdiscord_partner_sdk.so /sdk/libdiscord_partner_sdk.so
```
Then, in `compose.yaml`, replace the `./libdiscord_partner_sdk.so:…` line with
`- remotemusicrp_sdk:/sdk:ro` and add `remotemusicrp_sdk: {external: true}`
under `volumes:`. Host networking (option A) needs Docker Desktop 4.34 or
newer with host networking turned on in its settings.

Check an image's signature:
```
cosign verify docker.io/ammarx4/remotemusicrp:latest-sdk \
  --certificate-identity-regexp '^https://github.com/iammarxg/RemoteMusicRP/\.github/workflows/docker\.yml@' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

## 7. Server mode: no Discord app needed (experimental)

Normally RemoteMusicRP talks to the Discord app on the same computer.
Server mode uses Discord's official **Social SDK** instead, so it works on a
server with no Discord app. Only on **Linux x86_64**.

**Risks, please read:** Discord's documentation doesn't describe running the
SDK on a server without a Discord app, so Discord could stop it working at
any time. If Discord objects, this mode will be removed.

**We can't include the SDK.** Its licence lets each developer ship it inside
their own app, but not pass it on to others. With RemoteMusicRP you are
the developer (it's your Discord application), so you download it yourself.
Nothing in the file is tied to your account.

### Get the SDK

1. In the [Discord Developer Portal](https://discord.com/developers/applications),
   open your application and go to **Social SDK → Getting Started**. Fill in
   the short form and submit it. This turns the SDK on for your app.
2. Go to **Social SDK → Downloads**. Accept the Social SDK terms and
   download the **C++/C SDK** zip.
3. In the zip, find `discord_social_sdk/lib/release/libdiscord_partner_sdk.so`.
   That one file is all you need.
4. Put it where the app can read it but nobody else can change it:
   ```
   sudo install -D -o root -g root -m 0644 libdiscord_partner_sdk.so \
     /opt/discord-social-sdk/libdiscord_partner_sdk.so
   ```
   RemoteMusicRP refuses to load the file if other users could change it,
   or if the path isn't absolute.
5. Debian/Ubuntu servers also need the libraries the SDK links against:
   `sudo apt install libatomic1 libasound2t64 libpulse0 libx11-6`
   (`libasound2` on older releases). On Fedora/RHEL:
   `sudo dnf install libatomic alsa-lib pulseaudio-libs libX11`. The Docker
   `-sdk` image already has them.

### Turn it on

In `/etc/remotemusicrp/remotemusicrp.toml` (packages):
```toml
[discord]
mode = "social-sdk"
sdk_library = "/opt/discord-social-sdk/libdiscord_partner_sdk.so"
```
Or with environment variables: `REMOTEMUSICRP_DISCORD_MODE=social-sdk` and
`REMOTEMUSICRP_DISCORD_SDK_LIBRARY=/path/to/libdiscord_partner_sdk.so`.
For Docker, the `-sdk` image and `compose.yaml` already set this. You only
mount the file.

Then restart the service (`sudo systemctl restart remotemusicrp`).

### Sign in

1. In the Developer Portal, open your app → **OAuth2**:
   - turn on **Public Client**;
   - under **Redirects**, add the address shown on RemoteMusicRP's
     **Connections** page. It's your web UI address followed by
     `/discord-callback`, for example `http://127.0.0.1:8484/discord-callback`.
2. In RemoteMusicRP, go to **Connections → Sign in with Discord** and
   approve.

You stay signed in after restarts. Only an encrypted renewal token is kept.
**Sign out** revokes it at Discord.

## 8. Opening the web UI from another computer

By default the web UI only answers on the same computer (`127.0.0.1`). To
open it from elsewhere:

- **Easiest and safest:** an SSH tunnel (see section 5).
- **Always on:** put a TLS reverse proxy in front. Examples ship in
  `examples/`: [Caddy](../packaging/examples/Caddyfile) (automatic
  certificates) and [nginx](../packaging/examples/nginx.conf). Then set
  `public_origin = "https://your.domain"` in the config. RemoteMusicRP
  refuses to listen on a non-local address without it.

An admin password is required as soon as the UI is reachable from another
computer. The first password needs a one-time setup code, which is printed in
the server log.

## 9. Updates

- **Windows, macOS, tarball and AppImage:** the app checks GitHub once a day
  and asks before installing. It only installs files signed with the key
  above, and rolls back automatically if the new version fails to start.
- **deb, rpm, Arch, Docker:** the app only tells you an update exists. Update
  with your package manager, or pull the new image.
