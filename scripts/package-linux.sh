#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
#
# Package one Linux build into every release format:
#   remotemusicrp-<target>                  raw binary (used by the updater)
#   remotemusicrp-<ver>-<target>.tar.gz     portable tarball
#   remotemusicrp-<target>.AppImage         AppImage (updater-aware)
#   remotemusicrp_<ver>-1_<arch>.deb        Debian/Ubuntu package
#   remotemusicrp-<ver>-1.<arch>.rpm        Fedora/RHEL package
#
#   scripts/package-linux.sh <version> <target> <out-dir>
#
# Expects: the release binary at target/<target>/release/remotemusicrp,
# THIRD-PARTY-LICENSES.html in the repo root, cargo-deb and
# cargo-generate-rpm installed, and $APPIMAGETOOL pointing to a verified
# appimagetool AppImage for this architecture.
set -euo pipefail

version=${1:?version}
target=${2:?target triple}
out=${3:?output folder}
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
mkdir -p "$out"
out=$(cd "$out" && pwd)

bin="target/$target/release/remotemusicrp"
[[ -x $bin ]] || { echo "missing $bin" >&2; exit 1; }
[[ -f THIRD-PARTY-LICENSES.html ]] || { echo "missing THIRD-PARTY-LICENSES.html" >&2; exit 1; }
case $target in
  x86_64-*) arch=x86_64 ;;
  aarch64-*) arch=aarch64 ;;
  *) echo "unsupported target $target" >&2; exit 1 ;;
esac
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# 1. Raw binary for the self-updater.
install -m755 "$bin" "$out/remotemusicrp-$target"

# 2. Tarball.
name="remotemusicrp-$version-$target"
stage="$tmp/$name"
install -Dm755 "$bin" "$stage/remotemusicrp"
install -Dm644 LICENSE "$stage/LICENSE"
install -Dm644 THIRD-PARTY-LICENSES.html "$stage/THIRD-PARTY-LICENSES.html"
install -Dm644 docs/INSTALL.md "$stage/INSTALL.md"
install -Dm644 packaging/linux/remotemusicrp.service "$stage/packaging/remotemusicrp.service"
install -Dm644 packaging/linux/remotemusicrp.toml "$stage/packaging/remotemusicrp.toml"
install -Dm644 packaging/examples/Caddyfile "$stage/examples/Caddyfile"
install -Dm644 packaging/examples/nginx.conf "$stage/examples/nginx.conf"
# Reproducible-ish: fixed owner, order and mtime.
tar --sort=name --owner=0 --group=0 --numeric-owner \
  --mtime="@${SOURCE_DATE_EPOCH:-0}" -C "$tmp" -czf "$out/$name.tar.gz" "$name"

# 3. AppImage. The runtime is taken from the pinned appimagetool itself, so
#    nothing unpinned is downloaded at build time.
tool=${APPIMAGETOOL:?set APPIMAGETOOL to a verified appimagetool AppImage}
appdir="$tmp/AppDir"
install -Dm755 "$bin" "$appdir/usr/bin/remotemusicrp"
install -Dm644 packaging/linux/remotemusicrp.desktop "$appdir/remotemusicrp.desktop"
install -Dm644 packaging/linux/remotemusicrp.desktop "$appdir/usr/share/applications/remotemusicrp.desktop"
install -Dm644 packaging/linux/remotemusicrp.svg "$appdir/remotemusicrp.svg"
install -Dm644 packaging/linux/remotemusicrp.svg "$appdir/usr/share/icons/hicolor/scalable/apps/remotemusicrp.svg"
install -Dm644 LICENSE "$appdir/usr/share/licenses/remotemusicrp/LICENSE"
install -Dm644 THIRD-PARTY-LICENSES.html "$appdir/usr/share/licenses/remotemusicrp/THIRD-PARTY-LICENSES.html"
ln -s usr/bin/remotemusicrp "$appdir/AppRun"
export APPIMAGE_EXTRACT_AND_RUN=1
offset=$("$tool" --appimage-offset)
head -c "$offset" "$tool" > "$tmp/runtime"
ARCH=$arch "$tool" --no-appstream --runtime-file "$tmp/runtime" "$appdir" \
  "$out/remotemusicrp-$target.AppImage" >/dev/null
chmod 755 "$out/remotemusicrp-$target.AppImage"

# 4. deb and rpm (same binary; the systemd unit marks it as a package install).
cargo deb --no-build --no-strip -p remotemusicrp --target "$target" --output "$out/" >/dev/null
cargo generate-rpm -p crates/app --target "$target" -o "$out/" >/dev/null

ls -1 "$out"
