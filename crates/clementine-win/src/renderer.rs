//! Software renderer for the 272x272 glass radial wheel. Output is premultiplied BGRA (`0xAARRGGBB`).
use clementine_core::*;
use std::ptr::null_mut;
use windows_sys::Win32::Graphics::Gdi::*;

pub const SIZE: usize = 272;
const N: usize = SIZE * SIZE;
const C: f64 = 136.0;
pub type Px = [f32; 4];

/// Top-down 32bpp DIB section with its own memory DC.
pub struct Dib {
    pub dc: HDC,
    pub bmp: HBITMAP,
    pub bits: *mut u32,
    old: HGDIOBJ,
    len: usize,
}

impl Dib {
    pub fn new() -> Dib {
        Dib::sized(SIZE, SIZE)
    }
    pub fn sized(w: usize, h: usize) -> Dib {
        unsafe {
            let dc = CreateCompatibleDC(null_mut());
            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w as i32;
            bmi.bmiHeader.biHeight = -(h as i32);
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB;
            let mut bits: *mut std::ffi::c_void = null_mut();
            let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            let old = SelectObject(dc, bmp);
            Dib { dc, bmp, bits: bits as *mut u32, old, len: w * h }
        }
    }
    pub fn slice(&mut self) -> &mut [u32] {
        unsafe { std::slice::from_raw_parts_mut(self.bits, self.len) }
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bmp);
            DeleteDC(self.dc);
        }
    }
}

/// Grabs the 272x272 screen area centered on `(cx, cy)`.
pub fn capture_screen(cx: i32, cy: i32) -> Option<Vec<u32>> {
    unsafe {
        let sdc = GetDC(null_mut());
        if sdc.is_null() {
            return None;
        }
        let mut d = Dib::new();
        let ok = BitBlt(d.dc, 0, 0, SIZE as i32, SIZE as i32, sdc, cx - 136, cy - 136, SRCCOPY);
        ReleaseDC(null_mut(), sdc);
        if ok == 0 {
            return None;
        }
        Some(d.slice().to_vec())
    }
}

fn blur_pass(src: &[f32], dst: &mut [f32], r: isize, sa: usize, sk: usize) {
    let n = SIZE as isize;
    let w = (2 * r + 1) as f32;
    for a in 0..SIZE {
        let at = |k: isize| src[a * sa + k.clamp(0, n - 1) as usize * sk];
        let mut sum: f32 = (-r..=r).map(at).sum();
        for k in 0..n {
            dst[a * sa + k as usize * sk] = sum / w;
            sum += at(k + r + 1) - at(k - r);
        }
    }
}

fn box_blur(p: &mut [f32], r: isize) {
    let mut t = vec![0f32; N];
    blur_pass(p, &mut t, r, SIZE, 1);
    blur_pass(&t, p, r, 1, SIZE);
}

/// Gaussian approximation: three box passes (sigma ~= radius / 2).
fn gauss(p: &mut [f32], radius: isize) {
    for _ in 0..3 {
        box_blur(p, (radius / 2).max(1));
    }
}

fn over(dst: &mut Px, rgb: [f32; 3], a: f32) {
    let k = 1.0 - a;
    for i in 0..3 {
        dst[i] = rgb[i] * a + dst[i] * k;
    }
    dst[3] = a + dst[3] * k;
}

fn rgb8(r: u8, g: u8, b: u8) -> [f32; 3] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}

