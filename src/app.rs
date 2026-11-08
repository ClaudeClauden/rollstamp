//! User interface (egui).

use crate::exif::ExifTool;
use crate::geo::{self, GeoHit};
use crate::jpeg::{export_many, JpegJob, JpegSettings, Magick};
use crate::loader::{Kind, Loader, Request};
use crate::model::*;
use crate::settings::{system_lang, PlacePreset, Settings};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontId, Key, Modifiers, Rect, RichText, Sense, Stroke, TextureHandle,
    TextureOptions, Vec2,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const TILE_W: f32 = 200.0;
const IMG_H: f32 = 156.0;
const TILE_H: f32 = IMG_H + 58.0;

const RED: Color32 = Color32::from_rgb(222, 64, 64);
const BLUE: Color32 = Color32::from_rgb(66, 135, 235);
const GREEN: Color32 = Color32::from_rgb(64, 185, 96);
const AMBER: Color32 = Color32::from_rgb(240, 170, 40);

const SUPPORTED: [&str; 5] = ["tif", "tiff", "jpg", "jpeg", "png"];
const UNDO_LIMIT: usize = 100;

// ------------------------------------------------------------------ helpers

fn status_color(s: Status) -> Color32 {
    match s {
        Status::Missing => RED,
        Status::Dated => BLUE,
        Status::Located => GREEN,
    }
}

fn fit(src: Vec2, max: Vec2) -> Vec2 {
    if src.x <= 0.0 || src.y <= 0.0 {
        return max;
    }
    let s = (max.x / src.x).min(max.y / src.y);
    src * s
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn is_supported(p: &Path) -> bool {
    let name = file_name(p);
    if name.starts_with('.') || name.to_lowercase().ends_with(".tmp.jpg") {
        return false;
    }
    p.extension()
        .map(|e| SUPPORTED.contains(&e.to_string_lossy().to_lowercase().as_str()))
        .unwrap_or(false)
}

fn list_images(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_supported(p))
            .collect(),
        Err(_) => Vec::new(),
    };
    files.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
    files
}

/// ExifTool backups ("*_original") in a folder and their total size.
fn list_backups(dir: &Path) -> (Vec<PathBuf>, u64) {
    let mut files = Vec::new();
    let mut bytes = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.is_file() && backup_original(&p).is_some() {
                bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
                files.push(p);
            }
        }
    }
    (files, bytes)
}

fn fmt_bytes(b: u64) -> String {
    let gb = b as f64 / 1024.0 / 1024.0 / 1024.0;
    let s = if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", b as f64 / 1024.0 / 1024.0)
    };
    if is_de() {
        s.replace('.', ",")
    } else {
        s
    }
}

/// Folders next to the current film folder, naturally sorted (for "next film").
fn sibling_dirs(dir: &Path) -> Vec<PathBuf> {
    let Some(parent) = dir.parent() else { return Vec::new() };
    let mut v: Vec<PathBuf> = match std::fs::read_dir(parent) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir() && !file_name(p).starts_with('.'))
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
    v
}

/// All folders below `root` (including `root`) that contain images.
fn collect_image_dirs(root: &Path, skip_names: &[String], skip_path: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut has_images = false;
        for e in rd.filter_map(|e| e.ok()) {
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() {
                let name = file_name(&p).to_lowercase();
                let skip = name.starts_with('.')
                    || skip_names.iter().any(|s| *s == name)
                    || skip_path.map_or(false, |sp| sp == p.as_path());
                if !skip && depth < 16 {
                    stack.push((p, depth + 1));
                }
            } else if ft.is_file() && !has_images && is_supported(&p) {
                has_images = true;
            }
        }
        if has_images {
            out.push(dir);
        }
    }
    out.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
    out
}

#[derive(Clone)]
struct FolderInfo {
    path: PathBuf,
    rel: String,
    stats: FolderStats,
    jpeg_sources: usize,
    jpegs: usize,
    backups: usize,
    error: Option<String>,
}

fn scan_folder(tool: &ExifTool, root: &Path, dir: &Path, target: Option<&JpegTarget>, keyword: &str) -> FolderInfo {
    let rel = dir
        .strip_prefix(root)
        .map(|r| r.display().to_string())
        .unwrap_or_else(|_| dir.display().to_string());
    let rel = if rel.is_empty() { ".".to_string() } else { rel };
    let files = list_images(dir);
    let (stats, error) = match tool.read_metadata(&files, keyword) {
        Ok(ex) => (folder_stats(&ex), None),
        Err(e) => (
            FolderStats {
                images: files.len(),
                no_date: files.len(),
                ..Default::default()
            },
            Some(e),
        ),
    };
    let sources: Vec<&PathBuf> = files.iter().filter(|p| is_jpeg_source(p)).collect();
    let jpegs = match target {
        Some(t) => sources
            .iter()
            .filter(|p| jpeg_path(p, dir, t).map_or(false, |j| j.exists()))
            .count(),
        None => 0,
    };
    FolderInfo {
        path: dir.to_path_buf(),
        rel,
        stats,
        jpeg_sources: sources.len(),
        jpegs,
        backups: list_backups(dir).0.len(),
        error,
    }
}

/// Draws a texture rotated by `turns` clockwise quarter turns, fitted into `area`.
fn paint_image(painter: &egui::Painter, tex: &TextureHandle, area: Rect, turns: u32, tint: Color32) -> Rect {
    let mut size = tex.size_vec2();
    if turns % 2 == 1 {
        size = vec2(size.y, size.x);
    }
    let r = Rect::from_center_size(area.center(), fit(size, area.size()));
    let base = [pos2(0.0, 0.0), pos2(1.0, 0.0), pos2(1.0, 1.0), pos2(0.0, 1.0)];
    let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
    let k = (turns % 4) as usize;
    let mut mesh = egui::epaint::Mesh::with_texture(tex.id());
    for (i, corner) in corners.iter().enumerate() {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: *corner,
            uv: base[(i + 4 - k) % 4],
            color: tint,
        });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
    r
}

// ------------------------------------------------------------------ state

#[derive(Clone, Copy)]
struct DragPayload(usize);

struct Photo {
    id: usize,
    path: PathBuf,
    name: String,
    existing: Existing,
    edits: Edits,
    thumb: Option<TextureHandle>,
    thumb_error: Option<String>,
    /// JPEG copy exists
    jpeg: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    All,
    Selection,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Film,
    Archive,
}

enum Busy {
    Idle,
    Reading,
    Working { label: String, done: usize, total: usize },
}

struct Archive {
    folders: Vec<FolderInfo>,
    running: bool,
    done: usize,
    total: usize,
    cancel: Arc<AtomicBool>,
    generation: u64,
    only_incomplete: bool,
}

enum Msg {
    Meta {
        generation: u64,
        result: Result<Vec<(usize, Existing)>, String>,
    },
    Progress {
        label: String,
        done: usize,
        total: usize,
    },
    Done {
        generation: u64,
        written: Vec<usize>,
        errors: Vec<String>,
        warnings: Vec<String>,
        reread: Result<Vec<(usize, Existing)>, String>,
        jpeg_created: usize,
        jpeg_updated: usize,
    },
    ScanTotal {
        generation: u64,
        total: usize,
    },
    ScanFolder {
        generation: u64,
        info: FolderInfo,
    },
    ScanDone {
        generation: u64,
        cancelled: bool,
    },
    GeoSearch {
        key: String,
        result: Result<Vec<GeoHit>, String>,
    },
    GeoReverse {
        result: Result<GeoHit, String>,
    },
}

#[derive(Clone)]
enum After {
    OpenDialog,
    Open(PathBuf),
    Quit,
}

/// Small dialogs opened from the right-click menu.
#[derive(Clone, Copy, PartialEq, Eq)]
enum QuickKind {
    Date,
    Location,
    Description,
}

/// Actions from the right-click menu (applied after drawing).
#[derive(Clone)]
enum MenuAction {
    Edit(QuickKind),
    Place(PlacePreset),
    NextDay,
    Rotate(bool),
    Rating(i8),
    Reject,
    DiscardDate,
    DiscardPlace,
    DiscardAll,
    RemoveDate,
    RemovePlace,
    Large,
    SelectAll,
    ClearSelection,
}

enum Dialog {
    None,
    Quick {
        ids: Vec<usize>,
        kind: QuickKind,
    },
    Confirm(Vec<ChangeRow>),
    Discard(After),
    DeleteBackups {
        files: Vec<PathBuf>,
        bytes: u64,
        skipped: usize,
    },
    Report {
        title: String,
        lines: Vec<String>,
    },
}

pub struct App {
    ctx: egui::Context,
    exiftool: Option<ExifTool>,
    magick: Option<Magick>,
    loader: Loader,
    generation: u64,
    system_lang: Lang,
    view: View,

    folder: Option<PathBuf>,
    siblings: Vec<PathBuf>,
    photos: Vec<Photo>,
    selection: HashSet<usize>,
    anchor: Option<usize>,
    last_sel_count: usize,
    multi_click: bool,
    undo: Vec<Vec<(usize, Edits)>>,
    backups: Vec<PathBuf>,
    backup_bytes: u64,

    viewer: Option<usize>,
    full: Option<(usize, TextureHandle)>,
    full_error: Option<(usize, String)>,

    target: Target,
    date_input: String,
    time_input: String,
    estimated: bool,
    coord_input: String,
    city_input: String,
    country_input: String,
    desc_input: String,

    place_query: String,
    geo_hits: Vec<GeoHit>,
    geo_busy: bool,
    geo_last: Option<Instant>,
    geo_cache: HashMap<String, Vec<GeoHit>>,
    geo_error: Option<String>,

    archive: Archive,

    settings: Settings,
    saved_json: String,
    jpeg_target_seen: Option<Option<JpegTarget>>,

    busy: Busy,
    msg_tx: mpsc::Sender<Msg>,
    msg_rx: mpsc::Receiver<Msg>,
    dialog: Dialog,
    status: Option<(String, Color32)>,
    /// status line currently shows the tool info (re-translated when the language changes)
    status_is_tools: bool,
    want_open: bool,
    /// folder change only after drawing
    nav: Option<PathBuf>,
    allow_close: bool,
}

