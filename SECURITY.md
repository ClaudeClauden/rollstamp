# Security policy

## Reporting a vulnerability

Please do **not** open a public issue. Use GitHub's private vulnerability reporting
(*Security → Report a vulnerability*) in this repository. You will get an answer within a few days.

Only the latest release is supported with fixes.

## How dependencies are kept up to date

| Component | Check | Automation |
|---|---|---|
| Rust crates | [RustSec](https://rustsec.org) advisories via `cargo-deny` on every push/PR and weekly | `.github/workflows/ci.yml` |
| Rust crates | update PRs | Dependabot (`.github/dependabot.yml`), weekly |
| GitHub Actions | update PRs | Dependabot, weekly |
| ExifTool, ImageMagick (bundled) | pinned in `tools/versions.txt`, weekly comparison with the latest release | `.github/workflows/tool-updates.yml` opens an issue |

When an advisory or tool update appears: update the dependency/version, let CI pass, push a new
tag `vX.Y.Z` – the release workflow builds and publishes the new portable package.

Please also enable *Dependabot alerts* and *Dependabot security updates* in the repository
settings (Settings → Code security).

## Network access

Rollstamp works offline. Only the optional place search contacts
`nominatim.openstreetmap.org`, and only when the user clicks *Search* or *Look up place name*.
No telemetry.
