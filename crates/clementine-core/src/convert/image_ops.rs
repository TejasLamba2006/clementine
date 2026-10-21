use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::imageops::{self, FilterType as Resize};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};

use super::doc_ops::{build_pdf, PdfPage};
use super::{ClementineError, Result, ToolParams};

pub fn is_image_ext(ext: &str) -> bool {
    matches!(
        ext,
        "jpg" | "jpeg" | "png" | "webp" | "bmp" | "tiff" | "tif" | "gif" | "ico" | "svg" | "heic" | "heif" | "avif"
    )
}

fn ie(e: impl std::fmt::Display) -> ClementineError {
    ClementineError::ImageError(e.to_string())
}

// ---------------------------------------------------------------- SVG input

fn attr(tag: &str, name: &str) -> Option<String> {
    let pat = format!(" {name}=\"");
    let i = tag.find(&pat)? + pat.len();
    let j = tag[i..].find('"')?;
    Some(tag[i..i + j].to_string())
}

fn num(s: &str) -> f32 {
    let end = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(s.len());
    s[..end].parse().unwrap_or(0.0)
}

fn color(s: &str) -> Option<[u8; 4]> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix('#') {
        let v = |i: usize, n: usize| u8::from_str_radix(&h[i..i + n], 16).ok();
        return match h.len() {
            3 => Some([v(0, 1)? * 17, v(1, 1)? * 17, v(2, 1)? * 17, 255]),
            6 => Some([v(0, 2)?, v(2, 2)?, v(4, 2)?, 255]),
            _ => None,
        };
    }
    match s {
        "black" => Some([0, 0, 0, 255]),
        "white" => Some([255, 255, 255, 255]),
        "red" => Some([255, 0, 0, 255]),
        "green" => Some([0, 128, 0, 255]),
        "blue" => Some([0, 0, 255, 255]),
        "yellow" => Some([255, 255, 0, 255]),
        "gray" | "grey" => Some([128, 128, 128, 255]),
        _ => None,
    }
}

fn path_num(b: &[char], i: &mut usize) -> Option<f32> {
    while *i < b.len() && (b[*i].is_whitespace() || b[*i] == ',') {
        *i += 1;
    }
    let s = *i;
    if *i < b.len() && (b[*i] == '-' || b[*i] == '+') {
        *i += 1;
    }
    while *i < b.len() && (b[*i].is_ascii_digit() || b[*i] == '.') {
        *i += 1;
    }
    b[s..*i].iter().collect::<String>().parse().ok()
}

/// Parses `M L H V Z` (plus lowercase relative forms) into polygons.
fn parse_path(d: &str) -> Vec<Vec<(f32, f32)>> {
    let b: Vec<char> = d.chars().collect();
    let (mut polys, mut cur) = (Vec::new(), Vec::new());
    let (mut pt, mut start, mut cmd, mut i) = ((0.0f32, 0.0f32), (0.0f32, 0.0f32), ' ', 0);
    while i < b.len() {
        if b[i].is_whitespace() || b[i] == ',' {
            i += 1;
        } else if b[i].is_ascii_alphabetic() {
            cmd = b[i];
            i += 1;
            if cmd.eq_ignore_ascii_case(&'z') {
                pt = start;
                if !cur.is_empty() {
                    polys.push(std::mem::take(&mut cur));
                }
            }
        } else {
            let rel = cmd.is_ascii_lowercase();
            let o = if rel { pt } else { (0.0, 0.0) };
            let Some(n) = path_num(&b, &mut i) else {
                i += 1;
                continue;
            };
            match cmd.to_ascii_uppercase() {
                'H' => pt.0 = o.0 + n,
                'V' => pt.1 = o.1 + n,
                'M' | 'L' => {
                    let Some(y) = path_num(&b, &mut i) else { break };
                    pt = (o.0 + n, o.1 + y);
                    if cmd.eq_ignore_ascii_case(&'m') {
                        if !cur.is_empty() {
                            polys.push(std::mem::take(&mut cur));
                        }
                        start = pt;
                        cmd = if rel { 'l' } else { 'L' };
                    }
                }
                _ => continue,
            }
            cur.push(pt);
        }
    }
    if !cur.is_empty() {
        polys.push(cur);
    }
    polys
}

/// Even-odd scanline fill.
fn fill_path(img: &mut RgbaImage, d: &str, fill: [u8; 4], vx: f32, vy: f32) {
    let polys = parse_path(d);
    for y in 0..img.height() {
        let yc = y as f32 + vy + 0.5;
        let mut xs = Vec::new();
        for poly in &polys {
            for (k, a) in poly.iter().enumerate() {
                let b = poly[(k + 1) % poly.len()];
                if (a.1 <= yc) != (b.1 <= yc) {
                    xs.push(a.0 + (yc - a.1) / (b.1 - a.1) * (b.0 - a.0));
                }
            }
        }
        xs.sort_by(|a, b| a.total_cmp(b));
        for pair in xs.chunks_exact(2) {
            for x in 0..img.width() {
                let xc = x as f32 + vx + 0.5;
                if xc >= pair[0] && xc < pair[1] {
                    img.put_pixel(x, y, Rgba(fill));
                }
            }
        }
    }
}

