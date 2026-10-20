//! 60fps 960x640 showcase frame generator (`--render-showcase <out_dir>`): three wheels, camera zoom, keycaps, progress HUD.
use crate::renderer::*;
use clementine_core::*;
use std::collections::HashMap;
use std::path::Path;

const W: usize = 960;
const H: usize = 640;
const FRAMES: usize = 450;
const WC: (f32, f32) = (480.0, 290.0);

type Rgb = [f32; 3];

fn c8(r: u8, g: u8, b: u8) -> Rgb {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
fn sm(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
fn seg(u: f32, a: f32, b: f32) -> f32 {
    ((u - a) / (b - a)).clamp(0.0, 1.0)
}
fn blend(d: &mut Px, c: Rgb, a: f32) {
    for i in 0..3 {
        d[i] = c[i] * a + d[i] * (1.0 - a);
    }
    d[3] = 1.0;
}

fn rrect(b: &mut [Px], cx: f32, cy: f32, hw: f32, hh: f32, r: f32, col: Rgb, a: f32) {
    let (x0, x1) = (((cx - hw - 2.0) as i32).max(0), ((cx + hw + 2.0) as i32).min(W as i32 - 1));
    let (y0, y1) = (((cy - hh - 2.0) as i32).max(0), ((cy + hh + 2.0) as i32).min(H as i32 - 1));
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d = rounded_box_sdf((x as f32 + 0.5 - cx) as f64, (y as f32 + 0.5 - cy) as f64, hw as f64, hh as f64, r as f64);
            let c = (0.5 - d).clamp(0.0, 1.0) as f32;
            if c > 0.0 {
                blend(&mut b[y as usize * W + x as usize], col, c * a);
            }
        }
    }
}

fn ring(b: &mut [Px], cx: f32, cy: f32, hw: f32, hh: f32, r: f32, width: f32, col: Rgb, a: f32) {
    for y in 0..H {
        for x in 0..W {
            let d = rounded_box_sdf((x as f32 + 0.5 - cx) as f64, (y as f32 + 0.5 - cy) as f64, hw as f64, hh as f64, r as f64) as f32;
            let c = (width / 2.0 + 0.5 - (d + width / 2.0).abs()).clamp(0.0, 1.0);
            if c > 0.0 {
                blend(&mut b[y * W + x], col, c * a);
            }
        }
    }
}

fn shadow(b: &mut [Px], cx: f32, cy: f32, hw: f32, hh: f32, r: f32, dy: f32, blur: f32, a: f32) {
    let m = blur + 2.0;
    let (x0, x1) = (((cx - hw - m) as i32).max(0), ((cx + hw + m) as i32).min(W as i32 - 1));
    let (y0, y1) = (((cy + dy - hh - m) as i32).max(0), ((cy + dy + hh + m) as i32).min(H as i32 - 1));
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d = rounded_box_sdf((x as f32 + 0.5 - cx) as f64, (y as f32 + 0.5 - cy - dy) as f64, hw as f64, hh as f64, r as f64) as f32;
            blend(&mut b[y as usize * W + x as usize], c8(38, 52, 56), sm((blur - d) / (2.0 * blur)) * a);
        }
    }
}

fn disc(b: &mut [Px], cx: f32, cy: f32, r: f32, col: Rgb, a: f32) {
    rrect(b, cx, cy, r, r, r, col, a);
}