fn read_ids(tool: &ExifTool, files: &[(usize, PathBuf)], keyword: &str) -> Result<Vec<(usize, Existing)>, String> {
    let paths: Vec<PathBuf> = files.iter().map(|(_, p)| p.clone()).collect();
    let list = tool.read_metadata(&paths, keyword)?;
    Ok(files.iter().map(|(id, _)| *id).zip(list).collect())
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        let mut style = (*ctx.style()).clone();
        style.spacing.button_padding = vec2(10.0, 5.0);
        style.spacing.item_spacing = vec2(8.0, 6.0);
        ctx.set_style(style);

        let settings = Settings::load();
        let system_lang = system_lang();
        set_lang(settings.resolved_lang(system_lang));

        let (msg_tx, msg_rx) = mpsc::channel();
        let exiftool = ExifTool::locate();
        let magick = Magick::locate();
        let saved_json = settings.to_json();
        let status = exiftool.as_ref().map(|t| {
            let im = if magick.is_some() {
                tr("ImageMagick gefunden", "ImageMagick found")
            } else {
                tr(
                    "ImageMagick fehlt (keine JPEG-Kopien)",
                    "ImageMagick missing (no JPEG copies)",
                )
            };
            (format!("ExifTool {} · {im}", t.version), Color32::GRAY)
        });

        Self {
            loader: Loader::new(ctx.clone()),
            ctx,
            exiftool,
            magick,
            generation: 0,
            system_lang,
            view: View::Film,
            folder: None,
            siblings: Vec::new(),
            photos: Vec::new(),
            selection: HashSet::new(),
            anchor: None,
            last_sel_count: 0,
            multi_click: false,
            undo: Vec::new(),
            backups: Vec::new(),
            backup_bytes: 0,
            viewer: None,
            full: None,
            full_error: None,
            target: Target::All,
            date_input: String::new(),
            time_input: "12:00".into(),
            estimated: false,
            coord_input: String::new(),
            city_input: String::new(),
            country_input: String::new(),
            desc_input: String::new(),
            place_query: String::new(),
            geo_hits: Vec::new(),
            geo_busy: false,
            geo_last: None,
            geo_cache: HashMap::new(),
            geo_error: None,
            archive: Archive {
                folders: Vec::new(),
                running: false,
                done: 0,
                total: 0,
                cancel: Arc::new(AtomicBool::new(false)),
                generation: 0,
                only_incomplete: false,
            },
            settings,
            saved_json,
            jpeg_target_seen: None,
            busy: Busy::Idle,
            msg_tx,
            msg_rx,
            dialog: Dialog::None,
            status,
            status_is_tools: true,
            want_open: false,
            nav: None,
            allow_close: false,
        }
    }

    // ---------------------------------------------------------------- small helpers

    fn set_status(&mut self, text: impl Into<String>, color: Color32) {
        self.status = Some((text.into(), color));
        self.status_is_tools = false;
    }

    fn tool_status(&self) -> Option<(String, Color32)> {
        let t = self.exiftool.as_ref()?;
        let im = if self.magick.is_some() {
            tr("ImageMagick gefunden", "ImageMagick found")
        } else {
            tr(
                "ImageMagick fehlt (keine JPEG-Kopien)",
                "ImageMagick missing (no JPEG copies)",
            )
        };
        Some((format!("ExifTool {} · {im}", t.version), Color32::GRAY))
    }

    fn pos_of(&self, id: usize) -> Option<usize> {
        self.photos.iter().position(|p| p.id == id)
    }

    fn planned(&self) -> Vec<Option<DateTime>> {
        planned_times(self.photos.iter().map(|p| &p.edits))
    }

    fn rows(&self, planned: &[Option<DateTime>]) -> Vec<ChangeRow> {
        self.photos
            .iter()
            .enumerate()
            .filter_map(|(i, p)| change_row(p.id, i + 1, &p.name, &p.existing, &p.edits, planned[i]))
            .collect()
    }

    fn working(&self) -> bool {
        matches!(self.busy, Busy::Working { .. })
    }

    fn target_ids(&self) -> Vec<usize> {
        match self.target {
            Target::All => self.photos.iter().map(|p| p.id).collect(),
            Target::Selection => self
                .photos
                .iter()
                .filter(|p| self.selection.contains(&p.id))
                .map(|p| p.id)
                .collect(),
        }
    }

    /// Images affected by keyboard actions: the one in the large view, otherwise the selection.
    fn action_ids(&self) -> Vec<usize> {
        match self.viewer {
            Some(id) => vec![id],
            None => self
                .photos
                .iter()
                .filter(|p| self.selection.contains(&p.id))
                .map(|p| p.id)
                .collect(),
        }
    }

    fn snapshot(&mut self) {
        let snap: Vec<(usize, Edits)> = self.photos.iter().map(|p| (p.id, p.edits.clone())).collect();
        self.undo.push(snap);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
    }

    fn undo(&mut self) {
        let Some(snap) = self.undo.pop() else {
            self.set_status(tr("Nichts zum Rückgängigmachen.", "Nothing to undo."), Color32::GRAY);
            return;
        };
        let mut map: HashMap<usize, Photo> = self.photos.drain(..).map(|p| (p.id, p)).collect();
        let mut restored = Vec::new();
        for (id, ed) in snap {
            if let Some(mut p) = map.remove(&id) {
                p.edits = ed;
                restored.push(p);
            }
        }
        restored.extend(map.into_values());
        self.photos = restored;
        self.set_status(tr("Rückgängig gemacht.", "Undone."), Color32::GRAY);
    }

    fn edit_targets(&mut self, f: impl Fn(&mut Edits)) -> usize {
        let ids = self.target_ids();
        self.edit_ids(&ids, f)
    }

    /// Applies a change to the given images (one undo step).
    fn edit_ids(&mut self, ids: &[usize], f: impl Fn(&mut Edits)) -> usize {
        self.snapshot();
        let set: HashSet<usize> = ids.iter().copied().collect();
        for p in self.photos.iter_mut().filter(|p| set.contains(&p.id)) {
            f(&mut p.edits);
        }
        set.len()
    }

    /// Selected images in film order.
    fn selected_ids(&self) -> Vec<usize> {
        self.photos
            .iter()
            .filter(|p| self.selection.contains(&p.id))
            .map(|p| p.id)
            .collect()
    }

    fn target_label(&self) -> &'static str {
        match self.target {
            Target::All => tr("alle Bilder", "all images"),
            Target::Selection => tr("die Auswahl", "the selection"),
        }
    }

    fn first_selected(&self) -> Option<usize> {
        self.photos.iter().position(|p| self.selection.contains(&p.id))
    }

    fn sibling(&self, step: i64) -> Option<PathBuf> {
        let cur = self.folder.as_ref()?;
        let i = self.siblings.iter().position(|p| p == cur)? as i64 + step;
        if i < 0 {
            return None;
        }
        self.siblings.get(i as usize).cloned()
    }

    fn persist_settings(&mut self) {
        let json = self.settings.to_json();
        if json != self.saved_json {
            Settings::save_json(&json);
            self.saved_json = json;
        }
    }

    fn lang_code(&self) -> &'static str {
        if is_de() {
            "de"
        } else {
            "en"
        }
    }

    // ---------------------------------------------------------------- loading folders

    fn has_pending(&self) -> bool {
        let planned = self.planned();
        !self.rows(&planned).is_empty()
    }

    fn request_open(&mut self) {
        if self.has_pending() {
            self.dialog = Dialog::Discard(After::OpenDialog);
        } else {
            self.open_folder_dialog();
        }
    }

    fn go_to(&mut self, dir: PathBuf) {
        if self.has_pending() {
            self.dialog = Dialog::Discard(After::Open(dir));
        } else {
            self.load_folder(dir);
        }
    }

    fn open_folder_dialog(&mut self) {
        let mut dlg = rfd::FileDialog::new().set_title(tr("Filmordner wählen", "Choose film folder"));
        let start = self
            .folder
            .as_ref()
            .and_then(|f| f.parent().map(Path::to_path_buf))
            .or_else(|| {
                let last = PathBuf::from(&self.settings.last_folder);
                last.parent().filter(|p| p.is_dir()).map(Path::to_path_buf)
            });
        if let Some(dir) = start {
            dlg = dlg.set_directory(dir);
        }
        if let Some(dir) = dlg.pick_folder() {
            self.load_folder(dir);
        }
    }

    fn load_folder(&mut self, dir: PathBuf) {
        if !dir.is_dir() {
            self.set_status(
                trs(
                    format!("Ordner nicht gefunden: {}", dir.display()),
                    format!("Folder not found: {}", dir.display()),
                ),
                RED,
            );
            return;
        }
        let files = list_images(&dir);
        self.generation += 1;
        self.loader.set_generation(self.generation);
        self.photos = files
            .into_iter()
            .enumerate()
            .map(|(id, path)| Photo {
                id,
                name: file_name(&path),
                path,
                existing: Existing::default(),
                edits: Edits::default(),
                thumb: None,
                thumb_error: None,
                jpeg: false,
            })
            .collect();
        self.selection.clear();
        self.anchor = None;
        self.viewer = None;
        self.full = None;
        self.full_error = None;
        self.target = Target::All;
        self.jpeg_target_seen = None;
        self.undo.clear();
        self.view = View::Film;

        for p in &self.photos {
            self.loader.request(Request {
                generation: self.generation,
                id: p.id,
                path: p.path.clone(),
                kind: Kind::Thumb,
            });
        }

        if let Some(guess) = guess_date_from_name(&file_name(&dir)) {
            self.date_input = guess;
        }
        let (b, bytes) = list_backups(&dir);
        self.backups = b;
        self.backup_bytes = bytes;
        self.siblings = sibling_dirs(&dir);
        self.settings.last_folder = dir.display().to_string();
        self.folder = Some(dir);

        if self.photos.is_empty() {
            self.set_status(
                tr(
                    "In diesem Ordner wurden keine TIFF-, JPEG- oder PNG-Dateien gefunden.",
                    "No TIFF, JPEG or PNG files found in this folder.",
                ),
                AMBER,
            );
        } else {
            self.start_read_meta();
        }
    }

    fn start_read_meta(&mut self) {
        let Some(tool) = self.exiftool.clone() else {
            self.set_status(
                tr(
                    "Ohne ExifTool können keine Metadaten gelesen werden.",
                    "Without ExifTool no metadata can be read.",
                ),
                RED,
            );
            return;
        };
        let files: Vec<(usize, PathBuf)> = self.photos.iter().map(|p| (p.id, p.path.clone())).collect();
        let keyword = self.settings.estimate_keyword.clone();
        let tx = self.msg_tx.clone();
        let ctx = self.ctx.clone();
        let generation = self.generation;
        self.busy = Busy::Reading;
        std::thread::spawn(move || {
            let result = read_ids(&tool, &files, &keyword);
            let _ = tx.send(Msg::Meta { generation, result });
            ctx.request_repaint();
        });
    }

    fn apply_existing(&mut self, list: Vec<(usize, Existing)>) {
        let map: HashMap<usize, Existing> = list.into_iter().collect();
        for p in &mut self.photos {
            if let Some(e) = map.get(&p.id) {
                p.existing = e.clone();
            }
        }
    }

    fn refresh_jpeg_flags(&mut self) {
        let target = self.settings.jpeg_target();
        let folder = self.folder.clone();
        for p in &mut self.photos {
            p.jpeg = match (&folder, &target) {
                (Some(f), Some(t)) if is_jpeg_source(&p.path) => jpeg_path(&p.path, f, t).map_or(false, |j| j.exists()),
                _ => false,
            };
        }
        self.jpeg_target_seen = Some(target);
    }

    fn refresh_backups(&mut self) {
        if let Some(f) = self.folder.clone() {
            let (b, bytes) = list_backups(&f);
            self.backups = b;
            self.backup_bytes = bytes;
        }
    }

    /// Updates the archive row of the current film after reading/writing.
    fn sync_archive_entry(&mut self) {
        let Some(folder) = self.folder.clone() else { return };
        let ex: Vec<Existing> = self.photos.iter().map(|p| p.existing.clone()).collect();
        let stats = folder_stats(&ex);
        let jpeg_sources = self.photos.iter().filter(|p| is_jpeg_source(&p.path)).count();
        let jpegs = self.photos.iter().filter(|p| p.jpeg).count();
        let backups = self.backups.len();
        if let Some(e) = self.archive.folders.iter_mut().find(|f| f.path == folder) {
            e.stats = stats;
            e.jpeg_sources = jpeg_sources;
            e.jpegs = jpegs;
            e.backups = backups;
            e.error = None;
        }
    }

    // ---------------------------------------------------------------- archive scan

    fn start_scan(&mut self, root: PathBuf) {
        let Some(tool) = self.exiftool.clone() else {
            self.set_status(
                tr(
                    "Ohne ExifTool ist keine Prüfung möglich.",
                    "ExifTool is required for scanning.",
                ),
                RED,
            );
            return;
        };
        self.archive.cancel.store(true, Ordering::SeqCst);
        let cancel = Arc::new(AtomicBool::new(false));
        self.archive.cancel = cancel.clone();
        self.archive.generation += 1;
        let generation = self.archive.generation;
        self.archive.folders.clear();
        self.archive.running = true;
        self.archive.done = 0;
        self.archive.total = 0;

        let mut skip: Vec<String> = vec![
            "exiftool_files".into(),
            "exiftool".into(),
            "imagemagick".into(),
            "cache".into(),
        ];
        let sub = self.settings.jpeg_subfolder.trim().to_lowercase();
        skip.push(if sub.is_empty() { "jpeg".into() } else { sub });
        let skip_path = if self.settings.jpeg_parallel {
            Some(PathBuf::from(self.settings.jpeg_base.trim())).filter(|p| !p.as_os_str().is_empty())
        } else {
            None
        };
        let target = self.settings.jpeg_target();
        let keyword = self.settings.estimate_keyword.clone();
        let tx = self.msg_tx.clone();
        let ctx = self.ctx.clone();

        std::thread::spawn(move || {
            let dirs = collect_image_dirs(&root, &skip, skip_path.as_deref());
            let _ = tx.send(Msg::ScanTotal {
                generation,
                total: dirs.len(),
            });
            ctx.request_repaint();
            let mut cancelled = false;
            for dir in dirs {
                if cancel.load(Ordering::SeqCst) {
                    cancelled = true;
                    break;
                }
                let info = scan_folder(&tool, &root, &dir, target.as_ref(), &keyword);
                let _ = tx.send(Msg::ScanFolder { generation, info });
                ctx.request_repaint();
            }
            let _ = tx.send(Msg::ScanDone { generation, cancelled });
            ctx.request_repaint();
        });
    }

    // ---------------------------------------------------------------- place search

    fn geo_ready(&self) -> bool {
        !self.geo_busy
            && self
                .geo_last
                .map_or(true, |t| t.elapsed() >= Duration::from_millis(1100))
    }

    fn start_geo_search(&mut self) {
        let q = self.place_query.trim().to_string();
        if q.is_empty() {
            return;
        }
        let key = format!("{}|{}", self.lang_code(), q.to_lowercase());
        if let Some(hits) = self.geo_cache.get(&key) {
            self.geo_hits = hits.clone();
            self.geo_error = None;
            return;
        }
        if !self.geo_ready() {
            return;
        }
        self.geo_busy = true;
        self.geo_last = Some(Instant::now());
        self.geo_error = None;
        let lang = self.lang_code();
        let tx = self.msg_tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = geo::search(&q, lang);
            let _ = tx.send(Msg::GeoSearch { key, result });
            ctx.request_repaint();
        });
    }

    fn start_geo_reverse(&mut self) {
        let Ok((lat, lon)) = parse_coords(&self.coord_input) else {
            return;
        };
        if !self.geo_ready() {
            return;
        }
        self.geo_busy = true;
        self.geo_last = Some(Instant::now());
        self.geo_error = None;
        let lang = self.lang_code();
        let tx = self.msg_tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = geo::reverse(lat, lon, lang);
            let _ = tx.send(Msg::GeoReverse { result });
            ctx.request_repaint();
        });
    }

    fn use_geo_hit(&mut self, h: &GeoHit) {
        self.coord_input = format!("{:.6}, {:.6}", h.lat, h.lon);
        self.city_input = h.city.clone();
        self.country_input = h.country.clone();
    }

    // ---------------------------------------------------------------- background results

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(done) = self.loader.results.try_recv() {
            if done.generation != self.generation {
                continue;
            }
            match done.kind {
                Kind::Thumb => {
                    if let Some(p) = self.photos.iter_mut().find(|p| p.id == done.id) {
                        match done.image {
                            Ok(img) => {
                                p.thumb = Some(ctx.load_texture(format!("thumb-{}", p.id), img, TextureOptions::LINEAR))
                            }
                            Err(e) => p.thumb_error = Some(e),
                        }
                    }
                }
                Kind::Full => match done.image {
                    Ok(img) => {
                        self.full = Some((done.id, ctx.load_texture("full", img, TextureOptions::LINEAR)));
                    }
                    Err(e) => self.full_error = Some((done.id, e)),
                },
            }
        }

        while let Ok(msg) = self.msg_rx.try_recv() {
            match msg {
                Msg::Meta { generation, result } => {
                    if generation != self.generation {
                        continue;
                    }
                    self.busy = Busy::Idle;
                    match result {
                        Ok(list) => {
                            self.apply_existing(list);
                            if self.settings.film_auto {
                                let info = self.settings.film_info();
                                if !info.is_empty() {
                                    for p in &mut self.photos {
                                        p.edits.film = Some(info.clone());
                                    }
                                }
                            }
                            self.refresh_jpeg_flags();
                            self.sync_archive_entry();
                            let missing = self.photos.iter().filter(|p| p.existing.datetime.is_none()).count();
                            let n = self.photos.len();
                            self.set_status(
                                trs(
                                    format!("{n} Bilder geladen – {missing} davon ohne Aufnahmedatum."),
                                    format!("{n} images loaded – {missing} without capture date."),
                                ),
                                Color32::GRAY,
                            );
                        }
                        Err(e) => self.set_status(
                            trs(
                                format!("Metadaten konnten nicht gelesen werden: {e}"),
                                format!("Could not read metadata: {e}"),
                            ),
                            RED,
                        ),
                    }
                }
                Msg::Progress { label, done, total } => self.busy = Busy::Working { label, done, total },
                Msg::Done {
                    generation,
                    written,
                    mut errors,
                    warnings,
                    reread,
                    jpeg_created,
                    jpeg_updated,
                } => {
                    self.busy = Busy::Idle;
                    if generation != self.generation {
                        continue;
                    }
                    let ok: HashSet<usize> = written.iter().copied().collect();
                    for p in &mut self.photos {
                        if ok.contains(&p.id) {
                            p.edits = Edits::default();
                        }
                    }
                    match reread {
                        Ok(list) => self.apply_existing(list),
                        Err(e) => errors.push(trs(
                            format!("Erneutes Einlesen fehlgeschlagen: {e}"),
                            format!("Re-reading failed: {e}"),
                        )),
                    }
                    let planned = self.planned();
                    for (i, p) in self.photos.iter_mut().enumerate() {
                        if change_row(p.id, i + 1, &p.name, &p.existing, &p.edits, planned[i]).is_none() {
                            p.edits = Edits::default();
                        }
                    }
                    self.undo.clear();
                    self.refresh_jpeg_flags();
                    self.refresh_backups();
                    self.sync_archive_entry();

                    let mut parts = Vec::new();
                    if !written.is_empty() {
                        parts.push(trs(
                            format!("{} Bild(er) geschrieben", written.len()),
                            format!("{} image(s) written", written.len()),
                        ));
                    }
                    if jpeg_created > 0 {
                        parts.push(trs(
                            format!("{jpeg_created} JPEG(s) erzeugt"),
                            format!("{jpeg_created} JPEG(s) created"),
                        ));
                    }
                    if jpeg_updated > 0 {
                        parts.push(trs(
                            format!("{jpeg_updated} JPEG(s) aktualisiert"),
                            format!("{jpeg_updated} JPEG(s) updated"),
                        ));
                    }
                    let summary = if parts.is_empty() {
                        tr("Nichts zu tun.", "Nothing to do.").to_string()
                    } else {
                        parts.join(", ")
                    };
                    if errors.is_empty() {
                        self.set_status(format!("✔ {summary}."), GREEN);
                        if !warnings.is_empty() {
                            self.dialog = Dialog::Report {
                                title: tr("Hinweise von ExifTool", "Notes from ExifTool").into(),
                                lines: warnings,
                            };
                        }
                    } else {
                        self.set_status(
                            trs(
                                format!("{summary} – {} Fehler.", errors.len()),
                                format!("{summary} – {} error(s).", errors.len()),
                            ),
                            RED,
                        );
                        let mut lines = errors;
                        lines.extend(warnings);
                        self.dialog = Dialog::Report {
                            title: tr("Es gab Fehler", "There were errors").into(),
                            lines,
                        };
                    }
                }
                Msg::ScanTotal { generation, total } => {
                    if generation == self.archive.generation {
                        self.archive.total = total;
                    }
                }
                Msg::ScanFolder { generation, info } => {
                    if generation == self.archive.generation {
                        self.archive.done += 1;
                        self.archive.folders.push(info);
                    }
                }
                Msg::ScanDone { generation, cancelled } => {
                    if generation == self.archive.generation {
                        self.archive.running = false;
                        self.archive.folders.sort_by(|a, b| natural_cmp(&a.rel, &b.rel));
                        let n = self.archive.folders.len();
                        let open = self.archive.folders.iter().filter(|f| f.stats.no_date > 0).count();
                        let text = if cancelled {
                            tr("Prüfung abgebrochen.", "Scan cancelled.").to_string()
                        } else {
                            trs(
                                format!("Archiv geprüft: {n} Filmordner, {open} mit fehlenden Daten."),
                                format!("Archive scanned: {n} film folders, {open} with missing dates."),
                            )
                        };
                        self.set_status(text, if open > 0 { AMBER } else { GREEN });
                    }
                }
                Msg::GeoSearch { key, result } => {
                    self.geo_busy = false;
                    match result {
                        Ok(hits) => {
                            if hits.is_empty() {
                                self.geo_error = Some(tr("Kein Ort gefunden.", "No place found.").into());
                            }
                            self.geo_cache.insert(key, hits.clone());
                            self.geo_hits = hits;
                        }
                        Err(e) => self.geo_error = Some(e),
                    }
                }
                Msg::GeoReverse { result } => {
                    self.geo_busy = false;
                    match result {
                        Ok(h) => {
                            self.city_input = h.city;
                            self.country_input = h.country;
                        }
                        Err(e) => self.geo_error = Some(e),
                    }
                }
            }
        }
    }

    // ---------------------------------------------------------------- actions

    fn current_date_spec(&self) -> Result<DateSpec, String> {
        let (d, p) = parse_date_input(&self.date_input)?;
        let t = parse_time(&self.time_input)?;
        Ok(DateSpec::new(d, p, t, self.estimated))
    }

    fn apply_date(&mut self) {
        let ids = self.target_ids();
        self.apply_date_to(&ids);
    }

    fn apply_date_to(&mut self, ids: &[usize]) -> bool {
        let spec = match self.current_date_spec() {
            Ok(s) => s,
            Err(e) => {
                self.set_status(e, RED);
                return false;
            }
        };
        let n = self.edit_ids(ids, |e| e.date = Some(Change::Set(spec)));
        let est = if spec.estimated { " (ca.)" } else { "" };
        let d = spec.describe();
        self.set_status(
            trs(
                format!("Datum {d}{est} für {n} Bild(er) vorgemerkt."),
                format!("Date {d}{est} planned for {n} image(s)."),
            ),
            BLUE,
        );
        true
    }

    fn next_day_from_selection(&mut self) {
        if let Some(pos) = self.first_selected() {
            self.next_day_from(pos);
        }
    }

    /// From image `pos` to the end of the film: the day after that image's date.
    fn next_day_from(&mut self, pos: usize) {
        let planned = self.planned();
        let p = &self.photos[pos];
        let Some(w) = effective_when(&p.existing, &p.edits, planned[pos]) else {
            self.set_status(
                tr(
                    "Das ausgewählte Bild hat noch kein Datum.",
                    "The selected image has no date yet.",
                ),
                AMBER,
            );
            return;
        };
        if w.precision != Precision::Day {
            self.set_status(
                tr(
                    "Nur bei Bildern mit genauem Tag möglich.",
                    "Only possible for images with an exact day.",
                ),
                AMBER,
            );
            return;
        }
        let Some(next) = w.dt.date.add_days(1) else { return };
        let start = parse_time(&self.time_input).unwrap_or((12, 0));
        let spec = DateSpec::new(next, Precision::Day, start, w.estimated);
        self.snapshot();
        let n = self.photos.len() - pos;
        for p in self.photos[pos..].iter_mut() {
            p.edits.date = Some(Change::Set(spec));
        }
        let d = spec.describe();
        self.set_status(
            trs(
                format!("Ab Bild {}: {d} für {n} Bild(er) vorgemerkt.", pos + 1),
                format!("From image {}: {d} planned for {n} image(s).", pos + 1),
            ),
            BLUE,
        );
    }

    fn apply_location(&mut self) {
        let ids = self.target_ids();
        self.apply_location_to(&ids);
    }

    fn apply_location_to(&mut self, ids: &[usize]) -> bool {
        let (lat, lon) = match parse_coords(&self.coord_input) {
            Ok(c) => c,
            Err(e) => {
                self.set_status(e, RED);
                return false;
            }
        };
        let preset = PlacePreset {
            lat,
            lon,
            city: self.city_input.trim().to_string(),
            country: self.country_input.trim().to_string(),
        };
        self.apply_place_preset(ids, preset);
        true
    }

    fn apply_place_preset(&mut self, ids: &[usize], pl: PlacePreset) {
        let loc = Location {
            lat: pl.lat,
            lon: pl.lon,
            city: pl.city.clone(),
            country: pl.country.clone(),
        };
        let n = self.edit_ids(ids, |e| e.loc = Some(Change::Set(loc.clone())));
        let name = pl.label();
        self.settings.remember_place(pl);
        self.set_status(
            trs(
                format!("Standort {name} für {n} Bild(er) vorgemerkt."),
                format!("Location {name} planned for {n} image(s)."),
            ),
            GREEN,
        );
    }

    fn apply_desc(&mut self) {
        let ids = self.target_ids();
        self.apply_desc_to(&ids);
    }

    fn apply_desc_to(&mut self, ids: &[usize]) -> bool {
        let d = self.desc_input.trim().to_string();
        let n = self.edit_ids(ids, |e| e.desc = Some(d.clone()));
        self.set_status(
            trs(
                format!("Beschreibung für {n} Bild(er) vorgemerkt."),
                format!("Description planned for {n} image(s)."),
            ),
            BLUE,
        );
        true
    }

    fn apply_film_info(&mut self) {
        let info = self.settings.film_info();
        if info.is_empty() {
            self.set_status(
                tr(
                    "Bitte mindestens eine Film-Angabe ausfüllen.",
                    "Please fill in at least one film field.",
                ),
                AMBER,
            );
            return;
        }
        let n = self.edit_targets(|e| e.film = Some(info.clone()));
        self.set_status(
            trs(
                format!("Film-Angaben für {n} Bild(er) vorgemerkt."),
                format!("Film info planned for {n} image(s)."),
            ),
            BLUE,
        );
    }

    fn rotate_images(&mut self, clockwise: bool) {
        let ids = self.action_ids();
        if ids.is_empty() {
            self.set_status(
                tr(
                    "Erst Bilder auswählen (oder Großansicht öffnen).",
                    "Select images first (or open the large view).",
                ),
                AMBER,
            );
            return;
        }
        self.rotate_ids(&ids, clockwise);
    }

    fn rotate_ids(&mut self, ids: &[usize], clockwise: bool) {
        if ids.is_empty() {
            return;
        }
        self.snapshot();
        let set: HashSet<usize> = ids.iter().copied().collect();
        for p in self.photos.iter_mut().filter(|p| set.contains(&p.id)) {
            let new = rotate_orientation(effective_orientation(&p.existing, &p.edits), clockwise);
            p.edits.orientation = if new == p.existing.orientation { None } else { Some(new) };
        }
    }

    fn set_rating(&mut self, r: i8) {
        let ids = self.action_ids();
        self.rate_ids(&ids, r);
    }

    fn rate_ids(&mut self, ids: &[usize], r: i8) {
        if ids.is_empty() {
            return;
        }
        self.snapshot();
        let set: HashSet<usize> = ids.iter().copied().collect();
        for p in self.photos.iter_mut().filter(|p| set.contains(&p.id)) {
            p.edits.rating = if r == p.existing.rating { None } else { Some(r) };
        }
    }

    fn toggle_reject(&mut self) {
        let ids = self.action_ids();
        self.toggle_reject_ids(&ids);
    }

    fn toggle_reject_ids(&mut self, ids: &[usize]) {
        if ids.is_empty() {
            return;
        }
        let all_rejected = self
            .photos
            .iter()
            .filter(|p| ids.contains(&p.id))
            .all(|p| effective_rating(&p.existing, &p.edits) < 0);
        self.rate_ids(ids, if all_rejected { 0 } else { -1 });
    }

    /// Executes a right-click menu action for the given images.
    fn run_menu_action(&mut self, action: MenuAction, ids: Vec<usize>) {
        if ids.is_empty() {
            return;
        }
        match action {
            MenuAction::Edit(kind) => {
                self.geo_hits.clear();
                self.geo_error = None;
                self.dialog = Dialog::Quick { ids, kind };
            }
            MenuAction::Place(pl) => self.apply_place_preset(&ids, pl),
            MenuAction::NextDay => {
                if let Some(pos) = ids.iter().filter_map(|id| self.pos_of(*id)).min() {
                    self.next_day_from(pos);
                }
            }
            MenuAction::Rotate(cw) => self.rotate_ids(&ids, cw),
            MenuAction::Rating(r) => self.rate_ids(&ids, r),
            MenuAction::Reject => self.toggle_reject_ids(&ids),
            MenuAction::DiscardDate => {
                self.edit_ids(&ids, |e| e.date = None);
            }
            MenuAction::DiscardPlace => {
                self.edit_ids(&ids, |e| e.loc = None);
            }
            MenuAction::DiscardAll => {
                self.edit_ids(&ids, |e| *e = Edits::default());
            }
            MenuAction::RemoveDate => {
                self.edit_ids(&ids, |e| e.date = Some(Change::Remove));
            }
            MenuAction::RemovePlace => {
                self.edit_ids(&ids, |e| e.loc = Some(Change::Remove));
            }
            MenuAction::Large => self.open_viewer(ids[0]),
            MenuAction::SelectAll => self.selection = self.photos.iter().map(|p| p.id).collect(),
            MenuAction::ClearSelection => self.selection.clear(),
        }
    }

    /// Location inputs for the right-click dialog (search, recent places, coordinates, place, country).
    fn location_form(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let ready = self.geo_ready() && !self.place_query.trim().is_empty();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let clicked = ui
                    .add_enabled(ready, egui::Button::new(tr("Suchen", "Search")))
                    .clicked();
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.place_query)
                        .hint_text(tr("Ort suchen, z. B. Borkum", "Search place, e.g. Borkum"))
                        .desired_width(f32::INFINITY),
                );
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if clicked || (enter && ready) {
                    self.start_geo_search();
                }
            });
        });
        if !self.geo_ready() {
            ui.ctx().request_repaint_after(Duration::from_millis(300));
        }
        if self.geo_busy {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.label(tr("Suche …", "Searching …"));
            });
        }
        if let Some(e) = &self.geo_error {
            ui.colored_label(AMBER, e.as_str());
        }
        if !self.geo_hits.is_empty() {
            let mut pick: Option<GeoHit> = None;
            egui::Frame::group(ui.style()).show(ui, |ui| {
                for h in &self.geo_hits {
                    if ui.selectable_label(false, h.display.as_str()).clicked() {
                        pick = Some(h.clone());
                    }
                }
            });
            if let Some(h) = pick {
                self.use_geo_hit(&h);
                self.geo_hits.clear();
            }
        }
        if !self.settings.recent_places.is_empty() {
            let mut pick: Option<PlacePreset> = None;
            ui.menu_button(tr("zuletzt verwendete Orte ▾", "recently used places ▾"), |ui| {
                for pl in &self.settings.recent_places {
                    if ui.button(pl.label()).clicked() {
                        pick = Some(pl.clone());
                        ui.close_menu();
                    }
                }
            });
            if let Some(pl) = pick {
                self.coord_input = format!("{:.6}, {:.6}", pl.lat, pl.lon);
                self.city_input = pl.city;
                self.country_input = pl.country;
            }
        }
        labeled_field(
            ui,
            tr("Koordinaten:", "Coordinates:"),
            &mut self.coord_input,
            "53.5862, 6.6667",
        );
        match parse_coords(&self.coord_input) {
            Ok((a, b)) => {
                ui.horizontal(|ui| {
                    ui.colored_label(GREEN, format!("✔ {a:.5}, {b:.5}"));
                    if ui.small_button(tr("Auf Karte zeigen", "Show on map")).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(geo::map_url(a, b)));
                    }
                    if ui
                        .add_enabled(
                            self.geo_ready(),
                            egui::Button::new(tr("Ortsname ermitteln", "Look up place name")).small(),
                        )
                        .clicked()
                    {
                        self.start_geo_reverse();
                    }
                });
            }
            Err(e) if !self.coord_input.trim().is_empty() => {
                ui.colored_label(RED, e.as_str());
            }
            Err(_) => {}
        }
        labeled_field(ui, tr("Ort:", "Place:"), &mut self.city_input, "");
        labeled_field(ui, tr("Land:", "Country:"), &mut self.country_input, "");
    }

    fn take_date_from_selection(&mut self) {
        let Some(pos) = self.first_selected() else { return };
        let planned = self.planned();
        let p = &self.photos[pos];
        if let Some(w) = effective_when(&p.existing, &p.edits, planned[pos]) {
            self.date_input = date_input_text(w.dt.date, w.precision);
            self.estimated = w.estimated;
        } else {
            self.set_status(
                tr(
                    "Das ausgewählte Bild hat noch kein Datum.",
                    "The selected image has no date yet.",
                ),
                AMBER,
            );
        }
    }

    fn take_place_from_selection(&mut self) {
        let Some(pos) = self.first_selected() else { return };
        let p = &self.photos[pos];
        if let Some(l) = effective_location(&p.existing, &p.edits) {
            self.coord_input = format!("{:.6}, {:.6}", l.lat, l.lon);
            self.city_input = l.city;
            self.country_input = l.country;
        } else {
            self.set_status(
                tr(
                    "Das ausgewählte Bild hat noch keinen Standort.",
                    "The selected image has no location yet.",
                ),
                AMBER,
            );
        }
    }

    fn click_tile(&mut self, pos: usize, m: Modifiers) {
        let id = self.photos[pos].id;
        if m.shift {
            if let Some(a) = self.anchor.and_then(|a| self.pos_of(a)) {
                let (lo, hi) = if a <= pos { (a, pos) } else { (pos, a) };
                if !m.command && !self.multi_click {
                    self.selection.clear();
                }
                for i in lo..=hi {
                    self.selection.insert(self.photos[i].id);
                }
                return;
            }
        }
        if m.command || self.multi_click {
            if !self.selection.remove(&id) {
                self.selection.insert(id);
            }
        } else if self.selection.len() == 1 && self.selection.contains(&id) {
            self.selection.clear();
        } else {
            self.selection.clear();
            self.selection.insert(id);
        }
        self.anchor = Some(id);
    }

    fn toggle_selected(&mut self, id: usize) {
        if !self.selection.remove(&id) {
            self.selection.insert(id);
        }
    }

    /// After any change of order: let times follow the order again.
    fn after_reorder(&mut self) {
        if !self.settings.resequence_on_move {
            return;
        }
        let specs = {
            let items: Vec<(&Existing, &Edits)> = self.photos.iter().map(|p| (&p.existing, &p.edits)).collect();
            resequence_specs(&items)
        };
        for (p, s) in self.photos.iter_mut().zip(specs) {
            if let Some(s) = s {
                p.edits.date = Some(Change::Set(s));
            }
        }
    }

    fn move_photo(&mut self, id: usize, to: usize) {
        if let Some(from) = self.pos_of(id) {
            if from != to && to < self.photos.len() {
                self.snapshot();
                let p = self.photos.remove(from);
                self.photos.insert(to, p);
                self.after_reorder();
            }
        }
    }

    fn move_selection(&mut self, dir: i32) {
        self.snapshot();
        let n = self.photos.len();
        let sel = |s: &Self, i: usize| s.selection.contains(&s.photos[i].id);
        if dir < 0 {
            for i in 1..n {
                if sel(self, i) && !sel(self, i - 1) {
                    self.photos.swap(i, i - 1);
                }
            }
        } else {
            for i in (0..n.saturating_sub(1)).rev() {
                if sel(self, i) && !sel(self, i + 1) {
                    self.photos.swap(i, i + 1);
                }
            }
        }
        self.after_reorder();
    }

    fn sort_by_name(&mut self) {
        self.snapshot();
        self.photos.sort_by(|a, b| natural_cmp(&a.name, &b.name));
        self.after_reorder();
    }

    fn sort_by_date(&mut self) {
        self.snapshot();
        let planned = self.planned();
        let keys: HashMap<usize, Option<DateTime>> = self
            .photos
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id, effective_when(&p.existing, &p.edits, planned[i]).map(|w| w.dt)))
            .collect();
        self.photos.sort_by_key(|p| {
            let k = keys.get(&p.id).copied().flatten();
            (k.is_none(), k)
        });
        self.after_reorder();
    }

    fn open_viewer(&mut self, id: usize) {
        self.viewer = Some(id);
        let have = matches!(&self.full, Some((fid, _)) if *fid == id);
        if !have {
            if let Some(pos) = self.pos_of(id) {
                self.full_error = None;
                self.loader.request(Request {
                    generation: self.generation,
                    id,
                    path: self.photos[pos].path.clone(),
                    kind: Kind::Full,
                });
            }
        }
    }

    fn prepare_backup_cleanup(&mut self) {
        let Some(folder) = self.folder.clone() else { return };
        let (files, _) = list_backups(&folder);
        let mut ok = Vec::new();
        let mut bytes = 0;
        let mut skipped = 0;
        for b in files {
            let original_fine = backup_original(&b)
                .and_then(|o| std::fs::metadata(o).ok())
                .map_or(false, |m| m.is_file() && m.len() > 0);
            if original_fine {
                bytes += std::fs::metadata(&b).map(|m| m.len()).unwrap_or(0);
                ok.push(b);
            } else {
                skipped += 1;
            }
        }
        self.dialog = Dialog::DeleteBackups {
            files: ok,
            bytes,
            skipped,
        };
    }

    /// Writes the metadata of `rows` and then (optionally) creates/updates the JPEG copies.
    fn start_job(&mut self, rows: Vec<ChangeRow>, with_jpeg: bool) {
        let Some(tool) = self.exiftool.clone() else { return };
        let planned = self.planned();
        let opts = self.settings.write_options();

        let mut meta_jobs: Vec<(usize, String, PathBuf, Vec<String>)> = Vec::new();
        for r in &rows {
            if let Some(pos) = self.pos_of(r.id) {
                let p = &self.photos[pos];
                meta_jobs.push((
                    p.id,
                    p.name.clone(),
                    p.path.clone(),
                    write_args(&p.edits, planned[pos], &opts),
                ));
            }
        }

        let mut jpeg_jobs: Vec<JpegJob> = Vec::new();
        if with_jpeg {
            if let (Some(folder), Some(target), Some(_)) = (&self.folder, self.settings.jpeg_target(), &self.magick) {
                for p in self.photos.iter().filter(|p| is_jpeg_source(&p.path)) {
                    if self.settings.jpeg_skip_rejects && effective_rating(&p.existing, &p.edits) < 0 {
                        continue;
                    }
                    if let Some(dst) = jpeg_path(&p.path, folder, &target) {
                        jpeg_jobs.push(JpegJob {
                            name: p.name.clone(),
                            src: p.path.clone(),
                            dst,
                            rotate: quarter_turns(effective_orientation(&p.existing, &p.edits)) * 90,
                            force: p.edits.orientation.is_some(),
                        });
                    }
                }
            }
        }
        let magick = self.magick.clone();
        let jset = JpegSettings {
            quality: self.settings.jpeg_quality,
            max_edge: self.settings.jpeg_max_edge,
            overwrite: self.settings.jpeg_overwrite,
            set_file_date: self.settings.set_file_date,
        };
        let threads = self.settings.jpeg_threads.max(1) as usize;
        let all: Vec<(usize, PathBuf)> = self.photos.iter().map(|p| (p.id, p.path.clone())).collect();
        let keyword = self.settings.estimate_keyword.clone();
        let total = meta_jobs.len() + jpeg_jobs.len();
        self.busy = Busy::Working {
            label: tr("Starte …", "Starting …").into(),
            done: 0,
            total,
        };
        let tx = self.msg_tx.clone();
        let ctx = self.ctx.clone();
        let generation = self.generation;

        std::thread::spawn(move || {
            let mut written = Vec::new();
            let mut errors = Vec::new();
            let mut warnings = Vec::new();
            let mut step = 0usize;

            for (id, name, path, args) in meta_jobs {
                let _ = tx.send(Msg::Progress {
                    label: trs(format!("Metadaten: {name}"), format!("Metadata: {name}")),
                    done: step,
                    total,
                });
                ctx.request_repaint();
                match tool.write_file(&path, &args) {
                    Ok(w) => {
                        written.push(id);
                        if !w.is_empty() {
                            warnings.push(format!("{name}: {w}"));
                        }
                    }
                    Err(e) => errors.push(format!("{name}: {e}")),
                }
                step += 1;
            }

            let (mut created, mut updated) = (0usize, 0usize);
            if let Some(m) = &magick {
                if !jpeg_jobs.is_empty() {
                    let counter = AtomicUsize::new(step);
                    let on_done = |name: &str| {
                        let d = counter.fetch_add(1, Ordering::SeqCst) + 1;
                        let _ = tx.send(Msg::Progress {
                            label: format!("JPEG: {name}"),
                            done: d,
                            total,
                        });
                        ctx.request_repaint();
                    };
                    let (c, u, errs) = export_many(m, &tool, jpeg_jobs, &jset, threads, &on_done);
                    created = c;
                    updated = u;
                    errors.extend(errs);
                    step = counter.load(Ordering::SeqCst);
                }
            }
            let _ = tx.send(Msg::Progress {
                label: tr("Lese Metadaten neu …", "Re-reading metadata …").into(),
                done: step,
                total,
            });
            ctx.request_repaint();

            let reread = read_ids(&tool, &all, &keyword);
            let _ = tx.send(Msg::Done {
                generation,
                written,
                errors,
                warnings,
                reread,
                jpeg_created: created,
                jpeg_updated: updated,
            });
            ctx.request_repaint();
        });
    }

    // ---------------------------------------------------------------- input

    fn handle_close(&mut self, ctx: &egui::Context, pending: usize) {
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.working() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.set_status(
                    tr(
                        "Bitte warten, bis das Schreiben abgeschlossen ist.",
                        "Please wait until writing has finished.",
                    ),
                    AMBER,
                );
            } else if pending > 0 && !self.allow_close {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.dialog = Dialog::Discard(After::Quit);
            }
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() || self.photos.is_empty() || self.view != View::Film {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Z)) {
            self.undo();
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A)) {
            self.selection = self.photos.iter().map(|p| p.id).collect();
            return;
        }
        let (esc, left, right, space, rot_r, rot_l, reject, rating) = ctx.input(|i| {
            let plain = !i.modifiers.command && !i.modifiers.alt;
            let mut rating: Option<i8> = None;
            for (k, v) in [
                (Key::Num0, 0i8),
                (Key::Num1, 1),
                (Key::Num2, 2),
                (Key::Num3, 3),
                (Key::Num4, 4),
                (Key::Num5, 5),
            ] {
                if plain && i.key_pressed(k) {
                    rating = Some(v);
                }
            }
            (
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                i.key_pressed(Key::Space),
                plain && i.key_pressed(Key::R),
                plain && i.key_pressed(Key::L),
                plain && i.key_pressed(Key::X),
                rating,
            )
        });
        if rot_r || rot_l {
            self.rotate_images(rot_r);
            return;
        }
        if reject {
            self.toggle_reject();
            return;
        }
        if let Some(r) = rating {
            self.set_rating(r);
            return;
        }
        if let Some(id) = self.viewer {
            if esc {
                self.viewer = None;
            } else if left || right {
                if let Some(pos) = self.pos_of(id) {
                    let next = if left {
                        pos.checked_sub(1)
                    } else if pos + 1 < self.photos.len() {
                        Some(pos + 1)
                    } else {
                        None
                    };
                    if let Some(np) = next {
                        let nid = self.photos[np].id;
                        self.open_viewer(nid);
                    }
                }
            } else if space {
                self.toggle_selected(id);
            }
        } else if esc {
            self.selection.clear();
        }
    }

    // ---------------------------------------------------------------- top bar

    fn top_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Rollstamp").strong().size(17.0));
            ui.separator();
            ui.selectable_value(&mut self.view, View::Film, tr("🎞 Film", "🎞 Film"));
            ui.selectable_value(
                &mut self.view,
                View::Archive,
                tr("🗂 Archiv-Übersicht", "🗂 Archive overview"),
            );
            ui.separator();
            if ui
                .button(tr("📂 Filmordner öffnen …", "📂 Open film folder …"))
                .clicked()
            {
                self.want_open = true;
            }
            let prev = self.sibling(-1);
            let next = self.sibling(1);
            if ui
                .add_enabled(
                    prev.is_some(),
                    egui::Button::new(tr("◀ Voriger Film", "◀ Previous film")),
                )
                .on_hover_text(prev.as_deref().map(file_name).unwrap_or_default())
                .clicked()
            {
                self.nav = prev.clone();
            }
            if ui
                .add_enabled(next.is_some(), egui::Button::new(tr("Nächster Film ▶", "Next film ▶")))
                .on_hover_text(next.as_deref().map(file_name).unwrap_or_default())
                .clicked()
            {
                self.nav = next.clone();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let current = match self.settings.language.as_str() {
                    "de" => "Deutsch",
                    "en" => "English",
                    _ => tr("Automatisch", "Automatic"),
                };
                egui::ComboBox::from_label("🌐")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.settings.language,
                            "auto".to_string(),
                            tr("Automatisch", "Automatic"),
                        );
                        ui.selectable_value(&mut self.settings.language, "de".to_string(), "Deutsch");
                        ui.selectable_value(&mut self.settings.language, "en".to_string(), "English");
                    });
            });
        });

        if self.view == View::Film {
            ui.horizontal(|ui| {
                match &self.folder {
                    Some(f) => {
                        ui.label(RichText::new(f.display().to_string()).strong());
                        ui.label(
                            RichText::new(trs(
                                format!("· {} Bilder", self.photos.len()),
                                format!("· {} images", self.photos.len()),
                            ))
                            .weak(),
                        );
                    }
                    None => {
                        ui.label(RichText::new(tr("Noch kein Ordner geladen", "No folder loaded yet")).weak());
                    }
                }
                if matches!(self.busy, Busy::Reading) {
                    ui.add(egui::Spinner::new());
                    ui.label(tr("Metadaten werden gelesen …", "Reading metadata …"));
                }
            });
        }

        if self.exiftool.is_none() {
            ui.colored_label(
                RED,
                tr(
                    "⚠ ExifTool wurde nicht gefunden. Lege den Ordner „exiftool“ (exiftool.exe + exiftool_files) neben rollstamp.exe und starte neu.",
                    "⚠ ExifTool not found. Put the folder “exiftool” (exiftool.exe + exiftool_files) next to rollstamp.exe and restart.",
                ),
            );
        }

        if self.view == View::Film && !self.photos.is_empty() {
            ui.horizontal(|ui| {
                let (mut r, mut b, mut g, mut x) = (0, 0, 0, 0);
                for p in &self.photos {
                    match status(&p.existing, &p.edits) {
                        Status::Missing => r += 1,
                        Status::Dated => b += 1,
                        Status::Located => g += 1,
                    }
                    if effective_rating(&p.existing, &p.edits) < 0 {
                        x += 1;
                    }
                }
                legend(ui, RED, &trs(format!("ohne Datum: {r}"), format!("no date: {r}")));
                legend(ui, BLUE, &trs(format!("mit Datum: {b}"), format!("dated: {b}")));
                legend(
                    ui,
                    GREEN,
                    &trs(format!("Datum + Ort: {g}"), format!("date + place: {g}")),
                );
                if x > 0 {
                    ui.label(trs(format!("✖ Ausschuss: {x}"), format!("✖ rejects: {x}")));
                }
                let (dot, _) = ui.allocate_exact_size(vec2(12.0, 16.0), Sense::hover());
                ui.painter().circle_filled(dot.center(), 5.0, AMBER);
                ui.label(tr("noch nicht geschrieben", "not written yet"));
                ui.separator();
                ui.label(trs(
                    format!("Ausgewählt: {}", self.selection.len()),
                    format!("Selected: {}", self.selection.len()),
                ));
                if ui.small_button(tr("Alle", "All")).clicked() {
                    self.selection = self.photos.iter().map(|p| p.id).collect();
                }
                if ui.small_button(tr("Keine", "None")).clicked() {
                    self.selection.clear();
                }
                ui.checkbox(
                    &mut self.multi_click,
                    tr("Klick wählt mehrere", "Click selects multiple"),
                );
                if ui
                    .add_enabled(
                        !self.undo.is_empty(),
                        egui::Button::new(tr("↶ Rückgängig", "↶ Undo")).small(),
                    )
                    .on_hover_text("Ctrl+Z")
                    .clicked()
                {
                    self.undo();
                }
            });
        }

        if let Some((text, color)) = &self.status {
            ui.colored_label(*color, text.as_str());
        }
        ui.add_space(4.0);
    }

    // ---------------------------------------------------------------- side panel

    fn side_ui(&mut self, ui: &mut egui::Ui, rows: &[ChangeRow]) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(6.0);
            ui.heading(tr("Metadaten setzen", "Set metadata"));
            ui.add_space(4.0);

            let n_all = self.photos.len();
            let n_sel = self.selection.len();
            ui.horizontal(|ui| {
                ui.label(tr("Gilt für:", "Applies to:"));
                ui.radio_value(&mut self.target, Target::All, trs(format!("ganzen Film ({n_all})"), format!("whole film ({n_all})")));
                ui.add_enabled_ui(n_sel > 0, |ui| {
                    ui.radio_value(&mut self.target, Target::Selection, trs(format!("Auswahl ({n_sel})"), format!("selection ({n_sel})")));
                });
            });
            let n_target = self.target_ids().len();
            let can_apply = n_target > 0;

            // ---------- date
            section(ui, tr("📅 Datum", "📅 Date"));
            ui.horizontal(|ui| {
                ui.label(tr("TT.MM.JJJJ, MM.JJJJ oder JJJJ:", "DD.MM.YYYY, MM.YYYY or YYYY:"));
                if ui
                    .add_enabled(n_sel > 0, egui::Button::new(tr("aus Auswahl", "from selection")).small())
                    .on_hover_text(tr("Datum des ersten ausgewählten Bildes übernehmen", "Take the date of the first selected image"))
                    .clicked()
                {
                    self.take_date_from_selection();
                }
            });
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.date_input)
                    .hint_text(tr("z. B. 14.07.2001", "e.g. 14.07.2001"))
                    .desired_width(f32::INFINITY),
            );
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            let parsed = parse_date_input(&self.date_input);
            match &parsed {
                Ok((d, p)) => {
                    ui.colored_label(GREEN, format!("✔ {} – {}", describe_date(*d, *p), p.label()));
                }
                Err(e) if !self.date_input.trim().is_empty() => {
                    ui.colored_label(RED, e.as_str());
                }
                Err(_) => {}
            }

            ui.horizontal(|ui| {
                ui.label(tr("Startzeit:", "Start time:"));
                ui.add(egui::TextEdit::singleline(&mut self.time_input).desired_width(56.0));
                ui.label(RichText::new(tr("danach +1 Minute pro Bild", "then +1 minute per image")).weak());
            });
            let time = parse_time(&self.time_input);
            if let Err(e) = &time {
                ui.colored_label(RED, e.as_str());
            }

            let forced = matches!(&parsed, Ok((_, p)) if *p != Precision::Day);
            if forced {
                let mut t = true;
                ui.add_enabled(false, egui::Checkbox::new(&mut t, tr("ungefähres Datum (bei Monat/Jahr immer)", "approximate date (always for month/year)")));
            } else {
                ui.checkbox(&mut self.estimated, tr("ungefähres Datum (Tag nicht sicher)", "approximate date (day uncertain)"));
            }

            if let (Ok((d, p)), Ok(t)) = (&parsed, &time) {
                if n_target > 0 {
                    let spec = DateSpec::new(*d, *p, *t, self.estimated);
                    let first = spec.at_offset(0).time_str();
                    let last = spec.at_offset(n_target as u32 - 1).time_str();
                    ui.label(RichText::new(trs(
                        format!("{n_target} Bild(er) in Film-Reihenfolge: {first} bis {last} Uhr"),
                        format!("{n_target} image(s) in film order: {first} to {last}"),
                    )).weak());
                }
            }

            let ok = parsed.is_ok() && time.is_ok() && can_apply;
            let label = trs(format!("Datum für {} übernehmen", self.target_label()), format!("Apply date to {}", self.target_label()));
            if ui.add_enabled(ok, egui::Button::new(label)).clicked() || (enter && ok) {
                self.apply_date();
            }
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(n_sel > 0, egui::Button::new(tr("Ab hier +1 Tag", "From here +1 day")).small())
                    .on_hover_text(tr(
                        "Ab dem ersten ausgewählten Bild bis zum Filmende: Tag nach dessen Datum",
                        "From the first selected image to the end of the film: the day after its date",
                    ))
                    .clicked()
                {
                    self.next_day_from_selection();
                }
                if ui
                    .add_enabled(can_apply, egui::Button::new(tr("Vormerkung verwerfen", "Discard planned")).small())
                    .clicked()
                {
                    self.edit_targets(|e| e.date = None);
                }
                if ui
                    .add_enabled(can_apply, egui::Button::new(tr("aus Datei löschen", "remove from file")).small())
                    .on_hover_text(tr("Beim Schreiben das vorhandene Aufnahmedatum entfernen", "Remove the existing capture date when writing"))
                    .clicked()
                {
                    self.edit_targets(|e| e.date = Some(Change::Remove));
                }
            });

            // ---------- location
            section(ui, tr("📍 Standort (optional)", "📍 Location (optional)"));
            // Button first (right-aligned), the text field then takes exactly the remaining width.
            // Never size a widget as "available width - fixed amount" in a resizable panel: if the
            // row ends up wider than the panel, the panel grows every frame.
            ui.horizontal(|ui| {
                let ready = self.geo_ready() && !self.place_query.trim().is_empty();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let clicked = ui.add_enabled(ready, egui::Button::new(tr("Suchen", "Search"))).clicked();
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.place_query)
                            .hint_text(tr("Ort suchen, z. B. Borkum", "Search place, e.g. Borkum"))
                            .desired_width(f32::INFINITY),
                    );
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if clicked || (enter && ready) {
                        self.start_geo_search();
                    }
                });
            });
            if !self.geo_ready() {
                ui.ctx().request_repaint_after(Duration::from_millis(300));
            }
            if self.geo_busy {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new());
                    ui.label(tr("Suche …", "Searching …"));
                });
            }
            if let Some(e) = &self.geo_error {
                ui.colored_label(AMBER, e.as_str());
            }
            if !self.geo_hits.is_empty() {
                let mut pick: Option<GeoHit> = None;
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    for h in &self.geo_hits {
                        if ui.selectable_label(false, h.display.as_str()).clicked() {
                            pick = Some(h.clone());
                        }
                    }
                });
                if let Some(h) = pick {
                    self.use_geo_hit(&h);
                    self.geo_hits.clear();
                }
            }
            if !self.settings.recent_places.is_empty() {
                let mut pick: Option<PlacePreset> = None;
                egui::ComboBox::from_label(tr("zuletzt verwendet", "recently used"))
                    .selected_text(tr("Ort wählen …", "Choose place …"))
                    .show_ui(ui, |ui| {
                        for pl in &self.settings.recent_places {
                            if ui.selectable_label(false, pl.label()).clicked() {
                                pick = Some(pl.clone());
                            }
                        }
                    });
                if let Some(pl) = pick {
                    self.coord_input = format!("{:.6}, {:.6}", pl.lat, pl.lon);
                    self.city_input = pl.city;
                    self.country_input = pl.country;
                }
            }
            ui.horizontal(|ui| {
                ui.label(tr("Koordinaten:", "Coordinates:"));
                if ui.add_enabled(n_sel > 0, egui::Button::new(tr("aus Auswahl", "from selection")).small()).clicked() {
                    self.take_place_from_selection();
                }
            });
            ui.add(egui::TextEdit::singleline(&mut self.coord_input).hint_text("53.5862, 6.6667").desired_width(f32::INFINITY));
            let coords = parse_coords(&self.coord_input);
            match &coords {
                Ok((a, b)) => {
                    let (a, b) = (*a, *b);
                    ui.horizontal(|ui| {
                        ui.colored_label(GREEN, format!("✔ {a:.5}, {b:.5}"));
                        if ui.small_button(tr("Auf Karte zeigen", "Show on map")).clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(geo::map_url(a, b)));
                        }
                        if ui
                            .add_enabled(self.geo_ready(), egui::Button::new(tr("Ortsname ermitteln", "Look up place name")).small())
                            .clicked()
                        {
                            self.start_geo_reverse();
                        }
                    });
                }
                Err(e) if !self.coord_input.trim().is_empty() => {
                    ui.colored_label(RED, e.as_str());
                }
                Err(_) => {}
            }
            labeled_field(ui, tr("Ort:", "Place:"), &mut self.city_input, "");
            labeled_field(ui, tr("Land:", "Country:"), &mut self.country_input, "");
            let label = trs(format!("Standort für {} übernehmen", self.target_label()), format!("Apply location to {}", self.target_label()));
            if ui.add_enabled(coords.is_ok() && can_apply, egui::Button::new(label)).clicked() {
                self.apply_location();
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(can_apply, egui::Button::new(tr("Vormerkung verwerfen", "Discard planned")).small()).clicked() {
                    self.edit_targets(|e| e.loc = None);
                }
                if ui.add_enabled(can_apply, egui::Button::new(tr("aus Datei löschen", "remove from file")).small()).clicked() {
                    self.edit_targets(|e| e.loc = Some(Change::Remove));
                }
            });
            ui.label(
                RichText::new(tr("Ortssuche: © OpenStreetMap-Mitwirkende (ODbL)", "Place search: © OpenStreetMap contributors (ODbL)"))
                    .weak()
                    .small(),
            );

            // ---------- description
            section(ui, tr("📝 Beschreibung (optional)", "📝 Description (optional)"));
            ui.add(
                egui::TextEdit::singleline(&mut self.desc_input)
                    .hint_text(tr("z. B. Sommerurlaub Borkum", "e.g. Summer holiday Borkum"))
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal(|ui| {
                if ui.add_enabled(can_apply, egui::Button::new(tr("Beschreibung übernehmen", "Apply description"))).clicked() {
                    self.apply_desc();
                }
                if ui.add_enabled(can_apply, egui::Button::new(tr("Verwerfen", "Discard")).small()).clicked() {
                    self.edit_targets(|e| e.desc = None);
                }
            });

            // ---------- rotate & rate
            section(ui, tr("⟳ Drehen & Bewerten", "⟳ Rotate & rate"));
            ui.label(
                RichText::new(tr(
                    "Wirkt auf die Auswahl bzw. das groß angezeigte Bild. Tasten: R/L drehen, 1–5 Sterne, 0 keine, X Ausschuss.",
                    "Applies to the selection or the image in the large view. Keys: R/L rotate, 1–5 stars, 0 none, X reject.",
                ))
                .weak()
                .small(),
            );
            ui.horizontal_wrapped(|ui| {
                if ui.button(tr("⟲ links", "⟲ left")).clicked() {
                    self.rotate_images(false);
                }
                if ui.button(tr("⟳ rechts", "⟳ right")).clicked() {
                    self.rotate_images(true);
                }
                ui.separator();
                for r in 1..=5i8 {
                    if ui.small_button(format!("{r}★")).clicked() {
                        self.set_rating(r);
                    }
                }
                if ui.small_button("0").clicked() {
                    self.set_rating(0);
                }
                if ui.small_button(tr("✖ Ausschuss", "✖ Reject")).clicked() {
                    self.toggle_reject();
                }
            });

            // ---------- film info
            ui.add_space(10.0);
            ui.separator();
            egui::CollapsingHeader::new(RichText::new(tr("🎞 Film-Angaben", "🎞 Film info")).strong().size(15.0))
                .default_open(true)
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(tr(
                            "Werden gespeichert und gelten meist für alle Filme. Leere Felder bleiben unverändert.",
                            "Saved for later films. Empty fields are left unchanged.",
                        ))
                        .weak()
                        .small(),
                    );
                    labeled_field(ui, tr("Fotograf:", "Photographer:"), &mut self.settings.creator, "");
                    labeled_field(
                        ui,
                        tr("Rechte:", "Rights:"),
                        &mut self.settings.rights,
                        tr("z. B. © Familie Muster", "e.g. © Smith family"),
                    );
                    let current = SourceType::from_key(&self.settings.source_type);
                    egui::ComboBox::from_label(tr("Vorlage (IPTC)", "Original (IPTC)")).selected_text(current.label()).show_ui(ui, |ui| {
                        for s in SourceType::ALL {
                            if ui.selectable_label(current == s, s.label()).clicked() {
                                self.settings.source_type = s.key().to_string();
                            }
                        }
                    });
                    ui.checkbox(&mut self.settings.film_auto, tr("bei jedem geöffneten Film automatisch vormerken", "plan automatically for every opened film"));
                    let label = trs(format!("Film-Angaben für {} übernehmen", self.target_label()), format!("Apply film info to {}", self.target_label()));
                    if ui.add_enabled(can_apply, egui::Button::new(label)).clicked() {
                        self.apply_film_info();
                    }
                });

            // ---------- order
            section(ui, tr("↔ Reihenfolge", "↔ Order"));
            ui.label(RichText::new(tr("Bilder per Ziehen verschieben oder:", "Drag images to move them, or:")).weak());
            ui.horizontal(|ui| {
                if ui.add_enabled(n_sel > 0, egui::Button::new(tr("◀ Auswahl nach vorne", "◀ Selection forward"))).clicked() {
                    self.move_selection(-1);
                }
                if ui.add_enabled(n_sel > 0, egui::Button::new(tr("nach hinten ▶", "backward ▶"))).clicked() {
                    self.move_selection(1);
                }
            });
            ui.horizontal(|ui| {
                if ui.button(tr("Nach Dateiname", "By file name")).clicked() {
                    self.sort_by_name();
                }
                if ui.button(tr("Nach Datum", "By date")).clicked() {
                    self.sort_by_date();
                }
            });
            ui.checkbox(&mut self.settings.resequence_on_move, tr("Uhrzeiten beim Verschieben neu durchzählen", "Renumber times when moving"));

            // ---------- JPEG
            ui.add_space(10.0);
            ui.separator();
            egui::CollapsingHeader::new(RichText::new(tr("🖼 JPEG-Kopien", "🖼 JPEG copies")).strong().size(15.0))
                .default_open(true)
                .show(ui, |ui| self.jpeg_section(ui, rows));

            // ---------- clean up
            section(ui, tr("🧹 Aufräumen", "🧹 Clean up"));
            if self.backups.is_empty() {
                ui.label(RichText::new(tr("Keine Sicherungskopien (*_original) in diesem Film.", "No backup copies (*_original) in this film.")).weak());
            } else {
                let n = self.backups.len();
                let size = fmt_bytes(self.backup_bytes);
                ui.label(trs(format!("{n} Sicherungskopie(n), zusammen {size}"), format!("{n} backup copies, {size} in total")));
                let idle = matches!(self.busy, Busy::Idle) && rows.is_empty();
                if ui.add_enabled(idle, egui::Button::new(tr("Sicherungskopien löschen …", "Delete backup copies …"))).clicked() {
                    self.prepare_backup_cleanup();
                }
            }

            // ---------- settings
            ui.add_space(10.0);
            ui.separator();
            egui::CollapsingHeader::new(RichText::new(tr("⚙ Einstellungen", "⚙ Settings")).strong().size(15.0))
                .default_open(false)
                .show(ui, |ui| {
                    ui.label(tr("Schlagwort für ungefähre Daten:", "Keyword for approximate dates:"));
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.settings.estimate_keyword).desired_width(150.0));
                        egui::ComboBox::from_label(tr("Vorschläge", "Presets")).selected_text("…").show_ui(ui, |ui| {
                            for k in ESTIMATE_PRESETS {
                                if ui.selectable_label(self.settings.estimate_keyword == k, k).clicked() {
                                    self.settings.estimate_keyword = k.to_string();
                                }
                            }
                        });
                    });
                    ui.add_space(4.0);
                    ui.checkbox(&mut self.settings.keep_backup, tr("Sicherungskopie der Originale anlegen (*_original)", "Keep backup of originals (*_original)"));
                    ui.checkbox(&mut self.settings.set_file_date, tr("Datei-Änderungsdatum = Aufnahmedatum", "File modification date = capture date"));
                    if crate::settings::is_portable() {
                        ui.label(RichText::new(tr("Portabler Modus: Einstellungen liegen neben rollstamp.exe.", "Portable mode: settings are stored next to rollstamp.exe.")).weak().small());
                    }
                });

            // ---------- write
            section(ui, tr("💾 In Dateien schreiben", "💾 Write to files"));
            let n = rows.len();
            let enabled = n > 0 && self.exiftool.is_some() && matches!(self.busy, Busy::Idle);
            let button = egui::Button::new(
                RichText::new(trs(format!("Änderungen prüfen & schreiben ({n})"), format!("Review & write changes ({n})"))).size(15.0).strong(),
            )
            .min_size(vec2(ui.available_width(), 36.0));
            if ui.add_enabled(enabled, button).clicked() {
                self.dialog = Dialog::Confirm(rows.to_vec());
            }
            if n == 0 {
                ui.label(RichText::new(tr("Keine offenen Änderungen.", "No pending changes.")).weak());
            }
            ui.add_space(12.0);
        });
    }

    fn jpeg_section(&mut self, ui: &mut egui::Ui, rows: &[ChangeRow]) {
        match &self.magick {
            Some(m) => {
                ui.label(RichText::new(format!("ImageMagick {}", m.version)).weak().small());
            }
            None => {
                ui.colored_label(
                    RED,
                    tr(
                        "ImageMagick nicht gefunden – Ordner „imagemagick“ neben rollstamp.exe legen oder ImageMagick installieren.",
                        "ImageMagick not found – put the folder “imagemagick” next to rollstamp.exe or install ImageMagick.",
                    ),
                );
            }
        }
        ui.checkbox(
            &mut self.settings.jpeg_enabled,
            tr(
                "beim Schreiben JPEG-Kopien erzeugen/aktualisieren",
                "create/update JPEG copies when writing",
            ),
        );
        ui.add(egui::Slider::new(&mut self.settings.jpeg_quality, 80..=100).text(tr("Qualität", "Quality")));
        ui.label(
            RichText::new(tr(
                "92 = mit bloßem Auge nicht vom TIFF zu unterscheiden; volle Farbauflösung (4:4:4), Farbprofil bleibt erhalten.",
                "92 = visually indistinguishable from the TIFF; full chroma (4:4:4), colour profile is kept.",
            ))
            .weak()
            .small(),
        );
        let edges: [(u32, &str); 4] = [
            (0, tr("volle Auflösung", "full resolution")),
            (6000, "max. 6000 px"),
            (4000, "max. 4000 px"),
            (3000, "max. 3000 px"),
        ];
        let cur = edges
            .iter()
            .find(|e| e.0 == self.settings.jpeg_max_edge)
            .map(|e| e.1)
            .unwrap_or("?");
        egui::ComboBox::from_label(tr("Größe", "Size"))
            .selected_text(cur)
            .show_ui(ui, |ui| {
                for (v, l) in edges {
                    ui.selectable_value(&mut self.settings.jpeg_max_edge, v, l);
                }
            });
        ui.horizontal(|ui| {
            ui.radio_value(
                &mut self.settings.jpeg_parallel,
                false,
                tr("Unterordner:", "Subfolder:"),
            );
            ui.add_enabled(
                !self.settings.jpeg_parallel,
                egui::TextEdit::singleline(&mut self.settings.jpeg_subfolder).desired_width(90.0),
            );
        });
        ui.horizontal(|ui| {
            ui.radio_value(
                &mut self.settings.jpeg_parallel,
                true,
                tr("eigener Ordner:", "own folder:"),
            );
            if ui
                .add_enabled(
                    self.settings.jpeg_parallel,
                    egui::Button::new(tr("Wählen …", "Choose …")).small(),
                )
                .clicked()
            {
                if let Some(d) = rfd::FileDialog::new()
                    .set_title(tr("Basisordner für JPEG-Kopien", "Base folder for JPEG copies"))
                    .pick_folder()
                {
                    self.settings.jpeg_base = d.display().to_string();
                }
            }
        });
        if self.settings.jpeg_parallel {
            ui.add(
                egui::TextEdit::singleline(&mut self.settings.jpeg_base)
                    .hint_text("D:\\Photos-JPEG")
                    .desired_width(f32::INFINITY),
            );
        }
        if let (Some(folder), Some(t)) = (&self.folder, self.settings.jpeg_target()) {
            if let Some(first) = self.photos.iter().find(|p| is_jpeg_source(&p.path)) {
                if let Some(dst) = jpeg_path(&first.path, folder, &t) {
                    ui.label(
                        RichText::new(trs(
                            format!("z. B. {}", dst.display()),
                            format!("e.g. {}", dst.display()),
                        ))
                        .weak()
                        .small(),
                    );
                }
            }
        }
        ui.checkbox(
            &mut self.settings.jpeg_skip_rejects,
            tr("Ausschuss-Bilder auslassen", "skip rejected images"),
        );
        ui.checkbox(
            &mut self.settings.jpeg_overwrite,
            tr("vorhandene JPEGs neu berechnen", "recompute existing JPEGs"),
        )
        .on_hover_text(tr(
            "Aus: vorhandene JPEGs bekommen nur neue Metadaten (schnell). Gedrehte Bilder werden immer neu berechnet.",
            "Off: existing JPEGs only get new metadata (fast). Rotated images are always recomputed.",
        ));
        ui.add(egui::Slider::new(&mut self.settings.jpeg_threads, 1..=4).text(tr("gleichzeitig", "in parallel")));
        let pending = !rows.is_empty();
        let n_jpeg = self.photos.iter().filter(|p| is_jpeg_source(&p.path)).count();
        let can_export = self.magick.is_some()
            && self.exiftool.is_some()
            && self.settings.jpeg_target().is_some()
            && matches!(self.busy, Busy::Idle)
            && !pending
            && n_jpeg > 0;
        let label = trs(
            format!("JPEG-Kopien jetzt erzeugen/aktualisieren ({n_jpeg})"),
            format!("Create/update JPEG copies now ({n_jpeg})"),
        );
        if ui.add_enabled(can_export, egui::Button::new(label)).clicked() {
            self.start_job(Vec::new(), true);
        }
        if pending {
            ui.label(
                RichText::new(tr(
                    "Erst die offenen Änderungen schreiben – dabei entstehen die JPEGs automatisch.",
                    "Write the pending changes first – the JPEGs are created along the way.",
                ))
                .weak()
                .small(),
            );
        }
    }

    // ---------------------------------------------------------------- thumbnail grid

    fn grid_ui(&mut self, ui: &mut egui::Ui, planned: &[Option<DateTime>], pending: &HashSet<usize>) {
        ui.label(
            RichText::new(tr(
                "Klick: auswählen · Strg/Shift+Klick: mehrere · Rechtsklick: Menü für die Auswahl · Doppelklick oder 🔍: groß · Ziehen: verschieben · R/L drehen · 1–5 Sterne · X Ausschuss · Strg+Z rückgängig",
                "Click: select · Ctrl/Shift+click: multiple · right-click: menu for the selection · double-click or 🔍: large · drag: move · R/L rotate · 1–5 stars · X reject · Ctrl+Z undo",
            ))
            .weak()
            .small(),
        );
        ui.add_space(4.0);

        let modifiers = ui.input(|i| i.modifiers);
        let dragging_from = egui::DragAndDrop::payload::<DragPayload>(ui.ctx()).and_then(|p| self.pos_of(p.0));
        let mut clicked: Option<usize> = None;
        let mut open: Option<usize> = None;
        let mut dropped: Option<(usize, usize)> = None;
        let mut right_clicked: Option<usize> = None;
        let mut menu_action: Option<MenuAction> = None;

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
            ui.horizontal_wrapped(|ui| {
                for (pos, p) in self.photos.iter().enumerate() {
                    let (rect, resp) = ui.allocate_exact_size(vec2(TILE_W, TILE_H), Sense::click_and_drag());
                    let mag_rect = Rect::from_min_size(pos2(rect.right() - 40.0, rect.top() + 12.0), vec2(28.0, 28.0));
                    let mag = ui.interact(mag_rect, egui::Id::new(("lupe", p.id)), Sense::click());

                    if resp.drag_started() {
                        egui::DragAndDrop::set_payload(ui.ctx(), DragPayload(p.id));
                    }
                    if let Some(payload) = resp.dnd_release_payload::<DragPayload>() {
                        dropped = Some((payload.0, pos));
                    }
                    let indicator = if resp.dnd_hover_payload::<DragPayload>().is_some() {
                        dragging_from.filter(|&f| f != pos).map(|f| f < pos)
                    } else {
                        None
                    };

                    if mag.clicked() || resp.double_clicked() {
                        open = Some(p.id);
                    } else if resp.clicked() {
                        clicked = Some(pos);
                    }

                    if ui.is_rect_visible(rect) {
                        let view = TileView {
                            pos,
                            photo: p,
                            selected: self.selection.contains(&p.id),
                            hovered: resp.hovered() || mag.hovered(),
                            pending: pending.contains(&p.id),
                            when: effective_when(&p.existing, &p.edits, planned[pos]),
                            place: effective_place(&p.existing, &p.edits),
                            status: status(&p.existing, &p.edits),
                            turns: quarter_turns(effective_orientation(&p.existing, &p.edits)),
                            rating: effective_rating(&p.existing, &p.edits),
                            indicator,
                        };
                        paint_tile(ui, rect, mag_rect, &view);
                    }

                    // right-click: menu for all selected images (or only this one if it isn't selected)
                    if resp.secondary_clicked() {
                        right_clicked = Some(pos);
                    }
                    let count = if self.selection.contains(&p.id) {
                        self.selection.len()
                    } else {
                        1
                    };
                    let recent = &self.settings.recent_places;
                    let _ = resp.context_menu(|ui| context_menu_ui(ui, count, recent, &mut menu_action));
                }
            });
        });

        if let Some(pos) = right_clicked {
            let id = self.photos[pos].id;
            if !self.selection.contains(&id) {
                self.selection.clear();
                self.selection.insert(id);
                self.anchor = Some(id);
            }
        }
        if let Some(a) = menu_action {
            let ids = self.selected_ids();
            self.run_menu_action(a, ids);
        }

        if let Some((id, to)) = dropped {
            self.move_photo(id, to);
        }
        if let Some(pos) = clicked {
            self.click_tile(pos, modifiers);
        }
        if let Some(id) = open {
            self.open_viewer(id);
        }
    }

    // ---------------------------------------------------------------- large view

    fn viewer_ui(&mut self, ui: &mut egui::Ui, id: usize, planned: &[Option<DateTime>]) {
        let Some(pos) = self.pos_of(id) else {
            self.viewer = None;
            return;
        };
        let mut go: Option<usize> = None;
        let mut close = false;
        let mut toggle = false;
        let mut rotate: Option<bool> = None;
        let len = self.photos.len();
        let p = &self.photos[pos];
        let st = status(&p.existing, &p.edits);
        let when = effective_when(&p.existing, &p.edits, planned[pos]);
        let place = effective_place(&p.existing, &p.edits);
        let turns = quarter_turns(effective_orientation(&p.existing, &p.edits));
        let rating = effective_rating(&p.existing, &p.edits);

        ui.horizontal(|ui| {
            if ui.button(tr("✖ Übersicht (Esc)", "✖ Overview (Esc)")).clicked() {
                close = true;
            }
            if ui
                .add_enabled(pos > 0, egui::Button::new(tr("◀ Vorheriges", "◀ Previous")))
                .clicked()
            {
                go = Some(pos - 1);
            }
            if ui
                .add_enabled(pos + 1 < len, egui::Button::new(tr("Nächstes ▶", "Next ▶")))
                .clicked()
            {
                go = Some(pos + 1);
            }
            if ui.button("⟲").on_hover_text("L").clicked() {
                rotate = Some(false);
            }
            if ui.button("⟳").on_hover_text("R").clicked() {
                rotate = Some(true);
            }
            let mut sel = self.selection.contains(&id);
            if ui
                .checkbox(&mut sel, tr("ausgewählt (Leertaste)", "selected (space)"))
                .changed()
            {
                toggle = true;
            }
            ui.separator();
            ui.label(RichText::new(format!("{} / {} · {}", pos + 1, len, p.name)).strong());
        });
        ui.horizontal(|ui| {
            match when {
                Some(w) => {
                    ui.colored_label(status_color(st), format!("📅 {}", w.label()));
                }
                None => {
                    ui.colored_label(RED, tr("📅 kein Datum", "📅 no date"));
                }
            }
            if let Some(pl) = &place {
                ui.colored_label(GREEN, format!("📍 {pl}"));
            }
            if rating != 0 {
                ui.label(rating_label(rating));
            }
            ui.label(RichText::new(tr("· ←/→ blättern", "· ←/→ browse")).weak());
        });

        let avail = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(avail, Sense::click());
        let tex = match &self.full {
            Some((fid, t)) if *fid == id => Some(t),
            _ => p.thumb.as_ref(),
        };
        let painter = ui.painter_at(avail);
        if let Some(tex) = tex {
            let inner = avail.shrink(12.0);
            let r = paint_image(&painter, tex, inner, turns, Color32::WHITE);
            painter.rect_stroke(r.expand(4.0), 2.0, Stroke::new(6.0_f32, status_color(st)));
            // draw again on top so the frame doesn't cover the edge
            paint_image(&painter, tex, inner, turns, Color32::WHITE);
        }
        let full_ready = matches!(&self.full, Some((fid, _)) if *fid == id);
        match &self.full_error {
            Some((eid, e)) if *eid == id => {
                painter.text(
                    avail.center_bottom() - vec2(0.0, 20.0),
                    Align2::CENTER_BOTTOM,
                    trs(
                        format!("Bild konnte nicht geladen werden: {e}"),
                        format!("Could not load image: {e}"),
                    ),
                    FontId::proportional(14.0),
                    RED,
                );
            }
            _ if !full_ready => {
                painter.text(
                    avail.center_bottom() - vec2(0.0, 20.0),
                    Align2::CENTER_BOTTOM,
                    tr("volle Auflösung wird geladen …", "loading full resolution …"),
                    FontId::proportional(14.0),
                    Color32::from_white_alpha(200),
                );
            }
            _ => {}
        }
        if resp.double_clicked() {
            close = true;
        }
        let mut menu_action: Option<MenuAction> = None;
        let recent = self.settings.recent_places.clone();
        let _ = resp.context_menu(|ui| context_menu_ui(ui, 1, &recent, &mut menu_action));

        if toggle {
            self.toggle_selected(id);
        }
        if let Some(a) = menu_action {
            self.run_menu_action(a, vec![id]);
        }
        if let Some(cw) = rotate {
            self.rotate_images(cw);
        }
        if close {
            self.viewer = None;
        } else if let Some(np) = go {
            let nid = self.photos[np].id;
            self.open_viewer(nid);
        }
    }

    // ---------------------------------------------------------------- archive view

    fn archive_ui(&mut self, ui: &mut egui::Ui) {
        let mut open_dir: Option<PathBuf> = None;
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.archive.running,
                    egui::Button::new(tr("📁 Archiv-Ordner wählen …", "📁 Choose archive folder …")),
                )
                .clicked()
            {
                if let Some(d) = rfd::FileDialog::new()
                    .set_title(tr("Archiv-Ordner wählen", "Choose archive folder"))
                    .pick_folder()
                {
                    self.settings.archive_root = d.display().to_string();
                    self.start_scan(d);
                }
            }
            let root = PathBuf::from(&self.settings.archive_root);
            let can_rescan = !self.settings.archive_root.is_empty() && root.is_dir() && !self.archive.running;
            if ui
                .add_enabled(can_rescan, egui::Button::new(tr("⟳ Neu prüfen", "⟳ Rescan")))
                .clicked()
            {
                self.start_scan(root);
            }
            if self.archive.running && ui.button(tr("Abbrechen", "Cancel")).clicked() {
                self.archive.cancel.store(true, Ordering::SeqCst);
            }
            ui.checkbox(
                &mut self.archive.only_incomplete,
                tr("nur Ordner mit fehlendem Datum", "only folders with missing dates"),
            );
        });
        if !self.settings.archive_root.is_empty() {
            ui.label(RichText::new(&self.settings.archive_root).strong());
        }
        if self.archive.running {
            let frac = if self.archive.total == 0 {
                0.0
            } else {
                self.archive.done as f32 / self.archive.total as f32
            };
            ui.add(egui::ProgressBar::new(frac).text(trs(
                format!("Prüfe Ordner {} / {} …", self.archive.done, self.archive.total),
                format!("Scanning folder {} / {} …", self.archive.done, self.archive.total),
            )));
        }

        if self.archive.folders.is_empty() {
            if !self.archive.running {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.label(tr(
                        "Wähle den Hauptordner deines Archivs – alle Unterordner mit Bildern werden auf fehlende Metadaten geprüft.",
                        "Choose the main folder of your archive – all subfolders with images are checked for missing metadata.",
                    ));
                });
            }
            return;
        }

        let mut total = FolderStats::default();
        let (mut jpegs, mut sources, mut backups) = (0usize, 0usize, 0usize);
        for f in &self.archive.folders {
            total.images += f.stats.images;
            total.no_date += f.stats.no_date;
            total.dated += f.stats.dated;
            total.located += f.stats.located;
            total.estimated += f.stats.estimated;
            total.rejected += f.stats.rejected;
            jpegs += f.jpegs;
            sources += f.jpeg_sources;
            backups += f.backups;
        }
        let nf = self.archive.folders.len();
        ui.label(trs(
            format!(
                "{nf} Filmordner · {} Bilder · ohne Datum: {} · mit Ort: {} · JPEGs: {jpegs}/{sources} · Sicherungen: {backups}",
                total.images, total.no_date, total.located
            ),
            format!(
                "{nf} film folders · {} images · no date: {} · with place: {} · JPEGs: {jpegs}/{sources} · backups: {backups}",
                total.images, total.no_date, total.located
            ),
        ));
        ui.add_space(6.0);

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("archive-grid")
                .striped(true)
                .num_columns(9)
                .spacing([18.0, 6.0])
                .show(ui, |ui| {
                    let headers = [
                        tr("Ordner", "Folder"),
                        tr("Bilder", "Images"),
                        tr("ohne Datum", "no date"),
                        tr("mit Datum", "dated"),
                        tr("mit Ort", "with place"),
                        "ca.",
                        tr("Ausschuss", "Rejects"),
                        "JPEG",
                        tr("Sicherungen", "Backups"),
                    ];
                    for h in headers {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    for f in &self.archive.folders {
                        if self.archive.only_incomplete && f.stats.no_date == 0 && f.error.is_none() {
                            continue;
                        }
                        let complete = f.stats.no_date == 0 && f.error.is_none();
                        let name = if complete {
                            format!("✔ {}", f.rel)
                        } else {
                            f.rel.clone()
                        };
                        let color = if f.error.is_some() || f.stats.no_date > 0 {
                            RED
                        } else {
                            GREEN
                        };
                        let link = ui.add(egui::Label::new(RichText::new(name).color(color)).sense(Sense::click()));
                        let link = match &f.error {
                            Some(e) => link.on_hover_text(e.as_str()),
                            None => link.on_hover_text(tr("Klicken zum Öffnen", "Click to open")),
                        };
                        if link.clicked() {
                            open_dir = Some(f.path.clone());
                        }
                        ui.label(f.stats.images.to_string());
                        if f.stats.no_date > 0 {
                            ui.colored_label(RED, f.stats.no_date.to_string());
                        } else {
                            ui.label("0");
                        }
                        ui.label(f.stats.dated.to_string());
                        ui.label(f.stats.located.to_string());
                        ui.label(f.stats.estimated.to_string());
                        ui.label(f.stats.rejected.to_string());
                        ui.label(if f.jpeg_sources == 0 {
                            "–".to_string()
                        } else {
                            format!("{}/{}", f.jpegs, f.jpeg_sources)
                        });
                        ui.label(f.backups.to_string());
                        ui.end_row();
                    }
                });
        });

        if let Some(d) = open_dir {
            self.nav = Some(d);
        }
    }

    // ---------------------------------------------------------------- dialogs

    fn dialogs(&mut self, ctx: &egui::Context) {
        let dialog = std::mem::replace(&mut self.dialog, Dialog::None);
        self.dialog = match dialog {
            Dialog::None => Dialog::None,
            Dialog::Quick { ids, kind } => self.quick_dialog(ctx, ids, kind),
            Dialog::Confirm(rows) => self.confirm_dialog(ctx, rows),
            Dialog::Discard(after) => self.discard_dialog(ctx, after),
            Dialog::DeleteBackups { files, bytes, skipped } => self.backup_dialog(ctx, files, bytes, skipped),
            Dialog::Report { title, lines } => report_dialog(ctx, title, lines),
        };
    }

    /// Date / location / description for the images chosen via right-click.
    fn quick_dialog(&mut self, ctx: &egui::Context, ids: Vec<usize>, kind: QuickKind) -> Dialog {
        let n = ids.len();
        let title = match kind {
            QuickKind::Date => trs(format!("Datum für {n} Bild(er)"), format!("Date for {n} image(s)")),
            QuickKind::Location => trs(
                format!("Standort für {n} Bild(er)"),
                format!("Location for {n} image(s)"),
            ),
            QuickKind::Description => trs(
                format!("Beschreibung für {n} Bild(er)"),
                format!("Description for {n} image(s)"),
            ),
        };
        let mut apply = false;
        let mut cancel = false;

        egui::Window::new(title)
            .id(egui::Id::new("quick-dialog"))
            .collapsible(false)
            .resizable(false)
            .default_width(440.0)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(420.0);
                match kind {
                    QuickKind::Date => {
                        ui.label(tr("TT.MM.JJJJ, MM.JJJJ oder JJJJ:", "DD.MM.YYYY, MM.YYYY or YYYY:"));
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.date_input)
                                .hint_text(tr("z. B. 14.07.2001", "e.g. 14.07.2001"))
                                .desired_width(f32::INFINITY),
                        );
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            apply = true;
                        }
                        let parsed = parse_date_input(&self.date_input);
                        match &parsed {
                            Ok((d, p)) => {
                                ui.colored_label(GREEN, format!("✔ {} – {}", describe_date(*d, *p), p.label()));
                            }
                            Err(e) if !self.date_input.trim().is_empty() => {
                                ui.colored_label(RED, e.as_str());
                            }
                            Err(_) => {}
                        }
                        ui.horizontal(|ui| {
                            ui.label(tr("Startzeit:", "Start time:"));
                            ui.add(egui::TextEdit::singleline(&mut self.time_input).desired_width(56.0));
                            ui.label(RichText::new(tr("danach +1 Minute pro Bild", "then +1 minute per image")).weak());
                        });
                        if matches!(&parsed, Ok((_, p)) if *p != Precision::Day) {
                            let mut t = true;
                            ui.add_enabled(
                                false,
                                egui::Checkbox::new(
                                    &mut t,
                                    tr(
                                        "ungefähres Datum (bei Monat/Jahr immer)",
                                        "approximate date (always for month/year)",
                                    ),
                                ),
                            );
                        } else {
                            ui.checkbox(
                                &mut self.estimated,
                                tr(
                                    "ungefähres Datum (Tag nicht sicher)",
                                    "approximate date (day uncertain)",
                                ),
                            );
                        }
                        if let (Ok((d, p)), Ok(t)) = (&parsed, parse_time(&self.time_input)) {
                            let spec = DateSpec::new(*d, *p, t, self.estimated);
                            let first = spec.at_offset(0).time_str();
                            let last = spec.at_offset(n as u32 - 1).time_str();
                            ui.label(
                                RichText::new(trs(
                                    format!("Uhrzeiten: {first} bis {last}"),
                                    format!("Times: {first} to {last}"),
                                ))
                                .weak(),
                            );
                        }
                    }
                    QuickKind::Location => self.location_form(ui),
                    QuickKind::Description => {
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.desc_input)
                                .hint_text(tr("z. B. Sommerurlaub Borkum", "e.g. Summer holiday Borkum"))
                                .desired_width(f32::INFINITY),
                        );
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            apply = true;
                        }
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(RichText::new(tr("✔ Übernehmen", "✔ Apply")).strong()))
                        .clicked()
                    {
                        apply = true;
                    }
                    if ui.button(tr("Abbrechen", "Cancel")).clicked() {
                        cancel = true;
                    }
                });
                ui.label(
                    RichText::new(tr(
                        "Wird nur vorgemerkt – geschrieben wird mit „Änderungen prüfen & schreiben“.",
                        "Only planned – written with “Review & write changes”.",
                    ))
                    .weak()
                    .small(),
                );
            });

        if cancel || ctx.input(|i| i.key_pressed(Key::Escape)) {
            return Dialog::None;
        }
        if apply {
            let ok = match kind {
                QuickKind::Date => self.apply_date_to(&ids),
                QuickKind::Location => self.apply_location_to(&ids),
                QuickKind::Description => self.apply_desc_to(&ids),
            };
            if ok {
                return Dialog::None;
            }
        }
        Dialog::Quick { ids, kind }
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context, rows: Vec<ChangeRow>) -> Dialog {
        let total = self.photos.len();
        let n = rows.len();
        let nd = rows.iter().filter(|r| r.date.is_some()).count();
        let nl = rows.iter().filter(|r| r.loc.is_some()).count();
        let no = rows.iter().filter(|r| !r.other_changes().is_empty()).count();
        let n_jpeg = self.photos.iter().filter(|p| is_jpeg_source(&p.path)).count();
        let jpeg_possible = self.magick.is_some() && self.settings.jpeg_target().is_some() && n_jpeg > 0;
        let mut decision: Option<bool> = None;

        egui::Window::new(tr("Änderungen anwenden?", "Apply changes?"))
            .collapsible(false)
            .resizable(true)
            .default_width(1000.0)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let head = if n == total {
                    trs(
                        format!("Die Änderungen betreffen alle {total} Bilder dieses Films."),
                        format!("The changes affect all {total} images of this film."),
                    )
                } else {
                    trs(
                        format!(
                            "{n} von {total} Bildern werden geändert, {} bleiben unverändert.",
                            total - n
                        ),
                        format!("{n} of {total} images will be changed, {} stay unchanged.", total - n),
                    )
                };
                ui.label(RichText::new(head).strong());
                ui.label(trs(
                    format!("Datum: {nd} · Standort: {nl} · Weiteres: {no}"),
                    format!("Date: {nd} · Location: {nl} · Other: {no}"),
                ));
                ui.add_space(6.0);

                egui::ScrollArea::vertical()
                    .max_height(400.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        egui::Grid::new("changes")
                            .striped(true)
                            .num_columns(5)
                            .spacing([16.0, 6.0])
                            .show(ui, |ui| {
                                for h in [
                                    tr("Nr.", "No."),
                                    tr("Datei", "File"),
                                    tr("Datum", "Date"),
                                    tr("Standort", "Location"),
                                    tr("Weiteres", "Other"),
                                ] {
                                    ui.label(RichText::new(h).strong());
                                }
                                ui.end_row();
                                for r in &rows {
                                    ui.label(r.position.to_string());
                                    ui.label(r.name.as_str());
                                    change_cell(ui, &r.date);
                                    change_cell(ui, &r.loc);
                                    let other = r.other_changes();
                                    if other.is_empty() {
                                        change_cell(ui, &None);
                                    } else {
                                        ui.vertical(|ui| {
                                            for c in other {
                                                change_cell(ui, &Some(c));
                                            }
                                        });
                                    }
                                    ui.end_row();
                                }
                            });
                    });

                ui.add_space(8.0);
                ui.checkbox(
                    &mut self.settings.keep_backup,
                    tr(
                        "Sicherungskopie anlegen (Original bleibt als *_original erhalten)",
                        "Keep backup (original stays as *_original)",
                    ),
                );
                ui.checkbox(
                    &mut self.settings.set_file_date,
                    tr(
                        "Datei-Änderungsdatum auf das Aufnahmedatum setzen",
                        "Set file modification date to capture date",
                    ),
                );
                ui.add_enabled(
                    jpeg_possible,
                    egui::Checkbox::new(
                        &mut self.settings.jpeg_enabled,
                        trs(
                            format!("danach JPEG-Kopien aller {n_jpeg} Bilder erzeugen/aktualisieren"),
                            format!("then create/update JPEG copies of all {n_jpeg} images"),
                        ),
                    ),
                );
                if !self.settings.keep_backup {
                    ui.colored_label(
                        AMBER,
                        tr(
                            "Ohne Sicherungskopie werden die Dateien direkt überschrieben.",
                            "Without backup the files are overwritten directly.",
                        ),
                    );
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let label = if n == total {
                        trs(
                            format!("✔ Auf alle {n} Bilder anwenden"),
                            format!("✔ Apply to all {n} images"),
                        )
                    } else {
                        trs(
                            format!("✔ Auf diese {n} Bilder anwenden"),
                            format!("✔ Apply to these {n} images"),
                        )
                    };
                    if ui
                        .add(
                            egui::Button::new(RichText::new(label).strong().color(Color32::WHITE))
                                .fill(GREEN.gamma_multiply(0.75)),
                        )
                        .clicked()
                    {
                        decision = Some(true);
                    }
                    if ui.button(tr("Abbrechen", "Cancel")).clicked() {
                        decision = Some(false);
                    }
                });
            });

        match decision {
            Some(true) => {
                let with_jpeg = self.settings.jpeg_enabled && jpeg_possible;
                self.start_job(rows, with_jpeg);
                Dialog::None
            }
            Some(false) => Dialog::None,
            None => Dialog::Confirm(rows),
        }
    }

    fn discard_dialog(&mut self, ctx: &egui::Context, after: After) -> Dialog {
        let mut decision: Option<bool> = None;
        egui::Window::new(tr("Nicht geschriebene Änderungen", "Unwritten changes"))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(tr(
                    "Es gibt vorgemerkte Änderungen, die noch nicht in die Dateien geschrieben wurden.",
                    "There are planned changes that have not been written to the files yet.",
                ));
                ui.label(tr("Möchtest du sie verwerfen?", "Do you want to discard them?"));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(tr("Verwerfen", "Discard")).clicked() {
                        decision = Some(true);
                    }
                    if ui.button(tr("Zurück", "Back")).clicked() {
                        decision = Some(false);
                    }
                });
            });
        match decision {
            Some(true) => {
                match after {
                    After::OpenDialog => self.open_folder_dialog(),
                    After::Open(dir) => self.load_folder(dir),
                    After::Quit => {
                        self.allow_close = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                Dialog::None
            }
            Some(false) => Dialog::None,
            None => Dialog::Discard(after),
        }
    }

    fn backup_dialog(&mut self, ctx: &egui::Context, files: Vec<PathBuf>, bytes: u64, skipped: usize) -> Dialog {
        let mut decision: Option<bool> = None;
        egui::Window::new(tr("Sicherungskopien löschen?", "Delete backup copies?"))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let n = files.len();
                let size = fmt_bytes(bytes);
                ui.label(RichText::new(trs(
                    format!("{n} Sicherungskopie(n) mit zusammen {size} werden endgültig gelöscht."),
                    format!("{n} backup copies with {size} in total will be deleted permanently."),
                )).strong());
                ui.label(tr(
                    "Geprüft: Zu jeder Sicherung existiert die aktuelle Datei. Das Löschen kann nicht rückgängig gemacht werden.",
                    "Checked: the current file exists for every backup. Deleting cannot be undone.",
                ));
                if skipped > 0 {
                    ui.colored_label(AMBER, trs(
                        format!("{skipped} Sicherung(en) werden behalten, weil die zugehörige Datei fehlt oder leer ist."),
                        format!("{skipped} backup(s) are kept because the matching file is missing or empty."),
                    ));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(n > 0, egui::Button::new(RichText::new(tr("Endgültig löschen", "Delete permanently")).color(Color32::WHITE)).fill(RED.gamma_multiply(0.8)))
                        .clicked()
                    {
                        decision = Some(true);
                    }
                    if ui.button(tr("Abbrechen", "Cancel")).clicked() {
                        decision = Some(false);
                    }
                });
            });
        match decision {
            Some(true) => {
                let mut deleted = 0usize;
                let mut errors = Vec::new();
                for f in &files {
                    match std::fs::remove_file(f) {
                        Ok(()) => deleted += 1,
                        Err(e) => errors.push(format!("{}: {e}", file_name(f))),
                    }
                }
                self.refresh_backups();
                self.sync_archive_entry();
                if errors.is_empty() {
                    let size = fmt_bytes(bytes);
                    self.set_status(
                        trs(
                            format!("✔ {deleted} Sicherungskopie(n) gelöscht, {size} frei."),
                            format!("✔ {deleted} backup copies deleted, {size} freed."),
                        ),
                        GREEN,
                    );
                    Dialog::None
                } else {
                    Dialog::Report {
                        title: tr("Nicht alles gelöscht", "Not everything deleted").into(),
                        lines: errors,
                    }
                }
            }
            Some(false) => Dialog::None,
            None => Dialog::DeleteBackups { files, bytes, skipped },
        }
    }

    fn draw_drag_ghost(&self, ctx: &egui::Context) {
        let Some(payload) = egui::DragAndDrop::payload::<DragPayload>(ctx) else {
            return;
        };
        let (Some(pos), Some(ptr)) = (self.pos_of(payload.0), ctx.pointer_latest_pos()) else {
            return;
        };
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("drag-ghost")));
        let photo = &self.photos[pos];
        let area = Rect::from_min_size(ptr + vec2(14.0, 14.0), vec2(120.0, 90.0));
        if let Some(tex) = &photo.thumb {
            let turns = quarter_turns(effective_orientation(&photo.existing, &photo.edits));
            let r = paint_image(&painter, tex, area, turns, Color32::from_white_alpha(210));
            painter.rect_stroke(r, 2.0, Stroke::new(2.0_f32, AMBER));
        }
        painter.text(
            area.left_bottom() + vec2(0.0, 4.0),
            Align2::LEFT_TOP,
            &photo.name,
            FontId::proportional(12.0),
            Color32::WHITE,
        );
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        set_lang(self.settings.resolved_lang(self.system_lang));
        if self.status_is_tools {
            self.status = self.tool_status();
        }
        self.poll(ctx);

        let n_sel = self.selection.len();
        if n_sel == 0 {
            self.target = Target::All;
        } else if self.last_sel_count == 0 {
            self.target = Target::Selection;
        }
        self.last_sel_count = n_sel;

        let target = self.settings.jpeg_target();
        if self.jpeg_target_seen.as_ref() != Some(&target) {
            self.refresh_jpeg_flags();
        }

        let planned = self.planned();
        let rows = self.rows(&planned);
        let pending: HashSet<usize> = rows.iter().map(|r| r.id).collect();

        self.handle_close(ctx, rows.len());
        let dialog_open = !matches!(self.dialog, Dialog::None);
        if !dialog_open {
            self.handle_keys(ctx);
        }
        let locked = dialog_open || self.working();

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_enabled_ui(!locked, |ui| self.top_ui(ui));
            if let Busy::Working { label, done, total } = &self.busy {
                let frac = if *total == 0 { 1.0 } else { *done as f32 / *total as f32 };
                ui.add(egui::ProgressBar::new(frac).text(format!("{label}  ({done} / {total})")));
                ui.add_space(4.0);
            }
        });

        if self.view == View::Film {
            egui::SidePanel::left("side")
                .resizable(true)
                .default_width(380.0)
                .min_width(320.0)
                .max_width(620.0)
                .show(ctx, |ui| {
                    ui.add_enabled_ui(!locked && !self.photos.is_empty(), |ui| self.side_ui(ui, &rows));
                });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_enabled_ui(!locked, |ui| {
                if self.view == View::Archive {
                    self.archive_ui(ui);
                } else if self.photos.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() * 0.3);
                        ui.heading(tr("Kein Film geladen", "No film loaded"));
                        ui.label(tr(
                            "Öffne einen Ordner mit den Scans eines Films (TIFF, JPEG oder PNG) – oder prüfe dein ganzes Archiv in der Archiv-Übersicht.",
                            "Open a folder with the scans of one film (TIFF, JPEG or PNG) – or check your whole archive in the archive overview.",
                        ));
                        ui.add_space(10.0);
                        if ui.button(RichText::new(tr("📂 Filmordner öffnen …", "📂 Open film folder …")).size(16.0)).clicked() {
                            self.want_open = true;
                        }
                        let last = PathBuf::from(&self.settings.last_folder);
                        if self.folder.is_none() && !self.settings.last_folder.is_empty() && last.is_dir() {
                            ui.add_space(6.0);
                            let label = trs(format!("Zuletzt geöffnet: {}", file_name(&last)), format!("Last opened: {}", file_name(&last)));
                            if ui.button(label).clicked() {
                                self.nav = Some(last);
                            }
                        }
                    });
                } else if let Some(id) = self.viewer {
                    self.viewer_ui(ui, id, &planned);
                } else {
                    self.grid_ui(ui, &planned, &pending);
                }
            });
        });

        self.dialogs(ctx);
        self.draw_drag_ghost(ctx);

        if self.want_open {
            self.want_open = false;
            self.request_open();
        }
        if let Some(dir) = self.nav.take() {
            self.go_to(dir);
        }
        self.persist_settings();
    }
}

