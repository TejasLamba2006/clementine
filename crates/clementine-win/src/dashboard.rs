//! Native Clementine Dashboard window (`760x520` layered GDI window).
//! Inspired by OG Converter's Home/Settings dashboard while keeping <9 MB RAM footprint.
//! Closing (`x`) hides to the System Tray so Clementine keeps running in the background.
use crate::renderer::{blit_mask_sized, fit_mask, rounded_box_sdf, seg_dist, Dib, Px};
use crate::{hook, install};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Mutex;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_LWIN, VK_MENU, VK_SHIFT, VK_SPACE};
use windows_sys::Win32::UI::Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, ShellExecuteW, HDROP};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const DW: usize = 760;
pub const DH: usize = 520;

#[derive(Clone)]
pub struct ActivityItem {
    pub title: String,
    pub detail: String,
    pub path: PathBuf,
}

static RECENT: Mutex<Vec<ActivityItem>> = Mutex::new(Vec::new());
static UPDATE_STATUS: Mutex<String> = Mutex::new(String::new());
static SOUND_ENABLED: AtomicBool = AtomicBool::new(true);
static DASH_HWND: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static WHEEL_HWND: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn is_sound_enabled() -> bool {
    SOUND_ENABLED.load(Relaxed)
}

pub fn set_update_status(status: String) {
    *UPDATE_STATUS.lock().unwrap() = status;
    refresh();
}

pub fn record_activity(title: String, detail: String, path: PathBuf) {
    let mut list = RECENT.lock().unwrap();
    list.insert(0, ActivityItem { title, detail, path });
    if list.len() > 5 {
        list.truncate(5);
    }
    drop(list);
    refresh();
}

pub fn refresh() {
    let h = DASH_HWND.load(Relaxed) as HWND;
    if !h.is_null() && unsafe { IsWindowVisible(h) } != 0 {
        present_dashboard(h);
    }
}

fn w16(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
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
    let x1 = ((cx + hw + 2.0).ceil() as usize).min(DW);
    let y0 = ((cy - hh - 2.0).floor() as i32).max(0) as usize;
    let y1 = ((cy + hh + 2.0).ceil() as usize).min(DH);
    for y in y0..y1 {
        for x in x0..x1 {
            let d = rounded_box_sdf(x as f64 + 0.5 - cx, y as f64 + 0.5 - cy, hw, hh, r);
            let c = (0.5 - d).clamp(0.0, 1.0) as f32;
            if c > 0.0 {
                over(&mut buf[y * DW + x], col, alpha * c);
            }
        }
    }
}

fn stroke_rrect(buf: &mut [Px], cx: f64, cy: f64, hw: f64, hh: f64, r: f64, w: f64, col: [f32; 3], alpha: f32) {
    let x0 = ((cx - hw - w - 2.0).floor() as i32).max(0) as usize;
    let x1 = ((cx + hw + w + 2.0).ceil() as usize).min(DW);
    let y0 = ((cy - hh - w - 2.0).floor() as i32).max(0) as usize;
    let y1 = ((cy + hh + w + 2.0).ceil() as usize).min(DH);
    for y in y0..y1 {
        for x in x0..x1 {
            let d = rounded_box_sdf(x as f64 + 0.5 - cx, y as f64 + 0.5 - cy, hw, hh, r).abs() - w * 0.5;
            let c = (0.5 - d).clamp(0.0, 1.0) as f32;
            if c > 0.0 {
                over(&mut buf[y * DW + x], col, alpha * c);
            }
        }
    }
}

fn fill_disc(buf: &mut [Px], cx: f64, cy: f64, r: f64, col: [f32; 3], alpha: f32) {
    let x0 = ((cx - r - 2.0).floor() as i32).max(0) as usize;
    let x1 = ((cx + r + 2.0).ceil() as usize).min(DW);
    let y0 = ((cy - r - 2.0).floor() as i32).max(0) as usize;
    let y1 = ((cy + r + 2.0).ceil() as usize).min(DH);
    for y in y0..y1 {
        for x in x0..x1 {
            let d = (x as f64 + 0.5 - cx).hypot(y as f64 + 0.5 - cy) - r;
            let c = (0.5 - d).clamp(0.0, 1.0) as f32;
            if c > 0.0 {
                over(&mut buf[y * DW + x], col, alpha * c);
            }
        }
    }
}

fn vk_name(vk: u16) -> &'static str {
    match vk {
        VK_SHIFT => "Shift",
        VK_MENU => "Alt",
        VK_CONTROL => "Ctrl",
        VK_LWIN => "Win",
        VK_SPACE => "Space",
        _ => "Key",
    }
}