/// ponytail: rasterizes `<rect>`, `<circle>`, `<ellipse>` and straight-line
/// `<path>` (M/L/H/V/Z) with solid fills; add `resvg` for curves, gradients
/// or text.
fn render_svg(text: &str) -> Result<RgbaImage> {
    let svg_start = text.find("<svg").ok_or_else(|| ie("not an svg document"))?;
    let root_end = text[svg_start..].find('>').ok_or_else(|| ie("malformed svg"))? + svg_start;
    let root = &text[svg_start..root_end];
    let (vx, vy, vw, vh) = match attr(root, "viewBox") {
        Some(vb) => {
            let n: Vec<f32> = vb.split([' ', ',']).filter(|s| !s.is_empty()).map(num).collect();
            if n.len() != 4 {
                return Err(ie("bad viewBox"));
            }
            (n[0], n[1], n[2], n[3])
        }
        None => (
            0.0,
            0.0,
            attr(root, "width").map(|w| num(&w)).unwrap_or(300.0),
            attr(root, "height").map(|h| num(&h)).unwrap_or(150.0),
        ),
    };
    if vw <= 0.0 || vh <= 0.0 || vw > 8192.0 || vh > 8192.0 {
        return Err(ie("svg size out of range"));
    }
    let (w, h) = (vw.ceil() as u32, vh.ceil() as u32);
    let mut img = RgbaImage::new(w, h);
    for tag in text[root_end..].split('<').skip(1) {
        let kind = tag.split(|c: char| c.is_whitespace() || c == '/' || c == '>').next().unwrap_or("");
        let fill = match attr(tag, "fill") {
            Some(f) if f == "none" => continue,
            Some(f) => color(&f).unwrap_or([0, 0, 0, 255]),
            None => [0, 0, 0, 255],
        };
        if kind == "path" {
            if let Some(d) = attr(tag, "d") {
                fill_path(&mut img, &d, fill, vx, vy);
            }
            continue;
        }
        let g = |n: &str| attr(tag, n).map(|v| num(&v)).unwrap_or(0.0);
        let inside: Box<dyn Fn(f32, f32) -> bool> = match kind {
            "rect" => {
                let (x, y, rw, rh) = (g("x"), g("y"), g("width"), g("height"));
                Box::new(move |px, py| px >= x && px < x + rw && py >= y && py < y + rh)
            }
            "circle" => {
                let (cx, cy, r) = (g("cx"), g("cy"), g("r"));
                Box::new(move |px, py| (px - cx).powi(2) + (py - cy).powi(2) <= r * r)
            }
            "ellipse" => {
                let (cx, cy, rx, ry) = (g("cx"), g("cy"), g("rx").max(0.001), g("ry").max(0.001));
                Box::new(move |px, py| ((px - cx) / rx).powi(2) + ((py - cy) / ry).powi(2) <= 1.0)
            }
            _ => continue,
        };
        for (x, y, p) in img.enumerate_pixels_mut() {
            if inside(x as f32 + vx + 0.5, y as f32 + vy + 0.5) {
                *p = Rgba(fill);
            }
        }
    }
    Ok(img)
}

// ------------------------------------------------------------ load / encode

fn load(path: &Path) -> Result<DynamicImage> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
    if ext.as_deref() == Some("svg") {
        return Ok(DynamicImage::ImageRgba8(render_svg(&fs::read_to_string(path)?)?));
    }
    if let Ok(img) = image::open(path) {
        return Ok(img);
    }
    if let Some(rgba) = super::media_ops::extract_video_frame_rgba(path, 0.0, 4096, 4096) {
        return Ok(DynamicImage::ImageRgba8(rgba));
    }
    image::open(path).map_err(ie)
}

fn flatten_white(img: &DynamicImage) -> image::RgbImage {
    let rgba = img.to_rgba8();
    image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let Rgba([r, g, b, a]) = *rgba.get_pixel(x, y);
        let mix = |c: u8| ((c as u32 * a as u32 + 255 * (255 - a as u32)) / 255) as u8;
        image::Rgb([mix(r), mix(g), mix(b)])
    })
}

fn encode_jpeg(img: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    JpegEncoder::new_with_quality(&mut buf, quality.clamp(1, 100))
        .encode_image(&flatten_white(img))
        .map_err(ie)?;
    Ok(buf)
}

fn image_page(img: &DynamicImage) -> Result<PdfPage> {
    let (w, h) = (img.width(), img.height());
    let jpeg = encode_jpeg(img, 90)?;
    Ok(PdfPage {
        w: w as f32,
        h: h as f32,
        content: format!("q {w} 0 0 {h} 0 0 cm /Im0 Do Q").into_bytes(),
        jpeg: Some((jpeg, w, h)),
    })
}

pub(crate) fn write_image_pub(img: &DynamicImage, ext: &str, out: &Path, quality: u8) -> Result<()> {
    write_image(img, ext, out, quality)
}

fn write_image(img: &DynamicImage, ext: &str, out: &Path, quality: u8) -> Result<()> {
    match ext {
        "jpg" | "jpeg" => fs::write(out, encode_jpeg(img, quality)?)?,
        "pdf" => fs::write(out, build_pdf(&[image_page(img)?]))?,
        "png" => {
            let mut buf = Vec::new();
            let enc = PngEncoder::new_with_quality(&mut buf, CompressionType::Best, FilterType::Adaptive);
            DynamicImage::ImageRgba8(img.to_rgba8()).write_with_encoder(enc).map_err(ie)?;
            fs::write(out, buf)?;
        }
        "webp" | "bmp" | "tiff" | "tif" | "gif" | "ico" => {
            let fmt = ImageFormat::from_extension(ext).ok_or_else(|| ie("unknown format"))?;
            let mut rgba = DynamicImage::ImageRgba8(img.to_rgba8());
            if ext == "ico" && (rgba.width() > 256 || rgba.height() > 256) {
                rgba = rgba.resize(256, 256, Resize::Lanczos3);
            }
            let mut buf = Cursor::new(Vec::new());
            rgba.write_to(&mut buf, fmt).map_err(ie)?;
            fs::write(out, buf.into_inner())?;
        }
        "heic" | "heif" | "avif" => {
            let mut buf = Cursor::new(Vec::new());
            DynamicImage::ImageRgba8(img.to_rgba8())
                .write_to(&mut buf, ImageFormat::WebP)
                .map_err(ie)?;
            fs::write(out, buf.into_inner())?;
        }
        other => return Err(ie(format!("cannot write .{other}"))),
    }
    Ok(())
}

pub fn convert(path: &Path, src: &str, target: &str, out: &Path) -> Result<()> {
    if !is_image_ext(target) && target != "pdf" || target == "svg" {
        return Err(ClementineError::UnsupportedConversion { from: src.into(), to: target.into() });
    }
    write_image(&load(path)?, target, out, 90)
}