// ------------------------------------------------------------------ drawing helpers

struct TileView<'a> {
    pos: usize,
    photo: &'a Photo,
    selected: bool,
    hovered: bool,
    pending: bool,
    when: Option<When>,
    place: Option<String>,
    status: Status,
    turns: u32,
    rating: i8,
    /// Some(true) = insert marker right, Some(false) = left
    indicator: Option<bool>,
}

fn paint_tile(ui: &egui::Ui, rect: Rect, mag_rect: Rect, t: &TileView) {
    let visuals = ui.visuals();
    let painter = ui.painter_at(rect.expand(2.0));

    let bg = if t.selected {
        visuals.selection.bg_fill.gamma_multiply(0.55)
    } else if t.hovered {
        visuals.widgets.hovered.weak_bg_fill
    } else {
        visuals.faint_bg_color
    };
    painter.rect_filled(rect, 8.0, bg);
    if t.selected {
        painter.rect_stroke(rect, 8.0, Stroke::new(2.0_f32, visuals.selection.stroke.color));
    }

    let rejected = t.rating < 0;
    let color = if rejected {
        Color32::GRAY
    } else {
        status_color(t.status)
    };
    let img_area = Rect::from_min_size(rect.min + vec2(12.0, 12.0), vec2(TILE_W - 24.0, IMG_H - 24.0));
    let mut img_rect = img_area;
    match (&t.photo.thumb, &t.photo.thumb_error) {
        (Some(tex), _) => {
            let tint = if rejected {
                Color32::from_gray(90)
            } else {
                Color32::WHITE
            };
            img_rect = paint_image(&painter, tex, img_area, t.turns, tint);
            painter.rect_stroke(img_rect.expand(3.0), 2.0, Stroke::new(4.0_f32, color));
            paint_image(&painter, tex, img_area, t.turns, tint);
        }
        (None, Some(_)) => {
            painter.rect_stroke(img_area, 2.0, Stroke::new(4.0_f32, color));
            painter.text(
                img_area.center(),
                Align2::CENTER_CENTER,
                tr("⚠ nicht lesbar", "⚠ unreadable"),
                FontId::proportional(13.0),
                RED,
            );
        }
        (None, None) => {
            painter.rect_stroke(img_area, 2.0, Stroke::new(4.0_f32, color));
            painter.text(
                img_area.center(),
                Align2::CENTER_CENTER,
                tr("lädt …", "loading …"),
                FontId::proportional(13.0),
                visuals.weak_text_color(),
            );
        }
    }

    // position badge
    let badge = Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(34.0, 20.0));
    let badge_bg = if t.selected {
        visuals.selection.bg_fill
    } else {
        Color32::from_black_alpha(180)
    };
    painter.rect_filled(badge, 4.0, badge_bg);
    let label = if t.selected {
        format!("✔{}", t.pos + 1)
    } else {
        format!("{}", t.pos + 1)
    };
    painter.text(
        badge.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(12.0),
        Color32::WHITE,
    );

    // reject / stars
    if rejected {
        painter.text(
            img_rect.center(),
            Align2::CENTER_CENTER,
            "✖",
            FontId::proportional(34.0),
            RED,
        );
    } else if t.rating > 0 {
        let sb = Rect::from_min_size(
            img_rect.left_bottom() + vec2(4.0, -19.0),
            vec2(14.0 * t.rating as f32 + 8.0, 16.0),
        );
        painter.rect_filled(sb, 3.0, Color32::from_black_alpha(190));
        painter.text(
            sb.center(),
            Align2::CENTER_CENTER,
            rating_label(t.rating),
            FontId::proportional(12.0),
            AMBER,
        );
    }

    // JPEG copy exists
    if t.photo.jpeg {
        let jb = Rect::from_min_size(img_rect.right_bottom() - vec2(34.0, 18.0), vec2(30.0, 15.0));
        painter.rect_filled(jb, 3.0, Color32::from_black_alpha(190));
        painter.text(
            jb.center(),
            Align2::CENTER_CENTER,
            "JPG",
            FontId::proportional(10.0),
            Color32::WHITE,
        );
    }

    if t.hovered {
        painter.circle_filled(mag_rect.center(), 14.0, Color32::from_black_alpha(190));
        painter.text(
            mag_rect.center(),
            Align2::CENTER_CENTER,
            "🔍",
            FontId::proportional(14.0),
            Color32::WHITE,
        );
    }

    // texts
    let x = rect.left() + 12.0;
    let mut y = rect.top() + IMG_H + 2.0;
    let name_clip = Rect::from_min_max(pos2(x, y), pos2(rect.right() - 24.0, y + 18.0));
    painter.with_clip_rect(name_clip).text(
        pos2(x, y),
        Align2::LEFT_TOP,
        &t.photo.name,
        FontId::proportional(13.0),
        visuals.text_color(),
    );
    if t.pending {
        painter.circle_filled(pos2(rect.right() - 14.0, y + 8.0), 5.0, AMBER);
    }
    y += 18.0;
    let (date_text, date_color) = match t.when {
        Some(w) => (w.label(), visuals.text_color()),
        None => (tr("kein Datum", "no date").to_string(), RED),
    };
    painter.text(
        pos2(x, y),
        Align2::LEFT_TOP,
        date_text,
        FontId::proportional(12.5),
        date_color,
    );
    y += 17.0;
    if let Some(place) = &t.place {
        painter.text(
            pos2(x, y),
            Align2::LEFT_TOP,
            format!("📍 {place}"),
            FontId::proportional(12.5),
            GREEN,
        );
    }

    if let Some(right) = t.indicator {
        let lx = if right { rect.right() + 6.0 } else { rect.left() - 6.0 };
        ui.painter().line_segment(
            [pos2(lx, rect.top()), pos2(lx, rect.bottom())],
            Stroke::new(4.0_f32, AMBER),
        );
    }
}