fn tri(b: &mut [Px], p: [(f32, f32); 3], col: Rgb, a: f32) {
    let poly: Vec<Point2D> = p.iter().map(|&(x, y)| Point2D { x: x as f64, y: y as f64 }).collect();
    let (x0, x1) = (p.iter().map(|q| q.0).fold(f32::MAX, f32::min) as i32 - 1, p.iter().map(|q| q.0).fold(f32::MIN, f32::max) as i32 + 1);
    let (y0, y1) = (p.iter().map(|q| q.1).fold(f32::MAX, f32::min) as i32 - 1, p.iter().map(|q| q.1).fold(f32::MIN, f32::max) as i32 + 1);
    for y in y0.max(0)..=y1.min(H as i32 - 1) {
        for x in x0.max(0)..=x1.min(W as i32 - 1) {
            let mut n = 0;
            for sy in 0..3 {
                for sx in 0..3 {
                    n += inside(&poly, x as f64 + (sx as f64 + 0.5) / 3.0, y as f64 + (sy as f64 + 0.5) / 3.0) as u32;
                }
            }
            if n > 0 {
                blend(&mut b[y as usize * W + x as usize], col, a * n as f32 / 9.0);
            }
        }
    }
}

fn inside(poly: &[Point2D], x: f64, y: f64) -> bool {
    let mut ins = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, c) = (poly[i], poly[j]);
        if (a.y > y) != (c.y > y) && x < (c.x - a.x) * (y - a.y) / (c.y - a.y) + a.x {
            ins = !ins;
        }
        j = i;
    }
    ins
}

#[derive(Default)]
struct Ctx {
    masks: HashMap<(String, i32, i32, bool), Mask>,
    wheels: HashMap<(usize, Option<usize>), Vec<u32>>,
}

impl Ctx {
    fn text(&mut self, b: &mut [Px], s: &str, px: i32, weight: i32, symbol: bool, cx: f32, cy: f32, col: Rgb) {
        let m = self.masks.entry((s.to_string(), px, weight, symbol)).or_insert_with(|| text_mask(s, px, weight, if symbol { "Segoe UI Symbol" } else { "Segoe UI" }));
        blit_mask_sized(b, W, H, m, cx as f64, cy as f64, col);
    }
    fn width(&mut self, s: &str, px: i32, weight: i32) -> f32 {
        self.masks.entry((s.to_string(), px, weight, false)).or_insert_with(|| text_mask(s, px, weight, "Segoe UI")).w as f32
    }
    fn pill(&mut self, b: &mut [Px], s: &str, cx: f32, cy: f32, blue: bool, a: f32) {
        let hw = self.width(s, 11, 600) / 2.0 + 8.0;
        let (bg, fg) = if blue { (c8(47, 111, 235), [1.0; 3]) } else { (c8(255, 255, 255), c8(30, 32, 31)) };
        shadow(b, cx, cy, hw, 10.0, 10.0, 2.0, 5.0, 0.15 * a);
        rrect(b, cx, cy, hw, 10.0, 10.0, bg, 0.92 * a);
        self.text(b, s, 11, 600, false, cx, cy, fg);
    }
    fn card(&mut self, b: &mut [Px], x: f32, y: f32, name: &str, mov: bool, a: f32, blue: bool) {
        shadow(b, x, y, 34.0, 34.0, 14.0, 5.0, 8.0, 0.22 * a);
        rrect(b, x, y, 34.0, 34.0, 14.0, [1.0; 3], 0.96 * a);
        let top = if mov { c8(130, 110, 205) } else { c8(96, 168, 240) };
        rrect(b, x, y, 28.0, 28.0, 10.0, top, a);
        rrect(b, x, y + 14.0, 28.0, 14.0, 10.0, if mov { c8(90, 72, 170) } else { c8(60, 130, 90) }, a * 0.9);
        if mov {
            tri(b, [(x - 7.0, y - 11.0), (x - 7.0, y + 11.0), (x + 11.0, y)], [1.0; 3], a);
        } else {
            disc(b, x + 10.0, y - 12.0, 5.0, c8(255, 235, 150), a);
            tri(b, [(x - 22.0, y + 22.0), (x - 4.0, y - 4.0), (x + 14.0, y + 22.0)], [1.0; 3], a * 0.9);
        }
        self.pill(b, name, x, y + 52.0, blue, a);
    }
}

