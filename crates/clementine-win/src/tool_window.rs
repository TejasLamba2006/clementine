//! Phase 2 interactive warm-glass Tool Windows (`460x540` layered Win32 modal).
use crate::renderer::{blit_mask_sized, fit_mask, rounded_box_sdf, seg_dist, Dib, Px};
use clementine_core::convert::doc_ops::extract_text_ocr_from_file;
use clementine_core::convert::image_ops::{
    apply_frame_bg, apply_photo_edits, apply_upscale_sharpen, detect_faces_and_text_boxes,
    onnx_bg_model_path, remove_background_local,
};
use clementine_core::convert::media_ops::{
    extract_video_filmstrip, extract_video_frame_rgba, is_video_ext, preview_media_range,
    probe_duration_secs,
};
use clementine_core::{tool_meta, ToolParams};
use image::{imageops, Rgba, RgbaImage};
use std::cell::RefCell;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const TW: usize = 460;
pub const TH: usize = 540;
const TIMER_AUTO_ID: usize = 77;

const SWATCHES: [[u8; 4]; 8] = [
    [138, 210, 255, 255],
    [255, 126, 71, 255],
    [164, 138, 235, 255],
    [38, 92, 82, 255],
    [232, 98, 134, 255],
    [36, 46, 66, 255],
    [245, 247, 250, 255],
    [240, 162, 42, 255],
];

#[derive(Clone)]
struct Slider {
    label: &'static str,
    min: f32,
    max: f32,
    val: f32,
    bipolar: bool,
    unit: &'static str,
}

struct ModalState {
    tool_id: String,
    title: String,
    paths: Vec<PathBuf>,
    source_dims: (u32, u32),
    duration: f32,
    thumb: RgbaImage,
    filmstrip: Vec<RgbaImage>,
    ocr_lines: Vec<String>,
    detected_boxes: Vec<(u32, u32, u32, u32)>,
    pdf_pages: Vec<usize>,
    pdf_rotations: Vec<u16>,
    meta_fields: [String; 3],
    active_meta_field: Option<usize>,
    sliders: Vec<Slider>,
    pills: Vec<&'static str>,
    active_pill: usize,
    active_swatch: usize,
    box_center: (f32, f32),
    dragging_canvas: bool,
    dragging_slider: Option<usize>,
    applied: Option<ToolParams>,
    auto_step: u32,
    auto_total_steps: u32,
}

thread_local! {
    static MODAL: RefCell<Option<ModalState>> = const { RefCell::new(None) };
}

fn c8(r: u8, g: u8, b: u8) -> [f32; 3] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}

fn over(d: &mut Px, rgb: [f32; 3], a: f32) {
    let a = a.clamp(0.0, 1.0);
    let k = 1.0 - a;
    for i in 0..3 {
        d[i] = rgb[i] * a + d[i] * k;
    }
    d[3] = a + d[3] * k;
}

fn fill_rrect(buf: &mut [Px], cx: f64, cy: f64, hw: f64, hh: f64, r: f64, col: [f32; 3], alpha: f32) {
    let x0 = ((cx - hw - 2.0).floor() as i32).max(0) as usize;
    let x1 = ((cx + hw + 2.0).ceil() as usize).min(TW);
    let y0 = ((cy - hh - 2.0).floor() as i32).max(0) as usize;
    let y1 = ((cy + hh + 2.0).ceil() as usize).min(TH);
    for y in y0..y1 {
        for x in x0..x1 {
            let d = rounded_box_sdf(x as f64 + 0.5 - cx, y as f64 + 0.5 - cy, hw, hh, r);
            let c = (0.5 - d).clamp(0.0, 1.0) as f32;
            if c > 0.0 {
                over(&mut buf[y * TW + x], col, alpha * c);
            }
        }
    }
}

fn stroke_rrect(buf: &mut [Px], cx: f64, cy: f64, hw: f64, hh: f64, r: f64, w: f64, col: [f32; 3], alpha: f32) {
    let x0 = ((cx - hw - w - 2.0).floor() as i32).max(0) as usize;
    let x1 = ((cx + hw + w + 2.0).ceil() as usize).min(TW);
    let y0 = ((cy - hh - w - 2.0).floor() as i32).max(0) as usize;
    let y1 = ((cy + hh + w + 2.0).ceil() as usize).min(TH);
    for y in y0..y1 {
        for x in x0..x1 {
            let d = rounded_box_sdf(x as f64 + 0.5 - cx, y as f64 + 0.5 - cy, hw, hh, r).abs() - w * 0.5;
            let c = (0.5 - d).clamp(0.0, 1.0) as f32;
            if c > 0.0 {
                over(&mut buf[y * TW + x], col, alpha * c);
            }
        }
    }
}

fn fill_disc(buf: &mut [Px], cx: f64, cy: f64, r: f64, col: [f32; 3], alpha: f32) {
    let x0 = ((cx - r - 2.0).floor() as i32).max(0) as usize;
    let x1 = ((cx + r + 2.0).ceil() as usize).min(TW);
    let y0 = ((cy - r - 2.0).floor() as i32).max(0) as usize;
    let y1 = ((cy + r + 2.0).ceil() as usize).min(TH);
    for y in y0..y1 {
        for x in x0..x1 {
            let d = (x as f64 + 0.5 - cx).hypot(y as f64 + 0.5 - cy) - r;
            let c = (0.5 - d).clamp(0.0, 1.0) as f32;
            if c > 0.0 {
                over(&mut buf[y * TW + x], col, alpha * c);
            }
        }
    }
}

fn sample_photo(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| {
        let u = x as f32 / w.max(1) as f32;
        let v = y as f32 / h.max(1) as f32;
        let mut r = (28.0 + 95.0 * v) as f32;
        let mut g = (138.0 + 75.0 * (1.0 - v * 0.5)) as f32;
        let mut b = (218.0 - 55.0 * v) as f32;
        let dx = u - 0.52;
        let dy = v - 0.44;
        let dist = (dx * dx + dy * dy).sqrt();
        let angle = dy.atan2(dx);
        let petal = 0.16 + 0.055 * (angle * 8.0).cos();
        if dist < 0.055 {
            r = 218.0;
            g = 96.0;
            b = 18.0;
        } else if dist < petal {
            r = 252.0;
            g = 178.0;
            b = 32.0;
        } else if v > 0.72 + 0.06 * (u * 5.0).sin() {
            r = 46.0;
            g = 122.0;
            b = 64.0;
        }
        Rgba([r as u8, g as u8, b as u8, 255])
    })
}

fn is_video_tool(tool_id: &str) -> bool {
    matches!(
        tool_id,
        "trimVideo" | "cropVideo" | "changeVideoSpeed" | "splitVideo" | "videoSnapshots" | "redactVideo" | "joinVideos"
    )
}

fn load_media_assets(tool_id: &str, paths: &[PathBuf]) -> (RgbaImage, (u32, u32), f32, Vec<RgbaImage>) {
    let mut duration = 4.0f32;
    if let Some(p) = paths.first() {
        let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if is_video_ext(&ext) || is_video_tool(tool_id) || matches!(tool_id, "trimAudio" | "redactAudio" | "normalizeAudio" | "audioChannels" | "audioToVideo") {
            let d = probe_duration_secs(p);
            if d > 0.1 {
                duration = d;
            }
        }
        if is_video_ext(&ext) || is_video_tool(tool_id) {
            if let Some(frame) = extract_video_frame_rgba(p, (duration * 0.2).min(0.8), 240, 134) {
                let dims = (frame.width().max(64) * 4, frame.height().max(36) * 4);
                let mut strip = extract_video_filmstrip(p, duration, 7, 50, 34);
                if strip.is_empty() {
                    let tile = imageops::resize(&frame, 50, 34, imageops::FilterType::Triangle);
                    strip = vec![tile; 7];
                }
                return (frame, dims, duration, strip);
            }
        }
        if let Ok(img) = image::open(p) {
            let rgba = img.to_rgba8();
            let dims = (rgba.width().max(1), rgba.height().max(1));
            let thumb = imageops::resize(&rgba, 220, 168, imageops::FilterType::Triangle);
            return (thumb, dims, duration, Vec::new());
        }
    }
    let fallback = sample_photo(220, 136);
    let tile = imageops::resize(&fallback, 50, 34, imageops::FilterType::Triangle);
    (fallback, (1280, 720), duration, vec![tile; 7])
}

