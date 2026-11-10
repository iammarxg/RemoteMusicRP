<!-- Edit before publishing. Add "[security]" anywhere to highlight a security release in the app. -->

## What's new

- …

## Downloads

**Desktop releases** need the Discord desktop app open on the same computer.
**Server releases** need no Discord app but only work in the experimental
server mode (Linux x86_64, with the Discord Social SDK you download yourself).

| System | File | Kind |
|---|---|---|
| Windows 11 x64 | `remotemusicrp-<version>-x86_64-pc-windows-msvc.zip`, or the single `.exe` | Desktop |
| macOS (Apple Silicon) | `remotemusicrp-<version>-aarch64-apple-darwin.tar.gz` | Desktop |
| Linux desktop (x86_64) | `remotemusicrp-x86_64-unknown-linux-gnu.AppImage` | Desktop |
| Debian / Ubuntu server (x86_64) | `remotemusicrp_<version>-1_amd64.deb` | **Server** |
| Fedora / RHEL server (x86_64) | `remotemusicrp-<version>-1.x86_64.rpm` | **Server** |
| Arch Linux server (x86_64) | `PKGBUILD` | **Server** |
| Any Linux (x86_64) | `remotemusicrp-<version>-x86_64-unknown-linux-gnu.tar.gz` | Either |
| Docker server (amd64) | `ammarx4/remotemusicrp:latest-sdk` | **Server** |
| Linux ARM64 | AppImage, `.deb`, `.rpm`, tarball (`aarch64`/`arm64`) | Desktop only; Discord has no official ARM64 Linux app |

Not sure which one? See [Which download do I need?](https://github.com/iammarxg/RemoteMusicRP/blob/main/docs/INSTALL.md#which-download-do-i-need).

Every file is signed. Check a download with
[minisign](https://jedisct1.github.io/minisign/):

```
minisign -Vm <file> -P RWQ0Snkh7EvjhsxdofSMvsiHaYLRSCY6wUHMQsrPAnNSQ2tXjnOqk+Ia
```

The Windows and macOS builds are not code-signed with a paid certificate, so
Windows SmartScreen and macOS Gatekeeper warn on first start. The install
guide shows how to allow them.

Third-party licences: `THIRD-PARTY-LICENSES.html`. Software bill of
materials: `remotemusicrp-<version>-sbom.cdx.tar.gz`.