/// Blurred, saturated, tinted donut ring built from the raw desktop capture.
pub fn make_base(backdrop: Option<&[u32]>) -> Vec<Px> {
    let mut ch = [vec![0.78f32; N], vec![0.80f32; N], vec![0.82f32; N]];
    if let Some(b) = backdrop {
        for (i, v) in b.iter().enumerate().take(N) {
            ch[0][i] = ((v >> 16) & 255) as f32 / 255.0;
            ch[1][i] = ((v >> 8) & 255) as f32 / 255.0;
            ch[2][i] = (v & 255) as f32 / 255.0;
        }
        for c in ch.iter_mut() {
            gauss(c, 16);
        }
    }
    let tint = rgb8(192, 201, 205);
    let mut out = vec![[0f32; 4]; N];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let r = (x as f64 + 0.5 - C).hypot(y as f64 + 0.5 - C);
            let m = ((r - HOLE_MASK_RADIUS + 0.5).clamp(0.0, 1.0)
                * (BACKDROP_RADIUS - r + 0.5).clamp(0.0, 1.0)) as f32;
            if m <= 0.0 {
                continue;
            }
            let i = y * SIZE + x;
            let (cr, cg, cb) = (ch[0][i], ch[1][i], ch[2][i]);
            let l = 0.299 * cr + 0.587 * cg + 0.114 * cb;
            let sat = |c: f32| (l + (c - l) * 1.45).clamp(0.0, 1.0);
            let mut p = [0f32; 4];
            over(&mut p, [sat(cr), sat(cg), sat(cb)], 1.0);
            over(&mut p, tint, 0.25);
            let rim = (r - 41.74).clamp(0.0, 1.0).min((43.74 - r).clamp(0.0, 1.0)) as f32;
            over(&mut p, [1.0; 3], 0x55 as f32 / 255.0 * rim);
            out[i] = p.map(|v| v * m);
        }
    }
    out
}

