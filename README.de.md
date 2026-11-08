# Rollstamp

[![CI](https://github.com/ClaudeClauden/rollstamp/actions/workflows/ci.yml/badge.svg)](https://github.com/ClaudeClauden/rollstamp/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/ClaudeClauden/rollstamp)](https://github.com/ClaudeClauden/rollstamp/releases/latest)
[![Lizenz: MIT](https://img.shields.io/badge/Lizenz-MIT-blue)](LICENSE)

[English README → README.md](README.md)

**Datum, Ort und Metadaten für gescannte Filme – ein portables Windows-Programm (Deutsch / English).**

Du hast deine alten Negative gescannt, ein Ordner pro Film – aber Google Fotos, Immich oder Lightroom
sortieren sie nach dem Scan-Tag. Mit Rollstamp gibst du einem ganzen Film (oder einzelnen Bildern) mit
wenigen Klicks das richtige Datum, optional einen Ort und weitere Angaben. Alles wird verlustfrei per ExifTool
in die Dateien geschrieben, auf Wunsch entstehen gleichzeitig platzsparende JPEG-Kopien.

| Markierung | Bedeutung |
|---|---|
| 🔴 roter Rahmen | kein Aufnahmedatum |
| 🔵 blauer Rahmen | Datum vorhanden |
| 🟢 grüner Rahmen | Datum **und** Ort |
| 🟡 Punkt | Änderung vorgemerkt, noch nicht geschrieben |
| `JPG` / ★ / ✖ | JPEG-Kopie vorhanden / Bewertung / Ausschuss |

## Funktionen

- **Ganzer Film auf einmal** – `14.07.2001`, `07.2001` oder nur `2001` eingeben. Ab 12:00 bekommt jedes Bild
  eine Minute mehr, damit die Reihenfolge in jedem Fototool stimmt. Verschobene Bilder bekommen neue Uhrzeiten.
- **Ungefähre Daten** standardkonform: `XMP DateCreated` mit reduzierter Genauigkeit (`2001:07`) plus
  Schlagwort `circa` (einstellbar).
- **„Ab hier +1 Tag“** für Filme über mehrere Tage.
- **Ortssuche** per Name über OpenStreetMap, zuletzt verwendete Orte, „Auf Karte zeigen“, Ortsname zu Koordinaten.
- **Verlustfrei drehen** (EXIF-Ausrichtung), **Sterne**, **Ausschuss** markieren (XMP-Rating −1).
- **Film-Angaben**: Fotograf, Rechte und Art der Vorlage nach IPTC *Digital Source Type* (Negativ, Dia, Abzug).
- **JPEG-Kopien** mit ImageMagick: Qualität 92, volle Farbauflösung 4:4:4, Farbprofil und alle Metadaten bleiben,
  richtig gedreht, Ausschuss wird ausgelassen, mehrere gleichzeitig.
- **Archiv-Übersicht**: prüft einen Ordner samt Unterordnern und listet jeden Filmordner mit fehlenden Daten,
  Orten, JPEGs und Sicherungen – per Klick öffnen.
- **Rückgängig** (Strg+Z), Bestätigungstabelle „vorher → nachher“ vor jedem Schreiben,
  ExifTool-Sicherungen (`*_original`) mit Aufräum-Knopf.
- **Portabel**: entpacken und starten, nichts wird installiert, Einstellungen bleiben im Programmordner.
- Oberfläche auf Deutsch und Englisch (automatisch oder wählbar).

## Herunterladen & starten

1. `Rollstamp-vX.Y.Z-windows-x64.zip` aus dem [neuesten Release](https://github.com/ClaudeClauden/rollstamp/releases/latest) laden.
2. Beliebig entpacken (auch auf einen USB-Stick) und `rollstamp.exe` starten.

ExifTool und ein portables ImageMagick sind enthalten. SmartScreen kann warnen, weil die exe nicht signiert ist:
*Weitere Informationen → Trotzdem ausführen*.

## Tastatur & Maus

| Aktion | Wirkung |
|---|---|
| Klick / Strg+Klick / Shift+Klick | auswählen / mehrere / Bereich |
| Rechtsklick | Menü für die markierten Bilder: Datum, Standort, zuletzt verwendete Orte, Beschreibung, Drehen, Bewertung, Ausschuss, Verwerfen |
| Doppelklick oder 🔍 | Großansicht (← → blättern, Leertaste auswählen, Esc zurück) |
| Ziehen | Bild verschieben – Uhrzeiten werden neu gezählt |
| R / L | rechts / links drehen |
| 1–5 / 0 / X | Sterne / keine Bewertung / Ausschuss |
| Strg+A / Esc / Strg+Z | alle / Auswahl aufheben / rückgängig |

Welche Felder geschrieben werden, steht in der [englischen README](README.md#what-is-written).

## Datenschutz

Rollstamp arbeitet offline. Nur die Ortssuche schickt den Suchbegriff bzw. die Koordinaten an
`nominatim.openstreetmap.org` – und nur, wenn du auf *Suchen* / *Ortsname ermitteln* klickst. Keine Telemetrie.

## Selbst bauen

Voraussetzungen: Windows, [Rust](https://rustup.rs), Visual Studio Build Tools mit „Desktopentwicklung mit C++“.
`Build.bat` doppelklicken – das portable Programm entsteht in `dist\Rollstamp`. Tests: `cargo test`.

## Lizenz

Rollstamp: [MIT](LICENSE). Mitgelieferte Programme und Rust-Bibliotheken: siehe
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