fn next_vk(vk: u16) -> u16 {
    match vk {
        VK_SHIFT => VK_MENU,
        VK_MENU => VK_CONTROL,
        VK_CONTROL => VK_SPACE,
        VK_SPACE => VK_LWIN,
        _ => VK_SHIFT,
    }
}

pub fn render_dashboard() -> Vec<u32> {
    let mut buf = vec![[0f32; 4]; DW * DH];
    let orange = c8(255, 82, 0);
    let green = c8(34, 197, 94);
    let card_bg = c8(28, 29, 33);
    let card_border = c8(52, 54, 62);

    // 1. Main dark obsidian surface (760x520, rx=20)
    fill_rrect(&mut buf, DW as f64 * 0.5, DH as f64 * 0.5, DW as f64 * 0.5 - 2.0, DH as f64 * 0.5 - 2.0, 20.0, c8(19, 20, 23), 0.98);
    stroke_rrect(&mut buf, DW as f64 * 0.5, DH as f64 * 0.5, DW as f64 * 0.5 - 2.5, DH as f64 * 0.5 - 2.5, 20.0, 1.2, c8(64, 66, 74), 0.75);

    // 2. Top Bar: Logo + Title + Listening Badge + Preview Wheel Button + Close (hide to tray)
    fill_disc(&mut buf, 34.0, 30.0, 11.0, orange, 1.0);
    fill_disc(&mut buf, 34.0, 30.0, 4.0, c8(19, 20, 23), 1.0);
    blit_mask_sized(&mut buf, DW, DH, &fit_mask("Clementine", 16, 700, 140), 98.0, 30.0, [1.0; 3]);

    // Status pill: "Listening for shortcuts"
    fill_rrect(&mut buf, 466.0, 30.0, 88.0, 14.0, 14.0, c8(20, 52, 34), 0.95);
    stroke_rrect(&mut buf, 466.0, 30.0, 88.0, 14.0, 14.0, 1.0, green, 0.45);
    fill_disc(&mut buf, 394.0, 30.0, 4.0, green, 1.0);
    blit_mask_sized(&mut buf, DW, DH, &fit_mask("Listening for shortcuts", 11, 600, 150), 474.0, 30.0, c8(134, 239, 172));

    // Preview Wheel button (574..694, 16..44)
    fill_rrect(&mut buf, 634.0, 30.0, 60.0, 14.0, 8.0, orange, 0.95);
    blit_mask_sized(&mut buf, DW, DH, &fit_mask("Preview Wheel", 11, 700, 108), 634.0, 30.0, [1.0; 3]);

    // Close to tray button `x` at (726, 30)
    fill_disc(&mut buf, 726.0, 30.0, 13.0, card_bg, 1.0);
    for y in 22..39 {
        for x in 718..735 {
            let d = seg_dist(x as f32 + 0.5, y as f32 + 0.5, 722.0, 26.0, 730.0, 34.0)
                .min(seg_dist(x as f32 + 0.5, y as f32 + 0.5, 730.0, 26.0, 722.0, 34.0));
            let c = (1.3 - d).clamp(0.0, 1.0);
            if c > 0.0 {
                over(&mut buf[y * DW + x], c8(210, 214, 220), c);
            }
        }
    }

    // 3. Three Shortcut Cards (y = 66..148)
    let (k_conv, k_tools, k_act) = hook::get_key_bindings();
    let cards = [
        (142.0, "Convert", "275 format paths", vk_name(k_conv), c8(255, 82, 0)),
        (380.0, "File Tools", "27 local tools + Remove BG", vk_name(k_tools), c8(245, 158, 11)),
        (618.0, "File Actions", "Share, Float, Copy, Path", vk_name(k_act), c8(139, 92, 246)),
    ];
    for (cx, title, sub, kname, accent) in cards {
        fill_rrect(&mut buf, cx, 106.0, 112.0, 38.0, 12.0, card_bg, 1.0);
        stroke_rrect(&mut buf, cx, 106.0, 112.0, 38.0, 12.0, 1.0, card_border, 0.85);
        fill_rrect(&mut buf, cx - 84.0, 106.0, 16.0, 16.0, 8.0, accent, 0.18);
        fill_disc(&mut buf, cx - 84.0, 106.0, 5.0, accent, 1.0);
        blit_mask_sized(&mut buf, DW, DH, &fit_mask(title, 13, 700, 110), cx - 18.0, 97.0, [1.0; 3]);
        blit_mask_sized(&mut buf, DW, DH, &fit_mask(sub, 10, 400, 116), cx - 16.0, 116.0, c8(156, 163, 175));
        // Key pill on right of card (clickable to cycle key)
        fill_rrect(&mut buf, cx + 76.0, 106.0, 26.0, 14.0, 7.0, c8(38, 40, 46), 1.0);
        stroke_rrect(&mut buf, cx + 76.0, 106.0, 26.0, 14.0, 7.0, 1.0, accent, 0.65);
        blit_mask_sized(&mut buf, DW, DH, &fit_mask(kname, 11, 700, 46), cx + 76.0, 106.0, [1.0; 3]);
    }

    // 4. Left Panel: "Try it here - Drop a file" (x = 30..370, y = 164..468)
    fill_rrect(&mut buf, 200.0, 316.0, 170.0, 152.0, 14.0, card_bg, 1.0);
    stroke_rrect(&mut buf, 200.0, 316.0, 170.0, 152.0, 14.0, 1.0, card_border, 0.85);
    blit_mask_sized(&mut buf, DW, DH, &fit_mask("Try it here", 14, 700, 160), 82.0, 190.0, [1.0; 3]);
    blit_mask_sized(&mut buf, DW, DH, &fit_mask("Drop any file onto this card or drag in File Explorer and tap Shift / Alt / Ctrl.", 10, 400, 310), 200.0, 212.0, c8(156, 163, 175));

    // Inner drop target box (200, 336, hw=146, hh=98)
    fill_rrect(&mut buf, 200.0, 338.0, 146.0, 98.0, 12.0, c8(22, 23, 27), 1.0);
    stroke_rrect(&mut buf, 200.0, 338.0, 146.0, 98.0, 12.0, 1.2, orange, 0.45);
    fill_disc(&mut buf, 200.0, 296.0, 20.0, orange, 0.16);
    fill_disc(&mut buf, 200.0, 296.0, 8.0, orange, 1.0);
    blit_mask_sized(&mut buf, DW, DH, &fit_mask("Drop a file here to open its wheel", 13, 700, 260), 200.0, 338.0, [1.0; 3]);
    blit_mask_sized(&mut buf, DW, DH, &fit_mask("Images, video, audio, PDF, docs, sheets, subtitles, archives", 10, 400, 270), 200.0, 358.0, c8(148, 154, 166));

    // "Test Convert Wheel" button inside drop zone (200, 398, hw=76, hh=16)
    fill_rrect(&mut buf, 200.0, 398.0, 78.0, 16.0, 8.0, orange, 0.96);
    blit_mask_sized(&mut buf, DW, DH, &fit_mask("Open Demo Wheel", 11, 700, 140), 200.0, 398.0, [1.0; 3]);

    // 5. Right Panel: Recent Activity & Settings (x = 390..730, y = 164..468)
    fill_rrect(&mut buf, 560.0, 274.0, 170.0, 110.0, 14.0, card_bg, 1.0);
    stroke_rrect(&mut buf, 560.0, 274.0, 170.0, 110.0, 14.0, 1.0, card_border, 0.85);
    blit_mask_sized(&mut buf, DW, DH, &fit_mask("Recent activity", 14, 700, 160), 458.0, 190.0, [1.0; 3]);

    let items = RECENT.lock().unwrap().clone();
    if items.is_empty() {
        blit_mask_sized(&mut buf, DW, DH, &fit_mask("No conversions yet in this session.", 11, 600, 260), 560.0, 268.0, c8(148, 154, 166));
        blit_mask_sized(&mut buf, DW, DH, &fit_mask("Drag any file in Explorer + tap Shift to convert.", 10, 400, 280), 560.0, 290.0, c8(115, 120, 132));
    } else {
        for (i, item) in items.iter().take(4).enumerate() {
            let ry = 224.0 + i as f64 * 36.0;
            fill_rrect(&mut buf, 560.0, ry, 152.0, 15.0, 8.0, c8(36, 38, 44), 1.0);
            fill_disc(&mut buf, 424.0, ry, 5.0, green, 1.0);
            blit_mask_sized(&mut buf, DW, DH, &fit_mask(&item.title, 11, 600, 170), 518.0, ry, [1.0; 3]);
            blit_mask_sized(&mut buf, DW, DH, &fit_mask(&item.detail, 10, 600, 95), 656.0, ry, orange);
        }
    }

    // Settings Box (x = 390..730, y = 394..468)
    fill_rrect(&mut buf, 560.0, 431.0, 170.0, 37.0, 14.0, card_bg, 1.0);
    stroke_rrect(&mut buf, 560.0, 431.0, 170.0, 37.0, 14.0, 1.0, card_border, 0.85);
    let startup_on = install::is_startup_enabled();
    let sound_on = SOUND_ENABLED.load(Relaxed);
    let upd_txt = {
        let s = UPDATE_STATUS.lock().unwrap().clone();
        if s.is_empty() { "Check Updates".to_string() } else { s }
    };

    // Toggle 1: Run at Windows Startup (448, 431)
    fill_rrect(&mut buf, 448.0, 431.0, 50.0, 18.0, 9.0, if startup_on { c8(22, 62, 38) } else { c8(38, 40, 46) }, 1.0);
    stroke_rrect(&mut buf, 448.0, 431.0, 50.0, 18.0, 9.0, 1.0, if startup_on { green } else { card_border }, 0.8);
    blit_mask_sized(&mut buf, DW, DH, &fit_mask(if startup_on { "Startup: ON" } else { "Startup: OFF" }, 10, 700, 92), 448.0, 431.0, [1.0; 3]);

    // Toggle 2: Sound Feedback (556, 431)
    fill_rrect(&mut buf, 556.0, 431.0, 48.0, 18.0, 9.0, if sound_on { c8(68, 34, 20) } else { c8(38, 40, 46) }, 1.0);
    stroke_rrect(&mut buf, 556.0, 431.0, 48.0, 18.0, 9.0, 1.0, if sound_on { orange } else { card_border }, 0.8);
    blit_mask_sized(&mut buf, DW, DH, &fit_mask(if sound_on { "Sound: ON" } else { "Sound: OFF" }, 10, 700, 88), 556.0, 431.0, [1.0; 3]);

    // Button 3: Check for Updates from GitHub (666, 431)
    fill_rrect(&mut buf, 666.0, 431.0, 54.0, 18.0, 9.0, c8(38, 40, 46), 1.0);
    stroke_rrect(&mut buf, 666.0, 431.0, 54.0, 18.0, 9.0, 1.0, orange, 0.75);
    blit_mask_sized(&mut buf, DW, DH, &fit_mask(&upd_txt, 10, 700, 100), 666.0, 431.0, [1.0; 3]);

    // Footer status line
    blit_mask_sized(
        &mut buf,
        DW,
        DH,
        &fit_mask("Closing this window keeps Clementine running in your System Tray. Right-click the tray icon to Quit.", 10, 400, 680),
        DW as f64 * 0.5,
        494.0,
        c8(130, 136, 148),
    );

    buf.iter()
        .map(|p| {
            let q = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u32;
            (q(p[3]) << 24) | (q(p[0]) << 16) | (q(p[1]) << 8) | q(p[2])
        })
        .collect()
}