// -------------------------------------------------------------------- tools

fn inside_round(x: u32, y: u32, w: u32, h: u32, r: u32) -> bool {
    let r = r.min(w / 2).min(h / 2) as f32;
    if r == 0.0 {
        return true;
    }
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    let dx = px - px.clamp(r, w as f32 - r);
    let dy = py - py.clamp(r, h as f32 - r);
    dx * dx + dy * dy <= r * r
}

fn round_corners(img: &mut RgbaImage, r: u32) {
    let (w, h) = img.dimensions();
    for (x, y, p) in img.enumerate_pixels_mut() {
        if !inside_round(x, y, w, h, r) {
            p.0[3] = 0;
        }
    }
}

fn crop(img: &DynamicImage, rect: Option<(u32, u32, u32, u32)>) -> Result<DynamicImage> {
    let (iw, ih) = (img.width(), img.height());
    let (x, y, w, h) = rect.unwrap_or((iw / 10, ih / 10, iw * 8 / 10, ih * 8 / 10));
    let (x, y) = (x.min(iw.saturating_sub(1)), y.min(ih.saturating_sub(1)));
    let (w, h) = (w.min(iw - x), h.min(ih - y));
    if w == 0 || h == 0 {
        return Err(ClementineError::InvalidInput("empty crop area".into()));
    }
    Ok(img.crop_imm(x, y, w, h))
}

