//! JPEG copies with ImageMagick (https://imagemagick.org). ImageMagick converts the pixels,
//! ExifTool then transfers all metadata incl. the colour profile from the TIFF.

use crate::exif::{command, ExifTool};
use crate::model::tr;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

#[cfg(windows)]
const EXE: &str = "magick.exe";
#[cfg(not(windows))]
const EXE: &str = "magick";

#[derive(Clone, Debug)]
pub struct Magick {
    pub path: PathBuf,
    pub version: String,
}

#[derive(Clone, Copy, Debug)]
pub struct JpegSettings {
    /// 1–100; 92 is visually indistinguishable from the original
    pub quality: u8,
    /// longest edge in pixels, 0 = full resolution
    pub max_edge: u32,
    /// recompute existing JPEGs (otherwise only update their metadata)
    pub overwrite: bool,
    pub set_file_date: bool,
}

#[derive(Clone, Debug)]
pub struct JpegJob {
    pub name: String,
    pub src: PathBuf,
    pub dst: PathBuf,
    /// clockwise rotation in degrees (0, 90, 180, 270)
    pub rotate: u32,
    /// recompute even if the JPEG exists (e.g. rotation changed)
    pub force: bool,
}

pub enum Outcome {
    Created,
    Updated,
}

fn probe(path: PathBuf) -> Option<Magick> {
    let out = command(&path).arg("-version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let first = text.lines().next()?.trim().to_string();
    if !first.contains("ImageMagick") {
        return None;
    }
    let version = first.trim_start_matches("Version:").trim().to_string();
    Some(Magick { path, version })
}

impl Magick {
    /// Looks in the subfolder "imagemagick" next to the exe (portable package), next to the exe,
    /// in PATH and under "Program Files\ImageMagick-7…".
    pub fn locate() -> Option<Self> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(dir) = crate::settings::exe_dir() {
            candidates.push(dir.join("imagemagick").join(EXE));
            candidates.push(dir.join(EXE));
        }
        candidates.push(PathBuf::from(EXE));
        #[cfg(windows)]
        {
            for var in ["ProgramFiles", "ProgramW6432"] {
                if let Some(pf) = std::env::var_os(var) {
                    if let Ok(rd) = std::fs::read_dir(pf) {
                        let mut dirs: Vec<PathBuf> = rd
                            .filter_map(|e| e.ok())
                            .map(|e| e.path())
                            .filter(|p| {
                                p.file_name()
                                    .map(|n| n.to_string_lossy().starts_with("ImageMagick-7"))
                                    .unwrap_or(false)
                            })
                            .collect();
                        dirs.sort();
                        dirs.reverse();
                        candidates.extend(dirs.into_iter().map(|d| d.join(EXE)));
                    }
                }
            }
        }
        candidates.into_iter().find_map(probe)
    }

    /// TIFF → JPEG (8 bit, full chroma 4:4:4). Metadata is stripped here and transferred
    /// completely by ExifTool afterwards.
    pub fn convert(&self, src: &Path, dst: &Path, quality: u8, max_edge: u32, rotate: u32) -> Result<(), String> {
        let mut input = src.as_os_str().to_os_string();
        input.push("[0]"); // main image only
        let mut cmd = command(&self.path);
        cmd.arg("-quiet").arg(&input).arg("-strip");
        if rotate % 360 != 0 {
            cmd.arg("-rotate").arg((rotate % 360).to_string());
        }
        cmd.arg("-depth").arg("8");
        if max_edge > 0 {
            cmd.arg("-resize").arg(format!("{max_edge}x{max_edge}>"));
        }
        cmd.arg("-sampling-factor")
            .arg("4:4:4")
            .arg("-quality")
            .arg(quality.clamp(1, 100).to_string())
            .arg(dst);
        let out = cmd.output().map_err(|e| {
            format!(
                "{}: {e}",
                tr(
                    "ImageMagick konnte nicht gestartet werden",
                    "Could not start ImageMagick"
                )
            )
        })?;
        if out.status.success() && dst.exists() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if err.is_empty() {
                tr("Unbekannter ImageMagick-Fehler.", "Unknown ImageMagick error.").into()
            } else {
                err
            })
        }
    }
}

/// Creates or updates the JPEG copy of one image. If the JPEG exists (and neither `overwrite`
/// nor `force` is set) only the metadata is transferred – that is fast.
pub fn export_one(magick: &Magick, tool: &ExifTool, job: &JpegJob, s: &JpegSettings) -> Result<Outcome, String> {
    if let Some(dir) = job.dst.parent() {
        std::fs::create_dir_all(dir).map_err(|e| {
            format!(
                "{} {}: {e}",
                tr("Ordner nicht anlegbar", "Cannot create folder"),
                dir.display()
            )
        })?;
    }
    let meta_err = |e: String| {
        format!(
            "{}: {e}",
            tr(
                "Metadaten konnten nicht übertragen werden",
                "Could not transfer metadata"
            )
        )
    };
    if s.overwrite || job.force || !job.dst.exists() {
        let tmp = job.dst.with_extension("tmp.jpg");
        let _ = std::fs::remove_file(&tmp);
        if let Err(e) = magick.convert(&job.src, &tmp, s.quality, s.max_edge, job.rotate) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        if let Err(e) = tool.copy_metadata(&job.src, &tmp, s.set_file_date, true) {
            let _ = std::fs::remove_file(&tmp);
            return Err(meta_err(e));
        }
        std::fs::rename(&tmp, &job.dst)
            .map_err(|e| format!("{}: {e}", tr("Umbenennen fehlgeschlagen", "Rename failed")))?;
        Ok(Outcome::Created)
    } else {
        tool.copy_metadata(&job.src, &job.dst, s.set_file_date, true)
            .map_err(meta_err)?;
        Ok(Outcome::Updated)
    }
}

/// Processes several JPEG jobs in parallel. `on_done` is called after each image (any thread).
/// Returns (created, updated, errors).
pub fn export_many(
    magick: &Magick,
    tool: &ExifTool,
    jobs: Vec<JpegJob>,
    s: &JpegSettings,
    threads: usize,
    on_done: &(dyn Fn(&str) + Sync),
) -> (usize, usize, Vec<String>) {
    let threads = threads.clamp(1, 8).min(jobs.len().max(1));
    let queue = Mutex::new(VecDeque::from(jobs));
    let created = AtomicUsize::new(0);
    let updated = AtomicUsize::new(0);
    let errors: Mutex<Vec<String>> = Mutex::new(Vec::new());

    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let next = match queue.lock() {
                    Ok(mut q) => q.pop_front(),
                    Err(_) => None,
                };
                let Some(job) = next else { break };
                match export_one(magick, tool, &job, s) {
                    Ok(Outcome::Created) => {
                        created.fetch_add(1, Ordering::SeqCst);
                    }
                    Ok(Outcome::Updated) => {
                        updated.fetch_add(1, Ordering::SeqCst);
                    }
                    Err(e) => {
                        if let Ok(mut v) = errors.lock() {
                            v.push(format!("JPEG {}: {e}", job.name));
                        }
                    }
                }
                on_done(&job.name);
            });
        }
    });

    (
        created.into_inner(),
        updated.into_inner(),
        errors.into_inner().unwrap_or_default(),
    )
}
