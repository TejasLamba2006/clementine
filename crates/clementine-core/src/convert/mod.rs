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
    /// -1..=1, added to every channel.
    pub brightness: f32,
    /// -1..=1, warm (R+, B-) when positive.
    pub temperature: f32,
    /// -1..=1, green shift when positive.
    pub tint: f32,
    /// Drop shadow blur/alpha for `frameImage` (0 = none).
    pub shadow: u32,
    /// Resize percentage 25..=100 during `compress`.
    pub scale_pct: u8,
    /// `redactImage` fills solid black instead of pixelating.
    pub redact_solid: bool,
    pub annotation_color: [u8; 4],
    pub collage_cols: u32,
    /// `audioChannels`: mono when true, else stereo.
    pub mono: bool,
    /// 0-based page/clip indices for `organizePDF`, `mergePDF`, and `joinVideos`.
    pub page_order: Vec<usize>,
    /// `trimAudio`: automatically strip leading and trailing silence while keeping internal pauses.
    pub trim_silence: bool,
    /// `audioChannels`: left channel volume multiplier `0.0..=1.0`.
    pub left_gain: f32,
    /// `audioChannels`: right channel volume multiplier `0.0..=1.0`.
    pub right_gain: f32,
    /// `normalizeAudio`: target integrated loudness in LUFS (`-24.0..=-8.0`).
    pub target_lufs: f32,
    /// `normalizeAudio`: loudness range in LU (`1.0..=20.0`).
    pub loudness_range: f32,
    /// `normalizeAudio`: true peak limit in dBTP (`-6.0..=0.0`).
    pub true_peak: f32,
    /// `audioToVideo`: `0 = 16:9 (1280x720)`, `1 = 9:16 (720x1280)`, `2 = 1:1 (720x720)`.
    pub visualizer_shape: u8,
    /// `audioToVideo`: `false = Waveform (cline)`, `true = Bars (p2p)`.
    pub visualizer_bars: bool,
    /// `splitVideo`: custom split timestamps in seconds.
    pub split_points: Vec<f32>,
    /// `videoSnapshots`: timestamps in seconds to export as PNG frames.
    pub snapshot_times: Vec<f32>,
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
            brightness: 0.0,
            temperature: 0.0,
            tint: 0.0,
            shadow: 24,
            scale_pct: 100,
            redact_solid: false,
            annotation_color: [255, 82, 0, 255],
            collage_cols: 2,
            mono: true,
            page_order: vec![],
            trim_silence: false,
            left_gain: 1.0,
            right_gain: 1.0,
            target_lufs: -16.0,
            loudness_range: 11.0,
            true_peak: -1.5,
            visualizer_shape: 0,
            visualizer_bars: false,
            split_points: vec![],
            snapshot_times: vec![],
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
    } else if matches!(src, "srt" | "vtt" | "csv" | "tsv" | "md" | "txt" | "xlsx" | "html") {
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
        "editImage" | "annotateImage" | "cropImage" | "frameImage" | "redactImage"
        | "createPDF" | "createCollage" => image_ops::tool(job, tool_id, params, out),
        "compress" | "removeMetadata" => {
            if image_ops::is_image_ext(ext) {
                image_ops::tool(job, tool_id, params, out)
            } else if ext == "pdf" {
                doc_ops::clean_pdf(&job[0], out)
            } else if media_ops::is_media_ext(ext) {
                media_ops::run_tool(job, tool_id, params, out)
            } else {
                Err(unsupported())
            }
        }
        "organizePDF" => doc_ops::organize_pdf(&job[0], &params.page_order, out),
        "splitPDF" => doc_ops::split_pdf(&job[0], out),
        "mergePDF" => {
            if params.page_order.is_empty() {
                doc_ops::merge_pdf(job, out)
            } else {
                let ordered: Vec<PathBuf> = params.page_order.iter().filter_map(|&idx| job.get(idx).cloned()).collect();
                doc_ops::merge_pdf(&ordered, out)
            }
        }
        "extractArchive" => archive_ops::extract(&job[0], ext, out),
        "muteVideo" | "trimVideo" | "cropVideo" | "redactVideo" | "trimAudio" | "changeVideoSpeed" | "normalizeAudio"
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

    fn txt_pdf(d: &Path, name: &str, lines: usize) -> PathBuf {
        let p = d.join(format!("{name}.txt"));
        let t: String = (0..lines).map(|i| format!("{name} line {i}\n")).collect();
        fs::write(&p, t).unwrap();
        execute_conversion(&p, "pdf").unwrap()
    }

    #[test]
    fn split_and_merge_pdf() {
        let d = tempfile::tempdir().unwrap();
        let a = txt_pdf(d.path(), "a", 120); // 3 pages
        let b = txt_pdf(d.path(), "b", 3); // 1 page
        let dir = execute_tool(&[a.clone()], "splitPDF", &ToolParams::default()).unwrap();
        for n in 1..=3 {
            let bytes = fs::read(dir.join(format!("Page {n}.pdf"))).unwrap();
            assert!(bytes.starts_with(b"%PDF-1.4") && bytes.ends_with(b"%%EOF\n"));
            assert!(String::from_utf8_lossy(&bytes).contains("/Count 1"));
        }
        assert!(!dir.join("Page 4.pdf").exists());
        assert!(String::from_utf8_lossy(&fs::read(dir.join("Page 1.pdf")).unwrap()).contains("a line 0"));
        let m = execute_tool(&[a, b], "mergePDF", &ToolParams::default()).unwrap();
        let s = String::from_utf8_lossy(&fs::read(m).unwrap()).into_owned();
        assert!(s.contains("/Count 4") && s.contains("b line 2") && s.contains("a line 119"));
        let png = sample_png(d.path(), "i.png", 8, 8);
        let ipdf = execute_conversion(&png, "pdf").unwrap();
        let idir = execute_tool(&[ipdf], "splitPDF", &ToolParams::default()).unwrap();
        let ib = fs::read(idir.join("Page 1.pdf")).unwrap();
        assert!(String::from_utf8_lossy(&ib).contains("DCTDecode"));
    }

    #[test]
    fn csv_xlsx_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let csv = "name,note\nann,\"a,b\"\nbob,x < y\n";
        let p = d.path().join("t.csv");
        fs::write(&p, csv).unwrap();
        let x = execute_conversion(&p, "xlsx").unwrap();
        let mut z = zip::ZipArchive::new(fs::File::open(&x).unwrap()).unwrap();
        for n in [
            "[Content_Types].xml",
            "_rels/.rels",
            "xl/workbook.xml",
            "xl/_rels/workbook.xml.rels",
            "xl/worksheets/sheet1.xml",
        ] {
            assert!(z.by_name(n).is_ok(), "{n}");
        }
        assert_eq!(fs::read_to_string(execute_conversion(&x, "csv").unwrap()).unwrap(), csv);
        let t = fs::read_to_string(execute_conversion(&x, "tsv").unwrap()).unwrap();
        assert!(t.starts_with("name\tnote\nann\ta,b\n"));
        let tp = d.path().join("u.tsv");
        fs::write(&tp, "a\tb\n1\t2\n").unwrap();
        let x2 = execute_conversion(&tp, "xlsx").unwrap();
        assert_eq!(fs::read_to_string(execute_conversion(&x2, "csv").unwrap()).unwrap(), "a,b\n1,2\n");
    }

    #[test]
    fn html_to_txt_md() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("h.html");
        fs::write(
            &p,
            "<html><body><h1>Title</h1>\n<p>Tom &amp; <b>Jerry</b> &lt;3 &quot;hi&quot;</p><ul><li>one</li><li>two<br>three</li></ul><h2>Sub</h2></body></html>",
        )
        .unwrap();
        let md = fs::read_to_string(execute_conversion(&p, "md").unwrap()).unwrap();
        assert_eq!(md, "# Title\n\nTom & Jerry <3 \"hi\"\n\n- one\n- two\nthree\n\n## Sub\n");
        let txt = fs::read_to_string(execute_conversion(&p, "txt").unwrap()).unwrap();
        assert!(txt.starts_with("Title\n\nTom & Jerry") && !txt.contains('#') && !txt.contains("<b>") && txt.contains("<3"));
    }

    #[test]
    fn default_params() {
        let p = ToolParams::default();
        assert_eq!((p.quality, p.padding, p.corner_radius), (75, 48, 24));
        assert_eq!(p.bg_color, [255, 255, 255, 255]);
        assert_eq!(p.speed_factor, 2.0);
        assert_eq!((p.shadow, p.scale_pct, p.collage_cols, p.mono), (24, 100, 2, true));
    }

    #[test]
    fn organize_and_clean_pdf() {
        let d = tempfile::tempdir().unwrap();
        let a = txt_pdf(d.path(), "a", 120); // 3 pages
        let text = |p: &Path| String::from_utf8_lossy(&fs::read(p).unwrap()).into_owned();
        let rev = execute_tool(&[a.clone()], "organizePDF", &ToolParams::default()).unwrap();
        let s = text(&rev);
        assert!(s.contains("/Count 3"));
        assert!(s.find("a line 100)").unwrap() < s.find("a line 0)").unwrap());
        let p = ToolParams { page_order: vec![1], ..Default::default() };
        let one = text(&execute_tool(&[a.clone()], "organizePDF", &p).unwrap());
        assert!(one.contains("/Count 1") && one.contains("a line 50)") && !one.contains("a line 0)"));
        let bad = ToolParams { page_order: vec![9], ..Default::default() };
        assert!(execute_tool(&[a.clone()], "organizePDF", &bad).is_err());
        for t in ["compress", "removeMetadata"] {
            let c = text(&execute_tool(&[a.clone()], t, &ToolParams::default()).unwrap());
            assert!(c.contains("/Count 3") && !c.contains("/Info") && !c.contains("/Author"));
        }
    }

    #[test]
    fn video_crop_redact_args() {
        let p = ToolParams { crop_rect: Some((10, 20, 300, 200)), ..Default::default() };
        let o = Path::new("o.mp4");
        let i = [PathBuf::from("v.mp4")];
        let c = media_ops::build_ffmpeg_args(&i, "cropVideo", &p, o).join(" ");
        assert!(c.ends_with("-vf crop=300:200:10:20 -c:a copy o.mp4"), "{c}");
        let r = media_ops::build_ffmpeg_args(&i, "redactVideo", &p, o).join(" ");
        assert!(r.ends_with("-vf drawbox=x=10:y=20:w=300:h=200:color=black@1:t=fill -c:a copy o.mp4"), "{r}");
    }

    #[test]
    fn edit_brightness_temperature_tint() {
        let d = tempfile::tempdir().unwrap();
        let src = sample_png(d.path(), "e.png", 20, 20);
        let base = image::open(&src).unwrap().to_rgba8();
        let at = |p: ToolParams| {
            let o = execute_tool(&[src.clone()], "editImage", &p).unwrap();
            image::open(o).unwrap().to_rgba8().get_pixel(10, 10).0
        };
        let b0 = base.get_pixel(10, 10).0;
        let br = at(ToolParams { brightness: 0.2, ..Default::default() });
        assert!(br[0] > b0[0] && br[1] > b0[1] && br[2] > b0[2]);
        let w = at(ToolParams { temperature: 0.5, ..Default::default() });
        assert!(w[0] > b0[0] && w[2] < b0[2]);
        let t = at(ToolParams { tint: 0.5, ..Default::default() });
        assert!(t[1] > b0[1]);
        assert_eq!(image_ops::apply_photo_edits(&base, &ToolParams::default()), base);
    }
}