/// Exposure, contrast, saturation, brightness, temperature and tint in one pass.
pub fn apply_photo_edits(img: &RgbaImage, p: &ToolParams) -> RgbaImage {
    let gain = 2f32.powf(p.exposure);
    let mut out = img.clone();
    for px in out.pixels_mut() {
        let mut c = [px.0[0] as f32 * gain, px.0[1] as f32 * gain, px.0[2] as f32 * gain];
        for v in &mut c {
            *v = (*v - 128.0) * p.contrast + 128.0 + p.brightness * 255.0;
        }
        c[0] += p.temperature * 40.0;
        c[2] -= p.temperature * 40.0;
        c[1] += p.tint * 40.0;
        let luma = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
        for (i, v) in c.iter().enumerate() {
            px.0[i] = (luma + (v - luma) * p.saturation).clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// Returns the local path to the cached `u2netp.onnx` (4.5 MB Silueta / U2Netp portrait & object segmentation model).
pub fn onnx_bg_model_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Clementine").join("models").join("u2netp.onnx")
}

/// Downloads `u2netp.onnx` (4.5 MB) to `%LOCALAPPDATA%\Clementine\models\u2netp.onnx` if not already cached.
pub fn ensure_onnx_bg_model() -> Result<PathBuf> {
    let dest = onnx_bg_model_path();
    if dest.is_file() && dest.metadata().map(|m| m.len() > 100_000).unwrap_or(false) {
        return Ok(dest);
    }
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let url = "https://github.com/danielgatis/rembg/releases/download/v0.0.0/u2netp.onnx";
    let ps = format!(
        "$ProgressPreference='SilentlyContinue'; Invoke-WebRequest -Uri '{}' -OutFile '{}' -TimeoutSec 45",
        url,
        dest.display().to_string().replace('\'', "''")
    );
    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &ps]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let st = cmd.status().map_err(ie)?;
    if st.success() && dest.is_file() {
        Ok(dest)
    } else {
        Err(ie("failed to download local u2netp.onnx model"))
    }
}

/// Runs pure-Rust `tract-onnx` inference on `u2netp.onnx` (320x320 RGB normalized tensor -> 320x320 saliency probability map).
fn remove_background_onnx(img: &RgbaImage, model_path: &Path, p: &ToolParams) -> Option<RgbaImage> {
    use tract_onnx::prelude::*;
    let model = tract_onnx::onnx()
        .model_for_path(model_path)
        .ok()?
        .with_input_fact(0, f32::fact([1, 3, 320, 320]).into())
        .ok()?
        .into_optimized()
        .ok()?
        .into_runnable()
        .ok()?;

    let resized = imageops::resize(img, 320, 320, Resize::Triangle);
    let mean = [0.485f32, 0.456, 0.406];
    let std = [0.229f32, 0.224, 0.225];
    let mut input = tract_ndarray::Array4::<f32>::zeros((1, 3, 320, 320));
    for y in 0..320usize {
        for x in 0..320usize {
            let px = resized.get_pixel(x as u32, y as u32).0;
            for c in 0..3 {
                let v = (px[c] as f32 / 255.0 - mean[c]) / std[c];
                input[[0, c, y, x]] = v;
            }
        }
    }
    let outputs = model.run(tvec!(input.into_tensor().into())).ok()?;
    let mask_view = outputs[0].to_array_view::<f32>().ok()?;
    let slice = mask_view.as_slice()?;
    if slice.len() < 320 * 320 {
        return None;
    }
    let mut min_v = f32::MAX;
    let mut max_v = f32::MIN;
    for &v in &slice[..320 * 320] {
        if v < min_v {
            min_v = v;
        }
        if v > max_v {
            max_v = v;
        }
    }
    let span = (max_v - min_v).max(1e-5);
    let cutoff = ((100 - p.quality.clamp(10, 90)) as f32 / 100.0).clamp(0.15, 0.85);
    let feather = (p.padding.clamp(1, 50) as f32 / 100.0).clamp(0.02, 0.45);
    let mut mask_img = image::GrayImage::new(320, 320);
    for y in 0..320u32 {
        for x in 0..320u32 {
            let prob = (slice[(y * 320 + x) as usize] - min_v) / span;
            let a = ((prob - (cutoff - feather * 0.5)) / feather).clamp(0.0, 1.0);
            mask_img.put_pixel(x, y, image::Luma([(a * 255.0).round() as u8]));
        }
    }
    let full_mask = imageops::resize(&mask_img, img.width(), img.height(), Resize::Lanczos3);
    let mut out = img.clone();
    for (x, y, px) in out.enumerate_pixels_mut() {
        let m = full_mask.get_pixel(x, y).0[0] as u32;
        px.0[3] = ((px.0[3] as u32 * m) / 255) as u8;
    }
    Some(out)
}

/// 100% local background removal: uses neural `u2netp.onnx` via `tract-onnx` when cached (or when `p.redact_solid` requests AI mode),
/// falling back to border-floodfill + perceptual chroma/luma edge matting.
pub fn remove_background_local(img: &RgbaImage, p: &ToolParams) -> RgbaImage {
    let model_path = onnx_bg_model_path();
    if model_path.is_file() {
        if let Some(neural) = remove_background_onnx(img, &model_path, p) {
            return neural;
        }
    } else if p.redact_solid {
        if let Ok(downloaded) = ensure_onnx_bg_model() {
            if let Some(neural) = remove_background_onnx(img, &downloaded, p) {
                return neural;
            }
        }
    }
    let (w, h) = img.dimensions();
    if w < 2 || h < 2 {
        return img.clone();
    }
    let tol = (p.quality.clamp(8, 95) as f32) * 1.45;
    let feather = (p.padding.clamp(1, 60) as f32) * 1.25;

    // 1. Sample perimeter border pixels to build background reference colors
    let mut samples: Vec<[f32; 3]> = Vec::new();
    let step_x = (w / 12).max(1);
    let step_y = (h / 12).max(1);
    for x in (0..w).step_by(step_x as usize) {
        for y in [0, h - 1] {
            let px = img.get_pixel(x, y).0;
            samples.push([px[0] as f32, px[1] as f32, px[2] as f32]);
        }
    }
    for y in (0..h).step_by(step_y as usize) {
        for x in [0, w - 1] {
            let px = img.get_pixel(x, y).0;
            samples.push([px[0] as f32, px[1] as f32, px[2] as f32]);
        }
    }

    let color_dist = |a: [u8; 4], b: [f32; 3]| -> f32 {
        let rm = (a[0] as f32 + b[0]) * 0.5 / 255.0;
        let dr = a[0] as f32 - b[0];
        let dg = a[1] as f32 - b[1];
        let db = a[2] as f32 - b[2];
        (((2.0 + rm) * dr * dr + 4.0 * dg * dg + (3.0 - rm) * db * db) / 9.0).sqrt()
    };

    let min_bg_dist = |px: [u8; 4]| -> f32 {
        samples
            .iter()
            .map(|&s| color_dist(px, s))
            .fold(f32::MAX, f32::min)
    };

    // 2. Flood-fill connected background from image borders
    let n = (w * h) as usize;
    let mut visited = vec![false; n];
    let mut is_bg = vec![false; n];
    let mut queue = std::collections::VecDeque::new();

    for x in 0..w {
        for y in [0, h - 1] {
            let idx = (y * w + x) as usize;
            if !visited[idx] {
                visited[idx] = true;
                if min_bg_dist(img.get_pixel(x, y).0) <= tol + feather * 0.5 {
                    is_bg[idx] = true;
                    queue.push_back((x, y));
                }
            }
        }
    }
    for y in 0..h {
        for x in [0, w - 1] {
            let idx = (y * w + x) as usize;
            if !visited[idx] {
                visited[idx] = true;
                if min_bg_dist(img.get_pixel(x, y).0) <= tol + feather * 0.5 {
                    is_bg[idx] = true;
                    queue.push_back((x, y));
                }
            }
        }
    }

    while let Some((cx, cy)) = queue.pop_front() {
        let cur_px = img.get_pixel(cx, cy).0;
        let cur_f = [cur_px[0] as f32, cur_px[1] as f32, cur_px[2] as f32];
        for (dx, dy) in [(-1i32, 0), (1, 0), (0, -1), (0, 1)] {
            let (nx, ny) = (cx as i32 + dx, cy as i32 + dy);
            if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                continue;
            }
            let (nx, ny) = (nx as u32, ny as u32);
            let idx = (ny * w + nx) as usize;
            if visited[idx] {
                continue;
            }
            let npx = img.get_pixel(nx, ny).0;
            let d_global = min_bg_dist(npx);
            let d_step = color_dist(npx, cur_f);
            if d_global <= tol && d_step <= 28.0 {
                visited[idx] = true;
                is_bg[idx] = true;
                queue.push_back((nx, ny));
            }
        }
    }

    // 3. Compute soft alpha matte with edge feathering
    let mut out = img.clone();
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;
            let px = out.get_pixel_mut(x, y);
            let d = min_bg_dist(px.0);
            if is_bg[idx] {
                if d <= tol * 0.75 {
                    px.0[3] = 0;
                } else {
                    let t = ((d - tol * 0.75) / feather.max(1.0)).clamp(0.0, 1.0);
                    px.0[3] = ((px.0[3] as f32) * t * t).round() as u8;
                }
            } else if d < tol + feather {
                // Check if adjacent to background for smooth anti-aliased boundary
                let mut adj_bg = false;
                for (dx, dy) in [(-1i32, 0), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx >= 0 && ny >= 0 && nx < w as i32 && ny < h as i32 {
                        if is_bg[(ny as u32 * w + nx as u32) as usize] {
                            adj_bg = true;
                            break;
                        }
                    }
                }
                if adj_bg {
                    let t = ((d - tol * 0.6) / (feather * 1.4).max(1.0)).clamp(0.15, 1.0);
                    px.0[3] = ((px.0[3] as f32) * t).round() as u8;
                }
            }
        }
    }
    out
}

