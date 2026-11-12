<!-- SPDX-License-Identifier: AGPL-3.0-only -->
# Releasing RemoteMusicRP

How a version goes from `main` to users: what to do by hand, what the
workflows do, and how to recover when something breaks.

```
 you                        GitHub Actions                       users
 ───                        ──────────────                       ─────
 bump version, commit
 push main ───────────────► CI (fmt, clippy, tests, deny)
 push tag vX.Y.Z ─────────► Release: build 4 targets ─► sign ─► DRAFT release
 edit notes, check, publish ─────────────────────────────────────► updater sees it
                            Docker: verify signed binaries ─► push + cosign ─► Docker Hub
```

Nothing reaches users until **you** publish the draft: the in-app updater and
the Docker workflow both ignore drafts.

## 1. One-time setup

| What | Where | Used by |
|---|---|---|
| `MINISIGN_SECRET_KEY` (the full contents of `remotemusicrp.key`) and `MINISIGN_PASSWORD` | Repository → Settings → Environments → **`release`** → secrets | The final "Sign and publish draft release" job only |
| `DOCKERHUB_USERNAME`, `DOCKERHUB_TOKEN` (an access token limited to Read & Write on `ammarx4/remotemusicrp`) | Repository secrets | Docker workflow |
| Public key `remotemusicrp.pub` (key ID `86E34BEC21794A34`) | Committed in the repo root | Embedded in every release binary; used to verify before publishing and in the Docker workflow |

Keep an offline backup of the private key and its password. **Don't replace
the key casually:** every installed copy only trusts the key it was built
with, so releases signed with a new key can't be installed by the updater.
Changing it needs a transition release (signed with the old key, embedding
the new one) and a decision log entry in `CLAUDE.md`.

## 2. Before a release

- `main` is green in CI (fmt, clippy with and without `social-sdk`, tests on
  Windows/macOS/Linux, web build, `cargo deny`).
- Re-check what `CLAUDE.md` asks for before each release:
  - the platform support matrix (§8), dropping versions that no longer get
    vendor security updates;
  - Apple's iTunes Search API terms (album art, §6).
- Anything user-visible since the last release is in the `CLAUDE.md`
  Decisions log. It's the easiest source for the release notes.

## 3. Cut the release

Replace `X.Y.Z` with the new version (no `v`).

1. Bump the version in the four files a release commit touches:

   ```bash
   sed -i 's/^version = ".*"$/version = "X.Y.Z"/' Cargo.toml
   ```
   ```bash
   (cd web && npm version X.Y.Z --no-git-tag-version)
   ```
   ```bash
   cargo update -w
   ```

   That changes `Cargo.toml` (`[workspace.package]`), `Cargo.lock`,
   `web/package.json` and `web/package-lock.json`. Every crate inherits the
   workspace version.

2. Check it builds, then commit and tag. Tags are annotated, named
   `vX.Y.Z`, with the message `RemoteMusicRP X.Y.Z`:

   ```bash
   cargo check --workspace
   ```
   ```bash
   git commit -am "Release X.Y.Z"
   ```
   ```bash
   git tag -a vX.Y.Z -m "RemoteMusicRP X.Y.Z"
   ```

3. Push `main` first, then the tag (the tag starts the Release workflow):

   ```bash
   git push origin main
   ```
   ```bash
   git push origin vX.Y.Z
   ```

A full run takes about 10 minutes. Watch it under **Actions → Release**.

## 4. What the Release workflow does

`.github/workflows/release.yml`, triggered by a tag matching
`v[0-9]+.[0-9]+.[0-9]+`. All actions are pinned by commit SHA, the default
token is read-only, and no build cache is used.

1. **prepare** (Ubuntu)
   - Fails unless the tag equals the version in `Cargo.toml`.
   - Builds the web UI (`npm ci`, `npm run build`); it's embedded in every
     binary.
   - Makes `THIRD-PARTY-LICENSES.html` (`cargo about`) and a CycloneDX SBOM
     (`remotemusicrp-X.Y.Z-sbom.cdx.tar.gz`).
2. **build**, four runners in parallel (`fail-fast: false`, so one broken
   platform doesn't hide another's error). Each embeds the public key
   (`REMOTEMUSICRP_UPDATE_PUBKEY`) and uses the commit time as
   `SOURCE_DATE_EPOCH`.

   | Target | Runner | Produces |
   |---|---|---|
   | `x86_64-pc-windows-msvc` | windows-2025 | `remotemusicrp-x86_64-pc-windows-msvc.exe`, and a zip with it plus `remotemusicrp-desktop.exe` (no-console launcher), licences and `INSTALL.md` |
   | `aarch64-apple-darwin` | macos-15 | Raw binary and `.tar.gz` |
   | `x86_64-unknown-linux-gnu` (with `social-sdk`) | ubuntu-24.04 | Raw binary, tarball, AppImage, `.deb`, `.rpm` (`scripts/package-linux.sh`) |
   | `aarch64-unknown-linux-gnu` | ubuntu-24.04-arm | Same as above |

   Linux builds link against **glibc 2.28** through `cargo zigbuild` (zig
   installed from hash-pinned wheels). The AppImage runtime comes from
   appimagetool 1.9.1, checked against a pinned SHA-256. The raw binaries
   are what the in-app updater downloads.