fn background() -> Vec<Px> {
    let mut v = vec![[0f32; 4]; W * H];
    for y in 0..H {
        for x in 0..W {
            let (u, t) = (x as f32 / (W - 1) as f32, y as f32 / (H - 1) as f32);
            let w = [(1.0 - u) * (1.0 - t), (1.0 - u) * t, u * (1.0 - t), u * t];
            let ch = |c: [f32; 4]| (c[0] * w[0] + c[1] * w[1] + c[2] * w[2] + c[3] * w[3]) / 255.0;
            // #F6B026 TL, #F75C42 BL, #D6803F TR (blend), #9E84CA BR
            v[y * W + x] = [ch([246.0, 247.0, 214.0, 158.0]), ch([176.0, 92.0, 128.0, 132.0]), ch([38.0, 66.0, 64.0, 202.0]), 1.0];
        }
    }
    ring(&mut v, 480.0, 320.0, 466.0, 306.0, 28.0, 2.0, [1.0; 3], 0.35);
    v
}

fn cursor(b: &mut [Px], x: f32, y: f32) {
    let pts: [(f32, f32); 7] = [(0.0, 0.0), (0.0, 20.0), (4.5, 15.5), (8.0, 24.0), (11.5, 22.5), (8.0, 14.5), (14.0, 14.5)];
    let poly: Vec<Point2D> = pts.iter().map(|&(a, c)| Point2D { x: a as f64, y: c as f64 }).collect();
    for py in -2..28 {
        for px in -2..18 {
            let (mut wh, mut bk) = (0.0, 0.0);
            for sy in 0..3 {
                for sx in 0..3 {
                    let (qx, qy) = (px as f64 + (sx as f64 + 0.5) / 3.0, py as f64 + (sy as f64 + 0.5) / 3.0);
                    let ins = inside(&poly, qx, qy);
                    let dmin = (0..7).map(|i| seg_dist(qx as f32, qy as f32, pts[i].0, pts[i].1, pts[(i + 1) % 7].0, pts[(i + 1) % 7].1)).fold(f32::MAX, f32::min);
                    if ins || dmin < 1.3 {
                        wh += 1.0;
                    }
                    if ins && dmin > 0.6 {
                        bk += 1.0;
                    }
                }
            }
            let (gx, gy) = ((x + px as f32).round() as i32, (y + py as f32).round() as i32);
            if gx >= 0 && gy >= 0 && gx < W as i32 && gy < H as i32 {
                let d = &mut b[gy as usize * W + gx as usize];
                blend(d, [1.0; 3], wh / 9.0);
                blend(d, [0.04; 3], bk / 9.0);
            }
        }
    }
}

fn composite(b: &mut [Px], f: &[u32], scale: f32, alpha: f32, cx: f32, cy: f32) {
    let s = SIZE as i32;
    for y in 0..s {
        for x in 0..s {
            let sx = ((x as f32 - 136.0) / scale + 136.0).round() as i32;
            let sy = ((y as f32 - 136.0) / scale + 136.0).round() as i32;
            if sx < 0 || sy < 0 || sx >= s || sy >= s {
                continue;
            }
            let p = f[sy as usize * SIZE + sx as usize];
            let a = (p >> 24) as f32 / 255.0 * alpha;
            if a <= 0.0 {
                continue;
            }
            let (gx, gy) = (cx as i32 - 136 + x, cy as i32 - 136 + y);
            if gx < 0 || gy < 0 || gx >= W as i32 || gy >= H as i32 {
                continue;
            }
            let d = &mut b[gy as usize * W + gx as usize];
            let q = |sh: u32| ((p >> sh) & 255) as f32 / 255.0 * alpha;
            let k = 1.0 - a;
            *d = [q(16) + d[0] * k, q(8) + d[1] * k, q(0) + d[2] * k, 1.0];
        }
    }
}