/// 100% local Lanczos3 super-resolution (`2x` / `4x`) + Unsharp Mask + luminance CAS (Contrast Adaptive Sharpening).
/// `p.collage_cols` = scale factor (`2` or `4`, or `1` for sharpen-only), `p.quality` = sharpness (`0..=100`), `p.padding` = denoise (`0..=40`).
pub fn apply_upscale_sharpen(img: &RgbaImage, p: &ToolParams) -> RgbaImage {
    let scale = p.collage_cols.clamp(1, 4);
    let (w, h) = img.dimensions();
    // Cap max output dimension at 7680px (8K) so memory stays bounded
    let target_w = (w * scale).min(7680).max(1);
    let target_h = (h * scale).min(7680).max(1);
    let mut base = if scale > 1 {
        imageops::resize(img, target_w, target_h, Resize::Lanczos3)
    } else {
        img.clone()
    };

    // Optional light 3x3 bilateral-style luma denoise pass before sharpening
    if p.padding > 4 {
        let strength = (p.padding.min(40) as f32) / 100.0;
        let copy = base.clone();
        let (bw, bh) = base.dimensions();
        for y in 1..bh.saturating_sub(1) {
            for x in 1..bw.saturating_sub(1) {
                let c = copy.get_pixel(x, y).0;
                let mut sum = [0f32; 3];
                let mut wsum = 0f32;
                for (dx, dy) in [(-1i32, 0), (1, 0), (0, -1), (0, 1), (0, 0)] {
                    let np = copy.get_pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32).0;
                    let diff = (c[0] as f32 - np[0] as f32).abs()
                        + (c[1] as f32 - np[1] as f32).abs()
                        + (c[2] as f32 - np[2] as f32).abs();
                    let wt = (-diff / 42.0).exp();
                    for k in 0..3 {
                        sum[k] += np[k] as f32 * wt;
                    }
                    wsum += wt;
                }
                let out_px = base.get_pixel_mut(x, y);
                for k in 0..3 {
                    let avg = sum[k] / wsum.max(1e-4);
                    out_px.0[k] = (c[k] as f32 * (1.0 - strength) + avg * strength).clamp(0.0, 255.0) as u8;
                }
            }
        }
    }

    // Contrast-Adaptive Unsharp Mask (CAS)
    let sharp = (p.quality as f32 / 100.0).clamp(0.0, 1.5);
    if sharp > 0.02 {
        let blurred = imageops::blur(&base, 1.15);
        let amount = sharp * 1.35;
        for (x, y, px) in base.enumerate_pixels_mut() {
            let b = blurred.get_pixel(x, y).0;
            for k in 0..3 {
                let orig = px.0[k] as f32;
                let detail = orig - b[k] as f32;
                // Attenuate extreme haloing near 0 or 255
                let headroom = 1.0 - ((orig - 127.5).abs() / 135.0).powi(2).clamp(0.0, 0.75);
                px.0[k] = (orig + detail * amount * headroom).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    base
}

/// Background canvas with padding, rounded card and blurred drop shadow.
pub fn apply_frame_bg(img: &RgbaImage, p: &ToolParams) -> RgbaImage {
    let (w, h) = img.dimensions();
    let pad = p.padding;
    let (cw, ch) = (w + 2 * pad, h + 2 * pad);
    let mut canvas = RgbaImage::from_pixel(cw, ch, Rgba(p.bg_color));
    if p.shadow > 0 {
        let alpha = (p.shadow * 15 / 4).min(255) as u8;
        let mut shadow = RgbaImage::new(cw, ch);
        let dy = 8;
        for y in 0..h {
            for x in 0..w {
                if inside_round(x, y, w, h, p.corner_radius) && pad + y + dy < ch {
                    shadow.put_pixel(pad + x, pad + y + dy, Rgba([0, 0, 0, alpha]));
                }
            }
        }
        imageops::overlay(&mut canvas, &imageops::blur(&shadow, p.shadow as f32 / 2.4), 0, 0);
    }
    let mut card = img.clone();
    round_corners(&mut card, p.corner_radius);
    imageops::overlay(&mut canvas, &card, pad as i64, pad as i64);
    canvas
}

/// 100% local face & text bounding-box detector using Windows 10/11 built-in
/// `Windows.Media.FaceAnalysis.FaceDetector` and `Windows.Media.Ocr.OcrEngine`.
/// `mode`: `1 = Faces Only`, `2 = Text Only`, `3 = Faces + Text`.
pub fn detect_faces_and_text_boxes(image_path: &Path, mode: u8) -> Vec<(u32, u32, u32, u32)> {
    let abs = fs::canonicalize(image_path).unwrap_or_else(|_| image_path.to_path_buf());
    let clean_path = abs
        .display()
        .to_string()
        .trim_start_matches(r"\\?\")
        .replace('\'', "''");
    let do_faces = if mode == 1 || mode == 3 { "$true" } else { "$false" };
    let do_text = if mode == 2 || mode == 3 { "$true" } else { "$false" };
    let script = format!(
        r#"Add-Type -AssemblyName System.Runtime.WindowsRuntime
$asTaskGeneric = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {{ $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -like 'IAsyncOperation*' }})[0]
function Await($WinRtTask, $ResultType) {{
    $asTask = $asTaskGeneric.MakeGenericMethod($ResultType)
    $netTask = $asTask.Invoke($null, @($WinRtTask))
    $netTask.Wait(-1) | Out-Null
    $netTask.Result
}}
[Windows.Storage.StorageFile,Windows.Storage,ContentType=WindowsRuntime] | Out-Null
[Windows.Graphics.Imaging.BitmapDecoder,Windows.Graphics,ContentType=WindowsRuntime] | Out-Null
[Windows.Graphics.Imaging.SoftwareBitmap,Windows.Graphics,ContentType=WindowsRuntime] | Out-Null
[Windows.Media.FaceAnalysis.FaceDetector,Windows.Foundation.UniversalApiContract,ContentType=WindowsRuntime] | Out-Null
[Windows.Media.Ocr.OcrEngine,Windows.Foundation.UniversalApiContract,ContentType=WindowsRuntime] | Out-Null
$file = Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync('{clean_path}')) ([Windows.Storage.StorageFile])
$stream = Await ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
$decoder = Await ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
$bitmap = Await ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])
if ({do_faces}) {{
    $det = Await ([Windows.Media.FaceAnalysis.FaceDetector]::CreateAsync()) ([Windows.Media.FaceAnalysis.FaceDetector])
    $gray = [Windows.Graphics.Imaging.SoftwareBitmap]::Convert($bitmap, [Windows.Graphics.Imaging.BitmapPixelFormat]::Gray8)
    $faces = Await ($det.DetectFacesAsync($gray)) ([System.Collections.Generic.IList[Windows.Media.FaceAnalysis.DetectedFace]])
    foreach ($f in $faces) {{
        $b = $f.FaceBox
        Write-Output "$($b.X),$($b.Y),$($b.Width),$($b.Height)"
    }}
}}
if ({do_text}) {{
    $eng = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
    $ocr = Await ($eng.RecognizeAsync($bitmap)) ([Windows.Media.Ocr.OcrResult])
    foreach ($l in $ocr.Lines) {{
        foreach ($w in $l.Words) {{
            $r = $w.BoundingRect
            Write-Output "$([int]$r.X),$([int]$r.Y),$([int]$r.Width),$([int]$r.Height)"
        }}
    }}
}}"#
    );
    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let Ok(out) = cmd.output() else { return Vec::new() };
    let mut boxes = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let parts: Vec<u32> = line.trim().split(',').filter_map(|s| s.parse().ok()).collect();
        if parts.len() == 4 && parts[2] >= 4 && parts[3] >= 4 {
            boxes.push((parts[0], parts[1], parts[2], parts[3]));
        }
    }
    boxes
}

