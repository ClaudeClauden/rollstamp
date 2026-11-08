//! Core logic without GUI, network or process calls – fully covered by `cargo test`.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering as AtomicOrdering};

// ---------------------------------------------------------------------------
// Language (German / English)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    De,
    En,
}

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn set_lang(l: Lang) {
    LANG.store(if l == Lang::En { 1 } else { 0 }, AtomicOrdering::Relaxed);
}

pub fn lang() -> Lang {
    if LANG.load(AtomicOrdering::Relaxed) == 1 {
        Lang::En
    } else {
        Lang::De
    }
}

pub fn is_de() -> bool {
    lang() == Lang::De
}

/// Picks the text for the current language.
pub fn tr(de: &'static str, en: &'static str) -> &'static str {
    if is_de() {
        de
    } else {
        en
    }
}

/// Like `tr`, for formatted texts.
pub fn trs(de: String, en: String) -> String {
    if is_de() {
        de
    } else {
        en
    }
}

/// Keywords for approximate dates. All are recognised when reading and replaced
/// by the configured one when writing.
pub const ESTIMATE_PRESETS: [&str; 3] = ["circa", "Datum geschätzt", "date approximate"];
pub const DEFAULT_ESTIMATE_KEYWORD: &str = "circa";

// ---------------------------------------------------------------------------
// Date & time (deliberately without external crates)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Date {
    pub y: i32,
    pub m: u32,
    pub d: u32,
}

pub fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

impl Date {
    pub fn new(y: i32, m: u32, d: u32) -> Option<Self> {
        if !(1800..=2200).contains(&y) || !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
            return None;
        }
        Some(Self { y, m, d })
    }

    /// Adds (or subtracts) whole days.
    pub fn add_days(self, n: i64) -> Option<Self> {
        let (mut y, mut m, mut d) = (self.y, self.m, self.d as i64 + n);
        while d > days_in_month(y, m) as i64 {
            d -= days_in_month(y, m) as i64;
            m += 1;
            if m > 12 {
                m = 1;
                y += 1;
            }
        }
        while d < 1 {
            if m == 1 {
                m = 12;
                y -= 1;
            } else {
                m -= 1;
            }
            d += days_in_month(y, m) as i64;
        }
        Date::new(y, m, d as u32)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DateTime {
    pub date: Date,
    pub h: u32,
    pub min: u32,
    pub s: u32,
}

impl DateTime {
    /// EXIF/ExifTool format: "YYYY:MM:DD hh:mm:ss"
    pub fn exif(&self) -> String {
        format!(
            "{:04}:{:02}:{:02} {:02}:{:02}:{:02}",
            self.date.y, self.date.m, self.date.d, self.h, self.min, self.s
        )
    }

    /// Display: "14.07.2001 12:03" (German) / "2001-07-14 12:03" (English)
    pub fn display(&self) -> String {
        format!("{} {:02}:{:02}", fmt_day(self.date), self.h, self.min)
    }

    pub fn time_str(&self) -> String {
        format!("{:02}:{:02}", self.h, self.min)
    }

    /// Reads "YYYY:MM:DD hh:mm:ss" (also '-' in the date; time zone/sub-seconds are ignored).
    pub fn parse_exif(s: &str) -> Option<Self> {
        let s = s.trim();
        let date_part = s.get(0..10)?;
        let mut it = date_part.split(|c| c == ':' || c == '-');
        let y: i32 = it.next()?.parse().ok()?;
        let m: u32 = it.next()?.parse().ok()?;
        let d: u32 = it.next()?.parse().ok()?;
        let date = Date::new(y, m, d)?;
        let (mut h, mut min, mut sec) = (0, 0, 0);
        if let Some(t) = s.get(11..19) {
            let parts: Vec<u32> = t.split(':').filter_map(|p| p.parse().ok()).collect();
            if parts.len() == 3 && parts[0] < 24 && parts[1] < 60 && parts[2] < 60 {
                h = parts[0];
                min = parts[1];
                sec = parts[2];
            }
        }
        Some(Self { date, h, min, s: sec })
    }
}

fn fmt_day(d: Date) -> String {
    if is_de() {
        format!("{:02}.{:02}.{:04}", d.d, d.m, d.y)
    } else {
        format!("{:04}-{:02}-{:02}", d.y, d.m, d.d)
    }
}

fn fmt_month(d: Date) -> String {
    if is_de() {
        format!("{:02}/{:04}", d.m, d.y)
    } else {
        format!("{:04}-{:02}", d.y, d.m)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Precision {
    Day,
    Month,
    Year,
}

impl Precision {
    pub fn label(self) -> &'static str {
        match self {
            Precision::Day => tr("genauer Tag", "exact day"),
            Precision::Month => tr("nur Monat bekannt", "month only"),
            Precision::Year => tr("nur Jahr bekannt", "year only"),
        }
    }

    /// Precision from XMP-photoshop:DateCreated: "2003" → year, "2003:03" → month, otherwise day.
    pub fn from_xmp(s: &str) -> Option<Self> {
        let date_part = s.trim().split(|c: char| c == ' ' || c == 'T').next().unwrap_or("");
        let parts = date_part
            .split(|c| c == ':' || c == '-')
            .filter(|p| !p.trim().is_empty())
            .count();
        match parts {
            0 => None,
            1 => Some(Precision::Year),
            2 => Some(Precision::Month),
            _ => Some(Precision::Day),
        }
    }
}

pub fn describe_date(d: Date, p: Precision) -> String {
    match p {
        Precision::Day => fmt_day(d),
        Precision::Month => fmt_month(d),
        Precision::Year => format!("{:04}", d.y),
    }
}

/// Text for the date input field (reversible with `parse_date_input`).
pub fn date_input_text(d: Date, p: Precision) -> String {
    match p {
        Precision::Day => format!("{:02}.{:02}.{:04}", d.d, d.m, d.y),
        Precision::Month => format!("{:02}.{:04}", d.m, d.y),
        Precision::Year => format!("{:04}", d.y),
    }
}

/// A point in time incl. precision – for display and comparison.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct When {
    pub dt: DateTime,
    pub estimated: bool,
    pub precision: Precision,
}

impl When {
    pub fn label(&self) -> String {
        let t = self.dt.time_str();
        match (self.precision, self.estimated) {
            (Precision::Day, false) => self.dt.display(),
            (Precision::Day, true) => format!("ca. {} · {t}", fmt_day(self.dt.date)),
            (Precision::Month, _) => format!("ca. {} · {t}", fmt_month(self.dt.date)),
            (Precision::Year, _) => format!("ca. {:04} · {t}", self.dt.date.y),
        }
    }
}

/// A planned date for a group of images. The time counts up one minute per image from `start`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DateSpec {
    pub date: Date,
    pub precision: Precision,
    pub start_h: u32,
    pub start_min: u32,
    pub estimated: bool,
}

