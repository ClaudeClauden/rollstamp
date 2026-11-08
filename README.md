# Rollstamp

[![CI](https://github.com/ClaudeClauden/rollstamp/actions/workflows/ci.yml/badge.svg)](https://github.com/ClaudeClauden/rollstamp/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/ClaudeClauden/rollstamp)](https://github.com/ClaudeClauden/rollstamp/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

**Date, place and metadata for scanned film rolls – a portable Windows tool (English / Deutsch).**

[Deutsche Anleitung → README.de.md](README.de.md)

You scanned your old negatives, one folder per film – but Google Photos, Immich or Lightroom sort
them by the day you scanned them. Rollstamp lets you give a whole film (or single frames) the right
date, an optional place and further metadata in a few clicks, and writes everything losslessly into
the files with ExifTool. Optionally it creates space-saving JPEG copies at the same time.

| Marker | Meaning |
|---|---|
| 🔴 red frame | no capture date |
| 🔵 blue frame | date set |
| 🟢 green frame | date **and** place |
| 🟡 dot | change planned, not written yet |
| `JPG` / ★ / ✖ | JPEG copy exists / rating / reject |

## Features

- **Whole film at once** – type `14.07.2001`, `07.2001` or just `2001`. Times count up from 12:00 one
  minute per frame, so the order is right in every photo app. Frames you drag to another position get new times.
- **Approximate dates** are stored the standard way: `XMP DateCreated` with reduced precision (`2001:07`)
  plus the keyword `circa` (configurable).
- **"From here +1 day"** for films that span several days.
- **Place search** by name via OpenStreetMap, recently used places, "show on map", reverse lookup.
- **Rotate losslessly** (EXIF orientation), **star ratings**, **reject** frames (XMP rating −1).
- **Film info**: photographer, rights and the original type per IPTC *Digital Source Type* (negative, slide, print).
- **JPEG copies** with ImageMagick: quality 92, full chroma 4:4:4, colour profile and all metadata kept,
  rotated correctly, rejects skipped, several in parallel.
- **Archive overview**: scans a folder and all subfolders and lists every film folder with missing dates,
  places, JPEGs and backups – click to open.
- **Undo** (Ctrl+Z), confirmation table "before → after" before anything is written,
  ExifTool backups (`*_original`) with a clean-up button.
- **Portable**: unzip and run, nothing is installed; settings stay in the program folder.
- German and English user interface (automatic or selectable).

## Download & start

1. Download `Rollstamp-vX.Y.Z-windows-x64.zip` from the [latest release](https://github.com/ClaudeClauden/rollstamp/releases/latest).
2. Unzip it anywhere (USB stick works too) and start `rollstamp.exe`.

The package contains ExifTool and a portable ImageMagick – nothing else is needed. Windows SmartScreen may
warn because the exe is not code-signed: *More info → Run anyway*.

## Keyboard & mouse

| Action | Effect |
|---|---|
| Click / Ctrl+click / Shift+click | select / multiple / range |
| Right-click | menu for the selected frames: date, location, recent places, description, rotate, rating, reject, discard |
| Double-click or 🔍 | large view (← → browse, Space select, Esc back) |
| Drag | move frame – times are renumbered |
| R / L | rotate right / left |
| 1–5 / 0 / X | stars / no rating / reject |
| Ctrl+A / Esc / Ctrl+Z | select all / clear selection / undo |

## What is written

| Field | Content |
|---|---|
| `EXIF:DateTimeOriginal`, `EXIF:CreateDate` | capture time (read by Google Photos) |
| `XMP-photoshop:DateCreated` | same date; month/year only as `2003:03` / `2003` |
| `XMP-dc:Subject` | keyword `circa` for approximate dates |
| `GPS:*` | coordinates (read by Google Photos) |
| `XMP-photoshop:City`, `Country` | place as text |
| `XMP-dc:Description`, `EXIF:ImageDescription` | description |
| `EXIF:Artist`, `XMP-dc:Creator` / `EXIF:Copyright`, `XMP-dc:Rights` | photographer / rights |
| `XMP-iptcExt:DigitalSourceType` | negative film, slide film or print (IPTC) |
| `IFD0:Orientation` | rotation (pixels untouched) |
| `XMP-xmp:Rating` | stars, −1 = reject |

## Privacy

Rollstamp works offline. Only the place search sends the search term or coordinates to
`nominatim.openstreetmap.org` – and only when you click *Search* / *Look up place name*. No telemetry.
Place data © OpenStreetMap contributors (ODbL).

## Build from source

Requirements: Windows, [Rust](https://rustup.rs), Visual Studio Build Tools with "Desktop development with C++".

```
Build.bat
```

builds `rollstamp.exe` and assembles the portable folder `dist\Rollstamp` (incl. ExifTool and ImageMagick in the
versions pinned in `tools/versions.txt`). `Build.bat -Latest` uses the newest tool versions instead.
Run the tests with `cargo test`.

## Maintenance & security

- **Dependabot** opens weekly PRs for Rust crates and GitHub Actions.
- **cargo-deny** checks RustSec advisories, licenses and sources on every push and weekly (`deny.toml`).
- **Bundled tools**: a weekly workflow compares `tools/versions.txt` with the latest ExifTool/ImageMagick
  and opens an issue when an update is available.
- **Release**: push a tag `vX.Y.Z` → GitHub Actions builds, tests and publishes the portable ZIP with SHA-256.

See [SECURITY.md](SECURITY.md).

## License

Rollstamp: [MIT](LICENSE). Bundled tools and Rust dependencies: see
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) and `THIRD-PARTY-LICENSES.html` in the release package.
