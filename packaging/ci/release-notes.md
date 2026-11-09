<!-- Edit before publishing. Add "[security]" anywhere to highlight a security release in the app. -->

## What's new

- …

## Downloads

| System | File |
|---|---|
| Windows 11 x64 | `remotemusicrp-<version>-x86_64-pc-windows-msvc.zip` (run `remotemusicrp-desktop.exe`) |
| macOS (Apple Silicon) | `remotemusicrp-<version>-aarch64-apple-darwin.tar.gz` |
| Linux desktop | `remotemusicrp-<target>.AppImage` |
| Debian / Ubuntu | `remotemusicrp_<version>-1_<arch>.deb` |
| Fedora / RHEL | `remotemusicrp-<version>-1.<arch>.rpm` |
| Arch Linux | `PKGBUILD` |
| Any Linux | `remotemusicrp-<version>-<target>.tar.gz` |
| Docker | `ammarx4/remotemusicrp` (see the install guide) |

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