impl DateSpec {
    /// Month/year are always approximate.
    pub fn new(date: Date, precision: Precision, start: (u32, u32), estimated: bool) -> Self {
        Self {
            date,
            precision,
            start_h: start.0,
            start_min: start.1,
            estimated: estimated || precision != Precision::Day,
        }
    }

    fn group_key(&self) -> (Date, Precision, u32, u32) {
        (self.date, self.precision, self.start_h, self.start_min)
    }

    /// Time for the n-th image of the group (start + n minutes).
    pub fn at_offset(&self, n: u32) -> DateTime {
        let total = self.start_h * 60 + self.start_min + n;
        if total < 24 * 60 {
            DateTime {
                date: self.date,
                h: total / 60,
                min: total % 60,
                s: 0,
            }
        } else {
            let extra = total - (24 * 60 - 1);
            DateTime {
                date: self.date,
                h: 23,
                min: 59,
                s: extra.min(59),
            }
        }
    }

    /// XMP-photoshop:DateCreated may be partial ("2001" or "2001:07") – this keeps the real precision.
    pub fn xmp_date(&self, dt: &DateTime) -> String {
        match self.precision {
            Precision::Year => format!("{:04}", self.date.y),
            Precision::Month => format!("{:04}:{:02}", self.date.y, self.date.m),
            Precision::Day => dt.exif(),
        }
    }

    pub fn describe(&self) -> String {
        describe_date(self.date, self.precision)
    }
}

// ---------------------------------------------------------------------------
// Parsing user input
// ---------------------------------------------------------------------------

/// Accepts "14.07.2001", "14.07.01", "07.2001", "2001", "2001-07-14", "2001-07".
/// Month → 1st of month, year → July 1st.
pub fn parse_date_input(input: &str) -> Result<(Date, Precision), String> {
    let s = input.trim();
    if s.is_empty() {
        return Err(tr("Bitte ein Datum eingeben.", "Please enter a date.").into());
    }
    let parts: Vec<&str> = if s.contains('-') && s.split('-').next().map_or(false, |p| p.trim().len() == 4) {
        let mut v: Vec<&str> = s.split('-').map(str::trim).collect();
        v.reverse();
        v
    } else {
        s.split(|c| c == '.' || c == '/' || c == ' ')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect()
    };

    let bad = |p: &str| {
        trs(
            format!("„{p}“ ist keine gültige Zahl."),
            format!("“{p}” is not a valid number."),
        )
    };
    let num = |p: &str| -> Result<u32, String> {
        if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) {
            p.parse::<u32>().map_err(|_| bad(p))
        } else {
            Err(bad(p))
        }
    };
    let year = |p: &str| -> Result<i32, String> {
        let v = num(p)?;
        match p.len() {
            4 => Ok(v as i32),
            2 => Ok(if v <= 49 { 2000 + v as i32 } else { 1900 + v as i32 }),
            _ => Err(trs(
                format!("„{p}“ ist kein gültiges Jahr."),
                format!("“{p}” is not a valid year."),
            )),
        }
    };
    let invalid = || tr("Dieses Datum gibt es nicht.", "This date does not exist.").to_string();

    match parts.as_slice() {
        [y] => {
            let y = year(y)?;
            Date::new(y, 7, 1).map(|d| (d, Precision::Year)).ok_or_else(invalid)
        }
        [m, y] => {
            let m = num(m)?;
            let y = year(y)?;
            Date::new(y, m, 1).map(|d| (d, Precision::Month)).ok_or_else(invalid)
        }
        [d, m, y] => {
            let d = num(d)?;
            let m = num(m)?;
            let y = year(y)?;
            Date::new(y, m, d).map(|d| (d, Precision::Day)).ok_or_else(invalid)
        }
        _ => Err(tr(
            "Format: TT.MM.JJJJ, MM.JJJJ oder JJJJ",
            "Format: DD.MM.YYYY, MM.YYYY or YYYY",
        )
        .into()),
    }
}

/// "12:00", "12", "9.30" → (hour, minute). Empty = 12:00.
pub fn parse_time(input: &str) -> Result<(u32, u32), String> {
    let s = input.trim();
    if s.is_empty() {
        return Ok((12, 0));
    }
    let err = || tr("Uhrzeit bitte als hh:mm angeben.", "Please enter the time as hh:mm.").to_string();
    let parts: Vec<&str> = s.split(|c| c == ':' || c == '.').map(str::trim).collect();
    if parts.len() > 2 {
        return Err(err());
    }
    let h: u32 = parts[0].parse().map_err(|_| err())?;
    let m: u32 = match parts.get(1) {
        Some(p) => p.parse().map_err(|_| err())?,
        None => 0,
    };
    if h > 23 || m > 59 {
        return Err(tr("Ungültige Uhrzeit.", "Invalid time.").into());
    }
    Ok((h, m))
}

/// Coordinates as copied from Google Maps: "53.45123, 5.77012" – also "53,45 5,77" or "53.45;5.77".
pub fn parse_coords(input: &str) -> Result<(f64, f64), String> {
    let s = input.trim().replace('°', "");
    if s.is_empty() {
        return Err(tr("Bitte Koordinaten eingeben.", "Please enter coordinates.").into());
    }
    let format_err = || {
        tr(
            "Format: Breite, Länge – z. B. 53.4512, 5.7701",
            "Format: latitude, longitude – e.g. 53.4512, 5.7701",
        )
    };
    let (a, b): (String, String) = if let Some((a, b)) = s.split_once(';') {
        (a.trim().to_string(), b.trim().to_string())
    } else {
        let tokens: Vec<&str> = s
            .split_whitespace()
            .map(|t| t.trim_end_matches(','))
            .filter(|t| !t.is_empty())
            .collect();
        if tokens.len() == 2 {
            (tokens[0].to_string(), tokens[1].to_string())
        } else if s.matches(',').count() == 1 {
            let (a, b) = s.split_once(',').unwrap_or_default();
            (a.trim().to_string(), b.trim().to_string())
        } else {
            return Err(format_err().into());
        }
    };
    let parse = |t: &str| {
        t.replace(',', ".")
            .parse::<f64>()
            .map_err(|_| trs(format!("„{t}“ ist keine Zahl."), format!("“{t}” is not a number.")))
    };
    let lat = parse(&a)?;
    let lon = parse(&b)?;
    if !(-90.0..=90.0).contains(&lat) {
        return Err(tr(
            "Breitengrad muss zwischen -90 und 90 liegen.",
            "Latitude must be between -90 and 90.",
        )
        .into());
    }
    if !(-180.0..=180.0).contains(&lon) {
        return Err(tr(
            "Längengrad muss zwischen -180 und 180 liegen.",
            "Longitude must be between -180 and 180.",
        )
        .into());
    }
    Ok((lat, lon))
}