fn menu_item(ui: &mut egui::Ui, text: &str, a: MenuAction, action: &mut Option<MenuAction>) {
    if ui.button(text).clicked() {
        *action = Some(a);
        ui.close_menu();
    }
}

/// Right-click menu for `count` images.
fn context_menu_ui(ui: &mut egui::Ui, count: usize, recent: &[PlacePreset], action: &mut Option<MenuAction>) {
    ui.label(RichText::new(trs(format!("{count} Bild(er)"), format!("{count} image(s)"))).strong());
    ui.separator();
    menu_item(
        ui,
        tr("📅 Datum setzen …", "📅 Set date …"),
        MenuAction::Edit(QuickKind::Date),
        action,
    );
    menu_item(
        ui,
        tr("📅 Ab hier +1 Tag", "📅 From here +1 day"),
        MenuAction::NextDay,
        action,
    );
    menu_item(
        ui,
        tr("📍 Standort setzen …", "📍 Set location …"),
        MenuAction::Edit(QuickKind::Location),
        action,
    );
    if !recent.is_empty() {
        ui.menu_button(tr("📍 Zuletzt verwendeter Ort", "📍 Recently used place"), |ui| {
            for pl in recent.iter().take(12) {
                menu_item(ui, &pl.label(), MenuAction::Place(pl.clone()), action);
            }
        });
    }
    menu_item(
        ui,
        tr("📝 Beschreibung …", "📝 Description …"),
        MenuAction::Edit(QuickKind::Description),
        action,
    );
    ui.separator();
    ui.horizontal(|ui| {
        menu_item(ui, tr("⟲ links", "⟲ left"), MenuAction::Rotate(false), action);
        menu_item(ui, tr("⟳ rechts", "⟳ right"), MenuAction::Rotate(true), action);
    });
    ui.menu_button(tr("★ Bewertung", "★ Rating"), |ui| {
        for r in (1..=5i8).rev() {
            menu_item(ui, &rating_label(r), MenuAction::Rating(r), action);
        }
        menu_item(ui, tr("keine", "none"), MenuAction::Rating(0), action);
    });
    menu_item(
        ui,
        tr("✖ Ausschuss an/aus", "✖ Toggle reject"),
        MenuAction::Reject,
        action,
    );
    ui.separator();
    ui.menu_button(tr("↶ Vormerkung verwerfen", "↶ Discard planned"), |ui| {
        menu_item(ui, tr("Datum", "Date"), MenuAction::DiscardDate, action);
        menu_item(ui, tr("Standort", "Location"), MenuAction::DiscardPlace, action);
        menu_item(ui, tr("alles", "everything"), MenuAction::DiscardAll, action);
    });
    ui.menu_button(tr("🗑 Aus Datei löschen", "🗑 Remove from file"), |ui| {
        menu_item(ui, tr("Datum", "Date"), MenuAction::RemoveDate, action);
        menu_item(ui, tr("Standort", "Location"), MenuAction::RemovePlace, action);
    });
    ui.separator();
    if count == 1 {
        menu_item(ui, tr("🔍 Groß anzeigen", "🔍 Large view"), MenuAction::Large, action);
    }
    menu_item(ui, tr("Alle auswählen", "Select all"), MenuAction::SelectAll, action);
    menu_item(
        ui,
        tr("Auswahl aufheben", "Clear selection"),
        MenuAction::ClearSelection,
        action,
    );
}

