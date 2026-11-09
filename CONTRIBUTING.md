# Contributing to RemoteMusicRP

Thanks for helping! RemoteMusicRP is free, self-hosted software licensed under the **AGPL-3.0**. By submitting a contribution you agree it is licensed under the same terms. No DCO sign-off or CLA is required.

## Before you start

- Read [`CLAUDE.md`](CLAUDE.md). It is the single source of truth for rules, architecture and decisions. The rules that matter most:
  - **Security first, then low resource use.**
  - **Discord compliance:** only official mechanisms. No user tokens, self-bots or undocumented endpoints.
  - **No scraping** of any website. Use documented public APIs only.
  - **No telemetry**, and no new outbound hosts without discussion.
- Security issues: **do not open a public issue**. See [`SECURITY.md`](SECURITY.md).

## Development

You need Rust. The toolchain is pinned in `rust-toolchain.toml`, so `rustup` picks it up automatically.

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The dependency policy is checked with [cargo-deny](https://github.com/EmbarkStudios/cargo-deny):

```bash
cargo deny check
```

## Guidelines

- `#![forbid(unsafe_code)]` in every crate.
- Keep secrets in `secrecy`/`zeroize` types. Never log secrets or put them in error messages.
- Keep dependencies minimal, and explain any new one in your PR.
- Unit-test crypto, auth, config parsing and provider parsing thoroughly. Elsewhere, use mocks plus a small smoke test.
- Keep commits small and focused.
- New source files start with `// SPDX-License-Identifier: AGPL-3.0-only`.
