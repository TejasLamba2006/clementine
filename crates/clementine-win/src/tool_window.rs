//! Phase 2 interactive warm-glass Tool Windows (`460x540` layered Win32 modal).
use crate::renderer::{blit_mask_sized, fit_mask, rounded_box_sdf, seg_dist, Dib, Px};
use clementine_core::convert::image_ops::{apply_frame_bg, apply_photo_edits};
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
    thumb: RgbaImage,
    sliders: Vec<Slider>,
    pills: Vec<&'static str>,
    active_pill: usize,
    active_swatch: usize,
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
        // Sky blue top with warm golden sun/flower in center and green hills at bottom
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

fn load_thumb(paths: &[PathBuf]) -> RgbaImage {
    if let Some(p) = paths.first() {
        if let Ok(img) = image::open(p) {
            return imageops::resize(&img.to_rgba8(), 220, 168, imageops::FilterType::Triangle);
        }
    }
    sample_photo(220, 168)
}

fn default_controls(tool_id: &str) -> (Vec<Slider>, Vec<&'static str>) {
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
                s("Crop Width", 30.0, 100.0, 82.0, false, "%"),
                s("Crop Height", 30.0, 100.0, 82.0, false, "%"),
            ],
            vec!["Free", "1:1", "16:9", "9:16", "4:3"],
        ),
        "redactImage" | "redactVideo" => (
            vec![
                s("Box Width", 20.0, 85.0, 48.0, false, "%"),
                s("Box Height", 15.0, 75.0, 34.0, false, "%"),
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
        "trimAudio" | "trimVideo" | "redactAudio" | "splitVideo" => (
            vec![
                s("Start Time", 0.0, 20.0, 2.5, false, " s"),
                s("End Time", 5.0, 60.0, 38.0, false, " s"),
            ],
            vec!["Keep Pauses", "Trim Silence"],
        ),
        "changeVideoSpeed" => (
            vec![s("Playback Speed", 0.25, 4.0, 2.0, false, "x")],
            vec!["0.5x", "1.25x", "1.5x", "2.0x"],
        ),
        "audioChannels" => (
            vec![
                s("Left Channel", 0.0, 100.0, 100.0, false, "%"),
                s("Right Channel", 0.0, 100.0, 100.0, false, "%"),
            ],
            vec!["Mono", "Stereo"],
        ),
        "normalizeAudio" | "audioToVideo" | "videoSnapshots" | "joinVideos" => (
            vec![s("Target Level", 40.0, 100.0, 85.0, false, "%")],
            vec!["16:9", "9:16", "1:1"],
        ),
        "organizePDF" | "mergePDF" => (
            vec![],
            vec!["Reverse Order", "Keep Order", "First Page"],
        ),
        _ => (
            vec![],
            vec!["Remove All Tags", "Keep Color Profile"],
        ),
    }
}

impl ModalState {
    fn new(tool_id: &str, paths: Vec<PathBuf>) -> Self {
        let title = tool_meta(tool_id).map(|m| m.name.to_string()).unwrap_or_else(|| tool_id.to_string());
        let thumb = load_thumb(&paths);
        let (sliders, pills) = default_controls(tool_id);
        Self {
            tool_id: tool_id.to_string(),
            title,
            thumb,
            sliders,
            pills,
            active_pill: 0,
            active_swatch: 0,
            dragging_slider: None,
            applied: None,
            auto_step: 0,
            auto_total_steps: 0,
        }
    }

