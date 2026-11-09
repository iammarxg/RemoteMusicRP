# Security Policy

Security is RemoteMusicRP's top priority. Thank you for helping keep its users safe.

## Reporting a vulnerability

**Please do not open a public issue for security problems.**

Report privately through GitHub's private vulnerability reporting:
https://github.com/iammarxg/RemoteMusicRP/security/advisories/new

Please include:
- the affected version and install type (desktop exe, deb/rpm/Arch, tarball, Docker)
- steps to reproduce, or a proof of concept
- the impact you observed or expect

## What to expect

- We acknowledge reports within **72 hours**.
- We give an initial assessment within **7 days**.
- For confirmed issues, we aim to release a fix within **30 days** (sooner for critical issues), then publish a GitHub Security Advisory crediting you, unless you prefer to stay anonymous.
- Please give us a reasonable time to fix the issue before disclosing it publicly.

## Supported versions

Only the **latest release** gets security fixes. The built-in updater (or your package manager or Docker image pull) is the supported way to stay current.

## Scope

In scope:
- the RemoteMusicRP application
- its release artifacts and Docker images
- its update mechanism

Out of scope:
- vulnerabilities in Discord, Last.fm or other third-party services themselves
- attacks that require an already-compromised owner OS account

The threat model is in [`docs/SECURITY.md`](docs/SECURITY.md).