fn default_controls(tool_id: &str, duration: f32) -> (Vec<Slider>, Vec<&'static str>) {
    let dur = duration.max(1.0);
    let s = |label, min, max, val, bipolar, unit| Slider { label, min, max, val, bipolar, unit };
    match tool_id {
        "editImage" => (
            vec![
                s("Exposure", -1.5, 1.5, 0.0, true, " EV"),
                s("Brightness", -60.0, 60.0, 0.0, true, ""),
                s("Contrast", -50.0, 50.0, 0.0, true, ""),
                s("Saturation", -60.0, 60.0, 0.0, true, ""),
                s("Temperature", -60.0, 60.0, 0.0, true, ""),
                s("Tint", -50.0, 50.0, 0.0, true, ""),
            ],
            vec![],
        ),
        "removeBg" => (
            vec![
                s("Tolerance", 10.0, 90.0, 42.0, false, "%"),
                s("Edge Feather", 1.0, 40.0, 18.0, false, " px"),
            ],
            vec!["Neural AI (U2Netp)", "Fast Flood-Fill", "Soft Feather"],
        ),
        "upscaleImage" => (
            vec![
                s("Sharpness", 0.0, 100.0, 62.0, false, "%"),
                s("Denoise", 0.0, 40.0, 12.0, false, "%"),
            ],
            vec!["2x Super-Res", "4x Ultra-Res", "1x Sharpen Only"],
        ),
        "extractText" => (
            vec![],
            vec!["Save .TXT & Copy", "Preserve Line Breaks", "Single Paragraph"],
        ),
        "frameImage" => (
            vec![
                s("Padding", 8.0, 96.0, 36.0, false, " px"),
                s("Corners", 0.0, 44.0, 20.0, false, " px"),
                s("Shadow", 0.0, 48.0, 26.0, false, " px"),
            ],
            vec!["Auto", "1:1", "16:9", "4:5"],
        ),
        "compress" => (
            vec![
                s("Quality", 15.0, 95.0, 74.0, false, "%"),
                s("Resize Scale", 25.0, 100.0, 100.0, false, "%"),
            ],
            vec!["Balanced", "Strong", "Custom"],
        ),
        "cropImage" | "cropVideo" => (
            vec![
                s("Crop Width", 25.0, 100.0, 82.0, false, "%"),
                s("Crop Height", 25.0, 100.0, 82.0, false, "%"),
            ],
            vec!["Free", "1:1", "16:9", "9:16", "4:3"],
        ),
        "redactImage" => (
            vec![
                s("Box Width", 15.0, 85.0, 48.0, false, "%"),
                s("Box Height", 15.0, 75.0, 34.0, false, "%"),
            ],
            vec!["Blur", "Solid Black", "Auto Faces", "Auto Text", "Faces + Text"],
        ),
        "redactVideo" => (
            vec![
                s("Box Width", 15.0, 85.0, 48.0, false, "%"),
                s("Box Height", 15.0, 75.0, 34.0, false, "%"),
                s("Start Time", 0.0, (dur - 0.2).max(0.5), 0.0, false, " s"),
                s("End Time", 0.2, dur, dur, false, " s"),
            ],
            vec!["Blur", "Solid Black"],
        ),
        "annotateImage" => (
            vec![
                s("Box Scale", 25.0, 85.0, 54.0, false, "%"),
                s("Stroke Weight", 2.0, 12.0, 5.0, false, " px"),
            ],
            vec!["Orange", "Red", "Green", "Blue"],
        ),
        "createCollage" => (
            vec![
                s("Columns", 1.0, 4.0, 2.0, false, ""),
                s("Corner Radius", 0.0, 36.0, 18.0, false, " px"),
            ],
            vec!["Grid", "Row", "Column"],
        ),
        "trimVideo" => (
            vec![
                s("Start Time", 0.0, (dur - 0.2).max(0.5), (dur * 0.1).min(1.0), false, " s"),
                s("End Time", 0.2, dur, (dur * 0.9).max(0.4), false, " s"),
            ],
            vec!["Full Range", "Middle 50%", "First Half"],
        ),
        "trimAudio" => (
            vec![
                s("Start Time", 0.0, (dur - 0.2).max(0.5), (dur * 0.08).min(0.8), false, " s"),
                s("End Time", 0.2, dur, (dur * 0.92).max(0.4), false, " s"),
            ],
            vec!["Keep All Audio", "Trim Silent Ends"],
        ),
        "redactAudio" => (
            vec![
                s("Bleep Start", 0.0, (dur - 0.2).max(0.5), dur * 0.25, false, " s"),
                s("Bleep End", 0.2, dur, dur * 0.65, false, " s"),
            ],
            vec!["1 kHz Censor Beep", "Silent Mute", "First 25% Beep"],
        ),
        "splitVideo" => (
            vec![s("Split Point", 0.2, (dur - 0.2).max(0.5), dur * 0.5, false, " s")],
            vec!["Split at Cursor", "2 Equal Clips", "3 Equal Clips", "4 Equal Clips"],
        ),
        "videoSnapshots" => (
            vec![s("Frame Time", 0.0, dur, dur * 0.35, false, " s")],
            vec!["Single Frame", "3 Keyframes", "5 Frames"],
        ),
        "changeVideoSpeed" => (
            vec![s("Playback Speed", 0.25, 4.0, 2.0, false, "x")],
            vec!["0.5x", "1.0x", "1.5x", "2.0x", "4.0x"],
        ),
        "audioChannels" => (
            vec![
                s("Left Volume", 0.0, 100.0, 100.0, false, "%"),
                s("Right Volume", 0.0, 100.0, 100.0, false, "%"),
            ],
            vec!["Mono", "Stereo"],
        ),
        "normalizeAudio" => (
            vec![
                s("Target Loudness", -24.0, -8.0, -16.0, false, " LUFS"),
                s("Loudness Range", 1.0, 20.0, 11.0, false, " LU"),
                s("True Peak", -6.0, 0.0, -1.5, false, " dBTP"),
            ],
            vec!["Podcast (-16)", "Broadcast (-23)", "Streaming (-14)"],
        ),
        "audioToVideo" => (
            vec![s("Wave Scale", 40.0, 100.0, 85.0, false, "%")],
            vec!["16:9 Wave", "9:16 Wave", "1:1 Wave", "16:9 Bars", "9:16 Bars"],
        ),
        "joinVideos" => (vec![], vec!["Keep Order", "Reverse Order"]),
        "organizePDF" => (
            vec![],
            vec!["Rotate 90°", "Reverse Order", "Duplicate 1st", "Remove Last"],
        ),
        "mergePDF" => (
            vec![],
            vec!["Keep Order", "Reverse Order", "First File Only"],
        ),
        _ => (
            vec![],
            vec!["Strip All Tags", "Keep Color Profile", "Save Custom Tags"],
        ),
    }
}