/// Tries to derive a date for the input field from folder names like
/// "2001-07-14 Ameland", "2001_07" or "14.07.2001".
pub fn guess_date_from_name(name: &str) -> Option<String> {
    let b = name.as_bytes();
    let is_d = |i: usize| i < b.len() && b[i].is_ascii_digit();
    let two = |i: usize| -> Option<u32> {
        if is_d(i) && is_d(i + 1) && !is_d(i + 2) {
            Some(((b[i] - b'0') as u32) * 10 + (b[i + 1] - b'0') as u32)
        } else {
            None
        }
    };
    let is_sep = |i: usize| i < b.len() && matches!(b[i], b'-' | b'_' | b'.' | b' ');

    let mut i = 0;
    while i + 4 <= b.len() {
        let starts_run = i == 0 || !is_d(i - 1);
        if starts_run && is_d(i) && is_d(i + 1) && is_d(i + 2) && is_d(i + 3) && !is_d(i + 4) {
            let y: i32 = name[i..i + 4].parse().ok()?;
            if (1900..=2099).contains(&y) {
                if is_sep(i + 4) {
                    if let Some(m) = two(i + 5) {
                        if (1..=12).contains(&m) {
                            if is_sep(i + 7) {
                                if let Some(d) = two(i + 8) {
                                    if Date::new(y, m, d).is_some() {
                                        return Some(format!("{d:02}.{m:02}.{y}"));
                                    }
                                }
                            }
                            return Some(format!("{m:02}.{y}"));
                        }
                    }
                }
                if i >= 3 && is_sep(i - 1) && is_d(i - 3) && is_d(i - 2) && (i == 3 || !is_d(i - 4)) {
                    let m = ((b[i - 3] - b'0') as u32) * 10 + (b[i - 2] - b'0') as u32;
                    if (1..=12).contains(&m) {
                        if i >= 6 && is_sep(i - 4) && is_d(i - 6) && is_d(i - 5) && (i == 6 || !is_d(i - 7)) {
                            let d = ((b[i - 6] - b'0') as u32) * 10 + (b[i - 5] - b'0') as u32;
                            if Date::new(y, m, d).is_some() {
                                return Some(format!("{d:02}.{m:02}.{y}"));
                            }
                        }
                        return Some(format!("{m:02}.{y}"));
                    }
                }
                return Some(format!("{y}"));
            }
        }
        i += 1;
    }
    None
}

// ---------------------------------------------------------------------------
// Natural sorting: "image 2" < "image 10"
// ---------------------------------------------------------------------------

enum Chunk<'a> {
    Num(&'a str),
    Text(&'a str),
}

fn chunks(s: &str) -> Vec<Chunk<'_>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut digit: Option<bool> = None;
    for (i, c) in s.char_indices() {
        let is_digit = c.is_ascii_digit();
        if let Some(prev) = digit {
            if prev != is_digit {
                let part = &s[start..i];
                out.push(if prev { Chunk::Num(part) } else { Chunk::Text(part) });
                start = i;
            }
        }
        digit = Some(is_digit);
    }
    if let Some(prev) = digit {
        let part = &s[start..];
        out.push(if prev { Chunk::Num(part) } else { Chunk::Text(part) });
    }
    out
}

pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let ca = chunks(a);
    let cb = chunks(b);
    for (x, y) in ca.iter().zip(cb.iter()) {
        let ord = match (x, y) {
            (Chunk::Num(nx), Chunk::Num(ny)) => {
                let tx = nx.trim_start_matches('0');
                let ty = ny.trim_start_matches('0');
                tx.len()
                    .cmp(&ty.len())
                    .then_with(|| tx.cmp(ty))
                    .then_with(|| nx.len().cmp(&ny.len()))
            }
            (Chunk::Text(tx), Chunk::Text(ty)) => tx.to_lowercase().cmp(&ty.to_lowercase()),
            (Chunk::Num(_), Chunk::Text(_)) => Ordering::Less,
            (Chunk::Text(_), Chunk::Num(_)) => Ordering::Greater,
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    ca.len().cmp(&cb.len()).then_with(|| a.cmp(b))
}

// ---------------------------------------------------------------------------
// Orientation & rating
// ---------------------------------------------------------------------------

/// EXIF orientation → clockwise quarter turns (mirrored variants are treated as unrotated).
pub fn quarter_turns(orientation: u8) -> u32 {
    match orientation {
        6 => 1,
        3 => 2,
        8 => 3,
        _ => 0,
    }
}

pub fn orientation_from_turns(turns: u32) -> u8 {
    match turns % 4 {
        1 => 6,
        2 => 3,
        3 => 8,
        _ => 1,
    }
}

pub fn rotate_orientation(orientation: u8, clockwise: bool) -> u8 {
    let t = quarter_turns(orientation);
    orientation_from_turns(if clockwise { t + 1 } else { t + 3 })
}

fn rotation_label(orientation: u8) -> String {
    format!("{}°", quarter_turns(orientation) * 90)
}

/// XMP rating: -1 = reject, 0 = none, 1–5 = stars.
pub fn rating_label(r: i8) -> String {
    match r {
        -1 => tr("Ausschuss", "Reject").into(),
        1..=5 => "★".repeat(r as usize),
        _ => "–".into(),
    }
}

// ---------------------------------------------------------------------------
// Film info (creator, rights, kind of original per IPTC)
// ---------------------------------------------------------------------------

/// IPTC "Digital Source Type" – international standard for what an image was digitised from.
const DST_BASE: &str = "http://cv.iptc.org/newscodes/digitalsourcetype/";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum SourceType {
    #[default]
    Unknown,
    NegativeFilm,
    PositiveFilm,
    Print,
}

impl SourceType {
    pub const ALL: [SourceType; 4] = [
        SourceType::Unknown,
        SourceType::NegativeFilm,
        SourceType::PositiveFilm,
        SourceType::Print,
    ];

    pub fn key(self) -> &'static str {
        match self {
            SourceType::Unknown => "",
            SourceType::NegativeFilm => "negativeFilm",
            SourceType::PositiveFilm => "positiveFilm",
            SourceType::Print => "print",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SourceType::Unknown => tr("keine Angabe", "not specified"),
            SourceType::NegativeFilm => tr("Negativfilm", "Negative film"),
            SourceType::PositiveFilm => tr("Diafilm (Positiv)", "Slide film (positive)"),
            SourceType::Print => tr("Papierabzug", "Photo print"),
        }
    }

    pub fn uri(self) -> Option<String> {
        match self {
            SourceType::Unknown => None,
            other => Some(format!("{DST_BASE}{}", other.key())),
        }
    }

    pub fn from_key(k: &str) -> Self {
        match k.trim() {
            "negativeFilm" => SourceType::NegativeFilm,
            "positiveFilm" => SourceType::PositiveFilm,
            "print" => SourceType::Print,
            _ => SourceType::Unknown,
        }
    }

    pub fn from_uri(u: &str) -> Self {
        Self::from_key(u.trim().trim_end_matches('/').rsplit('/').next().unwrap_or(""))
    }
}

