//! Local zero-server conversion and tool engine.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::matrix::{next_conversion_filename, next_tool_filename};

pub mod archive_ops;
pub mod doc_ops;
pub mod image_ops;
pub mod media_ops;

#[derive(Debug, thiserror::Error)]
pub enum ClementineError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported conversion from {from} to {to}")]
    UnsupportedConversion { from: String, to: String },
    #[error("unsupported tool: {0}")]
    UnsupportedTool(String),
    #[error("image error: {0}")]
    ImageError(String),
    #[error("archive error: {0}")]
    ArchiveError(String),
    #[error("media error: {0}")]
    MediaError(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

pub type Result<T> = std::result::Result<T, ClementineError>;

/// Tool parameters. `crop_rect` is `(x, y, width, height)` in pixels.
/// `exposure` is in stops (0 = unchanged); `contrast` and `saturation` are
/// multipliers (1 = unchanged).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolParams {
    pub quality: u8,
    pub crop_rect: Option<(u32, u32, u32, u32)>,
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub bg_color: [u8; 4],
    pub padding: u32,
    pub corner_radius: u32,
    pub speed_factor: f32,
    pub start_sec: f32,
    pub end_sec: Option<f32>,
}

impl Default for ToolParams {
    fn default() -> Self {
        ToolParams {
            quality: 75,
            crop_rect: None,
            exposure: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            bg_color: [255, 255, 255, 255],
            padding: 48,
            corner_radius: 24,
            speed_factor: 2.0,
            start_sec: 0.0,
            end_sec: None,
        }
    }
}