impl ModalState {
    fn new(tool_id: &str, paths: Vec<PathBuf>) -> Self {
        let title = tool_meta(tool_id).map(|m| m.name.to_string()).unwrap_or_else(|| tool_id.to_string());
        let (thumb, source_dims, duration, filmstrip) = load_media_assets(tool_id, &paths);
        let (sliders, pills) = default_controls(tool_id, duration);
        let active_pill = if tool_id == "changeVideoSpeed" { 3 } else { 0 };
        let ocr_lines = if tool_id == "extractText" {
            paths
                .first()
                .and_then(|p| extract_text_ocr_from_file(p).ok())
                .map(|t| t.lines().map(|l| l.to_string()).filter(|l| !l.trim().is_empty()).collect::<Vec<_>>())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| {
                    vec![
                        "Clementine Offline Windows OCR (Windows.Media.Ocr)".into(),
                        "100% local neural text recognition on Windows 10 & 11.".into(),
                        "Extracts text from photos, screenshots, and scanned PDFs.".into(),
                        "Zero cloud uploads · Saved to .TXT and copied to Clipboard.".into(),
                    ]
                })
        } else {
            Vec::new()
        };
        let stem = paths
            .first()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled Document".into());
        Self {
            tool_id: tool_id.to_string(),
            title,
            paths,
            source_dims,
            duration,
            thumb,
            filmstrip,
            ocr_lines,
            detected_boxes: Vec::new(),
            pdf_pages: vec![0, 1, 2],
            pdf_rotations: vec![0, 0, 0],
            meta_fields: [stem, "Clementine Local".into(), "Processed 100% Offline".into()],
            active_meta_field: None,
            sliders,
            pills,
            active_pill,
            active_swatch: 0,
            box_center: (0.5, 0.5),
            dragging_canvas: false,
            dragging_slider: None,
            applied: None,
            auto_step: 0,
            auto_total_steps: 0,
        }
    }

    fn on_pill_clicked(&mut self, pill_idx: usize) {
        self.active_pill = pill_idx;
        let dur = self.duration.max(1.0);
        match self.tool_id.as_str() {
            "changeVideoSpeed" => {
                let spd = match pill_idx {
                    0 => 0.5,
                    1 => 1.0,
                    2 => 1.5,
                    3 => 2.0,
                    _ => 4.0,
                };
                if let Some(s) = self.sliders.first_mut() {
                    s.val = spd;
                }
            }
            "normalizeAudio" => {
                let (lufs, lra, tp) = match pill_idx {
                    1 => (-23.0, 7.0, -1.0),
                    2 => (-14.0, 9.0, -1.0),
                    _ => (-16.0, 11.0, -1.5),
                };
                if self.sliders.len() >= 3 {
                    self.sliders[0].val = lufs;
                    self.sliders[1].val = lra;
                    self.sliders[2].val = tp;
                }
            }
            "cropImage" | "cropVideo" if self.sliders.len() >= 2 => {
                let (w_pct, h_pct) = match pill_idx {
                    1 => (75.0, 75.0),
                    2 => (90.0, 52.0),
                    3 => (52.0, 90.0),
                    4 => (84.0, 63.0),
                    _ => (82.0, 82.0),
                };
                self.sliders[0].val = w_pct;
                self.sliders[1].val = h_pct;
            }
            "trimVideo" if self.sliders.len() >= 2 => {
                let (s0, s1) = match pill_idx {
                    1 => (dur * 0.25, dur * 0.75),
                    2 => (0.0, dur * 0.5),
                    _ => (0.0, dur),
                };
                self.sliders[0].val = s0;
                self.sliders[1].val = s1;
            }
            "redactAudio" if self.sliders.len() >= 2 && pill_idx == 2 => {
                self.sliders[0].val = 0.0;
                self.sliders[1].val = (dur * 0.25).max(0.2);
            }
            "redactImage" => {
                if pill_idx >= 2 {
                    let mode = (pill_idx - 1) as u8; // 1 = Faces, 2 = Text, 3 = Faces+Text
                    if let Some(p) = self.paths.first() {
                        self.detected_boxes = detect_faces_and_text_boxes(p, mode);
                    }
                    if self.detected_boxes.is_empty() {
                        let (w, h) = self.source_dims;
                        self.detected_boxes = vec![
                            (w / 5, h / 5, w / 4, h / 5),
                            (w / 2, h / 2, w / 3, h / 6),
                        ];
                    }
                } else {
                    self.detected_boxes.clear();
                }
            }
            "organizePDF" => {
                match pill_idx {
                    0 => {
                        for r in &mut self.pdf_rotations {
                            *r = (*r + 90) % 360;
                        }
                    }
                    1 => {
                        self.pdf_pages.reverse();
                        self.pdf_rotations.reverse();
                    }
                    2 => {
                        if let Some(&first) = self.pdf_pages.first() {
                            self.pdf_pages.push(first);
                            self.pdf_rotations.push(0);
                        }
                    }
                    3 => {
                        if self.pdf_pages.len() > 1 {
                            self.pdf_pages.pop();
                            self.pdf_rotations.pop();
                        }
                    }
                    _ => {}
                }
            }
            "createCollage" if !self.sliders.is_empty() => {
                self.sliders[0].val = match pill_idx {
                    1 => 4.0,
                    2 => 1.0,
                    _ => 2.0,
                };
            }
            "removeBg" if self.sliders.len() >= 2 => {
                let (tol, fth) = match pill_idx {
                    1 => (32.0, 6.0),
                    2 => (52.0, 32.0),
                    _ => (42.0, 18.0),
                };
                self.sliders[0].val = tol;
                self.sliders[1].val = fth;
            }
            _ => {}
        }
    }

    fn refresh_video_frame_at(&mut self, time_sec: f32) {
        if let Some(p) = self.paths.first() {
            if let Some(fr) = extract_video_frame_rgba(p, time_sec, 240, 134) {
                self.thumb = fr;
            }
        }
    }

    fn build_params(&self) -> ToolParams {
        let mut p = ToolParams::default();
        let val = |i: usize, def: f32| self.sliders.get(i).map(|s| s.val).unwrap_or(def);
        let dur = self.duration.max(0.5);
        match self.tool_id.as_str() {
            "editImage" => {
                p.exposure = val(0, 0.0);
                p.brightness = val(1, 0.0) / 100.0;
                p.contrast = (1.0 + val(2, 0.0) / 100.0).max(0.1);
                p.saturation = (1.0 + val(3, 0.0) / 100.0).max(0.0);
                p.temperature = val(4, 0.0) / 100.0;
                p.tint = val(5, 0.0) / 100.0;
            }
            "frameImage" => {
                p.bg_color = SWATCHES[self.active_swatch % SWATCHES.len()];
                p.padding = val(0, 36.0).round() as u32;
                p.corner_radius = val(1, 20.0).round() as u32;
                p.shadow = val(2, 26.0).round() as u32;
            }
            "removeBg" => {
                p.quality = val(0, 42.0).round().clamp(8.0, 95.0) as u8;
                p.padding = val(1, 18.0).round().clamp(1.0, 60.0) as u32;
                p.redact_solid = self.active_pill == 0;
            }
            "upscaleImage" => {
                p.quality = val(0, 62.0).round().clamp(0.0, 100.0) as u8;
                p.padding = val(1, 12.0).round().clamp(0.0, 40.0) as u32;
                p.collage_cols = match self.active_pill {
                    1 => 4,
                    2 => 1,
                    _ => 2,
                };
            }
            "compress" => {
                p.quality = if self.active_pill == 1 { 52 } else { val(0, 74.0).round() as u8 };
                p.scale_pct = val(1, 100.0).round() as u8;
            }
            "cropImage" | "cropVideo" => {
                let (w, h) = self.source_dims;
                let cw = (((w as f32 * val(0, 82.0) / 100.0).round() as u32) & !1).max(8);
                let ch = (((h as f32 * val(1, 82.0) / 100.0).round() as u32) & !1).max(8);
                let cx = (w as f32 * self.box_center.0).round() as u32;
                let cy = (h as f32 * self.box_center.1).round() as u32;
                let x = cx.saturating_sub(cw / 2).min(w.saturating_sub(cw));
                let y = cy.saturating_sub(ch / 2).min(h.saturating_sub(ch));
                p.crop_rect = Some((x, y, cw, ch));
            }
            "redactImage" => {
                p.redact_solid = self.active_pill == 1;
                if !self.detected_boxes.is_empty() {
                    p.redact_rects = self.detected_boxes.clone();
                }
                let (w, h) = self.source_dims;
                let rw = ((w as f32 * val(0, 48.0) / 100.0).round() as u32).max(8);
                let rh = ((h as f32 * val(1, 34.0) / 100.0).round() as u32).max(8);
                let cx = (w as f32 * self.box_center.0).round() as u32;
                let cy = (h as f32 * self.box_center.1).round() as u32;
                let x = cx.saturating_sub(rw / 2).min(w.saturating_sub(rw));
                let y = cy.saturating_sub(rh / 2).min(h.saturating_sub(rh));
                p.crop_rect = Some((x, y, rw, rh));
            }
            "redactVideo" => {
                p.redact_solid = self.active_pill == 1;
                let (w, h) = self.source_dims;
                let rw = ((w as f32 * val(0, 48.0) / 100.0).round() as u32).max(8);
                let rh = ((h as f32 * val(1, 34.0) / 100.0).round() as u32).max(8);
                let cx = (w as f32 * self.box_center.0).round() as u32;
                let cy = (h as f32 * self.box_center.1).round() as u32;
                let x = cx.saturating_sub(rw / 2).min(w.saturating_sub(rw));
                let y = cy.saturating_sub(rh / 2).min(h.saturating_sub(rh));
                p.crop_rect = Some((x, y, rw, rh));
                p.start_sec = val(2, 0.0);
                p.end_sec = Some(val(3, dur).max(p.start_sec + 0.1));
            }
            "annotateImage" => {
                p.annotation_color = match self.active_pill {
                    1 => [255, 52, 66, 255],
                    2 => [36, 188, 92, 255],
                    3 => [24, 128, 255, 255],
                    _ => [255, 82, 0, 255],
                };
                let (w, h) = self.source_dims;
                let scale = val(0, 54.0) / 100.0;
                let aw = ((w as f32 * scale).round() as u32).max(12);
                let ah = ((h as f32 * scale).round() as u32).max(12);
                let cx = (w as f32 * self.box_center.0).round() as u32;
                let cy = (h as f32 * self.box_center.1).round() as u32;
                let x = cx.saturating_sub(aw / 2).min(w.saturating_sub(aw));
                let y = cy.saturating_sub(ah / 2).min(h.saturating_sub(ah));
                p.crop_rect = Some((x, y, aw, ah));
            }
            "createCollage" => {
                p.collage_cols = val(0, 2.0).round().clamp(1.0, 4.0) as u32;
                p.corner_radius = val(1, 18.0).round() as u32;
            }
            "trimVideo" => {
                p.start_sec = val(0, 0.0);
                p.end_sec = Some(val(1, dur).max(p.start_sec + 0.1));
            }
            "redactAudio" => {
                p.start_sec = val(0, 0.0);
                p.end_sec = Some(val(1, dur).max(p.start_sec + 0.1));
                p.bleep_tone = self.active_pill != 1;
            }
            "trimAudio" => {
                p.start_sec = val(0, 0.0);
                p.end_sec = Some(val(1, dur).max(p.start_sec + 0.1));
                p.trim_silence = self.active_pill == 1;
            }
            "splitVideo" => {
                let pt = val(0, dur * 0.5).clamp(0.1, (dur - 0.1).max(0.2));
                p.split_points = match self.active_pill {
                    1 => vec![dur * 0.5],
                    2 => vec![dur / 3.0, dur * 2.0 / 3.0],
                    3 => vec![dur * 0.25, dur * 0.5, dur * 0.75],
                    _ => vec![pt],
                };
                p.end_sec = Some(pt);
            }
            "videoSnapshots" => {
                let t0 = val(0, dur * 0.35).clamp(0.0, dur);
                p.start_sec = t0;
                p.snapshot_times = match self.active_pill {
                    1 => vec![dur * 0.15, dur * 0.5, dur * 0.85],
                    2 => (0..5).map(|i| dur * (i as f32 + 0.5) / 5.0).collect(),
                    _ => vec![t0],
                };
            }
            "changeVideoSpeed" => {
                p.speed_factor = val(0, 2.0).clamp(0.25, 4.0);
            }
            "audioChannels" => {
                p.mono = self.active_pill == 0;
                p.left_gain = (val(0, 100.0) / 100.0).clamp(0.0, 1.0);
                p.right_gain = (val(1, 100.0) / 100.0).clamp(0.0, 1.0);
            }
            "normalizeAudio" => {
                p.target_lufs = val(0, -16.0).clamp(-24.0, -8.0);
                p.loudness_range = val(1, 11.0).clamp(1.0, 20.0);
                p.true_peak = val(2, -1.5).clamp(-6.0, 0.0);
            }
            "audioToVideo" => {
                p.visualizer_shape = match self.active_pill {
                    1 | 4 => 1,
                    2 => 2,
                    _ => 0,
                };
                p.visualizer_bars = self.active_pill >= 3;
                p.bg_color = if self.active_swatch == 0 {
                    [0, 0, 0, 255]
                } else {
                    SWATCHES[self.active_swatch % SWATCHES.len()]
                };
            }
            "joinVideos" => {
                let n = self.paths.len().max(2);
                p.page_order = if self.active_pill == 1 {
                    (0..n).rev().collect()
                } else {
                    (0..n).collect()
                };
            }
            "organizePDF" => {
                p.page_order = self.pdf_pages.clone();
                p.page_rotations = self.pdf_rotations.clone();
            }
            "mergePDF" => {
                let n = self.paths.len().max(2);
                p.page_order = match self.active_pill {
                    1 => (0..n).rev().collect(),
                    2 => vec![0],
                    _ => (0..n).collect(),
                };
            }
            "removeMetadata" => {
                if self.active_pill == 2 || self.active_meta_field.is_some() {
                    p.custom_metadata = Some((
                        self.meta_fields[0].clone(),
                        self.meta_fields[1].clone(),
                        self.meta_fields[2].clone(),
                    ));
                }
            }
            _ => {}
        }
        p
    }
}

fn blit_rgba(buf: &mut [Px], img: &RgbaImage, ox: i32, oy: i32, r_corner: f64) {
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let (cx, cy) = (ox as f64 + iw as f64 * 0.5, oy as f64 + ih as f64 * 0.5);
    for y in 0..ih {
        for x in 0..iw {
            let (gx, gy) = (ox + x, oy + y);
            if gx < 0 || gy < 0 || gx >= TW as i32 || gy >= TH as i32 {
                continue;
            }
            let d = rounded_box_sdf(gx as f64 + 0.5 - cx, gy as f64 + 0.5 - cy, iw as f64 * 0.5, ih as f64 * 0.5, r_corner);
            let cov = (0.5 - d).clamp(0.0, 1.0) as f32;
            if cov > 0.0 {
                let p = img.get_pixel(x as u32, y as u32).0;
                over(&mut buf[gy as usize * TW + gx as usize], c8(p[0], p[1], p[2]), (p[3] as f32 / 255.0) * cov);
            }
        }
    }
}