/// Information that usually applies to a whole film. Empty fields = leave unchanged.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FilmInfo {
    pub creator: String,
    pub rights: String,
    pub source: SourceType,
}

impl FilmInfo {
    pub fn is_empty(&self) -> bool {
        self.creator.trim().is_empty() && self.rights.trim().is_empty() && self.source == SourceType::Unknown
    }

    /// Result of writing `self` over `base` (empty fields keep `base`).
    pub fn merged_over(&self, base: &FilmInfo) -> FilmInfo {
        let pick = |new: &str, old: &str| {
            if new.trim().is_empty() {
                old.trim().to_string()
            } else {
                new.trim().to_string()
            }
        };
        FilmInfo {
            creator: pick(&self.creator, &base.creator),
            rights: pick(&self.rights, &base.rights),
            source: if self.source == SourceType::Unknown {
                base.source
            } else {
                self.source
            },
        }
    }

    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if !self.creator.trim().is_empty() {
            parts.push(format!("{}: {}", tr("Fotograf", "Photographer"), self.creator.trim()));
        }
        if !self.rights.trim().is_empty() {
            parts.push(format!("{}: {}", tr("Rechte", "Rights"), self.rights.trim()));
        }
        if self.source != SourceType::Unknown {
            parts.push(self.source.label().to_string());
        }
        if parts.is_empty() {
            "–".into()
        } else {
            parts.join(" · ")
        }
    }
}

// ---------------------------------------------------------------------------
// State of an image
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Location {
    pub lat: f64,
    pub lon: f64,
    pub city: String,
    pub country: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Change<T> {
    Set(T),
    Remove,
}

/// What is currently stored in the file.
#[derive(Clone, Debug, PartialEq)]
pub struct Existing {
    pub datetime: Option<DateTime>,
    /// from XMP-photoshop:DateCreated ("2003" = year, "2003:03" = month)
    pub precision: Option<Precision>,
    pub estimated: bool,
    pub gps: Option<(f64, f64)>,
    pub city: String,
    pub country: String,
    pub description: String,
    pub film: FilmInfo,
    /// EXIF orientation 1–8
    pub orientation: u8,
    /// XMP rating (-1 = reject, 0 = none, 1–5)
    pub rating: i8,
}

impl Default for Existing {
    fn default() -> Self {
        Self {
            datetime: None,
            precision: None,
            estimated: false,
            gps: None,
            city: String::new(),
            country: String::new(),
            description: String::new(),
            film: FilmInfo::default(),
            orientation: 1,
            rating: 0,
        }
    }
}

/// Planned changes that have not been written yet. `None` = leave unchanged.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Edits {
    pub date: Option<Change<DateSpec>>,
    pub loc: Option<Change<Location>>,
    pub desc: Option<String>,
    pub film: Option<FilmInfo>,
    pub orientation: Option<u8>,
    pub rating: Option<i8>,
}

pub fn effective_orientation(ex: &Existing, ed: &Edits) -> u8 {
    ed.orientation.unwrap_or(ex.orientation)
}

pub fn effective_rating(ex: &Existing, ed: &Edits) -> i8 {
    ed.rating.unwrap_or(ex.rating)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// red: no date
    Missing,
    /// blue: date present
    Dated,
    /// green: date and location
    Located,
}

pub fn status(ex: &Existing, ed: &Edits) -> Status {
    let has_date = match &ed.date {
        Some(Change::Set(_)) => true,
        Some(Change::Remove) => false,
        None => ex.datetime.is_some(),
    };
    let has_gps = match &ed.loc {
        Some(Change::Set(_)) => true,
        Some(Change::Remove) => false,
        None => ex.gps.is_some(),
    };
    match (has_date, has_gps) {
        (false, _) => Status::Missing,
        (true, false) => Status::Dated,
        (true, true) => Status::Located,
    }
}

/// Planned times for all images (in current order). Images with the same date and start time
/// form a group and get start, start+1 min, start+2 min, …
pub fn planned_times<'a, I>(edits: I) -> Vec<Option<DateTime>>
where
    I: IntoIterator<Item = &'a Edits>,
{
    let mut counters: HashMap<(Date, Precision, u32, u32), u32> = HashMap::new();
    edits
        .into_iter()
        .map(|e| match &e.date {
            Some(Change::Set(spec)) => {
                let c = counters.entry(spec.group_key()).or_insert(0);
                let dt = spec.at_offset(*c);
                *c += 1;
                Some(dt)
            }
            _ => None,
        })
        .collect()
}

/// After reordering: images whose date is only stored in the file get a planned date so their
/// time follows the order again. Per day (and precision) counting restarts at the earliest existing
/// time; if a planned group for that day exists, its start time is used.
pub fn resequence_specs(items: &[(&Existing, &Edits)]) -> Vec<Option<DateSpec>> {
    let mut planned_start: HashMap<(Date, Precision), (u32, u32)> = HashMap::new();
    for (_, ed) in items {
        if let Some(Change::Set(s)) = &ed.date {
            planned_start
                .entry((s.date, s.precision))
                .or_insert((s.start_h, s.start_min));
        }
    }
    let mut min_existing: HashMap<(Date, Precision), (u32, u32)> = HashMap::new();
    for (ex, ed) in items {
        if ed.date.is_none() {
            if let Some(dt) = ex.datetime {
                let key = (dt.date, ex.precision.unwrap_or(Precision::Day));
                let t = (dt.h, dt.min);
                let e = min_existing.entry(key).or_insert(t);
                if t < *e {
                    *e = t;
                }
            }
        }
    }
    items
        .iter()
        .map(|(ex, ed)| {
            if ed.date.is_some() {
                return None;
            }
            let dt = ex.datetime?;
            let prec = ex.precision.unwrap_or(Precision::Day);
            let key = (dt.date, prec);
            let start = planned_start.get(&key).or_else(|| min_existing.get(&key)).copied()?;
            Some(DateSpec::new(dt.date, prec, start, ex.estimated))
        })
        .collect()
}

