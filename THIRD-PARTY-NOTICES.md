# Third-party notices

Rollstamp itself is licensed under the MIT License (see `LICENSE`). The portable release
package additionally contains or uses the following third-party components.

## ExifTool (bundled in `exiftool/`)

- Author: Phil Harvey – <https://exiftool.org>
- Copyright 2003–2026, Phil Harvey
- License: "This is free software; you can redistribute it and/or modify it under the same terms
  as Perl itself" – i.e. the Artistic License or the GNU General Public License (version 1 or later),
  at your option. <https://dev.perl.org/licenses/>
- The Windows package contains a Perl runtime (`exiftool_files`), distributed under the same Perl terms.
- Rollstamp runs ExifTool as a separate program; it is not linked into `rollstamp.exe`.

## ImageMagick (bundled in `imagemagick/`)

- Copyright ImageMagick Studio LLC – <https://imagemagick.org>
- License: ImageMagick License – <https://imagemagick.org/script/license.php>
  (the license and notice files of the original package are included in `imagemagick/`).
- Rollstamp runs `magick.exe` as a separate program; it is not linked into `rollstamp.exe`.

## OpenStreetMap / Nominatim (online service, optional)

- The place search sends the search term (or the coordinates for "look up place name") to
  <https://nominatim.openstreetmap.org>, only when the user clicks the button.
- Data © OpenStreetMap contributors, available under the Open Database License (ODbL).
  <https://www.openstreetmap.org/copyright>
- Usage policy: <https://operations.osmfoundation.org/policies/nominatim/>

## Rust crates (compiled into `rollstamp.exe`)

The licenses of all Rust dependencies are listed in `THIRD-PARTY-LICENSES.html`, which the build
script generates with [cargo-about](https://github.com/EmbarkStudios/cargo-about) for every release.
The bundled egui fonts (Ubuntu font, Noto Emoji, emoji-icon-font) are covered by the Ubuntu Font
Licence 1.0 and the SIL Open Font License 1.1.
