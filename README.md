# RemoteMusicRP

Show what you're playing on Last.fm as your Discord status, from any device
that scrobbles. Listen on your phone, and your Discord profile shows the
song, artist, album art and a progress bar.

- **Free and open source** (AGPL-3.0). No accounts, no hosted service, no
  telemetry.
- **Self-hosted:** run it on your PC (Windows, macOS, Linux) or on a server
  or Docker.
- **Your own keys:** you use your own Discord application and Last.fm API
  key. They're stored encrypted on your machine and never leave it, except
  to talk to Discord and Last.fm.
- **Light:** about 20 MB of memory and no measurable CPU when idle.

## Install

See **[docs/INSTALL.md](docs/INSTALL.md)**. Downloads are on the
[releases page](https://github.com/iammarxg/RemoteMusicRP/releases), and
every file is signed:

```
minisign -Vm <file> -P RWQ0Snkh7EvjhsxdofSMvsiHaYLRSCY6wUHMQsrPAnNSQ2tXjnOqk+Ia
```

## How it works

RemoteMusicRP asks Last.fm what you're playing every few seconds and
sends it to Discord as Rich Presence, through one of:

- the **Discord app on the same computer** (desktop releases: Windows,
  macOS, Linux AppImage), or
- **server mode** (experimental, Linux x86_64 server releases: `.deb`,
  `.rpm`, Arch, Docker `-sdk`): Discord's official Social SDK, so no
  Discord app is needed. You download the SDK yourself.

[Which download do I need?](docs/INSTALL.md#which-download-do-i-need)

Album art comes from the iTunes Search API and the MusicBrainz Cover Art
Archive. Data comes from Last.fm (powered by AudioScrobbler).

## Contributing and security

- [CONTRIBUTING.md](CONTRIBUTING.md)
- Report vulnerabilities privately: [SECURITY.md](SECURITY.md)
- Design and decisions: [CLAUDE.md](CLAUDE.md) and [docs/](docs/)

RemoteMusicRP isn't affiliated with Discord or Last.fm.