/// Date that would apply after writing.
pub fn effective_when(ex: &Existing, ed: &Edits, planned: Option<DateTime>) -> Option<When> {
    match &ed.date {
        Some(Change::Set(spec)) => planned.map(|dt| When {
            dt,
            estimated: spec.estimated,
            precision: spec.precision,
        }),
        Some(Change::Remove) => None,
        None => ex.datetime.map(|dt| {
            let precision = ex.precision.unwrap_or(Precision::Day);
            When {
                dt,
                estimated: ex.estimated || precision != Precision::Day,
                precision,
            }
        }),
    }
}

/// Location that would apply after writing (only with coordinates).
pub fn effective_location(ex: &Existing, ed: &Edits) -> Option<Location> {
    match &ed.loc {
        Some(Change::Set(l)) => Some(l.clone()),
        Some(Change::Remove) => None,
        None => ex.gps.map(|(lat, lon)| Location {
            lat,
            lon,
            city: ex.city.clone(),
            country: ex.country.clone(),
        }),
    }
}

pub fn place_name(city: &str, country: &str) -> String {
    [city.trim(), country.trim()]
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(", ")
}

fn short_place(gps: Option<(f64, f64)>, city: &str, country: &str) -> String {
    let name = place_name(city, country);
    if !name.is_empty() {
        name
    } else if let Some((a, b)) = gps {
        format!("{a:.4}, {b:.4}")
    } else {
        String::new()
    }
}