struct Act {
    mode: usize,
    ids: Vec<String>,
    labels: Vec<String>,
    a: usize,
    b: usize,
    key: usize,
    card: (f32, f32),
    name: &'static str,
    zmax: f32,
    hud: (&'static str, &'static str, &'static str),
    start: (f32, f32),
    end: usize,
    len: usize,
}

fn petal_pos(n: usize, i: usize) -> (f32, f32) {
    let p = label_position(i, n);
    (WC.0 + p.x as f32, WC.1 + p.y as f32)
}

fn build_acts() -> Vec<Act> {
    let conv: Vec<String> = conversion_targets("png").into_iter().take(9).map(|s| s.to_string()).collect();
    let clabels: Vec<String> = conv.iter().map(|s| format_label(s).to_string()).collect();
    let tools: Vec<String> = tool_targets("jpg", 1).into_iter().take(9).map(|s| s.to_string()).collect();
    let acts: Vec<String> = action_targets().into_iter().take(9).map(|s| s.to_string()).collect();
    let title = |v: &[String]| v.iter().map(|id| tool_meta(id).map(|t| t.title.to_string()).unwrap_or_else(|| id.clone())).collect::<Vec<_>>();
    let idx = |v: &[String], k: &str, d: usize| v.iter().position(|s| s == k).unwrap_or(d.min(v.len().saturating_sub(1)));
    let (tl, al) = (title(&tools), title(&acts));
    let (ja, jb) = (idx(&conv, "jpg", 0), idx(&conv, "webp", 1));
    let (ta, tb) = (idx(&tools, "compress", 0), idx(&tools, "removeMetadata", 1));
    let (aa, ab) = (idx(&acts, "airDrop", 0), idx(&acts, "float", 1));
    let s1 = (720.0, 560.0);
    let s2 = petal_pos(conv.len(), jb);
    let s3 = petal_pos(tools.len(), tb);
    vec![
        Act { mode: 0, ids: conv, labels: clabels, a: ja, b: jb, key: 0, card: (140.0, 140.0), name: "holiday.png", zmax: 1.28, hud: ("CONVERTING TO WEBP", "holiday.webp", "SAVED holiday.webp"), start: s1, end: 150, len: 150 },
        Act { mode: 1, ids: tools, labels: tl, a: ta, b: tb, key: 1, card: (140.0, 300.0), name: "portrait.jpg", zmax: 1.28, hud: ("STRIPPING METADATA", "Removing EXIF, camera & GPS data...", "\u{2713} 0 METADATA TAGS \u{b7} SAVED"), start: s2, end: 310, len: 160 },
        Act { mode: 2, ids: acts, labels: al, a: aa, b: ab, key: 2, card: (140.0, 460.0), name: "weekend.mov", zmax: 1.25, hud: ("PINNING TO FLOAT SHELF", "weekend.mov", "\u{2713} PINNED 1 FILE TO SHELF"), start: s3, end: 450, len: 140 },
    ]
}

fn scene(f: usize, acts: &[Act], bg: &[Px], base: &[Px], cx: &mut Ctx) -> (Vec<Px>, f32, (f32, f32), (f32, f32)) {
    let ai = acts.iter().position(|a| f < a.end).unwrap_or(2);
    let act = &acts[ai];
    let u = (f - (act.end - act.len)) as f32 / act.len as f32;
    let mut b = bg.to_vec();
    let n = act.ids.len();

    // desktop cards and finished artifacts of earlier/current acts
    let fin = |a: &Act| ((f as f32 - (a.end as f32 - a.len as f32 * 0.1)) / 12.0).clamp(0.0, 1.0);
    for (i, a) in acts.iter().enumerate() {
        cx.card(&mut b, a.card.0, a.card.1, a.name, i == 2, 1.0, false);
    }
    let k = sm(fin(&acts[0]));
    if k > 0.0 {
        cx.card(&mut b, 250.0, 140.0, "holiday.webp", false, k, false);
    }
    let k = sm(fin(&acts[1]));
    if k > 0.0 {
        cx.card(&mut b, 250.0, 300.0, "portrait No Metadata.jpg", false, k, false);
    }
    let shelf = sm(fin(&acts[2]));
    if shelf > 0.0 {
        let x = lerp(1010.0, 838.0, shelf);
        shadow(&mut b, x, 300.0, 96.0, 70.0, 22.0, 6.0, 12.0, 0.25);
        rrect(&mut b, x, 300.0, 96.0, 70.0, 22.0, c8(255, 252, 249), 0.92);
        ring(&mut b, x, 300.0, 96.0, 70.0, 22.0, 1.5, [1.0; 3], 0.9);
        cx.text(&mut b, "FLOAT SHELF", 12, 700, false, x, 262.0, c8(30, 32, 31));
        cx.card(&mut b, x, 300.0, "weekend.mov", true, 1.0, false);
    }

    // cursor path
    let (ca, cb) = (petal_pos(n, act.a), petal_pos(n, act.b));
    let ease = |p: (f32, f32), q: (f32, f32), t: f32| (lerp(p.0, q.0, sm(t)), lerp(p.1, q.1, sm(t)));
    let grab = (act.card.0, act.card.1);
    let cur = if u < 0.05 {
        ease(act.start, grab, u / 0.05)
    } else if u < 0.2 {
        ease(grab, WC, seg(u, 0.05, 0.2))
    } else if u < 0.34 {
        WC
    } else if u < 0.46 {
        ease(WC, ca, seg(u, 0.34, 0.46))
    } else if u < 0.52 {
        ca
    } else if u < 0.64 {
        ease(ca, cb, seg(u, 0.52, 0.64))
    } else {
        cb
    };
    let dropped = u >= 0.72;
    let sel = if u >= 0.2 && !dropped { hit_test_petal((cur.0 - WC.0) as f64, (cur.1 - WC.1) as f64, n) } else { None };

    if u >= 0.2 && !dropped {
        let fr = cx.wheels.entry((act.mode, sel)).or_insert_with(|| {
            let center = sel.map(|i| act.labels[i].clone()).unwrap_or_else(|| "2.2 MB".into());
            let icons = (act.mode != 0).then_some(act.ids.as_slice());
            render(base, &act.labels, icons, sel, &center)
        });
        let e = bloom_ease(seg(u, 0.2, 0.32) as f64) as f32;
        composite(&mut b, fr, 0.45 + 0.55 * e, e, WC.0, WC.1);
    }
    if dropped {
        let p = sm(seg(u, 0.72, 0.88));
        let done = u >= 0.88;
        let sub = if done { act.hud.2.to_string() } else if ai == 0 { act.hud.1.to_string() } else { act.hud.1.to_string() };
        let fr = render_progress_hud(base, act.hud.0, &sub, p, done);
        composite(&mut b, &fr, 1.0, 1.0, WC.0, WC.1);
    }

    // keycap
    let press = if (0.12..0.95).contains(&u) { sm(seg(u, 0.12, 0.18)) } else { 0.0 };
    let ka = sm(seg(u, 0.08, 0.12)) * (1.0 - sm(seg(u, 0.95, 1.0)));
    if ka > 0.0 {
        let (kx, ky) = (480.0, 590.0);
        let accent = c8(213, 82, 35);
        if press > 0.0 {
            shadow(&mut b, kx, ky, 58.0, 28.0, 12.0, 0.0, 14.0, 0.0);
            disc(&mut b, kx, ky, 0.0, accent, 0.0);
        }
        rrect(&mut b, kx, ky + 3.0, 58.0, 28.0, 12.0, c8(220, 225, 219), ka);
        let face_y = ky - 4.0 + 4.0 * press;
        shadow(&mut b, kx, face_y, 58.0, 28.0, 12.0, 4.0 * (1.0 - press), 6.0, 0.2 * ka);
        let face = [lerp(0.99, 1.0, press), lerp(0.99, 0.94, press), lerp(0.98, 0.894, press)];
        rrect(&mut b, kx, face_y, 58.0, 28.0, 12.0, face, ka);
        let tc = [lerp(0.12, accent[0], press), lerp(0.13, accent[1], press), lerp(0.12, accent[2], press)];
        let (sym, word) = [("\u{21e7}", "shift"), ("\u{2387}", "alt"), ("\u{2303}", "ctrl")][act.key];
        cx.text(&mut b, sym, 22, 400, true, kx - 24.0, face_y, tc);
        cx.text(&mut b, word, 18, 600, false, kx + 12.0, face_y, tc);
    }

    // dragged ghost
    if (0.05..0.72).contains(&u) {
        cx.card(&mut b, cur.0 + 12.0, cur.1 + 12.0, act.name, act.mode == 2, 0.85, true);
        if sel == Some(act.b) {
            disc(&mut b, cur.0 + 40.0, cur.1 - 16.0, 9.0, c8(30, 174, 91), 1.0);
            rrect(&mut b, cur.0 + 40.0, cur.1 - 16.0, 5.0, 1.2, 0.6, [1.0; 3], 1.0);
            rrect(&mut b, cur.0 + 40.0, cur.1 - 16.0, 1.2, 5.0, 0.6, [1.0; 3], 1.0);
        }
    }

    let za = sm(seg(u, 0.2, 0.4)) * (1.0 - sm(seg(u, 0.78, 0.98)));
    let z = 1.0 + (act.zmax - 1.0) * za;
    (b, z, (lerp(480.0, WC.0, za), lerp(320.0, WC.1, za)), cur)
}

pub fn render_showcase_frames(out_dir: &Path) -> std::result::Result<(), String> {
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let bg = background();
    // wheel backdrop = the sunset gradient behind the wheel
    let backdrop: Vec<u32> = (0..SIZE * SIZE)
        .map(|i| {
            let p = bg[(WC.1 as usize - 136 + i / SIZE) * W + WC.0 as usize - 136 + i % SIZE];
            let q = |v: f32| (v * 255.0) as u32;
            0xFF00_0000 | (q(p[0]) << 16) | (q(p[1]) << 8) | q(p[2])
        })
        .collect();
    let base = make_base(Some(&backdrop));
    let acts = build_acts();
    let mut cx = Ctx::default();
    for f in 0..FRAMES {
        let (sc, z, (fx, fy), cur) = scene(f, &acts, &bg, &base, &mut cx);
        let mut img = image::RgbImage::new(W as u32, H as u32);
        for y in 0..H {
            for x in 0..W {
                let (sx, sy) = (fx + (x as f32 + 0.5 - W as f32 / 2.0) / z - 0.5, fy + (y as f32 + 0.5 - H as f32 / 2.0) / z - 0.5);
                let (x0, y0) = (sx.floor(), sy.floor());
                let (tx, ty) = (sx - x0, sy - y0);
                let g = |dx: i32, dy: i32| sc[((y0 as i32 + dy).clamp(0, H as i32 - 1) as usize) * W + (x0 as i32 + dx).clamp(0, W as i32 - 1) as usize];
                let (p00, p10, p01, p11) = (g(0, 0), g(1, 0), g(0, 1), g(1, 1));
                let mut o = [0u8; 3];
                for i in 0..3 {
                    let v = lerp(lerp(p00[i], p10[i], tx), lerp(p01[i], p11[i], tx), ty);
                    o[i] = (v * 255.0).round().clamp(0.0, 255.0) as u8;
                }
                img.put_pixel(x as u32, y as u32, image::Rgb(o));
            }
        }
        // cursor stays crisp: drawn in screen space after the zoom
        let mut px: Vec<Px> = img.pixels().map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, 1.0]).collect();
        cursor(&mut px, W as f32 / 2.0 + (cur.0 - fx) * z, H as f32 / 2.0 + (cur.1 - fy) * z);
        for (p, o) in px.iter().zip(img.pixels_mut()) {
            *o = image::Rgb([0, 1, 2].map(|i| (p[i] * 255.0).round().clamp(0.0, 255.0) as u8));
        }
        img.save(out_dir.join(format!("frame_{f:04}.png"))).map_err(|e| e.to_string())?;
    }
    Ok(())
}