pub fn to_dashboard_image(px: &[u32]) -> image::RgbaImage {
    image::RgbaImage::from_fn(DW as u32, DH as u32, |x, y| {
        let v = px[y as usize * DW + x as usize];
        let a = v >> 24;
        let un = |c: u32| if a == 0 { 0 } else { ((c * 255 + a / 2) / a).min(255) as u8 };
        image::Rgba([un((v >> 16) & 255), un((v >> 8) & 255), un(v & 255), a as u8])
    })
}

fn present_dashboard(hwnd: HWND) {
    let px = render_dashboard();
    let mut d = Dib::sized(DW, DH);
    d.slice().copy_from_slice(&px);
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut rc);
        let pt = POINT { x: rc.left, y: rc.top };
        let sz = SIZE { cx: DW as i32, cy: DH as i32 };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        UpdateLayeredWindow(hwnd, null_mut(), &pt, &sz, d.dc, &src, 0, &blend, ULW_ALPHA);
    }
}

unsafe extern "system" fn dash_proc(hwnd: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let mx = (lp as i16) as i32;
    let my = ((lp >> 16) as i16) as i32;
    match m {
        WM_LBUTTONDOWN => {
            // 1. Close-to-tray button `x` at (726, 30) -> hides dashboard, keeps tray running!
            if (mx - 726) * (mx - 726) + (my - 30) * (my - 30) <= 225 {
                ShowWindow(hwnd, SW_HIDE);
                return 0;
            }
            // 2. Preview Wheel button (574..694, 16..44) or Open Demo Wheel button (122..278, 380..416)
            if ((574..=694).contains(&mx) && (16..=44).contains(&my))
                || ((122..=278).contains(&mx) && (380..=416).contains(&my))
            {
                let wh = WHEEL_HWND.load(Relaxed) as HWND;
                if !wh.is_null() {
                    PostMessageW(wh, WM_COMMAND, 1, 0);
                }
                return 0;
            }
            // 3. Keybinding cards (cycle key on click)
            if (68..=144).contains(&my) {
                let (mut kc, mut kt, mut ka) = hook::get_key_bindings();
                if (30..=254).contains(&mx) {
                    kc = next_vk(kc);
                    hook::set_key_bindings(kc, kt, ka);
                    present_dashboard(hwnd);
                    return 0;
                } else if (268..=492).contains(&mx) {
                    kt = next_vk(kt);
                    hook::set_key_bindings(kc, kt, ka);
                    present_dashboard(hwnd);
                    return 0;
                } else if (506..=730).contains(&mx) {
                    ka = next_vk(ka);
                    hook::set_key_bindings(kc, kt, ka);
                    present_dashboard(hwnd);
                    return 0;
                }
            }
            // 4. Startup toggle (398..498, 412..450)
            if (398..=498).contains(&mx) && (412..=450).contains(&my) {
                let inst = install::installed_exe_path();
                let exe = if inst.exists() { inst } else { std::env::current_exe().unwrap_or_default() };
                let _ = install::set_startup_enabled(!install::is_startup_enabled(), &exe);
                present_dashboard(hwnd);
                return 0;
            }
            // 5. Sound toggle (508..604, 412..450)
            if (508..=604).contains(&mx) && (412..=450).contains(&my) {
                let cur = SOUND_ENABLED.load(Relaxed);
                SOUND_ENABLED.store(!cur, Relaxed);
                present_dashboard(hwnd);
                return 0;
            }
            // 5b. Check for Updates button (612..720, 412..450)
            if (612..=720).contains(&mx) && (412..=450).contains(&my) {
                install::check_for_updates(false);
                return 0;
            }
            // 6. Recent activity item click -> reveal in Explorer
            if (408..=712).contains(&mx) && (208..=360).contains(&my) {
                let idx = ((my - 208) / 36).max(0) as usize;
                let items = RECENT.lock().unwrap().clone();
                if let Some(it) = items.get(idx) {
                    let op = w16("open");
                    let p = w16(&it.path.display().to_string());
                    ShellExecuteW(hwnd, op.as_ptr(), p.as_ptr(), null(), null(), SW_SHOW);
                }
                return 0;
            }
            // 7. Drag window by top bar
            if my < 58 {
                SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0);
            }
            0
        }
        WM_DROPFILES => {
            let hdrop = wp as HDROP;
            let n = DragQueryFileW(hdrop, u32::MAX, null_mut(), 0);
            let mut dropped = vec![];
            for i in 0..n {
                let len = DragQueryFileW(hdrop, i, null_mut(), 0) as usize;
                let mut b = vec![0u16; len + 1];
                DragQueryFileW(hdrop, i, b.as_mut_ptr(), b.len() as u32);
                dropped.push(PathBuf::from(String::from_utf16_lossy(&b[..len])));
            }
            DragFinish(hdrop);
            if !dropped.is_empty() {
                *crate::drop_target::PATHS.lock().unwrap() = dropped;
                let wh = WHEEL_HWND.load(Relaxed) as HWND;
                if !wh.is_null() {
                    PostMessageW(wh, hook::WM_APP_WHEEL_KEY, 0, 0);
                }
            }
            0
        }
        WM_CLOSE => {
            // Hide to tray instead of quitting!
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        _ => DefWindowProcW(hwnd, m, wp, lp),
    }
}

pub fn show(wheel_hwnd: HWND) {
    WHEEL_HWND.store(wheel_hwnd as usize, Relaxed);
    unsafe {
        let mut h = DASH_HWND.load(Relaxed) as HWND;
        if h.is_null() || IsWindow(h) == 0 {
            let hi = GetModuleHandleW(null());
            let cls = w16("ClementineDashboard");
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(dash_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hi,
                hIcon: crate::load_icon(),
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                hbrBackground: null_mut(),
                lpszMenuName: null(),
                lpszClassName: cls.as_ptr(),
            };
            RegisterClassW(&wc);
            let cx = GetSystemMetrics(SM_CXSCREEN) / 2;
            let cy = GetSystemMetrics(SM_CYSCREEN) / 2;
            let title = w16("Clementine");
            h = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_APPWINDOW,
                cls.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                cx - DW as i32 / 2,
                cy - DH as i32 / 2,
                DW as i32,
                DH as i32,
                null_mut(),
                null_mut(),
                hi,
                null(),
            );
            DragAcceptFiles(h, 1);
            DASH_HWND.store(h as usize, Relaxed);
        }
        present_dashboard(h);
        ShowWindow(h, SW_SHOW);
        SetForegroundWindow(h);
    }
}