fn draw_filmstrip_timeline(
    buf: &mut [Px],
    strip: &[RgbaImage],
    cy: f64,
    s_norm: f32,
    e_norm: f32,
    splits: &[f32],
    dur: f32,
) {
    let orange = c8(255, 82, 0);
    fill_rrect(buf, 230.0, cy, 186.0, 22.0, 8.0, c8(28, 22, 18), 0.92);
    let n = strip.len().max(1);
    for (i, tile) in strip.iter().enumerate() {
        let tx = 50 + (i * 356 / n) as i32;
        blit_rgba(buf, tile, tx, cy as i32 - 17, 4.0);
    }
    let lx = 50.0 + (s_norm.clamp(0.0, 0.95) as f64) * 360.0;
    let rx = 50.0 + (e_norm.clamp(s_norm + 0.05, 1.0) as f64) * 360.0;
    if lx > 52.0 {
        fill_rrect(buf, (50.0 + lx) * 0.5, cy, (lx - 50.0) * 0.5, 19.0, 4.0, c8(12, 10, 8), 0.68);
    }
    if rx < 408.0 {
        fill_rrect(buf, (rx + 410.0) * 0.5, cy, (410.0 - rx) * 0.5, 19.0, 4.0, c8(12, 10, 8), 0.68);
    }
    let bw = ((rx - lx) * 0.5).max(6.0);
    stroke_rrect(buf, (lx + rx) * 0.5, cy, bw, 19.0, 6.0, 2.4, orange, 1.0);
    fill_rrect(buf, lx, cy, 4.5, 20.0, 3.0, orange, 1.0);
    fill_rrect(buf, rx, cy, 4.5, 20.0, 3.0, orange, 1.0);
    fill_rrect(buf, lx, cy, 1.2, 8.0, 1.0, [1.0; 3], 0.95);
    fill_rrect(buf, rx, cy, 1.2, 8.0, 1.0, [1.0; 3], 0.95);

    for &sp in splits {
        let sn = (sp / dur.max(0.5)).clamp(0.05, 0.95) as f64;
        let sx = 50.0 + sn * 360.0;
        fill_rrect(buf, sx, cy, 1.8, 22.0, 1.5, orange, 1.0);
        fill_disc(buf, sx, cy - 20.0, 6.0, orange, 1.0);
    }
}

