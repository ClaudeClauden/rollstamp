//! ExifTool (https://exiftool.org) for reading and writing metadata.

use crate::model::{is_estimate_keyword, tr, DateTime, Existing, FilmInfo, Precision, SourceType};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(windows)]
const EXE: &str = "exiftool.exe";
#[cfg(not(windows))]
const EXE: &str = "exiftool";

static ARGFILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct ExifTool {
    pub path: PathBuf,
    pub version: String,
}

/// Starts a program without a flashing console window (Windows).
pub(crate) fn command(path: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

impl ExifTool {
    /// Looks next to the exe, in a subfolder "exiftool" and in PATH.
    pub fn locate() -> Option<Self> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(dir) = crate::settings::exe_dir() {
            candidates.push(dir.join("exiftool").join(EXE));
            candidates.push(dir.join(EXE));
        }
        candidates.push(PathBuf::from(EXE));

        candidates.into_iter().find_map(|path| {
            let out = command(&path).arg("-ver").output().ok()?;
            if !out.status.success() {
                return None;
            }
            let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if version.is_empty() {
                None
            } else {
                Some(Self { path, version })
            }
        })
    }

    /// Passes arguments through a UTF-8 argument file – reliable with umlauts and spaces on Windows.
    fn run(&self, args: &[String]) -> Result<Output, String> {
        let n = ARGFILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let file = std::env::temp_dir().join(format!("rollstamp-{}-{n}.args", std::process::id()));
        let mut content = String::new();
        for a in args {
            content.push_str(a);
            content.push('\n');
        }
        std::fs::write(&file, content).map_err(|e| {
            format!(
                "{}: {e}",
                tr("Temporäre Datei nicht schreibbar", "Cannot write temporary file")
            )
        })?;
        let out = command(&self.path)
            .arg("-charset")
            .arg("filename=utf8")
            .arg("-@")
            .arg(&file)
            .output();
        let _ = std::fs::remove_file(&file);
        out.map_err(|e| {
            format!(
                "{}: {e}",
                tr("ExifTool konnte nicht gestartet werden", "Could not start ExifTool")
            )
        })
    }

    /// Reads the relevant metadata of all files (same order as `files`).
    pub fn read_metadata(&self, files: &[PathBuf], estimate_keyword: &str) -> Result<Vec<Existing>, String> {
        if files.is_empty() {
            return Ok(Vec::new());
        }
        let mut args: Vec<String> = [
            "-json",
            "-n",
            "-EXIF:DateTimeOriginal",
            "-XMP-photoshop:DateCreated",
            "-Composite:GPSLatitude",
            "-Composite:GPSLongitude",
            "-XMP-dc:Subject",
            "-XMP-photoshop:City",
            "-XMP-photoshop:Country",
            "-XMP-dc:Description",
            "-EXIF:ImageDescription",
            "-EXIF:Artist",
            "-XMP-dc:Creator",
            "-EXIF:Copyright",
            "-XMP-dc:Rights",
            "-XMP-iptcExt:DigitalSourceType",
            "-IFD0:Orientation",
            "-XMP-xmp:Rating",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        args.extend(files.iter().map(|f| f.to_string_lossy().into_owned()));

        let out = self.run(&args)?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        if stdout.trim().is_empty() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(if err.is_empty() {
                tr("Keine Ausgabe von ExifTool.", "No output from ExifTool.").into()
            } else {
                err
            });
        }
        let json: Value = serde_json::from_str(&stdout).map_err(|e| {
            format!(
                "{}: {e}",
                tr("Ausgabe von ExifTool unlesbar", "Cannot parse ExifTool output")
            )
        })?;
        let items = json
            .as_array()
            .ok_or(tr("Unerwartete Ausgabe von ExifTool.", "Unexpected ExifTool output."))?;

        let by_path: HashMap<String, &Value> = items
            .iter()
            .filter_map(|it| Some((norm(it.get("SourceFile")?.as_str()?), it)))
            .collect();

        Ok(files
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let item = by_path.get(&norm(&f.to_string_lossy())).copied().or_else(|| {
                    if items.len() == files.len() {
                        items.get(i)
                    } else {
                        None
                    }
                });
                item.map(|v| parse_item(v, estimate_keyword)).unwrap_or_default()
            })
            .collect())
    }

    /// Writes the arguments into a file. Ok(warnings) on success.
    pub fn write_file(&self, path: &Path, args: &[String]) -> Result<String, String> {
        let mut all = args.to_vec();
        all.push(path.to_string_lossy().into_owned());
        let out = self.run(&all)?;
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let has_error = stderr.lines().any(|l| l.trim_start().starts_with("Error"));
        if !out.status.success() || has_error {
            Err(if stderr.is_empty() { stdout } else { stderr })
        } else {
            Ok(stderr)
        }
    }

    /// Replaces all metadata of `dst` with those of `src` (incl. colour profile).
    /// `reset_orientation`: the JPEG pixels are already rotated, so the flag must be "normal".
    pub fn copy_metadata(
        &self,
        src: &Path,
        dst: &Path,
        set_file_date: bool,
        reset_orientation: bool,
    ) -> Result<String, String> {
        let mut args: Vec<String> = vec![
            "-m".into(),
            "-all=".into(),
            "-tagsFromFile".into(),
            src.to_string_lossy().into_owned(),
            "-all:all".into(),
            "-ICC_Profile".into(),
        ];
        if set_file_date {
            args.push("-FileModifyDate<DateTimeOriginal".into());
        }
        if reset_orientation {
            args.push("-Orientation#=1".into());
        }
        args.push("-overwrite_original".into());
        self.write_file(dst, &args)
    }
}