3. **publish** (Ubuntu, environment `release`, the only job with the signing
   secrets and `contents: write`)
   - Adds the licence report, SBOM and an Arch `PKGBUILD` filled in with the
     tarballs' SHA-256 (`packaging/arch/PKGBUILD.in`).
   - Installs minisign from Ubuntu's archive and runs
     `scripts/sign-release.sh`. That signs every file and a `SHA256SUMS` with
     the trusted comment `remotemusicrp X.Y.Z <file name>`, then verifies
     everything against `remotemusicrp.pub` and fails if anything doesn't
     verify. The key file is written with `umask 077` and shredded when the
     step ends.
   - Creates the **draft** release `vX.Y.Z` (`--verify-tag`) with the notes
     template from `packaging/ci/release-notes.md`.

## 5. Check and publish the draft

1. Open the draft under **Releases**.
2. Replace the `## What's new` placeholder (and the HTML comment) in the
   notes. Put `[security]` anywhere in the notes to make the app highlight
   the update as a security release.
3. Optionally download a file and verify it independently (needs
   [minisign](https://jedisct1.github.io/minisign/)):

   ```bash
   minisign -Vm remotemusicrp-x86_64-pc-windows-msvc.exe -P RWQ0Snkh7EvjhsxdofSMvsiHaYLRSCY6wUHMQsrPAnNSQ2tXjnOqk+Ia
   ```

   The output must show `Trusted comment: remotemusicrp X.Y.Z <that file>`.
4. Click **Publish release**.

## 6. After publishing

- **Docker** (`.github/workflows/docker.yml`) starts on its own when the
  release is published. It never builds from source:
  1. downloads the two Linux binaries and `SHA256SUMS` from the release;
  2. checks each minisign signature and its trusted comment, and the
     checksums;
  3. builds `packaging/docker/Dockerfile` and pushes to Docker Hub, with
     BuildKit SBOM and provenance:
     - `ammarx4/remotemusicrp:X.Y.Z` and `:latest`: distroless,
       amd64 + arm64;
     - `:X.Y.Z-sdk` and `:latest-sdk`: Debian slim with the libraries the
       Discord Social SDK needs, amd64 only, and the SDK itself not
       included;
  4. signs both images with cosign (keyless, tied to this workflow).

  Check a pushed image:

  ```bash
  cosign verify docker.io/ammarx4/remotemusicrp:X.Y.Z --certificate-identity-regexp '^https://github.com/iammarxg/RemoteMusicRP/\.github/workflows/docker\.yml@' --certificate-oidc-issuer https://token.actions.githubusercontent.com
  ```

- **In-app updates:** Windows, macOS, tarball and AppImage installs check
  GitHub about once a day. They offer the update (or install it, if the owner
  chose silent updates). Before replacing anything they verify the
  signature, the trusted comment (version and file name) and the checksum.
  They roll back automatically after 3 failed starts. Package (deb/rpm/Arch)
  and Docker installs only show "update available".
- **Arch:** the `PKGBUILD` is attached to the release. Publishing it to the
  AUR, if wanted, is a separate manual step.

## 7. When something goes wrong

| Problem | Fix |
|---|---|
| "tag vX.Y.Z does not match Cargo.toml version" | The version bump wasn't in the tagged commit. Delete the tag (below), fix, tag again. |
| One platform failed for a flaky reason (runner, network) | **Re-run failed jobs** on the run. `publish` waits for every build. |
| A build needs a code fix | Delete the draft release and the tag, fix on `main`, then tag again (below). |
| Signing failed | Check that the `release` environment secrets are present and match `remotemusicrp.pub`; the script refuses to publish anything that doesn't verify. |
| Docker failed or needs a rebuild | **Actions → Docker → Run workflow** with the tag (e.g. `vX.Y.Z`). It only works for a published release. |
| A bad release is already published | Publish a fixed `X.Y.Z+1`. Don't move or reuse tags: installed copies may already have that version. |

To delete a tag that has **not** been published (draft release deleted
first):

```bash
git push origin :refs/tags/vX.Y.Z
```
```bash
git tag -d vX.Y.Z
```

## 8. Related files

- `.github/workflows/release.yml`, `docker.yml`, `ci.yml`
- `scripts/sign-release.sh` (signing), `scripts/package-linux.sh` (Linux
  formats), `scripts/make-update-fixture.sh` (test fixture with a throwaway
  key)
- `packaging/ci/release-notes.md` (notes template),
  `packaging/ci/zig-requirements.txt` (hash-pinned zig),
  `packaging/arch/PKGBUILD.in`, `packaging/docker/`, `packaging/linux/`
- `remotemusicrp.pub` (release public key)