    fn build_params(&self) -> ToolParams {
        let mut p = ToolParams::default();
        let val = |i: usize, def: f32| self.sliders.get(i).map(|s| s.val).unwrap_or(def);
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
            "compress" => {
                p.quality = if self.active_pill == 1 { 52 } else { val(0, 74.0).round() as u8 };
                p.scale_pct = val(1, 100.0).round() as u8;
            }
            "cropImage" | "cropVideo" => {
                let (w, h) = (self.thumb.width(), self.thumb.height());
                let cw = ((w as f32 * val(0, 82.0) / 100.0).round() as u32).max(8);
                let ch = ((h as f32 * val(1, 82.0) / 100.0).round() as u32).max(8);
                p.crop_rect = Some(((w - cw) / 2, (h - ch) / 2, cw, ch));
            }
            "redactImage" | "redactVideo" => {
                p.redact_solid = self.active_pill == 1;
            }
            "annotateImage" => {
                p.annotation_color = match self.active_pill {
                    1 => [255, 52, 66, 255],
                    2 => [36, 188, 92, 255],
                    3 => [24, 128, 255, 255],
                    _ => [255, 82, 0, 255],
                };
            }
            "createCollage" => {
                p.collage_cols = val(0, 2.0).round().clamp(1.0, 4.0) as u32;
                p.corner_radius = val(1, 18.0).round() as u32;
            }
            "trimAudio" | "trimVideo" | "redactAudio" | "splitVideo" => {
                p.start_sec = val(0, 2.5);
                p.end_sec = Some(val(1, 38.0).max(p.start_sec + 1.0));
            }
            "changeVideoSpeed" => {
                p.speed_factor = val(0, 2.0);
            }
            "audioChannels" => {
                p.mono = self.active_pill == 0;
            }
            "organizePDF" => {
                p.page_order = if self.active_pill == 2 { vec![0] } else { vec![] };
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

fn render_modal(st: &ModalState) -> Vec<u32> {
    let mut buf = vec![[0f32; 4]; TW * TH];
    let orange = c8(255, 82, 0);

    // 1. Warm frosted glass card surface (matching Tangerine press screenshots)
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

    // 3. Live Preview Area (y = 58..258)
    let params = st.build_params();
    match st.tool_id.as_str() {
        "trimAudio" | "trimVideo" | "redactAudio" | "normalizeAudio" | "audioChannels"
        | "audioToVideo" | "changeVideoSpeed" | "splitVideo" | "videoSnapshots" | "joinVideos" => {
            fill_rrect(&mut buf, 230.0, 158.0, 202.0, 92.0, 14.0, c8(255, 234, 208), 0.52);
            stroke_rrect(&mut buf, 230.0, 158.0, 190.0, 62.0, 8.0, 2.0, orange, 0.90);
            let s_norm = (st.sliders.first().map(|s| (s.val - s.min) / (s.max - s.min)).unwrap_or(0.08)).clamp(0.0, 0.85);
            let e_norm = (st.sliders.get(1).map(|s| (s.val - s.min) / (s.max - s.min)).unwrap_or(0.88)).clamp(s_norm + 0.1, 1.0);
            for i in 0..46 {
                let t = i as f32 / 45.0;
                let bx = 52.0 + t as f64 * 356.0;
                let h = 10.0 + ((i as f32 * 0.65).sin().abs() * (i as f32 * 0.19).cos().abs()) as f64 * 42.0;
                let active = t >= s_norm && t <= e_norm;
                fill_rrect(&mut buf, bx, 158.0, 2.0, h, 2.0, if active { orange } else { c8(150, 118, 96) }, if active { 0.92 } else { 0.42 });
            }
            let lx = 48.0 + s_norm as f64 * 364.0;
            let rx = 48.0 + e_norm as f64 * 364.0;
            fill_rrect(&mut buf, lx, 158.0, 5.0, 62.0, 4.0, orange, 1.0);
            fill_rrect(&mut buf, rx, 158.0, 5.0, 62.0, 4.0, orange, 1.0);
            blit_mask_sized(&mut buf, TW, TH, &fit_mask(&format!("Start {:.1}s", params.start_sec), 11, 600, 120), 86.0, 234.0, c8(72, 48, 30));
            blit_mask_sized(&mut buf, TW, TH, &fit_mask(&format!("End {:.1}s", params.end_sec.unwrap_or(38.0)), 11, 600, 120), 374.0, 234.0, c8(72, 48, 30));
        }
        "removeMetadata" => {
            fill_rrect(&mut buf, 230.0, 158.0, 202.0, 94.0, 14.0, [1.0; 3], 0.78);
            let rows = [
                ("Exif Image Dimensions", "1600 x 1200 px"),
                ("Color Space Profile", "sRGB IEC61966-2.1"),
                ("Camera Make & Lens", "Stripped on Save"),
                ("GPS Latitude / Longitude", "Stripped on Save"),
                ("IPTC / XMP Digest", "Stripped on Save"),
            ];
            for (i, (k, v)) in rows.iter().enumerate() {
                let ry = 84.0 + i as f64 * 34.0;
                if i > 0 {
                    fill_rrect(&mut buf, 230.0, ry - 16.0, 184.0, 0.5, 0.0, c8(120, 110, 100), 0.16);
                }
                blit_mask_sized(&mut buf, TW, TH, &fit_mask(k, 11, 600, 180), 130.0, ry, c8(42, 36, 32));
                blit_mask_sized(&mut buf, TW, TH, &fit_mask(v, 11, 500, 160), 336.0, ry, orange);
            }
        }
        "organizePDF" | "mergePDF" => {
            fill_rrect(&mut buf, 230.0, 158.0, 202.0, 94.0, 14.0, c8(255, 245, 235), 0.55);
            let order = if st.active_pill == 0 { [2, 1, 3] } else { [1, 2, 3] };
            for (i, pg) in order.iter().enumerate() {
                let px = 114.0 + i as f64 * 116.0;
                fill_rrect(&mut buf, px, 158.0, 44.0, 62.0, 8.0, [1.0; 3], 0.95);
                fill_disc(&mut buf, px - 24.0, 114.0, 11.0, orange, 1.0);
                blit_mask_sized(&mut buf, TW, TH, &fit_mask(&pg.to_string(), 11, 700, 24), px - 24.0, 114.0, [1.0; 3]);
                for ln in 0..4 {
                    fill_rrect(&mut buf, px, 142.0 + ln as f64 * 12.0, 28.0, 1.5, 1.0, c8(190, 185, 180), 0.8);
                }
            }
        }
        "frameImage" => {
            let sw = SWATCHES[st.active_swatch % SWATCHES.len()];
            fill_rrect(&mut buf, 230.0, 158.0, 148.0, 94.0, 14.0, c8(sw[0], sw[1], sw[2]), 0.96);
            let pad = (params.padding as i32 / 3).clamp(4, 34);
            let inner = imageops::resize(&st.thumb, (240 - pad as u32 * 2).max(60), (160 - pad as u32 * 2).max(40), imageops::FilterType::Triangle);
            blit_rgba(&mut buf, &inner, 230 - inner.width() as i32 / 2, 158 - inner.height() as i32 / 2, (params.corner_radius as f64 * 0.5).clamp(2.0, 22.0));
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
            if st.tool_id == "cropImage" || st.tool_id == "cropVideo" {
                let cw = preview.width() as f64 * (st.sliders.first().map(|s| s.val).unwrap_or(82.0) as f64 / 100.0) * 0.5;
                let ch = preview.height() as f64 * (st.sliders.get(1).map(|s| s.val).unwrap_or(82.0) as f64 / 100.0) * 0.5;
                stroke_rrect(&mut buf, 230.0, 158.0, cw, ch, 2.0, 2.5, orange, 1.0);
                for (dx, dy) in [(-cw, -ch), (cw, -ch), (-cw, ch), (cw, ch)] {
                    fill_rrect(&mut buf, 230.0 + dx, 158.0 + dy, 4.5, 4.5, 2.0, orange, 1.0);
                }
            } else if st.tool_id == "redactImage" || st.tool_id == "redactVideo" {
                let col = if st.active_pill == 1 { c8(8, 8, 8) } else { c8(90, 110, 120) };
                fill_rrect(&mut buf, 230.0, 152.0, 52.0, 26.0, 4.0, col, 0.94);
                stroke_rrect(&mut buf, 230.0, 152.0, 52.0, 26.0, 4.0, 2.2, orange, 1.0);
            } else if st.tool_id == "annotateImage" {
                let ac = c8(params.annotation_color[0], params.annotation_color[1], params.annotation_color[2]);
                stroke_rrect(&mut buf, 230.0, 152.0, 56.0, 34.0, 6.0, 3.0, ac, 1.0);
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

    // 4. Swatches row for Add BG (`frameImage`)
    let mut y_cursor = 282.0;
    if st.tool_id == "frameImage" {
        blit_mask_sized(&mut buf, TW, TH, &fit_mask("Background", 12, 600, 90), 66.0, y_cursor, c8(68, 44, 28));
        for (i, sw) in SWATCHES.iter().enumerate() {
            let sx = 142.0 + i as f64 * 38.0;
            fill_rrect(&mut buf, sx, y_cursor, 15.0, 12.0, 6.0, c8(sw[0], sw[1], sw[2]), 1.0);
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
        let v_str = if s.unit == " EV" || s.unit == "x" {
            format!("{:+.2}{}", s.val, s.unit)
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
        let pw = (360.0 / n).min(108.0);
        let start_x = 230.0 - (pw * n) * 0.5 + pw * 0.5;
        for (i, label) in st.pills.iter().enumerate() {
            let px = start_x + i as f64 * pw;
            let active = i == st.active_pill;
            fill_rrect(&mut buf, px, py, pw * 0.5 - 4.0, 14.0, 8.0, if active { c8(148, 92, 58) } else { [1.0; 3] }, if active { 0.85 } else { 0.38 });
            blit_mask_sized(&mut buf, TW, TH, &fit_mask(label, 11, 600, pw as usize - 12), px, py, if active { [1.0; 3] } else { c8(58, 42, 30) });
        }
    }

    // 7. Bottom Footer Bar (`Reset` + `#FF5200 Apply`)
    fill_rrect(&mut buf, TW as f64 * 0.5, 484.0, TW as f64 * 0.5 - 16.0, 0.5, 0.0, c8(60, 40, 20), 0.12);
    fill_rrect(&mut buf, 64.0, 510.0, 38.0, 15.0, 8.0, c8(90, 64, 48), 0.16);
    blit_mask_sized(&mut buf, TW, TH, &fit_mask("Reset", 12, 600, 64), 64.0, 510.0, c8(58, 42, 30));

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
                        let (sliders, pills) = default_controls(&st.tool_id);
                        st.sliders = sliders;
                        st.pills = pills;
                        st.active_pill = 0;
                        st.active_swatch = 0;
                    }
                });
                present_modal(hwnd);
                return 0;
            }
            // 4. Swatches / Sliders / Pills
            let mut changed = false;
            MODAL.with(|c| {
                let mut g = c.borrow_mut();
                let Some(st) = g.as_mut() else { return };
                let mut y_cur = 282;
                if st.tool_id == "frameImage" {
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
                        let pw = (360.0 / n).min(108.0);
                        let start_x = 230.0 - (pw * n) * 0.5 + pw * 0.5;
                        for i in 0..st.pills.len() {
                            let px = (start_x + i as f64 * pw) as i32;
                            let hw = (pw * 0.5) as i32;
                            if (px - hw..=px + hw).contains(&mx) {
                                st.active_pill = i;
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
                if let Some(idx) = st.dragging_slider {
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
            MODAL.with(|c| {
                if let Some(st) = c.borrow_mut().as_mut() {
                    if st.dragging_slider.take().is_some() {
                        ReleaseCapture();
                    }
                }
            });
            0
        }
        WM_KEYDOWN if wp == VK_ESCAPE as usize => {
            DestroyWindow(hwnd);
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
            crate::record_real_desktop_frame(1.16);
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