/// Makes paths comparable (ExifTool reports forward slashes on Windows).
fn norm(p: &str) -> String {
    let s = p.replace('\\', "/");
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

fn val_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Array(a) => a.iter().map(val_str).collect::<Vec<_>>().join(", "),
        _ => String::new(),
    }
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn has_estimate_keyword(v: Option<&Value>, custom: &str) -> bool {
    match v {
        Some(Value::Array(a)) => a.iter().any(|x| is_estimate_keyword(&val_str(x), custom)),
        Some(x) => val_str(x).split(',').any(|p| is_estimate_keyword(p, custom)),
        None => false,
    }
}

fn parse_item(v: &Value, estimate_keyword: &str) -> Existing {
    let s = |k: &str| v.get(k).map(val_str).unwrap_or_default().trim().to_string();
    let first = |a: &str, b: &str| {
        let x = s(a);
        if x.is_empty() {
            s(b)
        } else {
            x
        }
    };
    let xmp_date = s("DateCreated");
    let datetime = DateTime::parse_exif(&s("DateTimeOriginal")).or_else(|| DateTime::parse_exif(&xmp_date));
    let gps = match (v.get("GPSLatitude").and_then(num), v.get("GPSLongitude").and_then(num)) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    };
    let orientation = v
        .get("Orientation")
        .and_then(num)
        .map(|o| o as i64)
        .filter(|o| (1..=8).contains(o))
        .unwrap_or(1) as u8;
    let rating = v
        .get("Rating")
        .and_then(num)
        .map(|r| r.round() as i64)
        .filter(|r| (-1..=5).contains(r))
        .unwrap_or(0) as i8;
    Existing {
        datetime,
        precision: Precision::from_xmp(&xmp_date),
        estimated: has_estimate_keyword(v.get("Subject"), estimate_keyword),
        gps,
        city: s("City"),
        country: s("Country"),
        description: first("Description", "ImageDescription"),
        film: FilmInfo {
            creator: first("Creator", "Artist"),
            rights: first("Rights", "Copyright"),
            source: SourceType::from_uri(&s("DigitalSourceType")),
        },
        orientation,
        rating,
    }
}