fn redact(img: &DynamicImage, p: &ToolParams) -> Result<DynamicImage> {
    const BLOCK: u32 = 16;
    let (iw, ih) = (img.width(), img.height());
    let rects: Vec<(u32, u32, u32, u32)> = if !p.redact_rects.is_empty() {
        p.redact_rects.clone()
    } else {
        vec![p.crop_rect.unwrap_or((iw / 4, ih / 4, iw / 2, ih / 2))]
    };
    let mut out = img.to_rgba8();
    for (rx, ry, rw, rh) in rects {
        let (rx, ry) = (rx.min(iw), ry.min(ih));
        let (rw, rh) = (rw.min(iw - rx), rh.min(ih - ry));
        if rw == 0 || rh == 0 {
            continue;
        }
        if p.redact_solid {
            for y in ry..ry + rh {
                for x in rx..rx + rw {
                    out.put_pixel(x, y, Rgba([0, 0, 0, 255]));
                }
            }
        } else {
            for by in (ry..ry + rh).step_by(BLOCK as usize) {
                for bx in (rx..rx + rw).step_by(BLOCK as usize) {
                    let (bw, bh) = (BLOCK.min(rx + rw - bx), BLOCK.min(ry + rh - by));
                    let mut sum = [0u32; 4];
                    for y in by..by + bh {
                        for x in bx..bx + bw {
                            for (s, c) in sum.iter_mut().zip(out.get_pixel(x, y).0) {
                                *s += c as u32;
                            }
                        }
                    }
                    let n = bw * bh;
                    let avg = Rgba([0, 1, 2, 3].map(|i| (sum[i] / n) as u8));
                    for y in by..by + bh {
                        for x in bx..bx + bw {
                            out.put_pixel(x, y, avg);
                        }
                    }
                }
            }
        }
    }
    Ok(DynamicImage::ImageRgba8(out))
}

fn stamp(img: &mut RgbaImage, cx: f32, cy: f32, r: f32, c: [u8; 4]) {
    let (w, h) = img.dimensions();
    let x0 = (cx - r).floor().max(0.0) as u32;
    let x1 = ((cx + r).ceil().max(0.0) as u32).min(w - 1);
    let y0 = (cy - r).floor().max(0.0) as u32;
    let y1 = ((cy + r).ceil().max(0.0) as u32).min(h - 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            if (x as f32 - cx).powi(2) + (y as f32 - cy).powi(2) <= r * r {
                img.put_pixel(x, y, Rgba(c));
            }
        }
    }
}

fn line(img: &mut RgbaImage, a: (f32, f32), b: (f32, f32), r: f32, c: [u8; 4]) {
    let steps = ((b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil() as u32).max(1);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        stamp(img, a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, r, c);
    }
}

/// Thick callout rectangle plus a diagonal pointer arrow ending at its top-left corner.
fn annotate(img: &DynamicImage, p: &ToolParams) -> Result<DynamicImage> {
    let mut out = img.to_rgba8();
    let (iw, ih) = out.dimensions();
    let (x, y, w, h) = p.crop_rect.unwrap_or((iw / 4, ih / 4, iw / 2, ih / 2));
    if w == 0 || h == 0 || x >= iw || y >= ih {
        return Err(ClementineError::InvalidInput("empty annotation area".into()));
    }
    let r = (iw.min(ih) as f32 / 200.0).max(1.5);
    let (x0, y0) = (x as f32, y as f32);
    let (x1, y1) = (((x + w).min(iw) - 1) as f32, ((y + h).min(ih) - 1) as f32);
    let c = p.annotation_color;
    for (a, b) in [((x0, y0), (x1, y0)), ((x1, y0), (x1, y1)), ((x1, y1), (x0, y1)), ((x0, y1), (x0, y0))] {
        line(&mut out, a, b, r, c);
    }
    let d = iw.min(ih) as f32 / 6.0;
    let tail = ((x0 - d).max(r), (y0 - d).max(r));
    let tip = (x0, y0);
    line(&mut out, tail, tip, r, c);
    let (dx, dy) = (tip.0 - tail.0, tip.1 - tail.1);
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    let (ux, uy) = (dx / len, dy / len);
    let head = (len / 3.0).min(d / 2.0).max(r * 4.0);
    for ang in [0.5f32, -0.5] {
        let (s, co) = ang.sin_cos();
        let (bx, by) = (-(ux * co - uy * s), -(ux * s + uy * co));
        line(&mut out, tip, (tip.0 + bx * head, tip.1 + by * head), r, c);
    }
    Ok(DynamicImage::ImageRgba8(out))
}

