#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
#
# Sign every release file in a folder with minisign, write SHA256SUMS, sign
# that too, then verify everything against the public key.
#
#   scripts/sign-release.sh <version> <folder> <public-key-file>
#
# The private key comes from the file named in $MINISIGN_KEY_FILE and its
# password from $MINISIGN_PASSWORD (empty for a key made with `-W`). Each
# signature carries the trusted comment the updater checks:
#   "remotemusicrp <version> <file name>"
set -euo pipefail

version=${1:?version}
dir=${2:?folder}
pub=$(cd "$(dirname "${3:?public key file}")" && pwd)/$(basename "$3")
key=${MINISIGN_KEY_FILE:?set MINISIGN_KEY_FILE}
key=$(cd "$(dirname "$key")" && pwd)/$(basename "$key")

[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]] || {
  echo "bad version: $version" >&2
  exit 1
}

cd "$dir"
shopt -s nullglob
files=()
for f in *; do
  [[ -f $f && $f != *.minisig && $f != SHA256SUMS ]] && files+=("$f")
done
((${#files[@]})) || { echo "no files to sign in $dir" >&2; exit 1; }

sign() { # file comment
  # minisign reads the password from stdin when stdin is not a terminal.
  printf '%s\n' "${MINISIGN_PASSWORD-}" |
    minisign -S -s "$key" -m "$1" -t "$2" -x "$1.minisig" >/dev/null
}

sha256sum -- "${files[@]}" > SHA256SUMS
for f in "${files[@]}"; do
  sign "$f" "remotemusicrp $version $f"
done
sign SHA256SUMS "remotemusicrp $version SHA256SUMS"

# Never publish something that doesn't verify.
for f in "${files[@]}" SHA256SUMS; do
  minisign -V -q -p "$pub" -m "$f" -x "$f.minisig"
done
sha256sum -c --quiet SHA256SUMS
echo "signed and verified ${#files[@]} files + SHA256SUMS"