fn render_modal(st: &ModalState) -> Vec<u32> {
    let mut buf = vec![[0f32; 4]; TW * TH];
    let orange = c8(255, 82, 0);

    // 1. Warm frosted glass card surface
    for y in 0..TH {
        let v = y as f32 / (TH - 1) as f32;
        for x in 0..TW {
            let u = x as f32 / (TW - 1) as f32;
            let d = rounded_box_sdf(x as f64 + 0.5 - TW as f64 * 0.5, y as f64 + 0.5 - TH as f64 * 0.5, TW as f64 * 0.5 - 2.0, TH as f64 * 0.5 - 2.0, 22.0);
            let c = (0.5 - d).clamp(0.0, 1.0) as f32;
            if c <= 0.0 {
                continue;
            }
            let r = 252.0 * (1.0 - v) + (244.0 - 16.0 * u) * v;
            let g = 194.0 * (1.0 - v) + (172.0 + 14.0 * u) * v;
            let b = 98.0 * (1.0 - v) + (152.0 + 58.0 * u) * v;
            let i = y * TW + x;
            over(&mut buf[i], [r / 255.0, g / 255.0, b / 255.0], 0.96 * c);
            let inner = (0.5 - (d + 1.5)).clamp(0.0, 1.0) as f32;
            over(&mut buf[i], [1.0; 3], 0.55 * (c - inner).max(0.0));
        }
    }

    // 2. Top Header Bar + Close button + Divider
    fill_disc(&mut buf, 28.0, 24.0, 12.0, c8(40, 32, 24), 0.10);
    for y in 16..33 {
        for x in 20..37 {
            let d = seg_dist(x as f32 + 0.5, y as f32 + 0.5, 24.0, 20.0, 32.0, 28.0)
                .min(seg_dist(x as f32 + 0.5, y as f32 + 0.5, 32.0, 20.0, 24.0, 28.0));
            let c = (1.3 - d).clamp(0.0, 1.0);
            if c > 0.0 {
                over(&mut buf[y * TW + x], c8(48, 36, 26), c);
            }
        }
    }
    blit_mask_sized(&mut buf, TW, TH, &fit_mask(&st.title, 15, 700, 280), TW as f64 * 0.5, 24.0, c8(42, 30, 20));
    fill_rrect(&mut buf, TW as f64 * 0.5, 46.0, TW as f64 * 0.5 - 4.0, 0.5, 0.0, c8(60, 40, 20), 0.12);

    // 3. Live Preview Area (y = 56..260)
    let params = st.build_params();
    let dur = st.duration.max(0.5);
    match st.tool_id.as_str() {
        "trimVideo" | "splitVideo" | "videoSnapshots" => {
            // Real 16:9 video monitor on top + 7-tile filmstrip timeline below
            fill_rrect(&mut buf, 230.0, 122.0, 198.0, 66.0, 12.0, c8(16, 14, 13), 0.95);
            let vframe = imageops::resize(&st.thumb, 216, 120, imageops::FilterType::Triangle);
            blit_rgba(&mut buf, &vframe, 230 - 108, 122 - 60, 6.0);

            // Timecode pill in top-right of video monitor
            fill_rrect(&mut buf, 372.0, 74.0, 46.0, 11.0, 6.0, c8(16, 14, 13), 0.78);
            let tc = if st.tool_id == "trimVideo" {
                format!("{:.1}s - {:.1}s", params.start_sec, params.end_sec.unwrap_or(dur))
            } else if st.tool_id == "splitVideo" {
                format!("{} clips", params.split_points.len() + 1)
            } else {
                format!("{} PNGs", params.snapshot_times.len().max(1))
            };
            blit_mask_sized(&mut buf, TW, TH, &fit_mask(&tc, 10, 700, 84), 372.0, 74.0, [1.0; 3]);

            let (s_norm, e_norm) = if st.tool_id == "trimVideo" {
                (
                    (params.start_sec / dur).clamp(0.0, 0.9),
                    (params.end_sec.unwrap_or(dur) / dur).clamp(0.1, 1.0),
                )
            } else {
                (0.0, 1.0)
            };
            let markers: &[f32] = if st.tool_id == "splitVideo" {
                &params.split_points
            } else if st.tool_id == "videoSnapshots" {
                &params.snapshot_times
            } else {
                &[]
            };
            draw_filmstrip_timeline(&mut buf, &st.filmstrip, 224.0, s_norm, e_norm, markers, dur);
        }
        "changeVideoSpeed" => {
            fill_rrect(&mut buf, 230.0, 152.0, 200.0, 92.0, 14.0, c8(16, 14, 13), 0.95);
            let vframe = imageops::resize(&st.thumb, 244, 138, imageops::FilterType::Triangle);
            blit_rgba(&mut buf, &vframe, 230 - 122, 142 - 69, 8.0);
            let out_dur = dur / params.speed_factor.max(0.25);
            fill_rrect(&mut buf, 230.0, 226.0, 104.0, 13.0, 7.0, orange, 0.94);
            blit_mask_sized(
                &mut buf,
                TW,
                TH,
                &fit_mask(&format!("Speed {:.2}x · Output {:.1}s", params.speed_factor, out_dur), 11, 700, 196),
                230.0,
                226.0,
                [1.0; 3],
            );
        }
        "trimAudio" | "redactAudio" => {
            fill_rrect(&mut buf, 230.0, 148.0, 202.0, 84.0, 14.0, c8(255, 234, 208), 0.56);
            let s_norm = (params.start_sec / dur).clamp(0.0, 0.88);
            let e_norm = (params.end_sec.unwrap_or(dur) / dur).clamp(s_norm + 0.08, 1.0);
            for i in 0..46 {
                let t = i as f32 / 45.0;
                let bx = 52.0 + t as f64 * 356.0;
                let h = 10.0 + ((i as f32 * 0.65).sin().abs() * (i as f32 * 0.19).cos().abs()) as f64 * 40.0;
                let in_range = t >= s_norm && t <= e_norm;
                let active = if st.tool_id == "redactAudio" { !in_range } else { in_range };
                let col = if st.tool_id == "redactAudio" && in_range {
                    c8(220, 48, 48)
                } else if active {
                    orange
                } else {
                    c8(150, 118, 96)
                };
                fill_rrect(&mut buf, bx, 144.0, 2.0, h, 2.0, col, if active || in_range { 0.92 } else { 0.38 });
            }
            let lx = 52.0 + s_norm as f64 * 356.0;
            let rx = 52.0 + e_norm as f64 * 356.0;
            stroke_rrect(&mut buf, (lx + rx) * 0.5, 144.0, ((rx - lx) * 0.5).max(6.0), 54.0, 8.0, 2.0, orange, 0.92);
            fill_rrect(&mut buf, lx, 144.0, 4.5, 56.0, 3.5, orange, 1.0);
            fill_rrect(&mut buf, rx, 144.0, 4.5, 56.0, 3.5, orange, 1.0);
            let status = if st.tool_id == "trimAudio" && params.trim_silence {
                format!("Start {:.1}s · End {:.1}s · Auto-trim silent ends ON", params.start_sec, params.end_sec.unwrap_or(dur))
            } else if st.tool_id == "redactAudio" {
                format!("Muting {:.1}s to {:.1}s", params.start_sec, params.end_sec.unwrap_or(dur))
            } else {
                format!("Keep {:.1}s to {:.1}s (Duration {:.1}s)", params.start_sec, params.end_sec.unwrap_or(dur), dur)
            };
            blit_mask_sized(&mut buf, TW, TH, &fit_mask(&status, 11, 600, 360), 230.0, 216.0, c8(72, 48, 30));
        }
        "normalizeAudio" => {
            fill_rrect(&mut buf, 230.0, 148.0, 202.0, 84.0, 14.0, c8(255, 234, 208), 0.56);
            let boost = ((params.target_lufs + 24.0) / 16.0).clamp(0.35, 1.25) as f64;
            let ceil_y = 148.0 - 44.0 * boost.min(1.05);
            fill_rrect(&mut buf, 230.0, ceil_y, 186.0, 1.0, 0.5, orange, 0.75);
            for i in 0..46 {
                let bx = 52.0 + (i as f64 / 45.0) * 356.0;
                let h = (8.0 + ((i as f32 * 0.62).sin().abs() * 34.0) as f64 * boost).min(52.0);
                fill_rrect(&mut buf, bx, 148.0, 2.2, h, 2.0, orange, 0.90);
            }
            blit_mask_sized(
                &mut buf,
                TW,
                TH,
                &fit_mask(&format!("Target {:.1} LUFS · LRA {:.1} LU · TP {:.1} dBTP", params.target_lufs, params.loudness_range, params.true_peak), 11, 600, 360),
                230.0,
                216.0,
                c8(72, 48, 30),
            );
        }
        "audioChannels" => {
            fill_rrect(&mut buf, 230.0, 152.0, 202.0, 88.0, 14.0, c8(255, 234, 208), 0.56);
            blit_mask_sized(&mut buf, TW, TH, &fit_mask("L", 12, 700, 24), 48.0, 120.0, c8(68, 44, 28));
            blit_mask_sized(&mut buf, TW, TH, &fit_mask("R", 12, 700, 24), 48.0, 178.0, c8(68, 44, 28));
            for i in 0..42 {
                let bx = 68.0 + (i as f64 / 41.0) * 336.0;
                let base = 6.0 + ((i as f32 * 0.58).sin().abs() * 18.0) as f64;
                let lh = (base * params.left_gain as f64).max(2.0);
                let rh = (base * params.right_gain as f64).max(2.0);
                fill_rrect(&mut buf, bx, 120.0, 2.0, lh, 2.0, orange, 0.92);
                fill_rrect(&mut buf, bx, 178.0, 2.0, rh, 2.0, if params.mono { orange } else { c8(218, 96, 28) }, 0.92);
            }
            let mode_lbl = if params.mono { "Mono Mixdown (1 ch)" } else { "Stereo Split (2 ch)" };
            blit_mask_sized(&mut buf, TW, TH, &fit_mask(mode_lbl, 11, 600, 260), 230.0, 222.0, c8(72, 48, 30));
        }
        "audioToVideo" => {
            let (cw, ch) = match params.visualizer_shape {
                1 => (58.0, 92.0),
                2 => (88.0, 88.0),
                _ => (162.0, 88.0),
            };
            let bg_col = if st.active_swatch == 0 {
                c8(16, 14, 13)
            } else {
                let sw = SWATCHES[st.active_swatch % SWATCHES.len()];
                c8(sw[0], sw[1], sw[2])
            };
            fill_rrect(&mut buf, 230.0, 152.0, cw, ch, 12.0, bg_col, 0.96);
            stroke_rrect(&mut buf, 230.0, 152.0, cw, ch, 12.0, 1.5, orange, 0.65);
            let scale = (st.sliders.first().map(|s| s.val).unwrap_or(85.0) / 100.0) as f64;
            let bars = 24;
            for i in 0..bars {
                let u = i as f64 / (bars - 1) as f64;
                let bx = 230.0 - (cw - 18.0) + u * (cw - 18.0) * 2.0;
                let h = (6.0 + ((i as f32 * 0.7).sin().abs() * 36.0) as f64 * scale).min(ch - 14.0);
                let w = if params.visualizer_bars { 2.8 } else { 1.4 };
                fill_rrect(&mut buf, bx, 152.0, w, h, 1.5, orange, 0.95);
            }
        }
        "joinVideos" => {
            fill_rrect(&mut buf, 230.0, 158.0, 202.0, 94.0, 14.0, c8(255, 245, 235), 0.62);
            let mut names: Vec<String> = st
                .paths
                .iter()
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .collect();
            if names.is_empty() {
                names = vec!["clip_01.mp4".into(), "clip_02.mp4".into(), "clip_03.mp4".into()];
            }
            if st.active_pill == 1 {
                names.reverse();
            }
            for (i, name) in names.iter().take(4).enumerate() {
                let ry = 94.0 + i as f64 * 40.0;
                fill_rrect(&mut buf, 230.0, ry, 182.0, 16.0, 8.0, [1.0; 3], 0.92);
                fill_disc(&mut buf, 68.0, ry, 10.0, orange, 1.0);
                blit_mask_sized(&mut buf, TW, TH, &fit_mask(&(i + 1).to_string(), 11, 700, 20), 68.0, ry, [1.0; 3]);
                blit_mask_sized(&mut buf, TW, TH, &fit_mask(name, 12, 600, 260), 218.0, ry, c8(42, 34, 28));
            }
        }
        "removeMetadata" => {
            fill_rrect(&mut buf, 230.0, 158.0, 202.0, 94.0, 14.0, [1.0; 3], 0.84);
            let labels = ["Title / Document", "Author / Artist", "Comment / Copyright"];
            for (i, k) in labels.iter().enumerate() {
                let ry = 88.0 + i as f64 * 42.0;
                let active = st.active_meta_field == Some(i);
                fill_rrect(&mut buf, 294.0, ry, 118.0, 14.0, 7.0, if active { c8(255, 240, 228) } else { c8(245, 242, 238) }, 0.95);
                stroke_rrect(&mut buf, 294.0, ry, 118.0, 14.0, 7.0, if active { 1.8 } else { 1.0 }, if active { orange } else { c8(190, 180, 170) }, 0.85);
                blit_mask_sized(&mut buf, TW, TH, &fit_mask(k, 11, 600, 116), 106.0, ry, c8(42, 36, 32));
                let val_str = if st.active_pill == 0 && st.active_meta_field.is_none() {
                    "Stripped on Save (Click to edit)".to_string()
                } else {
                    st.meta_fields[i].clone()
                };
                blit_mask_sized(&mut buf, TW, TH, &fit_mask(&val_str, 11, 600, 220), 294.0, ry, if active || st.active_pill == 2 { c8(32, 26, 22) } else { orange });
            }
            blit_mask_sized(
                &mut buf,
                TW,
                TH,
                &fit_mask("GPS Latitude/Longitude, Camera Serial & XMP Tracking: Always Stripped", 10, 600, 380),
                230.0,
                222.0,
                c8(96, 72, 54),
            );
        }
        "organizePDF" | "mergePDF" => {
            fill_rrect(&mut buf, 230.0, 158.0, 202.0, 94.0, 14.0, c8(255, 245, 235), 0.55);
            let pages: Vec<usize> = if st.tool_id == "organizePDF" {
                st.pdf_pages.iter().map(|&p| p + 1).collect()
            } else if st.active_pill == 1 {
                vec![3, 2, 1]
            } else if st.active_pill == 2 {
                vec![1]
            } else {
                vec![1, 2, 3]
            };
            let n = pages.len().min(4).max(1);
            let step = 340.0 / n as f64;
            let start_x = 230.0 - step * (n as f64 - 1.0) * 0.5;
            for (i, pg) in pages.iter().take(4).enumerate() {
                let px = start_x + i as f64 * step;
                let rot = st.pdf_rotations.get(i).copied().unwrap_or(0);
                let (pw_card, ph_card) = if rot % 180 == 90 { (58.0, 42.0) } else { (42.0, 58.0) };
                fill_rrect(&mut buf, px, 154.0, pw_card, ph_card, 8.0, [1.0; 3], 0.96);
                fill_disc(&mut buf, px - 22.0, 114.0, 11.0, orange, 1.0);
                blit_mask_sized(&mut buf, TW, TH, &fit_mask(&pg.to_string(), 11, 700, 24), px - 22.0, 114.0, [1.0; 3]);
                if rot != 0 {
                    blit_mask_sized(&mut buf, TW, TH, &fit_mask(&format!("{rot}°"), 10, 700, 36), px + 18.0, 114.0, orange);
                }
                for ln in 0..4 {
                    fill_rrect(&mut buf, px, 140.0 + ln as f64 * 11.0, pw_card - 14.0, 1.5, 1.0, c8(190, 185, 180), 0.8);
                }
            }
            if st.tool_id == "organizePDF" {
                blit_mask_sized(&mut buf, TW, TH, &fit_mask("Click a page card to rotate 90°, or use pills below", 10, 600, 340), 230.0, 234.0, c8(86, 62, 44));
            }
        }
        "frameImage" => {
            let sw = SWATCHES[st.active_swatch % SWATCHES.len()];
            fill_rrect(&mut buf, 230.0, 158.0, 148.0, 94.0, 14.0, c8(sw[0], sw[1], sw[2]), 0.96);
            let pad = (params.padding as i32 / 3).clamp(4, 34);
            let inner = imageops::resize(&st.thumb, (240 - pad as u32 * 2).max(60), (160 - pad as u32 * 2).max(40), imageops::FilterType::Triangle);
            blit_rgba(&mut buf, &inner, 230 - inner.width() as i32 / 2, 158 - inner.height() as i32 / 2, (params.corner_radius as f64 * 0.5).clamp(2.0, 22.0));
        }
        "removeBg" => {
            // Checkerboard transparency backdrop + live local foreground cutout
            let (pw, ph) = (st.thumb.width() as i32, st.thumb.height() as i32);
            let (ox, oy) = (230 - pw / 2, 156 - ph / 2);
            for y in 0..ph {
                for x in 0..pw {
                    let (gx, gy) = (ox + x, oy + y);
                    if gx >= 0 && gy >= 0 && gx < TW as i32 && gy < TH as i32 {
                        let cell = ((x / 10) + (y / 10)) & 1 == 0;
                        let col = if cell { c8(242, 240, 236) } else { c8(208, 204, 198) };
                        over(&mut buf[gy as usize * TW + gx as usize], col, 0.96);
                    }
                }
            }
            let mut preview_params = params.clone();
            preview_params.redact_solid = onnx_bg_model_path().is_file();
            let cutout = remove_background_local(&st.thumb, &preview_params);
            blit_rgba(&mut buf, &cutout, ox, oy, 8.0);
            stroke_rrect(&mut buf, 230.0, 156.0, pw as f64 * 0.5, ph as f64 * 0.5, 8.0, 1.5, orange, 0.75);
            let engine_badge = if st.active_pill == 0 {
                if onnx_bg_model_path().is_file() {
                    "Neural AI (U2Netp ONNX Ready) · Transparent PNG"
                } else {
                    "Neural AI (U2Netp ONNX · Auto-downloads 4.4 MB on Apply)"
                }
            } else {
                "100% Local Flood-Fill Matte · Transparent PNG"
            };
            blit_mask_sized(&mut buf, TW, TH, &fit_mask(engine_badge, 10, 700, 340), 230.0, 236.0, c8(68, 44, 28));
        }
        "upscaleImage" => {
            fill_rrect(&mut buf, 230.0, 152.0, 202.0, 92.0, 14.0, c8(18, 16, 15), 0.94);
            let mut preview_p = params.clone();
            preview_p.collage_cols = 1;
            let sharp_img = apply_upscale_sharpen(&st.thumb, &preview_p);
            let (pw, ph) = (st.thumb.width() as i32, st.thumb.height() as i32);
            let (ox, oy) = (230 - pw / 2, 144 - ph / 2);
            // Split-screen comparison: left = original, right = CAS super-res sharpened
            let mut split_view = st.thumb.clone();
            for y in 0..ph as u32 {
                for x in (pw as u32 / 2)..pw as u32 {
                    split_view.put_pixel(x, y, *sharp_img.get_pixel(x, y));
                }
            }
            blit_rgba(&mut buf, &split_view, ox, oy, 8.0);
            fill_rrect(&mut buf, 230.0, 144.0, 1.2, ph as f64 * 0.5, 1.0, orange, 1.0);
            let scale = params.collage_cols.max(1);
            let (ow, oh) = (st.source_dims.0 * scale, st.source_dims.1 * scale);
            fill_rrect(&mut buf, 230.0, 226.0, 126.0, 12.0, 7.0, orange, 0.94);
            blit_mask_sized(
                &mut buf,
                TW,
                TH,
                &fit_mask(&format!("{scale}x Super-Res · {ow} x {oh} px"), 11, 700, 236),
                230.0,
                226.0,
                [1.0; 3],
            );
        }
        "extractText" => {
            fill_rrect(&mut buf, 230.0, 162.0, 202.0, 98.0, 14.0, [1.0; 3], 0.88);
            stroke_rrect(&mut buf, 230.0, 162.0, 202.0, 98.0, 14.0, 1.2, orange, 0.55);
            fill_rrect(&mut buf, 230.0, 82.0, 186.0, 12.0, 6.0, c8(255, 236, 220), 0.95);
            blit_mask_sized(
                &mut buf,
                TW,
                TH,
                &fit_mask("Windows.Media.Ocr · 100% Offline Text Extraction", 10, 700, 340),
                230.0,
                82.0,
                orange,
            );
            for (i, line) in st.ocr_lines.iter().take(5).enumerate() {
                let ry = 112.0 + i as f64 * 26.0;
                blit_mask_sized(&mut buf, TW, TH, &fit_mask(line, 11, 600, 368), 230.0, ry, c8(38, 32, 28));
            }
            blit_mask_sized(
                &mut buf,
                TW,
                TH,
                &fit_mask("Apply saves .TXT next to file and copies text to Clipboard", 10, 600, 360),
                230.0,
                244.0,
                c8(110, 84, 64),
            );
        }
        _ => {
            fill_rrect(&mut buf, 230.0, 158.0, 202.0, 96.0, 14.0, c8(18, 16, 15), 0.94);
            let preview = if st.tool_id == "editImage" {
                apply_photo_edits(&st.thumb, &params)
            } else {
                st.thumb.clone()
            };
            let ox = 230 - preview.width() as i32 / 2;
            let oy = 158 - preview.height() as i32 / 2;
            blit_rgba(&mut buf, &preview, ox, oy, 8.0);
            let pw = preview.width() as f64;
            let ph = preview.height() as f64;
            if st.tool_id == "cropImage" || st.tool_id == "cropVideo" {
                let cw = pw * (st.sliders.first().map(|s| s.val).unwrap_or(82.0) as f64 / 100.0) * 0.5;
                let ch = ph * (st.sliders.get(1).map(|s| s.val).unwrap_or(82.0) as f64 / 100.0) * 0.5;
                let bcx = (ox as f64 + pw * st.box_center.0 as f64).clamp(ox as f64 + cw, ox as f64 + pw - cw);
                let bcy = (oy as f64 + ph * st.box_center.1 as f64).clamp(oy as f64 + ch, oy as f64 + ph - ch);
                stroke_rrect(&mut buf, bcx, bcy, cw, ch, 2.0, 2.5, orange, 1.0);
                for (dx, dy) in [(-cw, -ch), (cw, -ch), (-cw, ch), (cw, ch)] {
                    fill_rrect(&mut buf, bcx + dx, bcy + dy, 4.5, 4.5, 2.0, orange, 1.0);
                }
            } else if st.tool_id == "redactImage" || st.tool_id == "redactVideo" {
                let col = if st.active_pill == 1 { c8(8, 8, 8) } else { c8(90, 110, 120) };
                if !st.detected_boxes.is_empty() {
                    let (sw, sh) = (st.source_dims.0.max(1) as f64, st.source_dims.1.max(1) as f64);
                    for &(bx, by, bw, bh) in &st.detected_boxes {
                        let rcx = ox as f64 + ((bx as f64 + bw as f64 * 0.5) / sw) * pw;
                        let rcy = oy as f64 + ((by as f64 + bh as f64 * 0.5) / sh) * ph;
                        let rhw = ((bw as f64 / sw) * pw * 0.5).max(8.0);
                        let rhh = ((bh as f64 / sh) * ph * 0.5).max(6.0);
                        fill_rrect(&mut buf, rcx, rcy, rhw, rhh, 3.0, col, 0.94);
                        stroke_rrect(&mut buf, rcx, rcy, rhw, rhh, 3.0, 1.8, orange, 1.0);
                    }
                } else {
                    let rw = pw * (st.sliders.first().map(|s| s.val).unwrap_or(48.0) as f64 / 100.0) * 0.5;
                    let rh = ph * (st.sliders.get(1).map(|s| s.val).unwrap_or(34.0) as f64 / 100.0) * 0.5;
                    let bcx = (ox as f64 + pw * st.box_center.0 as f64).clamp(ox as f64 + rw, ox as f64 + pw - rw);
                    let bcy = (oy as f64 + ph * st.box_center.1 as f64).clamp(oy as f64 + rh, oy as f64 + ph - rh);
                    fill_rrect(&mut buf, bcx, bcy, rw, rh, 4.0, col, 0.94);
                    stroke_rrect(&mut buf, bcx, bcy, rw, rh, 4.0, 2.2, orange, 1.0);
                }
            } else if st.tool_id == "annotateImage" {
                let scale = st.sliders.first().map(|s| s.val).unwrap_or(54.0) as f64 / 100.0;
                let aw = pw * scale * 0.5;
                let ah = ph * scale * 0.5;
                let bcx = (ox as f64 + pw * st.box_center.0 as f64).clamp(ox as f64 + aw, ox as f64 + pw - aw);
                let bcy = (oy as f64 + ph * st.box_center.1 as f64).clamp(oy as f64 + ah, oy as f64 + ph - ah);
                let ac = c8(params.annotation_color[0], params.annotation_color[1], params.annotation_color[2]);
                stroke_rrect(&mut buf, bcx, bcy, aw, ah, 6.0, 3.0, ac, 1.0);
            } else if st.tool_id == "compress" {
                let q = st.sliders.first().map(|s| s.val).unwrap_or(74.0);
                let pct = (100.0 - q * 0.65).round() as i32;
                fill_rrect(&mut buf, 230.0, 234.0, 96.0, 13.0, 8.0, orange, 0.92);
                blit_mask_sized(&mut buf, TW, TH, &fit_mask(&format!("Estimated -{pct}% smaller"), 11, 700, 180), 230.0, 234.0, [1.0; 3]);
            } else if st.tool_id == "frameImage" {
                let _ = apply_frame_bg(&st.thumb, &params);
            }
        }
    }

    // 4. Swatches row for Add BG (`frameImage`) and Audio Visualizer (`audioToVideo`)
    let mut y_cursor = 282.0;
    if st.tool_id == "frameImage" || st.tool_id == "audioToVideo" {
        blit_mask_sized(&mut buf, TW, TH, &fit_mask("Background", 12, 600, 90), 66.0, y_cursor, c8(68, 44, 28));
        for (i, sw) in SWATCHES.iter().enumerate() {
            let sx = 142.0 + i as f64 * 38.0;
            let col = if st.tool_id == "audioToVideo" && i == 0 {
                c8(16, 14, 13)
            } else {
                c8(sw[0], sw[1], sw[2])
            };
            fill_rrect(&mut buf, sx, y_cursor, 15.0, 12.0, 6.0, col, 1.0);
            if i == st.active_swatch {
                stroke_rrect(&mut buf, sx, y_cursor, 16.5, 13.5, 7.0, 2.2, orange, 1.0);
            }
        }
        y_cursor += 36.0;
    }

    // 5. Sliders
    for (idx, s) in st.sliders.iter().enumerate() {
        let sy = y_cursor + idx as f64 * 32.0;
        blit_mask_sized(&mut buf, TW, TH, &fit_mask(s.label, 12, 600, 98), 72.0, sy, c8(54, 38, 26));
        let (tx0, tx1) = (138.0, 358.0);
        let tw = tx1 - tx0;
        let norm = ((s.val - s.min) / (s.max - s.min)).clamp(0.0, 1.0) as f64;
        let thumb_x = tx0 + norm * tw;
        fill_rrect(&mut buf, (tx0 + tx1) * 0.5, sy, tw * 0.5, 2.5, 2.5, c8(60, 40, 25), 0.18);
        let fill_x0 = if s.bipolar { tx0 + tw * 0.5 } else { tx0 };
        let (a0, a1) = (fill_x0.min(thumb_x), fill_x0.max(thumb_x));
        if (a1 - a0) > 1.0 {
            fill_rrect(&mut buf, (a0 + a1) * 0.5, sy, (a1 - a0) * 0.5, 2.5, 2.5, orange, 0.95);
        }
        fill_disc(&mut buf, thumb_x, sy + 1.5, 8.5, c8(40, 30, 20), 0.18);
        fill_disc(&mut buf, thumb_x, sy, 8.0, [1.0; 3], 1.0);
        let v_str = if s.unit == " EV" {
            format!("{:+.2}{}", s.val, s.unit)
        } else if s.unit == "x" {
            format!("{:.2}{}", s.val, s.unit)
        } else if s.unit == " s" || s.unit == " LUFS" || s.unit == " dBTP" || s.unit == " LU" {
            format!("{:.1}{}", s.val, s.unit)
        } else if s.bipolar {
            format!("{:+.0}{}", s.val, s.unit)
        } else {
            format!("{:.0}{}", s.val, s.unit)
        };
        blit_mask_sized(&mut buf, TW, TH, &fit_mask(&v_str, 11, 600, 68), 402.0, sy, c8(72, 52, 38));
    }

    // 6. Segmented Pills Row
    if !st.pills.is_empty() {
        let py = (y_cursor + st.sliders.len() as f64 * 32.0 + 10.0).min(458.0);
        let n = st.pills.len() as f64;
        let pw = (380.0 / n).min(118.0);
        let start_x = 230.0 - (pw * n) * 0.5 + pw * 0.5;
        for (i, label) in st.pills.iter().enumerate() {
            let px = start_x + i as f64 * pw;
            let active = i == st.active_pill;
            fill_rrect(&mut buf, px, py, pw * 0.5 - 3.5, 14.0, 8.0, if active { c8(148, 92, 58) } else { [1.0; 3] }, if active { 0.88 } else { 0.38 });
            blit_mask_sized(&mut buf, TW, TH, &fit_mask(label, 11, 600, pw as usize - 10), px, py, if active { [1.0; 3] } else { c8(58, 42, 30) });
        }
    }

    // 7. Bottom Footer Bar (`Reset` + optional `Preview` + `#FF5200 Apply`)
    fill_rrect(&mut buf, TW as f64 * 0.5, 484.0, TW as f64 * 0.5 - 16.0, 0.5, 0.0, c8(60, 40, 20), 0.12);
    fill_rrect(&mut buf, 64.0, 510.0, 38.0, 15.0, 8.0, c8(90, 64, 48), 0.16);
    blit_mask_sized(&mut buf, TW, TH, &fit_mask("Reset", 12, 600, 64), 64.0, 510.0, c8(58, 42, 30));

    if matches!(
        st.tool_id.as_str(),
        "trimAudio" | "redactAudio" | "normalizeAudio" | "audioChannels" | "trimVideo"
    ) {
        fill_rrect(&mut buf, 212.0, 510.0, 54.0, 15.0, 8.0, c8(90, 64, 48), 0.22);
        stroke_rrect(&mut buf, 212.0, 510.0, 54.0, 15.0, 8.0, 1.0, orange, 0.65);
        blit_mask_sized(&mut buf, TW, TH, &fit_mask("Preview Audio", 11, 700, 96), 212.0, 510.0, c8(58, 42, 30));
    }

    fill_rrect(&mut buf, 376.0, 510.0, 58.0, 16.0, 9.0, orange, 0.96);
    blit_mask_sized(&mut buf, TW, TH, &fit_mask("Apply", 13, 700, 96), 376.0, 510.0, [1.0; 3]);

    buf.iter()
        .map(|p| {
            let q = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u32;
            (q(p[3]) << 24) | (q(p[0]) << 16) | (q(p[1]) << 8) | q(p[2])
        })
        .collect()
}