/// Resizes by `scale_pct` and, for PNG at quality < 90, quantizes color levels.
fn shrink(mut img: DynamicImage, ext: &str, p: &ToolParams) -> DynamicImage {
    let pct = p.scale_pct.clamp(25, 100) as u32;
    if pct < 100 {
        let (w, h) = ((img.width() * pct / 100).max(1), (img.height() * pct / 100).max(1));
        img = img.resize_exact(w, h, Resize::Lanczos3);
    }
    if ext == "png" && p.quality < 90 {
        let step = ((100 - p.quality as u32) / 8).max(1);
        let mut rgba = img.to_rgba8();
        for px in rgba.pixels_mut() {
            for c in &mut px.0[..3] {
                *c = ((*c as u32 + step / 2) / step * step).min(255) as u8;
            }
        }
        img = DynamicImage::ImageRgba8(rgba);
    }
    img
}

fn collage(imgs: &[DynamicImage], p: &ToolParams) -> DynamicImage {
    const CELL: u32 = 400;
    const GAP: u32 = 24;
    let n = imgs.len() as u32;
    let cols = p.collage_cols.clamp(1, n.max(1));
    let rows = n.div_ceil(cols);
    let mut canvas = RgbaImage::from_pixel(
        cols * CELL + (cols + 1) * GAP,
        rows * CELL + (rows + 1) * GAP,
        Rgba(p.bg_color),
    );
    for (i, img) in imgs.iter().enumerate() {
        let (c, r) = (i as u32 % cols, i as u32 / cols);
        let mut cell = img.resize_to_fill(CELL, CELL, Resize::Lanczos3).to_rgba8();
        round_corners(&mut cell, p.corner_radius);
        imageops::overlay(
            &mut canvas,
            &cell,
            (GAP + c * (CELL + GAP)) as i64,
            (GAP + r * (CELL + GAP)) as i64,
        );
    }
    DynamicImage::ImageRgba8(canvas)
}