pub fn existing_sibling_names(dir: &Path) -> HashSet<String> {
    fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn parent_dir(p: &Path) -> PathBuf {
    match p.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Lowercase extension; `.tar.gz` and `.tgz` both report `gz`.
pub(crate) fn source_ext(p: &Path) -> String {
    let n = file_name(p).to_ascii_lowercase();
    if n.ends_with(".tar.gz") || n.ends_with(".tgz") {
        return "gz".into();
    }
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

enum Kind {
    Archive,
    Doc,
    Image,
    Media,
}

fn route(src: &str, target: &str) -> Option<Kind> {
    if matches!(src, "zip" | "tar" | "gz") {
        Some(Kind::Archive)
    } else if matches!(src, "srt" | "vtt" | "csv" | "tsv" | "md" | "txt") {
        Some(Kind::Doc)
    } else if src == "gif" && media_ops::is_video_ext(target) {
        Some(Kind::Media)
    } else if image_ops::is_image_ext(src) {
        Some(Kind::Image)
    } else if media_ops::is_media_ext(src) {
        Some(Kind::Media)
    } else {
        None
    }
}

fn cleanup(out: &Path) {
    if out.is_dir() {
        let _ = fs::remove_dir_all(out);
    } else {
        let _ = fs::remove_file(out);
    }
}

/// Converts `source_path` next to itself into `target_ext`; never overwrites.
pub fn execute_conversion(source_path: &Path, target_ext: &str) -> Result<PathBuf> {
    let target = target_ext.trim_start_matches('.').to_ascii_lowercase();
    let src = source_ext(source_path);
    let dir = parent_dir(source_path);
    let name = next_conversion_filename(
        &file_name(source_path),
        &target,
        &existing_sibling_names(&dir),
    );
    let out = dir.join(name);
    let res = match route(&src, &target) {
        Some(Kind::Archive) => archive_ops::convert(source_path, &src, &target, &out),
        Some(Kind::Doc) => doc_ops::convert(source_path, &src, &target, &out),
        Some(Kind::Image) => image_ops::convert(source_path, &src, &target, &out),
        Some(Kind::Media) => media_ops::convert(source_path, &target, &out),
        None => Err(ClementineError::UnsupportedConversion { from: src, to: target }),
    };
    match res {
        Ok(()) => Ok(out),
        Err(e) => {
            cleanup(&out);
            Err(e)
        }
    }
}

/// Runs `tool_id`. Per-file tools write one sibling output per source and the
/// first output path is returned; combining tools (`createPDF`,
/// `createCollage`, `joinVideos`, `mergePDF`) write a single output.
pub fn execute_tool(
    source_paths: &[PathBuf],
    tool_id: &str,
    params: &ToolParams,
) -> Result<PathBuf> {
    if source_paths.is_empty() {
        return Err(ClementineError::InvalidInput("no source files".into()));
    }
    let combine = matches!(tool_id, "createPDF" | "createCollage" | "joinVideos" | "mergePDF");
    let jobs: Vec<&[PathBuf]> =
        if combine { vec![source_paths] } else { source_paths.chunks(1).collect() };
    let mut first = None;
    for job in jobs {
        let p = &job[0];
        let ext = source_ext(p);
        let dir = parent_dir(p);
        let (name, fmt) =
            next_tool_filename(&file_name(p), &ext, tool_id, &existing_sibling_names(&dir));
        let out = dir.join(name);
        if fmt == "folder" {
            fs::create_dir_all(&out)?;
        }
        if let Err(e) = dispatch(job, &ext, tool_id, params, &out) {
            cleanup(&out);
            return Err(e);
        }
        first.get_or_insert(out);
    }
    Ok(first.expect("jobs is never empty"))
}

fn dispatch(
    job: &[PathBuf],
    ext: &str,
    tool_id: &str,
    params: &ToolParams,
    out: &Path,
) -> Result<()> {
    let unsupported = || ClementineError::UnsupportedTool(tool_id.to_string());
    match tool_id {
        "editImage" | "cropImage" | "frameImage" | "redactImage" | "createPDF"
        | "createCollage" => image_ops::tool(job, tool_id, params, out),
        "compress" | "removeMetadata" => {
            if image_ops::is_image_ext(ext) {
                image_ops::tool(job, tool_id, params, out)
            } else if media_ops::is_media_ext(ext) {
                media_ops::run_tool(job, tool_id, params, out)
            } else {
                Err(unsupported())
            }
        }
        "extractArchive" => archive_ops::extract(&job[0], ext, out),
        "muteVideo" | "trimVideo" | "trimAudio" | "changeVideoSpeed" | "normalizeAudio"
        | "audioChannels" | "audioToVideo" | "redactAudio" | "splitVideo" | "joinVideos"
        | "videoSnapshots" => media_ops::run_tool(job, tool_id, params, out),
        _ => Err(unsupported()),
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use std::path::{Path, PathBuf};

    pub fn sample_png(dir: &Path, name: &str, w: u32, h: u32) -> PathBuf {
        let img = image::RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([(x * 255 / w) as u8, (y * 255 / h) as u8, 128, 255])
        });
        let p = dir.join(name);
        img.save(&p).unwrap();
        p
    }

    pub fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = super::existing_sibling_names(dir).into_iter().collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::*;
    use super::*;

    #[test]
    fn collision_safe_siblings() {
        let d = tempfile::tempdir().unwrap();
        let src = sample_png(d.path(), "photo.png", 16, 16);
        let a = execute_conversion(&src, "jpg").unwrap();
        let b = execute_conversion(&src, "jpg").unwrap();
        assert_eq!(file_name(&a), "photo.jpg");
        assert_eq!(file_name(&b), "photo 2.jpg");
        assert!(src.exists());
    }

    #[test]
    fn unsupported_cleans_up() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("a.xyz");
        fs::write(&src, b"x").unwrap();
        assert!(matches!(
            execute_conversion(&src, "png"),
            Err(ClementineError::UnsupportedConversion { .. })
        ));
        assert!(matches!(
            execute_tool(&[src], "nope", &ToolParams::default()),
            Err(ClementineError::UnsupportedTool(_))
        ));
        assert_eq!(names(d.path()), vec!["a.xyz"]);
    }

    #[test]
    fn default_params() {
        let p = ToolParams::default();
        assert_eq!((p.quality, p.padding, p.corner_radius), (75, 48, 24));
        assert_eq!(p.bg_color, [255, 255, 255, 255]);
        assert_eq!(p.speed_factor, 2.0);
    }
}