pub fn render_tool_card(tool_id: &str, paths: &[PathBuf]) -> Vec<u32> {
    let mut st = ModalState::new(tool_id, paths.to_vec());
    if tool_id == "editImage" && st.sliders.len() >= 4 {
        st.sliders[0].val = 0.35;
        st.sliders[2].val = 18.0;
        st.sliders[3].val = 24.0;
    }
    render_modal(&st)
}

pub fn to_modal_image(px: &[u32]) -> RgbaImage {
    RgbaImage::from_fn(TW as u32, TH as u32, |x, y| {
        let v = px[y as usize * TW + x as usize];
        let a = v >> 24;
        let un = |c: u32| if a == 0 { 0 } else { ((c * 255 + a / 2) / a).min(255) as u8 };
        Rgba([un((v >> 16) & 255), un((v >> 8) & 255), un(v & 255), a as u8])
    })
}

fn present_modal(hwnd: HWND) {
    let px = MODAL.with(|c| c.borrow().as_ref().map(render_modal));
    let Some(px) = px else { return };
    let mut d = Dib::sized(TW, TH);
    d.slice().copy_from_slice(&px);
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut rc);
        let pt = POINT { x: rc.left, y: rc.top };
        let sz = SIZE { cx: TW as i32, cy: TH as i32 };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        UpdateLayeredWindow(hwnd, null_mut(), &pt, &sz, d.dc, &src, 0, &blend, ULW_ALPHA);
    }
}