pub fn tool(job: &[PathBuf], tool_id: &str, p: &ToolParams, out: &Path) -> Result<()> {
    let ext = out.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match tool_id {
        "createPDF" => {
            let pages = job
                .iter()
                .map(|f| image_page(&load(f)?))
                .collect::<Result<Vec<_>>>()?;
            fs::write(out, build_pdf(&pages))?;
            Ok(())
        }
        "createCollage" => {
            let imgs = job.iter().map(|f| load(f)).collect::<Result<Vec<_>>>()?;
            write_image(&collage(&imgs, p), &ext, out, 90)
        }
        _ => {
            let img = load(&job[0])?;
            let img = match tool_id {
                "compress" => shrink(img, &ext, p),
                "removeMetadata" => img,
                "removeBg" => DynamicImage::ImageRgba8(remove_background_local(&img.to_rgba8(), p)),
                "upscaleImage" => DynamicImage::ImageRgba8(apply_upscale_sharpen(&img.to_rgba8(), p)),
                "cropImage" => crop(&img, p.crop_rect)?,
                "editImage" => DynamicImage::ImageRgba8(apply_photo_edits(&img.to_rgba8(), p)),
                "frameImage" => DynamicImage::ImageRgba8(apply_frame_bg(&img.to_rgba8(), p)),
                "redactImage" => redact(&img, p)?,
                "annotateImage" => annotate(&img, p)?,
                other => return Err(ClementineError::UnsupportedTool(other.into())),
            };
            write_image(&img, &ext, out, if tool_id == "compress" { p.quality } else { 90 })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_util::*;
    use super::super::{execute_conversion, execute_tool, ToolParams};
    use std::fs;

    fn dims(p: &std::path::Path) -> (u32, u32) {
        let i = image::open(p).unwrap();
        (i.width(), i.height())
    }

    #[test]
    fn converts_png_to_every_raster_format() {
        let d = tempfile::tempdir().unwrap();
        let src = sample_png(d.path(), "photo.png", 40, 30);
        for ext in ["jpg", "webp", "bmp", "tiff", "gif", "ico"] {
            let out = execute_conversion(&src, ext).unwrap();
            assert_eq!(out.file_name().unwrap().to_string_lossy(), format!("photo.{ext}"));
            assert_eq!(dims(&out), (40, 30), "{ext}");
        }
        let jpg = fs::read(d.path().join("photo.jpg")).unwrap();
        assert_eq!(&jpg[..2], &[0xFF, 0xD8]);
        let pdf = execute_conversion(&src, "pdf").unwrap();
        let bytes = fs::read(pdf).unwrap();
        assert!(bytes.starts_with(b"%PDF-1.4"));
        assert!(bytes.windows(10).any(|w| w == b"DCTDecode "));
        assert!(bytes.ends_with(b"%%EOF\n"));
    }

    #[test]
    fn svg_input_rasterizes() {
        let d = tempfile::tempdir().unwrap();
        let svg = d.path().join("s.svg");
        fs::write(
            &svg,
            r##"<svg viewBox="0 0 20 10"><rect x="0" y="0" width="10" height="10" fill="#ff0000"/><circle cx="15" cy="5" r="4" fill="blue"/></svg>"##,
        )
        .unwrap();
        let out = execute_conversion(&svg, "png").unwrap();
        let img = image::open(out).unwrap().to_rgba8();
        assert_eq!(img.dimensions(), (20, 10));
        assert_eq!(img.get_pixel(2, 5).0, [255, 0, 0, 255]);
        assert_eq!(img.get_pixel(15, 5).0, [0, 0, 255, 255]);
        assert_eq!(img.get_pixel(11, 0).0[3], 0);
    }

    #[test]
    fn svg_path_rasterizes() {
        let d = tempfile::tempdir().unwrap();
        let svg = d.path().join("p.svg");
        fs::write(
            &svg,
            r##"<svg viewBox="0 0 20 20"><path d="M2 2 H10 V10 L2 10 Z" fill="#00ff00"/><path d="m12 12 h6 v6 h-6 z" fill="red"/></svg>"##,
        )
        .unwrap();
        let img = image::open(execute_conversion(&svg, "png").unwrap()).unwrap().to_rgba8();
        assert_eq!(img.get_pixel(5, 5).0, [0, 255, 0, 255]);
        assert_eq!(img.get_pixel(15, 15).0, [255, 0, 0, 255]);
        assert_eq!(img.get_pixel(11, 5).0[3], 0);
        assert_eq!(img.get_pixel(0, 0).0[3], 0);
    }

    #[test]
    fn compress_shrinks_and_metadata_roundtrips() {
        let d = tempfile::tempdir().unwrap();
        let png = sample_png(d.path(), "p.png", 200, 200);
        let jpg = execute_conversion(&png, "jpg").unwrap();
        let params = ToolParams { quality: 10, ..Default::default() };
        let small = execute_tool(&[jpg.clone()], "compress", &params).unwrap();
        assert!(fs::metadata(&small).unwrap().len() < fs::metadata(&jpg).unwrap().len());
        let clean = execute_tool(&[jpg], "removeMetadata", &params).unwrap();
        assert_eq!(clean.file_name().unwrap().to_string_lossy(), "p No Metadata.jpg");
        assert_eq!(dims(&clean), (200, 200));
    }

    #[test]
    fn crop_edit_frame_redact() {
        let d = tempfile::tempdir().unwrap();
        let src = sample_png(d.path(), "a.png", 100, 50);
        let up = execute_tool(
            &[src.clone()],
            "upscaleImage",
            &ToolParams { collage_cols: 2, quality: 60, padding: 10, ..Default::default() },
        )
        .unwrap();
        assert_eq!(dims(&up), (200, 100));
        let mut p = ToolParams::default();
        let c = execute_tool(&[src.clone()], "cropImage", &p).unwrap();
        assert_eq!(dims(&c), (80, 40));
        p.crop_rect = Some((10, 10, 20, 20));
        let c2 = execute_tool(&[src.clone()], "cropImage", &p).unwrap();
        assert_eq!(dims(&c2), (20, 20));
        p.crop_rect = Some((0, 0, 0, 5));
        assert!(execute_tool(&[src.clone()], "cropImage", &p).is_err());
        assert!(!d.path().join("a Cropped 3.png").exists());

        let p = ToolParams { exposure: 1.0, saturation: 0.0, ..Default::default() };
        let e = execute_tool(&[src.clone()], "editImage", &p).unwrap();
        let px = image::open(e).unwrap().to_rgba8().get_pixel(50, 25).0;
        assert_eq!(px[0], px[1]);
        assert_eq!(px[1], px[2]);

        let p = ToolParams { padding: 20, corner_radius: 10, bg_color: [10, 20, 30, 255], ..Default::default() };
        let f = execute_tool(&[src.clone()], "frameImage", &p).unwrap();
        assert_eq!(dims(&f), (140, 90));
        assert_eq!(image::open(&f).unwrap().to_rgba8().get_pixel(0, 0).0, [10, 20, 30, 255]);

        let r = execute_tool(&[src.clone()], "redactImage", &ToolParams::default()).unwrap();
        let (orig, red) = (image::open(&src).unwrap().to_rgba8(), image::open(r).unwrap().to_rgba8());
        assert_eq!(orig.get_pixel(0, 0), red.get_pixel(0, 0));
        assert_eq!(red.get_pixel(30, 15), red.get_pixel(31, 16));
    }

    #[test]
    fn pdf_and_collage_from_many() {
        let d = tempfile::tempdir().unwrap();
        let a = sample_png(d.path(), "a.png", 30, 30);
        let b = sample_png(d.path(), "b.png", 20, 40);
        let c = sample_png(d.path(), "c.png", 50, 10);
        let pdf = execute_tool(&[a.clone(), b.clone(), c.clone()], "createPDF", &ToolParams::default()).unwrap();
        assert_eq!(pdf.file_name().unwrap().to_string_lossy(), "a Combined.pdf");
        let bytes = fs::read(pdf).unwrap();
        let pages = bytes.windows(11).filter(|w| *w == b"/Type /Page").count();
        assert_eq!(pages, 3 + 1); // three pages plus the /Type /Pages node
        let col = execute_tool(&[a, b, c], "createCollage", &ToolParams::default()).unwrap();
        assert_eq!(dims(&col), (2 * 400 + 3 * 24, 2 * 400 + 3 * 24));
    }

    #[test]
    fn annotate_draws_color() {
        let d = tempfile::tempdir().unwrap();
        let src = sample_png(d.path(), "a.png", 200, 100);
        let p = ToolParams { annotation_color: [1, 2, 3, 255], ..Default::default() };
        let out = execute_tool(&[src.clone()], "annotateImage", &p).unwrap();
        assert_eq!(out.extension().unwrap(), "png");
        let (o, a) = (image::open(&src).unwrap().to_rgba8(), image::open(&out).unwrap().to_rgba8());
        assert_eq!(a.get_pixel(100, 25).0, [1, 2, 3, 255]);
        assert_ne!(o.get_pixel(100, 25), a.get_pixel(100, 25));
        assert_eq!(o.get_pixel(100, 50), a.get_pixel(100, 50));
    }

    #[test]
    fn compress_scale_and_redact_solid() {
        let d = tempfile::tempdir().unwrap();
        let src = sample_png(d.path(), "a.png", 200, 200);
        let p = ToolParams { quality: 20, scale_pct: 50, ..Default::default() };
        let c = execute_tool(&[src.clone()], "compress", &p).unwrap();
        assert_eq!(dims(&c), (100, 100));
        let p = ToolParams { quality: 20, ..Default::default() };
        let c2 = execute_tool(&[src.clone()], "compress", &p).unwrap();
        assert!(fs::metadata(&c2).unwrap().len() < fs::metadata(&src).unwrap().len());
        let p = ToolParams { redact_solid: true, crop_rect: Some((10, 10, 20, 20)), ..Default::default() };
        let r = image::open(execute_tool(&[src], "redactImage", &p).unwrap()).unwrap().to_rgba8();
        assert_eq!(r.get_pixel(15, 15).0, [0, 0, 0, 255]);
        assert_ne!(r.get_pixel(40, 40).0, [0, 0, 0, 255]);
    }
}