/// Short location text for the tile (after writing).
pub fn effective_place(ex: &Existing, ed: &Edits) -> Option<String> {
    match &ed.loc {
        Some(Change::Set(l)) => Some(short_place(Some((l.lat, l.lon)), &l.city, &l.country)),
        Some(Change::Remove) => None,
        None => {
            if ex.gps.is_some() || !place_name(&ex.city, &ex.country).is_empty() {
                Some(short_place(ex.gps, &ex.city, &ex.country))
            } else {
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Change overview for the confirmation dialog
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct ChangeRow {
    pub id: usize,
    /// 1-based position in the film
    pub position: usize,
    pub name: String,
    /// (before, after)
    pub date: Option<(String, String)>,
    pub loc: Option<(String, String)>,
    pub desc: Option<(String, String)>,
    pub film: Option<(String, String)>,
    pub rotation: Option<(String, String)>,
    pub rating: Option<(String, String)>,
}

impl ChangeRow {
    /// Description, film info, rotation and rating as one list (for a compact table column).
    pub fn other_changes(&self) -> Vec<(String, String)> {
        [&self.desc, &self.film, &self.rotation, &self.rating]
            .iter()
            .filter_map(|c| (*c).clone())
            .collect()
    }
}

fn when_label(w: Option<When>) -> String {
    w.map(|w| w.label()).unwrap_or_else(|| "–".into())
}

fn full_loc_label(gps: Option<(f64, f64)>, city: &str, country: &str) -> String {
    let name = place_name(city, country);
    match (gps, name.is_empty()) {
        (Some((a, b)), true) => format!("{a:.5}, {b:.5}"),
        (Some((a, b)), false) => format!("{name} ({a:.5}, {b:.5})"),
        (None, true) => "–".into(),
        (None, false) => format!("{name} ({})", tr("ohne GPS", "no GPS")),
    }
}

fn or_dash(s: &str) -> String {
    if s.trim().is_empty() {
        "–".into()
    } else {
        s.trim().to_string()
    }
}

/// `None` if nothing would change for this image.
pub fn change_row(
    id: usize,
    position: usize,
    name: &str,
    ex: &Existing,
    ed: &Edits,
    planned: Option<DateTime>,
) -> Option<ChangeRow> {
    let date = ed.date.as_ref().and_then(|_| {
        let old = when_label(effective_when(ex, &Edits::default(), None));
        let new = when_label(effective_when(ex, ed, planned));
        (old != new).then_some((old, new))
    });
    let loc = ed.loc.as_ref().and_then(|c| {
        let old = full_loc_label(ex.gps, &ex.city, &ex.country);
        let new = match c {
            Change::Set(l) => full_loc_label(Some((l.lat, l.lon)), &l.city, &l.country),
            Change::Remove => "–".into(),
        };
        (old != new).then_some((old, new))
    });
    let desc = ed
        .desc
        .as_ref()
        .and_then(|d| (d.trim() != ex.description.trim()).then(|| (or_dash(&ex.description), or_dash(d))));
    let film = ed.film.as_ref().and_then(|f| {
        let new = f.merged_over(&ex.film);
        let old = FilmInfo::default().merged_over(&ex.film);
        (new != old).then(|| (old.label(), new.label()))
    });
    let rotation = ed.orientation.and_then(|o| {
        (quarter_turns(o) != quarter_turns(ex.orientation)).then(|| (rotation_label(ex.orientation), rotation_label(o)))
    });
    let rating = ed
        .rating
        .and_then(|r| (r != ex.rating).then(|| (rating_label(ex.rating), rating_label(r))));
    if date.is_none() && loc.is_none() && desc.is_none() && film.is_none() && rotation.is_none() && rating.is_none() {
        return None;
    }
    Some(ChangeRow {
        id,
        position,
        name: name.to_string(),
        date,
        loc,
        desc,
        film,
        rotation,
        rating,
    })
}

// ---------------------------------------------------------------------------
// ExifTool arguments
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct WriteOptions {
    /// ExifTool keeps "file.tif_original"
    pub keep_backup: bool,
    /// file modification date = capture date (for Explorer & some photo tools)
    pub set_file_date: bool,
    /// keyword for approximate dates
    pub estimate_keyword: String,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            keep_backup: true,
            set_file_date: true,
            estimate_keyword: DEFAULT_ESTIMATE_KEYWORD.into(),
        }
    }
}

/// Configured keyword (empty → default).
pub fn estimate_keyword(custom: &str) -> String {
    let k = clean(custom);
    if k.is_empty() {
        DEFAULT_ESTIMATE_KEYWORD.into()
    } else {
        k
    }
}

/// All keywords that count as "approximate" (presets + own).
pub fn estimate_keywords(custom: &str) -> Vec<String> {
    let mut v: Vec<String> = ESTIMATE_PRESETS.iter().map(|s| s.to_string()).collect();
    let own = estimate_keyword(custom);
    if !v.iter().any(|k| k.eq_ignore_ascii_case(&own)) {
        v.push(own);
    }
    v
}

pub fn is_estimate_keyword(word: &str, custom: &str) -> bool {
    let w = word.trim();
    estimate_keywords(custom).iter().any(|k| k.eq_ignore_ascii_case(w))
}

/// Values must not contain line breaks (one line = one argument in the argument file).
fn clean(s: &str) -> String {
    s.replace(['\r', '\n', '\t'], " ").trim().to_string()
}

/// Builds the ExifTool arguments for one image (without file name).
pub fn write_args(ed: &Edits, planned: Option<DateTime>, opts: &WriteOptions) -> Vec<String> {
    let mut a = Vec::new();
    let mut file_date_set = false;
    let remove_keywords = |a: &mut Vec<String>| {
        for k in estimate_keywords(&opts.estimate_keyword) {
            a.push(format!("-XMP-dc:Subject-={k}"));
        }
    };

    match (&ed.date, planned) {
        (Some(Change::Set(spec)), Some(dt)) => {
            let v = dt.exif();
            a.push(format!("-EXIF:DateTimeOriginal={v}"));
            a.push(format!("-EXIF:CreateDate={v}"));
            a.push(format!("-XMP-photoshop:DateCreated={}", spec.xmp_date(&dt)));
            remove_keywords(&mut a);
            if spec.estimated {
                a.push(format!("-XMP-dc:Subject+={}", estimate_keyword(&opts.estimate_keyword)));
            }
            if opts.set_file_date {
                a.push(format!("-FileModifyDate={v}"));
                file_date_set = true;
            }
        }
        (Some(Change::Remove), _) => {
            a.push("-EXIF:DateTimeOriginal=".into());
            a.push("-EXIF:CreateDate=".into());
            a.push("-XMP-photoshop:DateCreated=".into());
            remove_keywords(&mut a);
        }
        _ => {}
    }

    match &ed.loc {
        Some(Change::Set(l)) => {
            a.push(format!("-GPS:GPSLatitude={:.6}", l.lat.abs()));
            a.push(format!("-GPS:GPSLatitudeRef={}", if l.lat >= 0.0 { "N" } else { "S" }));
            a.push(format!("-GPS:GPSLongitude={:.6}", l.lon.abs()));
            a.push(format!("-GPS:GPSLongitudeRef={}", if l.lon >= 0.0 { "E" } else { "W" }));
            a.push(format!("-XMP-photoshop:City={}", clean(&l.city)));
            a.push(format!("-XMP-photoshop:Country={}", clean(&l.country)));
        }
        Some(Change::Remove) => {
            a.push("-GPS:all=".into());
            a.push("-XMP-photoshop:City=".into());
            a.push("-XMP-photoshop:Country=".into());
        }
        None => {}
    }

    if let Some(d) = &ed.desc {
        let d = clean(d);
        a.push(format!("-XMP-dc:Description={d}"));
        a.push(format!("-EXIF:ImageDescription={d}"));
    }

    if let Some(f) = &ed.film {
        let c = clean(&f.creator);
        if !c.is_empty() {
            a.push(format!("-EXIF:Artist={c}"));
            a.push(format!("-XMP-dc:Creator={c}"));
        }
        let r = clean(&f.rights);
        if !r.is_empty() {
            a.push(format!("-EXIF:Copyright={r}"));
            a.push(format!("-XMP-dc:Rights={r}"));
        }
        if let Some(u) = f.source.uri() {
            a.push(format!("-XMP-iptcExt:DigitalSourceType={u}"));
        }
    }

    if let Some(o) = ed.orientation {
        // lossless: only the orientation flag changes, the pixels stay untouched
        a.push(format!("-IFD0:Orientation#={o}"));
    }
    if let Some(r) = ed.rating {
        if r == 0 {
            a.push("-XMP-xmp:Rating=".into());
        } else {
            a.push(format!("-XMP-xmp:Rating={r}"));
        }
    }

    if !opts.keep_backup {
        a.push("-overwrite_original".into());
    }
    if !file_date_set {
        a.push("-P".into());
    }
    a
}

// ---------------------------------------------------------------------------
// JPEG copies, backups, folder statistics
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum JpegTarget {
    /// subfolder inside the film folder, e.g. "…\2003-03 Borkum\JPEG\image_1.jpg"
    Subfolder(String),
    /// own base folder with one folder per film: "D:\Photos-JPEG\2003-03 Borkum\image_1.jpg"
    Parallel(PathBuf),
}

/// Files JPEG copies are created from.
pub fn is_jpeg_source(p: &Path) -> bool {
    p.extension()
        .map(|e| matches!(e.to_string_lossy().to_lowercase().as_str(), "tif" | "tiff" | "png"))
        .unwrap_or(false)
}

pub fn jpeg_path(src: &Path, film_dir: &Path, target: &JpegTarget) -> Option<PathBuf> {
    let mut name = src.file_stem()?.to_os_string();
    name.push(".jpg");
    let dir = match target {
        JpegTarget::Subfolder(s) => {
            let s = s.trim();
            film_dir.join(if s.is_empty() { "JPEG" } else { s })
        }
        JpegTarget::Parallel(base) => {
            if base.as_os_str().is_empty() {
                return None;
            }
            base.join(film_dir.file_name()?)
        }
    };
    Some(dir.join(name))
}

/// ExifTool backup ("image.tif_original") → original file, if the name matches.
pub fn backup_original(p: &Path) -> Option<PathBuf> {
    let name = p.file_name()?.to_string_lossy().to_string();
    let base = name.strip_suffix("_original")?;
    if base.is_empty() {
        return None;
    }
    Some(p.with_file_name(base))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FolderStats {
    pub images: usize,
    pub no_date: usize,
    pub dated: usize,
    pub located: usize,
    pub estimated: usize,
    pub rejected: usize,
}

pub fn folder_stats(items: &[Existing]) -> FolderStats {
    let mut s = FolderStats {
        images: items.len(),
        ..Default::default()
    };
    for e in items {
        if e.datetime.is_some() {
            s.dated += 1;
            if e.gps.is_some() {
                s.located += 1;
            }
            if e.estimated || matches!(e.precision, Some(Precision::Month | Precision::Year)) {
                s.estimated += 1;
            }
        } else {
            s.no_date += 1;
        }
        if e.rating < 0 {
            s.rejected += 1;
        }
    }
    s
}

/// Percent-encoding for URL query values (RFC 3986 unreserved characters stay as they are).
pub fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// The language is global – tests that depend on it must not run concurrently.
    static LANG_LOCK: Mutex<()> = Mutex::new(());

    fn with_lang(l: Lang) -> MutexGuard<'static, ()> {
        let g = LANG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_lang(l);
        g
    }

    fn spec(s: &str, est: bool) -> DateSpec {
        let (d, p) = parse_date_input(s).unwrap();
        DateSpec::new(d, p, (12, 0), est)
    }

    fn existing_at(s: &str) -> Existing {
        Existing {
            datetime: DateTime::parse_exif(s),
            ..Default::default()
        }
    }

    #[test]
    fn date_input_variants() {
        let _g = with_lang(Lang::De);
        assert_eq!(
            parse_date_input("14.07.2001").unwrap(),
            (Date::new(2001, 7, 14).unwrap(), Precision::Day)
        );
        assert_eq!(
            parse_date_input("14.7.01").unwrap(),
            (Date::new(2001, 7, 14).unwrap(), Precision::Day)
        );
        assert_eq!(
            parse_date_input("3.2.98").unwrap(),
            (Date::new(1998, 2, 3).unwrap(), Precision::Day)
        );
        assert_eq!(
            parse_date_input("07.2001").unwrap(),
            (Date::new(2001, 7, 1).unwrap(), Precision::Month)
        );
        assert_eq!(
            parse_date_input("2001").unwrap(),
            (Date::new(2001, 7, 1).unwrap(), Precision::Year)
        );
        assert_eq!(
            parse_date_input("2001-07-14").unwrap(),
            (Date::new(2001, 7, 14).unwrap(), Precision::Day)
        );
        assert_eq!(
            parse_date_input("2001-07").unwrap(),
            (Date::new(2001, 7, 1).unwrap(), Precision::Month)
        );
        assert!(parse_date_input("31.02.2001").is_err());
        assert!(parse_date_input("29.02.2000").is_ok());
        assert!(parse_date_input("29.02.1999").is_err());
        assert!(parse_date_input("abc").is_err());
        for s in ["14.07.2001", "07.2001", "2001"] {
            let (d, p) = parse_date_input(s).unwrap();
            assert_eq!(date_input_text(d, p), s);
        }
    }

    #[test]
    fn add_days_across_boundaries() {
        let d = |y, m, dd| Date::new(y, m, dd).unwrap();
        assert_eq!(d(2003, 3, 1).add_days(1), Some(d(2003, 3, 2)));
        assert_eq!(d(2003, 2, 28).add_days(1), Some(d(2003, 3, 1)));
        assert_eq!(d(2004, 2, 28).add_days(1), Some(d(2004, 2, 29)));
        assert_eq!(d(2003, 12, 31).add_days(1), Some(d(2004, 1, 1)));
        assert_eq!(d(2004, 1, 1).add_days(-1), Some(d(2003, 12, 31)));
        assert_eq!(d(2003, 3, 1).add_days(-1), Some(d(2003, 2, 28)));
    }

    #[test]
    fn time_input() {
        assert_eq!(parse_time("").unwrap(), (12, 0));
        assert_eq!(parse_time("9:30").unwrap(), (9, 30));
        assert!(parse_time("25:00").is_err());
    }

    #[test]
    fn minutes_count_up_per_group() {
        let a = Edits {
            date: Some(Change::Set(spec("14.07.2001", false))),
            ..Default::default()
        };
        let b = Edits {
            date: Some(Change::Set(spec("15.07.2001", false))),
            ..Default::default()
        };
        let list = vec![a.clone(), a.clone(), b.clone(), Edits::default(), a.clone(), b.clone()];
        let s: Vec<String> = planned_times(list.iter())
            .iter()
            .map(|x| x.map(|d| d.exif()).unwrap_or_default())
            .collect();
        assert_eq!(
            s,
            vec![
                "2001:07:14 12:00:00",
                "2001:07:14 12:01:00",
                "2001:07:15 12:00:00",
                "",
                "2001:07:14 12:02:00",
                "2001:07:15 12:01:00"
            ]
        );
    }

    #[test]
    fn overflow_after_midnight_stays_on_day() {
        let s = DateSpec::new(Date::new(2001, 7, 14).unwrap(), Precision::Day, (23, 58), false);
        assert_eq!(s.at_offset(2).exif(), "2001:07:14 23:59:01");
    }

    #[test]
    fn precision_and_labels() {
        let _g = with_lang(Lang::De);
        assert_eq!(Precision::from_xmp("2003"), Some(Precision::Year));
        assert_eq!(Precision::from_xmp("2003:03"), Some(Precision::Month));
        assert_eq!(Precision::from_xmp("2003:03:01 12:30:00"), Some(Precision::Day));
        let dt = DateTime::parse_exif("2003:03:01 12:32:00").unwrap();
        let w = |p, e| {
            When {
                dt,
                estimated: e,
                precision: p,
            }
            .label()
        };
        assert_eq!(w(Precision::Day, false), "01.03.2003 12:32");
        assert_eq!(w(Precision::Month, true), "ca. 03/2003 · 12:32");
        assert_eq!(w(Precision::Year, true), "ca. 2003 · 12:32");
    }

    #[test]
    fn natural_sorting() {
        let mut v = vec!["Bild 10.tif", "Bild 2.tif", "Bild 0.tif", "bild 37.tif"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["Bild 0.tif", "Bild 2.tif", "Bild 10.tif", "bild 37.tif"]);
    }

    #[test]
    fn coords() {
        let _g = with_lang(Lang::De);
        assert_eq!(parse_coords("53.4512, 5.7701").unwrap(), (53.4512, 5.7701));
        assert_eq!(parse_coords("53,4512 5,7701").unwrap(), (53.4512, 5.7701));
        assert!(parse_coords("95, 10").is_err());
    }

    #[test]
    fn folder_names() {
        assert_eq!(
            guess_date_from_name("2001-07-14 Ameland").as_deref(),
            Some("14.07.2001")
        );
        assert_eq!(guess_date_from_name("2001_07 Urlaub").as_deref(), Some("07.2001"));
        assert_eq!(
            guess_date_from_name("14.07.2001 Ameland").as_deref(),
            Some("14.07.2001")
        );
        assert_eq!(guess_date_from_name("Film 0001"), None);
    }

    #[test]
    fn moving_resequences_saved_times() {
        let _g = with_lang(Lang::De);
        let exs: Vec<Existing> = (0..5)
            .map(|i| existing_at(&format!("2003:03:01 12:{i:02}:00")))
            .collect();
        let order = [0usize, 4, 1, 2, 3];
        let mut edits: Vec<Edits> = vec![Edits::default(); 5];
        let specs = {
            let items: Vec<(&Existing, &Edits)> = order.iter().map(|&i| (&exs[i], &edits[i])).collect();
            resequence_specs(&items)
        };
        for (k, s) in specs.into_iter().enumerate() {
            edits[order[k]].date = s.map(Change::Set);
        }
        let ordered: Vec<&Edits> = order.iter().map(|&i| &edits[i]).collect();
        let t = planned_times(ordered.iter().copied());
        let times: Vec<String> = t.iter().map(|d| d.unwrap().time_str()).collect();
        assert_eq!(times, vec!["12:00", "12:01", "12:02", "12:03", "12:04"]);
        let rows: Vec<bool> = order
            .iter()
            .enumerate()
            .map(|(k, &i)| change_row(i, k + 1, "x", &exs[i], &edits[i], t[k]).is_some())
            .collect();
        assert_eq!(rows, vec![false, true, true, true, true]);
    }

    #[test]
    fn orientation_math() {
        assert_eq!(rotate_orientation(1, true), 6);
        assert_eq!(rotate_orientation(6, true), 3);
        assert_eq!(rotate_orientation(3, true), 8);
        assert_eq!(rotate_orientation(8, true), 1);
        assert_eq!(rotate_orientation(1, false), 8);
        assert_eq!(quarter_turns(2), 0);
    }

    #[test]
    fn rotation_and_rating_rows_and_args() {
        let _g = with_lang(Lang::De);
        let ex = Existing::default();
        let ed = Edits {
            orientation: Some(6),
            rating: Some(-1),
            ..Default::default()
        };
        let row = change_row(0, 1, "x", &ex, &ed, None).unwrap();
        assert_eq!(row.rotation, Some(("0°".into(), "90°".into())));
        assert_eq!(row.rating, Some(("–".into(), "Ausschuss".into())));
        assert_eq!(row.other_changes().len(), 2);
        let args = write_args(&ed, None, &WriteOptions::default());
        assert!(args.contains(&"-IFD0:Orientation#=6".to_string()));
        assert!(args.contains(&"-XMP-xmp:Rating=-1".to_string()));
        let clear = Edits {
            rating: Some(0),
            ..Default::default()
        };
        let ex3 = Existing {
            rating: 3,
            ..Default::default()
        };
        assert!(write_args(&clear, None, &WriteOptions::default()).contains(&"-XMP-xmp:Rating=".to_string()));
        assert_eq!(
            change_row(0, 1, "x", &ex3, &clear, None).unwrap().rating,
            Some(("★★★".into(), "–".into()))
        );
    }

    #[test]
    fn estimate_keywords_are_replaced() {
        let _g = with_lang(Lang::De);
        assert!(is_estimate_keyword("Datum geschätzt", "circa"));
        assert!(is_estimate_keyword("Circa", ""));
        let s = spec("07.2001", false);
        let ed = Edits {
            date: Some(Change::Set(s)),
            ..Default::default()
        };
        let args = write_args(&ed, Some(s.at_offset(2)), &WriteOptions::default());
        assert!(args.contains(&"-XMP-photoshop:DateCreated=2001:07".to_string()));
        assert!(args.contains(&"-XMP-dc:Subject-=Datum geschätzt".to_string()));
        assert!(args.contains(&"-XMP-dc:Subject+=circa".to_string()));
    }

    #[test]
    fn source_type_roundtrip() {
        for s in SourceType::ALL {
            if let Some(u) = s.uri() {
                assert_eq!(SourceType::from_uri(&u), s);
            }
        }
    }

    #[test]
    fn english_labels() {
        let _g = with_lang(Lang::En);
        let dt = DateTime::parse_exif("2003:03:01 12:32:00").unwrap();
        assert_eq!(dt.display(), "2003-03-01 12:32");
        assert_eq!(
            When {
                dt,
                estimated: true,
                precision: Precision::Month
            }
            .label(),
            "ca. 2003-03 · 12:32"
        );
        assert_eq!(rating_label(-1), "Reject");
        assert!(parse_date_input("xx").unwrap_err().contains("not a valid"));
    }

    #[test]
    fn jpeg_paths_and_backups() {
        let src = Path::new("/scans/2003-03 Borkum/Bild_1.tif");
        let film = Path::new("/scans/2003-03 Borkum");
        assert_eq!(
            jpeg_path(src, film, &JpegTarget::Subfolder("JPEG".into())).unwrap(),
            PathBuf::from("/scans/2003-03 Borkum/JPEG/Bild_1.jpg")
        );
        assert_eq!(
            jpeg_path(src, film, &JpegTarget::Parallel(PathBuf::from("/jpeg"))).unwrap(),
            PathBuf::from("/jpeg/2003-03 Borkum/Bild_1.jpg")
        );
        assert_eq!(
            backup_original(Path::new("/a/Bild_1.tif_original")),
            Some(PathBuf::from("/a/Bild_1.tif"))
        );
        assert_eq!(backup_original(Path::new("/a/Bild_1.tif")), None);
    }

    #[test]
    fn stats() {
        let mut a = existing_at("2003:03:01 12:00:00");
        a.gps = Some((1.0, 2.0));
        let mut b = existing_at("2003:03:01 12:01:00");
        b.estimated = true;
        let mut c = Existing::default();
        c.rating = -1;
        let s = folder_stats(&[a, b, c]);
        assert_eq!(
            s,
            FolderStats {
                images: 3,
                no_date: 1,
                dated: 2,
                located: 1,
                estimated: 1,
                rejected: 1
            }
        );
    }

    #[test]
    fn encoding() {
        assert_eq!(url_encode("Borkum Nordsee"), "Borkum%20Nordsee");
        assert_eq!(url_encode("Köln"), "K%C3%B6ln");
    }
}