fn update_slider_at(st: &mut ModalState, idx: usize, mx: i32) {
    if let Some(s) = st.sliders.get_mut(idx) {
        let t = ((mx as f32 - 138.0) / (358.0 - 138.0)).clamp(0.0, 1.0);
        s.val = s.min + (s.max - s.min) * t;
    }
}

fn update_canvas_box_at(st: &mut ModalState, mx: i32, my: i32) {
    let pw = st.thumb.width().max(1) as f32;
    let ph = st.thumb.height().max(1) as f32;
    let ox = 230.0 - pw * 0.5;
    let oy = 158.0 - ph * 0.5;
    st.box_center = (
        ((mx as f32 - ox) / pw).clamp(0.1, 0.9),
        ((my as f32 - oy) / ph).clamp(0.1, 0.9),
    );
}

unsafe extern "system" fn modal_proc(hwnd: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let mx = (lp as i16) as i32;
    let my = ((lp >> 16) as i16) as i32;
    match m {
        WM_LBUTTONDOWN => {
            // 1. Close button `x` at (28, 24)
            if (mx - 28) * (mx - 28) + (my - 24) * (my - 24) <= 256 {
                DestroyWindow(hwnd);
                return 0;
            }
            // 2. Apply button (318..434, 494..526)
            if (318..=434).contains(&mx) && (494..=526).contains(&my) {
                MODAL.with(|c| {
                    if let Some(st) = c.borrow_mut().as_mut() {
                        st.applied = Some(st.build_params());
                    }
                });
                DestroyWindow(hwnd);
                return 0;
            }
            // 3. Reset button (26..102, 494..526)
            if (26..=102).contains(&mx) && (494..=526).contains(&my) {
                MODAL.with(|c| {
                    if let Some(st) = c.borrow_mut().as_mut() {
                        let (sliders, pills) = default_controls(&st.tool_id, st.duration);
                        st.sliders = sliders;
                        st.pills = pills;
                        st.active_pill = 0;
                        st.active_swatch = 0;
                        st.box_center = (0.5, 0.5);
                    }
                });
                present_modal(hwnd);
                return 0;
            }
            // 3b. Preview Audio button (158..266, 494..526)
            if (158..=266).contains(&mx) && (494..=526).contains(&my) {
                MODAL.with(|c| {
                    if let Some(st) = c.borrow().as_ref() {
                        if matches!(
                            st.tool_id.as_str(),
                            "trimAudio" | "redactAudio" | "normalizeAudio" | "audioChannels" | "trimVideo"
                        ) {
                            let p = st.build_params();
                            let path = st.paths.first().cloned().unwrap_or_default();
                            preview_media_range(
                                &path,
                                p.start_sec,
                                p.end_sec.unwrap_or(st.duration),
                                st.tool_id == "redactAudio" && p.bleep_tone,
                            );
                        }
                    }
                });
                return 0;
            }
            // 4. Direct timeline scrubbing or canvas box dragging
            let mut changed = false;
            MODAL.with(|c| {
                let mut g = c.borrow_mut();
                let Some(st) = g.as_mut() else { return };
                if st.tool_id == "removeMetadata" && (72..=190).contains(&my) && (170..=418).contains(&mx) {
                    let idx = ((my - 72) / 42).clamp(0, 2) as usize;
                    st.active_meta_field = Some(idx);
                    st.active_pill = 2;
                    changed = true;
                    return;
                }
                if st.tool_id == "organizePDF" && (90..=218).contains(&my) && (56..=404).contains(&mx) {
                    let n = st.pdf_pages.len().min(4).max(1);
                    let idx = (((mx - 56) as usize * n) / 348).min(n - 1);
                    if let Some(r) = st.pdf_rotations.get_mut(idx) {
                        *r = (*r + 90) % 360;
                        changed = true;
                        return;
                    }
                }
                if matches!(
                    st.tool_id.as_str(),
                    "cropImage" | "cropVideo" | "redactImage" | "redactVideo" | "annotateImage"
                ) && (60..=254).contains(&my)
                    && (48..=412).contains(&mx)
                {
                    st.detected_boxes.clear();
                    st.dragging_canvas = true;
                    update_canvas_box_at(st, mx, my);
                    SetCapture(hwnd);
                    changed = true;
                    return;
                }
                if matches!(st.tool_id.as_str(), "trimVideo" | "splitVideo" | "videoSnapshots")
                    && (198..=248).contains(&my)
                    && (50..=410).contains(&mx)
                {
                    let norm = ((mx - 50) as f32 / 360.0).clamp(0.0, 1.0);
                    let target_idx = if st.sliders.len() >= 2 && norm > 0.55 { 1 } else { 0 };
                    if let Some(sl) = st.sliders.get_mut(target_idx) {
                        sl.val = sl.min + (sl.max - sl.min) * norm;
                    }
                    changed = true;
                    return;
                }
                let mut y_cur = 282;
                if st.tool_id == "frameImage" || st.tool_id == "audioToVideo" {
                    if (y_cur - 14..=y_cur + 14).contains(&my) {
                        for i in 0..SWATCHES.len() {
                            let sx = 142 + i as i32 * 38;
                            if (sx - 16..=sx + 16).contains(&mx) {
                                st.active_swatch = i;
                                changed = true;
                            }
                        }
                    }
                    y_cur += 36;
                }
                for idx in 0..st.sliders.len() {
                    let sy = y_cur + idx as i32 * 32;
                    if (sy - 13..=sy + 13).contains(&my) && (128..=368).contains(&mx) {
                        st.dragging_slider = Some(idx);
                        update_slider_at(st, idx, mx);
                        SetCapture(hwnd);
                        changed = true;
                        return;
                    }
                }
                if !st.pills.is_empty() {
                    let py = (y_cur + st.sliders.len() as i32 * 32 + 10).min(458);
                    if (py - 16..=py + 16).contains(&my) {
                        let n = st.pills.len() as f64;
                        let pw = (380.0 / n).min(118.0);
                        let start_x = 230.0 - (pw * n) * 0.5 + pw * 0.5;
                        for i in 0..st.pills.len() {
                            let px = (start_x + i as f64 * pw) as i32;
                            let hw = (pw * 0.5) as i32;
                            if (px - hw..=px + hw).contains(&mx) {
                                st.on_pill_clicked(i);
                                changed = true;
                            }
                        }
                    }
                }
            });
            if changed {
                present_modal(hwnd);
            } else if my < 48 {
                ReleaseCapture();
                SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0);
            }
            0
        }
        WM_MOUSEMOVE => {
            let updated = MODAL.with(|c| {
                let mut g = c.borrow_mut();
                let Some(st) = g.as_mut() else { return false };
                if st.dragging_canvas {
                    update_canvas_box_at(st, mx, my);
                    true
                } else if let Some(idx) = st.dragging_slider {
                    update_slider_at(st, idx, mx);
                    true
                } else {
                    false
                }
            });
            if updated {
                present_modal(hwnd);
            }
            0
        }
        WM_LBUTTONUP => {
            let mut need_redraw = false;
            MODAL.with(|c| {
                if let Some(st) = c.borrow_mut().as_mut() {
                    if st.dragging_canvas {
                        st.dragging_canvas = false;
                        ReleaseCapture();
                    }
                    if let Some(idx) = st.dragging_slider.take() {
                        ReleaseCapture();
                        if matches!(st.tool_id.as_str(), "trimVideo" | "splitVideo" | "videoSnapshots") {
                            if let Some(sl) = st.sliders.get(idx) {
                                let t = sl.val;
                                st.refresh_video_frame_at(t);
                                need_redraw = true;
                            }
                        }
                    }
                }
            });
            if need_redraw {
                present_modal(hwnd);
            }
            0
        }
        WM_KEYDOWN if wp == VK_ESCAPE as usize => {
            DestroyWindow(hwnd);
            0
        }
        WM_CHAR => {
            let ch = (wp as u16) as u8 as char;
            let updated = MODAL.with(|c| {
                let mut g = c.borrow_mut();
                let Some(st) = g.as_mut() else { return false };
                let Some(idx) = st.active_meta_field else { return false };
                if wp == 8 {
                    st.meta_fields[idx].pop();
                    true
                } else if ch >= ' ' && ch <= '~' && st.meta_fields[idx].len() < 42 {
                    st.meta_fields[idx].push(ch);
                    true
                } else {
                    false
                }
            });
            if updated {
                present_modal(hwnd);
            }
            0
        }
        WM_TIMER if wp == TIMER_AUTO_ID => {
            let mut rc: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut rc);
            let (should_apply, cur_xy) = MODAL.with(|c| {
                let mut g = c.borrow_mut();
                let Some(st) = g.as_mut() else { return (false, None) };
                st.auto_step += 1;
                let t = st.auto_step as f32 / st.auto_total_steps.max(1) as f32;
                let y_cur = if st.tool_id == "frameImage" { 318 } else { 282 };
                let xy = if t < 0.78 && !st.sliders.is_empty() {
                    let s0 = &mut st.sliders[0];
                    let norm = 0.5 + 0.36 * ((t / 0.78) * 3.14).sin();
                    s0.val = s0.min + (s0.max - s0.min) * norm;
                    let tx = 138 + (norm * 220.0) as i32;
                    Some((rc.left + tx, rc.top + y_cur))
                } else {
                    Some((rc.left + 376, rc.top + 510))
                };
                if st.sliders.len() > 2 && t < 0.78 {
                    let s2 = &mut st.sliders[2];
                    s2.val = s2.min + (s2.max - s2.min) * (0.5 + 0.30 * (t * 2.4).sin());
                }
                if st.tool_id == "frameImage" && st.auto_step == st.auto_total_steps / 3 {
                    st.active_swatch = 1;
                }
                if st.auto_step >= st.auto_total_steps {
                    st.applied = Some(st.build_params());
                    (true, xy)
                } else {
                    (false, xy)
                }
            });
            if let Some((cx, cy)) = cur_xy {
                SetCursorPos(cx, cy);
            }
            present_modal(hwnd);
            crate::record_real_desktop_frame(1.12);
            if should_apply {
                KillTimer(hwnd, TIMER_AUTO_ID);
                DestroyWindow(hwnd);
            }
            0
        }
        _ => DefWindowProcW(hwnd, m, wp, lp),
    }
}