/// Label with fixed width + text field filling exactly the rest of the row.
fn labeled_field(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) {
    ui.horizontal(|ui| {
        ui.add_sized([96.0, 20.0], egui::Label::new(label));
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(hint)
                .desired_width(f32::INFINITY),
        );
    });
}

fn legend(ui: &mut egui::Ui, color: Color32, text: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
    ui.painter()
        .rect_stroke(rect.shrink(2.0), 3.0, Stroke::new(3.0_f32, color));
    ui.label(text);
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    ui.separator();
    ui.label(RichText::new(title).strong().size(15.0));
    ui.add_space(2.0);
}

fn change_cell(ui: &mut egui::Ui, c: &Option<(String, String)>) {
    match c {
        Some((old, new)) => {
            ui.horizontal(|ui| {
                ui.label(RichText::new(old.as_str()).weak());
                ui.label("→");
                ui.label(RichText::new(new.as_str()).color(GREEN).strong());
            });
        }
        None => {
            ui.label(RichText::new(tr("unverändert", "unchanged")).weak());
        }
    }
}

fn report_dialog(ctx: &egui::Context, title: String, lines: Vec<String>) -> Dialog {
    let mut close = false;
    egui::Window::new(title.as_str())
        .collapsible(false)
        .resizable(true)
        .default_width(700.0)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for l in &lines {
                    ui.label(l.as_str());
                }
            });
            ui.add_space(6.0);
            if ui.button("OK").clicked() {
                close = true;
            }
        });
    if close {
        Dialog::None
    } else {
        Dialog::Report { title, lines }
    }
}
