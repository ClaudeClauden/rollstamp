//! Loads thumbnails in the background (several threads) and large views (own thread,
//! only the newest request counts). Thumbnails are cached as PNG so a film opens instantly
//! the second time – decoding 16-bit TIFFs takes a few seconds otherwise.

use eframe::egui::{self, ColorImage};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};

pub const THUMB_MAX: u32 = 360;
pub const FULL_MAX: u32 = 2600;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Thumb,
    Full,
}

pub struct Request {
    pub generation: u64,
    pub id: usize,
    pub path: PathBuf,
    pub kind: Kind,
}

pub struct Loaded {
    pub generation: u64,
    pub id: usize,
    pub kind: Kind,
    pub image: Result<ColorImage, String>,
}

pub struct Loader {
    thumb_tx: mpsc::Sender<Request>,
    full_tx: mpsc::Sender<Request>,
    current: Arc<AtomicU64>,
    pub results: mpsc::Receiver<Loaded>,
}

impl Loader {
    pub fn new(ctx: egui::Context) -> Self {
        let (res_tx, results) = mpsc::channel::<Loaded>();
        let current = Arc::new(AtomicU64::new(0));

        // Vorschau-Threads (große TIFFs brauchen viel RAM → höchstens 4 gleichzeitig)
        let (thumb_tx, thumb_rx) = mpsc::channel::<Request>();
        let thumb_rx = Arc::new(Mutex::new(thumb_rx));
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .clamp(1, 4);
        for _ in 0..workers {
            let rx = Arc::clone(&thumb_rx);
            let tx = res_tx.clone();
            let ctx = ctx.clone();
            let current = Arc::clone(&current);
            std::thread::spawn(move || loop {
                let req = {
                    let guard = match rx.lock() {
                        Ok(g) => g,
                        Err(_) => return,
                    };
                    match guard.recv() {
                        Ok(r) => r,
                        Err(_) => return,
                    }
                };
                // Anfragen aus einem zuvor geöffneten Ordner überspringen
                if req.generation != current.load(Ordering::SeqCst) {
                    continue;
                }
                let image = load_thumbnail(&req.path);
                if tx
                    .send(Loaded {
                        generation: req.generation,
                        id: req.id,
                        kind: Kind::Thumb,
                        image,
                    })
                    .is_err()
                {
                    return;
                }
                ctx.request_repaint();
            });
        }

        // Großansicht
        let (full_tx, full_rx) = mpsc::channel::<Request>();
        {
            let tx = res_tx;
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                while let Ok(mut req) = full_rx.recv() {
                    while let Ok(newer) = full_rx.try_recv() {
                        req = newer;
                    }
                    let image = decode(&req.path, FULL_MAX).map(|img| to_color_image(&img));
                    if tx
                        .send(Loaded {
                            generation: req.generation,
                            id: req.id,
                            kind: Kind::Full,
                            image,
                        })
                        .is_err()
                    {
                        return;
                    }
                    ctx.request_repaint();
                }
            });
        }

        Self {
            thumb_tx,
            full_tx,
            current,
            results,
        }
    }

    pub fn set_generation(&self, generation: u64) {
        self.current.store(generation, Ordering::SeqCst);
    }

    pub fn request(&self, req: Request) {
        let kind = req.kind;
        let _ = match kind {
            Kind::Thumb => self.thumb_tx.send(req),
            Kind::Full => self.full_tx.send(req),
        };
    }
}

fn decode(path: &Path, max: u32) -> Result<image::DynamicImage, String> {
    let mut reader = image::ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    // Hochauflösende 16-Bit-Scans überschreiten sonst das Standard-Speicherlimit
    reader.no_limits();
    let img = reader.decode().map_err(|e| e.to_string())?;
    Ok(if img.width() > max || img.height() > max {
        img.thumbnail(max, max)
    } else {
        img
    })
}

fn to_color_image(img: &image::DynamicImage) -> ColorImage {
    let rgba = img.to_rgba8();
    ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw())
}

fn load_thumbnail(path: &Path) -> Result<ColorImage, String> {
    let cache = cache_path(path);
    if let Some(c) = &cache {
        if let Ok(img) = image::open(c) {
            return Ok(to_color_image(&img));
        }
    }
    let img = decode(path, THUMB_MAX)?;
    if let Some(c) = &cache {
        let _ = img.to_rgba8().save_with_format(c, image::ImageFormat::Png);
    }
    Ok(to_color_image(&img))
}

/// Cache-Schlüssel aus Pfad, Größe und Änderungszeit → nach jeder Änderung neu.
fn cache_path(path: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(path).ok()?;
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    meta.len().hash(&mut h);
    if let Ok(m) = meta.modified() {
        m.hash(&mut h);
    }
    THUMB_MAX.hash(&mut h);
    Some(crate::settings::cache_dir()?.join(format!("{:016x}.png", h.finish())))
}