pub fn run_modal(tool_id: &str, paths: Vec<PathBuf>, auto_apply_ms: Option<u32>) -> Option<ToolParams> {
    let mut st = ModalState::new(tool_id, paths);
    if let Some(ms) = auto_apply_ms {
        st.auto_total_steps = (ms / 40).max(5);
    }
    MODAL.with(|c| *c.borrow_mut() = Some(st));
    unsafe {
        let hi = GetModuleHandleW(null());
        let cls: Vec<u16> = "ClementineToolModal\0".encode_utf16().collect();
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(modal_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hi,
            hIcon: null_mut(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: cls.as_ptr(),
        };
        RegisterClassW(&wc);
        let cx = GetSystemMetrics(SM_CXSCREEN) / 2;
        let cy = GetSystemMetrics(SM_CYSCREEN) / 2;
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST,
            cls.as_ptr(),
            cls.as_ptr(),
            WS_POPUP,
            cx - TW as i32 / 2,
            cy - TH as i32 / 2,
            TW as i32,
            TH as i32,
            null_mut(),
            null_mut(),
            hi,
            null(),
        );
        present_modal(hwnd);
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        if auto_apply_ms.is_some() {
            SetTimer(hwnd, TIMER_AUTO_ID, 40, None);
        }
        let mut msg: MSG = std::mem::zeroed();
        while IsWindow(hwnd) != 0 && GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    MODAL.with(|c| c.borrow_mut().take().and_then(|s| s.applied))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_and_audio_tool_modals_have_distinct_controls_and_params() {
        let mut trim_vid = ModalState::new("trimVideo", vec![]);
        assert_eq!(trim_vid.pills[0], "Full Range");
        trim_vid.on_pill_clicked(1);
        let pv = trim_vid.build_params();
        assert!(pv.start_sec > 0.5 && pv.end_sec.unwrap() < trim_vid.duration);
        assert!(!pv.trim_silence);

        let mut trim_aud = ModalState::new("trimAudio", vec![]);
        assert_eq!(trim_aud.pills[1], "Trim Silent Ends");
        trim_aud.on_pill_clicked(1);
        let pa = trim_aud.build_params();
        assert!(pa.trim_silence);

        let mut spd = ModalState::new("changeVideoSpeed", vec![]);
        spd.on_pill_clicked(0);
        assert_eq!(spd.build_params().speed_factor, 0.5);
        spd.on_pill_clicked(4);
        assert_eq!(spd.build_params().speed_factor, 4.0);

        let mut split = ModalState::new("splitVideo", vec![]);
        split.on_pill_clicked(2); // 3 Equal Clips -> 2 split points
        assert_eq!(split.build_params().split_points.len(), 2);

        let mut norm = ModalState::new("normalizeAudio", vec![]);
        norm.on_pill_clicked(1); // Broadcast (-23)
        assert_eq!(norm.build_params().target_lufs, -23.0);

        let mut ch = ModalState::new("audioChannels", vec![]);
        ch.sliders[0].val = 50.0;
        ch.on_pill_clicked(1); // Stereo
        let pch = ch.build_params();
        assert!(!pch.mono && (pch.left_gain - 0.5).abs() < 0.01);

        let mut crop = ModalState::new("cropImage", vec![]);
        let center_rect = crop.build_params().crop_rect.unwrap();
        update_canvas_box_at(&mut crop, 140, 100);
        let moved_rect = crop.build_params().crop_rect.unwrap();
        assert!(moved_rect.0 < center_rect.0 && moved_rect.1 < center_rect.1);

        let mut up = ModalState::new("upscaleImage", vec![]);
        assert_eq!(up.build_params().collage_cols, 2);
        up.on_pill_clicked(1);
        assert_eq!(up.build_params().collage_cols, 4);

        let ocr = ModalState::new("extractText", vec![]);
        assert!(!ocr.ocr_lines.is_empty());
    }
}
