//! Persistent settings and storage locations.
//!
//! Portable mode: if a file `portable.txt` (or `rollstamp-settings.json`) sits next to the exe,
//! settings and the thumbnail cache are stored in that folder – nothing is written elsewhere.
//! Otherwise: `%APPDATA%\Rollstamp\settings.json` and `%LOCALAPPDATA%\Rollstamp\thumbnails`.

use crate::model::{
    estimate_keyword, place_name, FilmInfo, JpegTarget, Lang, SourceType, WriteOptions, DEFAULT_ESTIMATE_KEYWORD,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const SETTINGS_FILE: &str = "rollstamp-settings.json";
const PORTABLE_MARKER: &str = "portable.txt";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PlacePreset {
    pub lat: f64,
    pub lon: f64,
    pub city: String,
    pub country: String,
}

impl PlacePreset {
    pub fn label(&self) -> String {
        let name = place_name(&self.city, &self.country);
        if name.is_empty() {
            format!("{:.4}, {:.4}", self.lat, self.lon)
        } else {
            name
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// "auto", "de" or "en"
    pub language: String,

    pub keep_backup: bool,
    pub set_file_date: bool,
    pub resequence_on_move: bool,
    pub estimate_keyword: String,

    pub jpeg_enabled: bool,
    pub jpeg_quality: u8,
    pub jpeg_max_edge: u32,
    pub jpeg_parallel: bool,
    pub jpeg_subfolder: String,
    pub jpeg_base: String,
    pub jpeg_overwrite: bool,
    pub jpeg_threads: u8,
    pub jpeg_skip_rejects: bool,

    pub creator: String,
    pub rights: String,
    pub source_type: String,
    pub film_auto: bool,

    pub recent_places: Vec<PlacePreset>,
    pub last_folder: String,
    pub archive_root: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            keep_backup: true,
            set_file_date: true,
            resequence_on_move: true,
            estimate_keyword: DEFAULT_ESTIMATE_KEYWORD.into(),
            jpeg_enabled: true,
            jpeg_quality: 92,
            jpeg_max_edge: 0,
            jpeg_parallel: false,
            jpeg_subfolder: "JPEG".into(),
            jpeg_base: String::new(),
            jpeg_overwrite: false,
            jpeg_threads: 3,
            jpeg_skip_rejects: true,
            creator: String::new(),
            rights: String::new(),
            source_type: SourceType::NegativeFilm.key().into(),
            film_auto: false,
            recent_places: Vec::new(),
            last_folder: String::new(),
            archive_root: String::new(),
        }
    }
}

pub fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(|p| p.to_path_buf())
}

pub fn is_portable() -> bool {
    exe_dir()
        .map(|d| d.join(PORTABLE_MARKER).exists() || d.join(SETTINGS_FILE).exists())
        .unwrap_or(false)
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

fn config_path() -> Option<PathBuf> {
    if is_portable() {
        return exe_dir().map(|d| d.join(SETTINGS_FILE));
    }
    let base = env_dir("APPDATA")
        .or_else(|| env_dir("XDG_CONFIG_HOME"))
        .or_else(|| env_dir("HOME").map(|h| h.join(".config")))?;
    Some(base.join("Rollstamp").join("settings.json"))
}

/// Settings of the predecessor "Film-Datierer" (taken over once).
fn legacy_path() -> Option<PathBuf> {
    env_dir("APPDATA").map(|b| b.join("film-datierer").join("einstellungen.json"))
}

/// Thumbnail cache (created if necessary).
pub fn cache_dir() -> Option<PathBuf> {
    let dir = if is_portable() {
        exe_dir()?.join("cache").join("thumbnails")
    } else {
        env_dir("LOCALAPPDATA")
            .or_else(|| env_dir("XDG_CACHE_HOME"))
            .or_else(|| env_dir("HOME").map(|h| h.join(".cache")))?
            .join("Rollstamp")
            .join("thumbnails")
    };
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Language of the operating system (German → German, everything else → English).
pub fn system_lang() -> Lang {
    match sys_locale::get_locale() {
        Some(l) if l.to_lowercase().starts_with("de") => Lang::De,
        _ => Lang::En,
    }
}

impl Settings {
    pub fn load() -> Self {
        let read = |p: Option<PathBuf>| -> Option<Settings> {
            let text = std::fs::read_to_string(p?).ok()?;
            serde_json::from_str(&text).ok()
        };
        read(config_path())
            .or_else(|| if is_portable() { None } else { read(legacy_path()) })
            .unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn save_json(json: &str) {
        if let Some(p) = config_path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(p, json);
        }
    }

    pub fn resolved_lang(&self, system: Lang) -> Lang {
        match self.language.as_str() {
            "de" => Lang::De,
            "en" => Lang::En,
            _ => system,
        }
    }

    pub fn write_options(&self) -> WriteOptions {
        WriteOptions {
            keep_backup: self.keep_backup,
            set_file_date: self.set_file_date,
            estimate_keyword: estimate_keyword(&self.estimate_keyword),
        }
    }

    pub fn film_info(&self) -> FilmInfo {
        FilmInfo {
            creator: self.creator.trim().to_string(),
            rights: self.rights.trim().to_string(),
            source: SourceType::from_key(&self.source_type),
        }
    }

    pub fn jpeg_target(&self) -> Option<JpegTarget> {
        if self.jpeg_parallel {
            let b = self.jpeg_base.trim();
            if b.is_empty() {
                None
            } else {
                Some(JpegTarget::Parallel(PathBuf::from(b)))
            }
        } else {
            Some(JpegTarget::Subfolder(self.jpeg_subfolder.trim().to_string()))
        }
    }

    /// Puts a place at the top of the "recently used" list.
    pub fn remember_place(&mut self, p: PlacePreset) {
        self.recent_places.retain(|q| {
            !(q.city == p.city
                && q.country == p.country
                && (q.lat - p.lat).abs() < 1e-6
                && (q.lon - p.lon).abs() < 1e-6)
        });
        self.recent_places.insert(0, p);
        self.recent_places.truncate(15);
    }
}