fn pip(poly: &[Point2D], x: f64, y: f64) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > y) != (b.y > y) && x < (b.x - a.x) * (y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// 2x2 supersampled coverage. Same even-odd test as `PetalGeometry::contains_point`,
/// on a polygon sampled once (contains_point resamples every call, too slow per subpixel).
fn coverage(poly: &[Point2D]) -> Vec<f32> {
    let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for p in poly {
        x0 = x0.min(p.x);
        x1 = x1.max(p.x);
        y0 = y0.min(p.y);
        y1 = y1.max(p.y);
    }
    let mut cov = vec![0f32; N];
    let lo = |v: f64| ((v + C).floor() as isize - 1).clamp(0, SIZE as isize - 1) as usize;
    for y in lo(y0)..=lo(y1 + 2.0) {
        for x in lo(x0)..=lo(x1 + 2.0) {
            let mut n = 0;
            for (sx, sy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                if pip(poly, x as f64 + sx - C, y as f64 + sy - C) {
                    n += 1;
                }
            }
            cov[y * SIZE + x] = n as f32 / 4.0;
        }
    }
    cov
}

fn gradient(t: f32) -> ([f32; 3], f32) {
    let stops = [(0.0, [255u8, 255, 255], 0.80), (0.68, [243, 246, 247], 0.58), (1.0, [244, 246, 246], 0.75)];
    let t = t.clamp(0.0, 1.0);
    for w in stops.windows(2) {
        let ((t0, c0, a0), (t1, c1, a1)) = (w[0], w[1]);
        if t <= t1 {
            let k = (t - t0) / (t1 - t0);
            let c0 = rgb8(c0[0], c0[1], c0[2]);
            let c1 = rgb8(c1[0], c1[1], c1[2]);
            return ([0, 1, 2].map(|i| c0[i] + (c1[i] - c0[i]) * k), a0 + (a1 - a0) * k);
        }
    }
    (rgb8(244, 246, 246), 0.75)
}

const SS: i32 = 3;
const BW: i32 = 80;
const BH: i32 = 32;

/// GDI text rendered at 3x into a small DIB, box-averaged 3x3 onto the wheel (even stem weight).
fn draw_text(buf: &mut [Px], text: &str, cx: f64, cy: f64, col: [f32; 3], base_h: i32, maxw: i32) {
    unsafe {
        let (bw3, bh3) = ((BW * SS) as usize, (BH * SS) as usize);
        let dib = Dib::sized(bw3, bh3);
        let wide: Vec<u16> = text.encode_utf16().collect();
        let face: Vec<u16> = "Segoe UI Semibold\0".encode_utf16().collect();
        let mut h = base_h;
        let (font, old, sz) = loop {
            let font = CreateFontW(
                -h * SS, 0, 0, 0, 600, 0, 0, 0, DEFAULT_CHARSET as u32, OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32, ANTIALIASED_QUALITY as u32,
                (DEFAULT_PITCH | FF_DONTCARE) as u32, face.as_ptr(),
            );
            let old = SelectObject(dib.dc, font);
            let mut sz: windows_sys::Win32::Foundation::SIZE = std::mem::zeroed();
            GetTextExtentPoint32W(dib.dc, wide.as_ptr(), wide.len() as i32, &mut sz);
            if sz.cx <= maxw * SS || h <= 6 {
                break (font, old, sz);
            }
            SelectObject(dib.dc, old);
            DeleteObject(font);
            h -= 1;
        };
        std::ptr::write_bytes(dib.bits, 0, bw3 * bh3);
        SetBkMode(dib.dc, TRANSPARENT as i32);
        SetTextColor(dib.dc, 0x00FF_FFFF);
        let (ox, oy) = ((cx - BW as f64 / 2.0).round(), (cy - BH as f64 / 2.0).round());
        TextOutW(
            dib.dc,
            ((cx - ox) * SS as f64 - sz.cx as f64 / 2.0).round() as i32,
            ((cy - oy) * SS as f64 - sz.cy as f64 / 2.0).round() as i32,
            wide.as_ptr(),
            wide.len() as i32,
        );
        GdiFlush();
        SelectObject(dib.dc, old);
        DeleteObject(font);
        let px = std::slice::from_raw_parts(dib.bits, bw3 * bh3);
        let ss = SS as usize;
        for by in 0..BH as usize {
            for bx in 0..BW as usize {
                let mut sum = 0u32;
                for dy in 0..ss {
                    for dx in 0..ss {
                        sum += (px[(by * ss + dy) * bw3 + bx * ss + dx] >> 8) & 255;
                    }
                }
                let a = sum as f32 / (255.0 * (ss * ss) as f32);
                let (x, y) = (ox as i32 + bx as i32, oy as i32 + by as i32);
                if a > 0.0 && x >= 0 && y >= 0 && x < SIZE as i32 && y < SIZE as i32 {
                    over(&mut buf[y as usize * SIZE + x as usize], col, a);
                }
            }
        }
    }
}

#[derive(Clone)]
enum Sh {
    Seg(f32, f32, f32, f32),
    Ring(f32, f32, f32),
    Disc(f32, f32, f32),
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Vec<Sh> {
    vec![Sh::Seg(x, y, x + w, y), Sh::Seg(x + w, y, x + w, y + h), Sh::Seg(x + w, y + h, x, y + h), Sh::Seg(x, y + h, x, y)]
}

fn poly(p: &[(f32, f32)]) -> Vec<Sh> {
    (0..p.len()).map(|i| Sh::Seg(p[i].0, p[i].1, p[(i + 1) % p.len()].0, p[(i + 1) % p.len()].1)).collect()
}

fn speaker() -> Vec<Sh> {
    poly(&[(2.0, 7.0), (5.0, 7.0), (9.0, 4.0), (9.0, 14.0), (5.0, 11.0), (2.0, 11.0)])
}

fn doc() -> Vec<Sh> {
    let mut v = poly(&[(4.0, 2.0), (11.0, 2.0), (14.0, 5.0), (14.0, 16.0), (4.0, 16.0)]);
    v.extend([Sh::Seg(11.0, 2.0, 11.0, 5.0), Sh::Seg(11.0, 5.0, 14.0, 5.0)]);
    v
}

/// 18x18 vector glyph for a tool/action id.
fn icon_shapes(id: &str) -> Vec<Sh> {
    use Sh::*;
    match id {
        "compress" => vec![
            Seg(2.0, 2.0, 7.0, 7.0), Seg(7.0, 3.0, 7.0, 7.0), Seg(3.0, 7.0, 7.0, 7.0),
            Seg(16.0, 16.0, 11.0, 11.0), Seg(11.0, 15.0, 11.0, 11.0), Seg(15.0, 11.0, 11.0, 11.0),
        ],
        "removeMetadata" => {
            let mut v = poly(&[(3.0, 3.0), (9.0, 3.0), (15.0, 9.0), (9.0, 15.0), (3.0, 9.0)]);
            v.push(Disc(6.0, 6.0, 1.2));
            v
        }
        "editImage" => vec![Seg(2.0, 5.0, 16.0, 5.0), Seg(2.0, 13.0, 16.0, 13.0), Disc(6.0, 5.0, 2.0), Disc(12.0, 13.0, 2.0)],
        "annotateImage" => poly(&[(3.0, 15.0), (4.0, 11.0), (12.0, 3.0), (15.0, 6.0), (7.0, 14.0)]),
        "frameImage" => [rect(2.0, 2.0, 14.0, 14.0), rect(5.0, 5.0, 8.0, 8.0)].concat(),
        "cropImage" | "cropVideo" => vec![Seg(5.0, 2.0, 5.0, 13.0), Seg(5.0, 13.0, 16.0, 13.0), Seg(2.0, 5.0, 13.0, 5.0), Seg(13.0, 5.0, 13.0, 16.0)],
        "redactImage" | "redactVideo" => {
            let mut v = poly(&[(2.0, 9.0), (6.0, 5.0), (12.0, 5.0), (16.0, 9.0), (12.0, 13.0), (6.0, 13.0)]);
            v.extend([Disc(9.0, 9.0, 2.0), Seg(3.0, 16.0, 15.0, 2.0)]);
            v
        }
        "createPDF" | "mergePDF" => doc(),
        "copy" => [rect(2.0, 5.0, 9.0, 11.0), rect(7.0, 2.0, 9.0, 11.0)].concat(),
        "copyPath" => {
            let mut v = doc();
            v.extend([Seg(6.0, 9.0, 12.0, 9.0), Seg(6.0, 12.0, 12.0, 12.0)]);
            v
        }
        "createCollage" | "organizePDF" => [(2.0, 2.0), (10.0, 2.0), (2.0, 10.0), (10.0, 10.0)].iter().flat_map(|&(x, y)| rect(x, y, 6.0, 6.0)).collect(),
        "normalizeAudio" | "audioToVideo" | "redactAudio" => {
            [(3.0, 7.0, 11.0), (6.0, 4.0, 14.0), (9.0, 2.0, 16.0), (12.0, 5.0, 13.0), (15.0, 7.0, 11.0)]
                .iter().map(|&(x, a, b)| Seg(x, a, x, b)).collect()
        }
        "muteVideo" => {
            let mut v = speaker();
            v.push(Seg(3.0, 16.0, 16.0, 2.0));
            v
        }
        "trimVideo" | "trimAudio" => vec![Ring(5.0, 14.0, 2.2), Ring(13.0, 14.0, 2.2), Seg(6.5, 12.5, 14.0, 2.0), Seg(11.5, 12.5, 4.0, 2.0)],
        "splitPDF" | "splitVideo" => [rect(2.0, 3.0, 14.0, 12.0), vec![Seg(9.0, 3.0, 9.0, 15.0)]].concat(),
        "extractArchive" => {
            let mut v = rect(2.0, 6.0, 14.0, 10.0);
            v.extend([Seg(2.0, 6.0, 4.0, 2.0), Seg(4.0, 2.0, 14.0, 2.0), Seg(14.0, 2.0, 16.0, 6.0), Seg(7.0, 9.0, 11.0, 9.0)]);
            v
        }
        "changeVideoSpeed" => vec![Ring(9.0, 10.0, 7.0), Seg(9.0, 10.0, 13.0, 6.0), Disc(9.0, 10.0, 1.3)],
        "joinVideos" => [rect(2.0, 3.0, 14.0, 12.0), vec![Seg(6.0, 3.0, 6.0, 15.0), Seg(12.0, 3.0, 12.0, 15.0), Seg(2.0, 9.0, 6.0, 9.0), Seg(12.0, 9.0, 16.0, 9.0)]].concat(),
        "videoSnapshots" => {
            let mut v = rect(2.0, 5.0, 14.0, 10.0);
            v.extend([Seg(6.0, 5.0, 7.0, 3.0), Seg(7.0, 3.0, 11.0, 3.0), Seg(11.0, 3.0, 12.0, 5.0), Ring(9.0, 10.0, 2.8)]);
            v
        }
        "audioChannels" => {
            let mut v = speaker();
            v.extend([Seg(12.0, 6.0, 14.0, 9.0), Seg(14.0, 9.0, 12.0, 12.0), Seg(14.0, 4.0, 16.5, 9.0), Seg(16.5, 9.0, 14.0, 14.0)]);
            v
        }
        "airDrop" => vec![Seg(9.0, 12.0, 9.0, 2.0), Seg(5.0, 6.0, 9.0, 2.0), Seg(13.0, 6.0, 9.0, 2.0), Seg(3.0, 9.0, 3.0, 16.0), Seg(3.0, 16.0, 15.0, 16.0), Seg(15.0, 16.0, 15.0, 9.0)],
        "float" => vec![Ring(9.0, 6.0, 3.5), Seg(9.0, 9.5, 9.0, 16.0)],
        "move" => poly(&[(2.0, 4.0), (7.0, 4.0), (9.0, 6.0), (16.0, 6.0), (16.0, 15.0), (2.0, 15.0)]),
        "openTerminal" => vec![Seg(3.0, 5.0, 8.0, 9.0), Seg(8.0, 9.0, 3.0, 13.0), Seg(10.0, 14.0, 15.0, 14.0)],
        _ => vec![Ring(9.0, 9.0, 6.0)],
    }
}

fn seg_dist(px: f32, py: f32, a: f32, b: f32, c: f32, d: f32) -> f32 {
    let (vx, vy) = (c - a, d - b);
    let l = (vx * vx + vy * vy).max(1e-6);
    let t = (((px - a) * vx + (py - b) * vy) / l).clamp(0.0, 1.0);
    (px - a - t * vx).hypot(py - b - t * vy)
}

/// 4x4 supersampled vector icon, centered at `(cx, cy)`.
fn draw_icon(buf: &mut [Px], id: &str, cx: f64, cy: f64, col: [f32; 3]) {
    const G: usize = 18;
    const Q: usize = 4;
    const HALF: f32 = 0.8;
    let shapes = icon_shapes(id);
    let (ox, oy) = ((cx - 9.0).round() as i32, (cy - 9.0).round() as i32);
    for y in 0..G {
        for x in 0..G {
            let mut n = 0;
            for sy in 0..Q {
                for sx in 0..Q {
                    let (px, py) = (x as f32 + (sx as f32 + 0.5) / Q as f32, y as f32 + (sy as f32 + 0.5) / Q as f32);
                    let hit = shapes.iter().any(|s| match *s {
                        Sh::Seg(a, b, c, d) => seg_dist(px, py, a, b, c, d) <= HALF,
                        Sh::Ring(a, b, r) => ((px - a).hypot(py - b) - r).abs() <= HALF,
                        Sh::Disc(a, b, r) => (px - a).hypot(py - b) <= r,
                    });
                    n += hit as u32;
                }
            }
            let (gx, gy) = (ox + x as i32, oy + y as i32);
            if n > 0 && gx >= 0 && gy >= 0 && gx < SIZE as i32 && gy < SIZE as i32 {
                over(&mut buf[gy as usize * SIZE + gx as usize], col, n as f32 / (Q * Q) as f32);
            }
        }
    }
}

fn rounded_box_sdf(x: f64, y: f64, hw: f64, hh: f64, r: f64) -> f64 {
    let (qx, qy) = (x.abs() - (hw - r), y.abs() - (hh - r));
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Renders the full wheel. `labels.len()` is the petal count (max 9).
pub fn render(base: &[Px], labels: &[String], icons: Option<&[String]>, selected: Option<usize>, center: &str) -> Vec<u32> {
    let mut buf = base.to_vec();
    let n = labels.len().min(9);
    let polys: Vec<Vec<Point2D>> = (0..n).map(|i| build_petal_geometry(i, n).sample_polygon(32)).collect();
    let covs: Vec<Vec<f32>> = polys.iter().map(|p| coverage(p)).collect();

    // drop shadow: union of petal masks shifted +5px, blurred
    let mut sh = vec![0f32; N];
    for c in &covs {
        for y in 5..SIZE {
            for x in 0..SIZE {
                let v = c[(y - 5) * SIZE + x];
                let s = &mut sh[y * SIZE + x];
                *s = s.max(v);
            }
        }
    }
    gauss(&mut sh, 8);
    let shc = rgb8(38, 52, 56);
    for i in 0..N {
        let a = sh[i] * 0.145 * base[i][3];
        if a > 0.0 {
            over(&mut buf[i], shc, a);
        }
    }

    for (pi, cov) in covs.iter().enumerate() {
        let hi = selected == Some(pi);
        for y in 0..SIZE {
            for x in 0..SIZE {
                let i = y * SIZE + x;
                let c = cov[i];
                if c <= 0.0 {
                    continue;
                }
                let r = (x as f64 + 0.5 - C).hypot(y as f64 + 0.5 - C);
                let (col, a) = if hi { (rgb8(255, 82, 0), 1.0) } else { gradient((r / OUTER_RADIUS) as f32) };
                over(&mut buf[i], col, a * c);
                let nb = |dx: isize, dy: isize| {
                    let (nx, ny) = (x as isize + dx, y as isize + dy);
                    if nx < 0 || ny < 0 || nx >= SIZE as isize || ny >= SIZE as isize {
                        0.0
                    } else {
                        cov[ny as usize * SIZE + nx as usize]
                    }
                };
                let m = nb(-1, 0).min(nb(1, 0)).min(nb(0, -1)).min(nb(0, 1));
                let edge = c * (1.0 - m);
                if edge > 0.0 {
                    if hi {
                        over(&mut buf[i], rgb8(194, 101, 52), 0.44 * edge);
                    } else {
                        over(&mut buf[i], rgb8(93, 102, 102), 0.22 * edge);
                    }
                }
            }
        }
    }

    // center pill
    for y in 0..SIZE {
        for x in 0..SIZE {
            let d = rounded_box_sdf(x as f64 + 0.5 - C, y as f64 + 0.5 - C, 35.0, 16.0, 16.0);
            let c = (0.5 - d).clamp(0.0, 1.0) as f32;
            if c > 0.0 {
                let inner = (0.5 - (d + 1.0)).clamp(0.0, 1.0) as f32;
                let i = y * SIZE + x;
                over(&mut buf[i], [1.0; 3], 0.86 * c);
                over(&mut buf[i], [1.0; 3], 0.70 * (c - inner).max(0.0));
            }
        }
    }

    for (i, l) in labels.iter().take(n).enumerate() {
        let p = label_position(i, n);
        let col = if selected == Some(i) { [1.0; 3] } else { [0.0; 3] };
        match icons.and_then(|v| v.get(i)) {
            Some(id) => {
                draw_icon(&mut buf, id, C + p.x, C + p.y - 7.5, col);
                draw_text(&mut buf, l, C + p.x, C + p.y + 10.5, col, 10, 62);
            }
            None => draw_text(&mut buf, l, C + p.x, C + p.y, col, 13, 62),
        }
    }
    draw_text(&mut buf, center, C, C, [0.0; 3], 13, 62);

    buf.iter()
        .map(|p| {
            let q = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u32;
            (q(p[3]) << 24) | (q(p[0]) << 16) | (q(p[1]) << 8) | q(p[2])
        })
        .collect()
}

/// Premultiplied BGRA buffer to straight RGBA image.
pub fn to_image(px: &[u32]) -> image::RgbaImage {
    image::RgbaImage::from_fn(SIZE as u32, SIZE as u32, |x, y| {
        let v = px[y as usize * SIZE + x as usize];
        let a = v >> 24;
        let un = |c: u32| if a == 0 { 0 } else { ((c * 255 + a / 2) / a).min(255) as u8 };
        image::Rgba([un((v >> 16) & 255), un((v >> 8) & 255), un(v & 255), a as u8])
    })
}

/// Deterministic fake desktop for tests and CLI renders.
pub fn synthetic_backdrop() -> Vec<u32> {
    (0..N)
        .map(|i| {
            let (x, y) = ((i % SIZE) as u32, (i / SIZE) as u32);
            0xFF00_0000 | ((40 + x / 2) << 16) | ((90 + y / 3) << 8) | (180 - x / 3)
        })
        .collect()
}
