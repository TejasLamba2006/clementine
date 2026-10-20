mod actions;
mod drop_target;
mod hook;
mod install;
mod renderer;
mod shelf;
mod showcase;

use clementine_core::*;
use drop_target::*;
use hook::*;
use renderer::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::Instant;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Media::Audio::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
use windows_sys::Win32::System::Ole::OleInitialize;
use windows_sys::Win32::UI::Shell::{DragQueryFileW, Shell_NotifyIconW, HDROP, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW};
use windows_sys::Win32::System::ProcessStatus::*;
use windows_sys::Win32::System::Threading::GetCurrentProcess;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

static CLICK_WAV: &[u8] = include_bytes!("../assets/highlight.wav");
const TIMER_ID: usize = 1;
const TIMER_CLOSE_ID: usize = 2;
const TIMER_DEMO_ID: usize = 3;
const TIMER_TOUR_ID: usize = 4;
const TIMER_PROGRESS_ID: usize = 5;
const PROGRESS_MS: f64 = 520.0;
const HOLD_MS: f64 = 320.0;
const WM_APP_TRAY: u32 = WM_APP + 20;
const BLOOM_MS: f64 = 200.0;
const S: i32 = SIZE as i32;

struct Progress {
    title: String,
    name: String,
    saved: String,
    t0: Instant,
    demo: bool,
}

struct Wheel {
    progress: Option<Progress>,
    open: bool,
    demo_until: Option<Instant>,
    mode: WheelMode,
    ext: String,
    paths: Vec<PathBuf>,
    ids: Vec<String>,
    labels: Vec<String>,
    selected: Option<usize>,
    center: String,
    base: Vec<Px>,
    frame: Vec<u32>,
    cx: i32,
    cy: i32,
    t0: Instant,
}

thread_local! {
    static W: RefCell<Option<Wheel>> = const { RefCell::new(None) };
    static TOUR_STEP: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn w16(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn human_size(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{:.0} KB", (bytes as f64 / 1024.0).max(1.0))
    }
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

/// Petal ids and labels for a mode (max 9 petals, the renderer limit).
fn targets(mode: WheelMode, ext: &str, count: usize) -> (Vec<String>, Vec<String>) {
    let ids: Vec<&str> = match mode {
        WheelMode::Conversions => conversion_targets(ext),
        WheelMode::Tools => tool_targets(ext, count),
        WheelMode::Actions => action_targets(),
    };
    let ids: Vec<&str> = ids.into_iter().take(9).collect();
    let labels = ids
        .iter()
        .map(|id| match mode {
            WheelMode::Conversions => format_label(id),
            _ => tool_meta(id).map(|t| t.title.to_string()).unwrap_or_else(|| id.to_string()),
        })
        .collect();
    (ids.iter().map(|s| s.to_string()).collect(), labels)
}

fn rerender(w: &mut Wheel) {
    let center = match w.selected {
        Some(i) => w.labels[i].clone(),
        None => w.center.clone(),
    };
    let icons = (w.mode != WheelMode::Conversions).then_some(w.ids.as_slice());
    w.frame = render(&w.base, &w.labels, icons, w.selected, &center);
}

fn refresh_targets(w: &mut Wheel) {
    let ext = w.paths.first().map(|p| ext_of(p)).unwrap_or_else(|| w.ext.clone());
    let (ids, labels) = targets(w.mode, &ext, w.paths.len().max(1));
    w.ids = ids;
    w.labels = labels;
    w.selected = None;
    if !w.paths.is_empty() {
        let total: u64 = w.paths.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
        w.center = human_size(total);
    }
    rerender(w);
}

fn present_hwnd(hwnd: HWND) {
    let snap = W.with(|c| {
        let g = c.borrow();
        let w = g.as_ref()?;
        Some((w.t0, w.cx, w.cy, w.frame.clone()))
    });
    let Some((t0, cx, cy, frame)) = snap else { return };
    if frame.len() < SIZE * SIZE {
        return;
    }
    let t = ((t0.elapsed().as_secs_f64() * 1000.0) / BLOOM_MS).min(1.0);
    let e = bloom_ease(t);
    let (scale, alpha) = (0.45 + 0.55 * e, e);
    let mut d = Dib::new();
    let out = d.slice();
    if t >= 1.0 {
        out.copy_from_slice(&frame);
    } else {
        for y in 0..SIZE {
            for x in 0..SIZE {
                let sx = ((x as f64 - 136.0) / scale + 136.0).round();
                let sy = ((y as f64 - 136.0) / scale + 136.0).round();
                out[y * SIZE + x] = if sx >= 0.0 && sy >= 0.0 && sx < SIZE as f64 && sy < SIZE as f64 {
                    frame[sy as usize * SIZE + sx as usize]
                } else {
                    0
                };
            }
        }
    }
    unsafe {
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: (alpha * 255.0) as u8, AlphaFormat: AC_SRC_ALPHA as u8 };
        let pt = POINT { x: cx - 136, y: cy - 136 };
        let sz = SIZE { cx: S, cy: S };
        let src = POINT { x: 0, y: 0 };
        UpdateLayeredWindow(hwnd, std::ptr::null_mut(), &pt, &sz, d.dc, &src, 0, &blend, ULW_ALPHA);
    }
}


/// `CF_HDROP` paths currently on the clipboard (fallback when OLE has not delivered any).
fn clipboard_paths(hwnd: HWND) -> Vec<PathBuf> {
    let mut out = vec![];
    unsafe {
        if OpenClipboard(hwnd) == 0 {
            return out;
        }
        let h = GetClipboardData(15) as HDROP;
        if !h.is_null() {
            let n = DragQueryFileW(h, u32::MAX, std::ptr::null_mut(), 0);
            for i in 0..n {
                let len = DragQueryFileW(h, i, std::ptr::null_mut(), 0) as usize;
                let mut b = vec![0u16; len + 1];
                DragQueryFileW(h, i, b.as_mut_ptr(), b.len() as u32);
                out.push(PathBuf::from(String::from_utf16_lossy(&b[..len])));
            }
        }
        CloseClipboard();
    }
    out
}

fn open_wheel(hwnd: HWND, mode: WheelMode, demo: bool) {
    let mut pt = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut pt) };
    let clip = clipboard_paths(hwnd);
    let (was_open, cx, cy) = W.with(|c| {
        let mut g = c.borrow_mut();
        let w = g.as_mut().unwrap();
        let was_open = w.open;
        w.mode = mode;
        if !was_open {
            if w.paths.is_empty() {
                w.paths = clip;
            }
            w.cx = pt.x;
            w.cy = pt.y;
            w.progress = None;
            w.base = make_base(capture_screen(pt.x, pt.y).as_deref());
            w.t0 = Instant::now();
            w.open = true;
            if demo {
                w.demo_until = Some(Instant::now() + std::time::Duration::from_secs(3));
            }
        }
        refresh_targets(w);
        (was_open, w.cx, w.cy)
    });
    unsafe {
        if !was_open {
            SetWindowPos(hwnd, HWND_TOPMOST, cx - 136, cy - 136, S, S, SWP_NOACTIVATE);
            present_hwnd(hwnd);
            ShowWindow(hwnd, SW_SHOWNA);
            KillTimer(hwnd, TIMER_PROGRESS_ID);
            SetTimer(hwnd, TIMER_ID, 16, None);
            // Nudge so an in-flight DoDragDrop fires DragEnter without waiting for real movement.
            SetCursorPos(pt.x + 1, pt.y);
            SetCursorPos(pt.x, pt.y);
        } else {
            present_hwnd(hwnd);
        }
    }
}

fn close_wheel(hwnd: HWND) {
    W.with(|c| {
        if let Some(w) = c.borrow_mut().as_mut() {
            w.open = false;
            w.progress = None;
            w.paths.clear();
        }
    });
    unsafe {
        KillTimer(hwnd, TIMER_ID);
        KillTimer(hwnd, TIMER_CLOSE_ID);
        KillTimer(hwnd, TIMER_DEMO_ID);
        KillTimer(hwnd, TIMER_PROGRESS_ID);
        ShowWindow(hwnd, SW_HIDE);
    }
}

fn select_at(hwnd: HWND, sx: i32, sy: i32) {
    let changed = W.with(|c| {
        let mut g = c.borrow_mut();
        let Some(w) = g.as_mut().filter(|w| w.open) else { return None };
        let sel = hit_test_petal((sx - w.cx) as f64, (sy - w.cy) as f64, w.labels.len());
        if sel != w.selected {
            w.selected = sel;
            rerender(w);
            Some(sel.is_some())
        } else {
            None
        }
    });
    if let Some(play) = changed {
        present_hwnd(hwnd);
        if play {
            unsafe { PlaySoundW(CLICK_WAV.as_ptr() as _, std::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT) };
        }
    }
}

fn commit(hwnd: HWND) {
    let job = W.with(|c| {
        let g = c.borrow();
        let w = g.as_ref()?;
        let i = w.selected?;
        Some((w.mode, w.ids[i].clone(), w.paths.clone()))
    });
    let demo = W.with(|c| c.borrow().as_ref().is_some_and(|w| w.demo_until.is_some()));
    let quit = || {
        if demo {
            unsafe { PostQuitMessage(0) };
        }
    };
    let Some((mode, id, paths)) = job else { return close_wheel(hwnd) };
    if paths.is_empty() {
        close_wheel(hwnd);
        println!("selected {id} (no files)");
        return;
    }
    let mut outs = vec![];
    match mode {
        WheelMode::Actions => {
            close_wheel(hwnd);
            actions::run(hwnd, &id, &paths);
            return quit();
        }
        WheelMode::Conversions => {
            for p in &paths {
                match execute_conversion(p, &id) {
                    Ok(o) => {
                        println!("wrote {}", o.display());
                        outs.push(o);
                    }
                    Err(e) => eprintln!("convert failed: {e}"),
                }
            }
        }
        WheelMode::Tools => match execute_tool(&paths, &id, &ToolParams::default()) {
            Ok(o) => {
                println!("wrote {}", o.display());
                outs.push(o);
            }
            Err(e) => eprintln!("tool failed: {e}"),
        },
    }
    let Some(last) = outs.last() else {
        close_wheel(hwnd);
        return quit();
    };
    let name = last.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (title, saved) = match (mode, id.as_str()) {
        (WheelMode::Tools, "removeMetadata") => ("STRIPPING METADATA".to_string(), "Removed EXIF & GPS tags".to_string()),
        (WheelMode::Tools, "compress") => ("COMPRESSING FILE".to_string(), format!("Saved {name}")),
        (WheelMode::Tools, _) => {
            let t = tool_meta(&id).map(|t| t.title.to_ascii_uppercase()).unwrap_or_else(|| id.to_ascii_uppercase());
            (format!("RUNNING {t}"), format!("Saved {name}"))
        }
        _ => (format!("CONVERTING TO {}", id.to_ascii_uppercase()), format!("Saved {name}")),
    };
    start_progress(hwnd, Progress { title, name, saved, t0: Instant::now(), demo });
}

fn present_frame(hwnd: HWND, cx: i32, cy: i32, frame: &[u32]) {
    let mut d = Dib::new();
    d.slice().copy_from_slice(frame);
    unsafe {
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        let pt = POINT { x: cx - 136, y: cy - 136 };
        let sz = SIZE { cx: S, cy: S };
        let src = POINT { x: 0, y: 0 };
        UpdateLayeredWindow(hwnd, std::ptr::null_mut(), &pt, &sz, d.dc, &src, 0, &blend, ULW_ALPHA);
    }
}

/// Swaps the wheel for the progress HUD; `progress_tick` animates it and finally closes the window.
fn start_progress(hwnd: HWND, p: Progress) {
    W.with(|c| {
        if let Some(w) = c.borrow_mut().as_mut() {
            w.open = false;
            w.selected = None;
            w.paths.clear();
            w.progress = Some(p);
        }
    });
    unsafe {
        KillTimer(hwnd, TIMER_ID);
        KillTimer(hwnd, TIMER_CLOSE_ID);
        KillTimer(hwnd, TIMER_DEMO_ID);
        SetTimer(hwnd, TIMER_PROGRESS_ID, 16, None);
    }
    progress_tick(hwnd);
}

fn progress_tick(hwnd: HWND) {
    let out = W.with(|c| {
        let g = c.borrow();
        let w = g.as_ref()?;
        let p = w.progress.as_ref()?;
        let ms = p.t0.elapsed().as_secs_f64() * 1000.0;
        if ms >= PROGRESS_MS + HOLD_MS {
            return Some((vec![], 0, 0, true, p.demo));
        }
        let t = (ms / PROGRESS_MS).min(1.0);
        let done = ms >= PROGRESS_MS;
        let sub = if done { p.saved.clone() } else { format!("{}  \u{b7}  {}%", p.name, (t * 100.0) as u32) };
        let e = (t * t * (3.0 - 2.0 * t)) as f32;
        Some((render_progress_hud(&w.base, &p.title, &sub, e, done), w.cx, w.cy, false, p.demo))
    });
    let Some((frame, cx, cy, finished, demo)) = out else { return };
    if finished {
        close_wheel(hwnd);
        if demo {
            unsafe { PostQuitMessage(0) };
        }
        return;
    }
    present_frame(hwnd, cx, cy, &frame);
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let (x, y) = (wp as i32, lp as i32);
    match m {
        WM_APP_WHEEL_KEY => {
            let mode = [WheelMode::Conversions, WheelMode::Tools, WheelMode::Actions][wp.min(2)];
            open_wheel(hwnd, mode, false);
            0
        }
        WM_APP_ENTER => {
            let open = W.with(|c| {
                if let Some(w) = c.borrow_mut().as_mut() {
                    w.paths = PATHS.lock().unwrap().clone();
                    if w.open {
                        refresh_targets(w);
                        return true;
                    }
                }
                false
            });
            if open {
                present_hwnd(hwnd);
            }
            select_at(hwnd, x, y);
            0
        }
        WM_APP_OVER => {
            select_at(hwnd, x, y);
            0
        }
        WM_APP_DROP => {
            select_at(hwnd, x, y);
            W.with(|c| {
                if let Some(w) = c.borrow_mut().as_mut() {
                    w.paths = PATHS.lock().unwrap().clone();
                }
            });
            commit(hwnd);
            0
        }
        WM_APP_DRAG_END => {
            let on = W.with(|c| c.borrow().as_ref().is_some_and(|w| w.open && w.demo_until.is_none()));
            if on {
                // Let IDropTarget::Drop (WM_APP_DROP) win the race; otherwise decide on the timer.
                SetTimer(hwnd, TIMER_CLOSE_ID, 50, None);
            }
            0
        }
        WM_APP_LEAVE => 0,
        WM_APP_ESC => {
            close_wheel(hwnd);
            0
        }
        WM_MOUSEMOVE => {
            let mut pt = POINT { x: 0, y: 0 };
            GetCursorPos(&mut pt);
            if W.with(|c| c.borrow().as_ref().is_some_and(|w| w.demo_until.is_some())) {
                select_at(hwnd, pt.x, pt.y);
            }
            0
        }
        WM_LBUTTONDOWN => {
            if W.with(|c| c.borrow().as_ref().is_some_and(|w| w.demo_until.is_some())) {
                commit(hwnd);
                PostQuitMessage(0);
            }
            0
        }
        WM_TIMER if wp == TIMER_DEMO_ID => {
            close_wheel(hwnd);
            0
        }
        WM_TIMER if wp == TIMER_PROGRESS_ID => {
            progress_tick(hwnd);
            0
        }
        WM_TIMER if wp == TIMER_TOUR_ID => {
            tour_tick(hwnd);
            0
        }
        WM_TIMER if wp == TIMER_CLOSE_ID => {
            KillTimer(hwnd, TIMER_CLOSE_ID);
            let commit_it = W.with(|c| c.borrow().as_ref().is_some_and(|w| w.open && w.selected.is_some() && !w.paths.is_empty()));
            if commit_it {
                commit(hwnd);
            } else {
                close_wheel(hwnd);
            }
            0
        }
        WM_APP_TRAY => {
            if matches!(lp as u32, WM_RBUTTONUP | WM_CONTEXTMENU) {
                let menu = CreatePopupMenu();
                let on = if install::is_startup_enabled() { MF_CHECKED } else { MF_UNCHECKED };
                for (id, s, f) in [
                    (1, "Open Demo Wheel", MF_STRING),
                    (2, "Show Floating Shelf", MF_STRING),
                    (3, "Run at Windows Startup", MF_STRING | on),
                    (4, "Install to Start Menu & Programs", MF_STRING),
                    (5, "Quit Clementine", MF_STRING),
                ] {
                    AppendMenuW(menu, f, id, w16(s).as_ptr());
                }
                let mut pt = POINT { x: 0, y: 0 };
                GetCursorPos(&mut pt);
                SetForegroundWindow(hwnd);
                TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, std::ptr::null());
                PostMessageW(hwnd, WM_NULL, 0, 0);
                DestroyMenu(menu);
            }
            0
        }
        WM_COMMAND => {
            match wp & 0xFFFF {
                1 => {
                    open_wheel(hwnd, WheelMode::Conversions, false);
                    SetTimer(hwnd, TIMER_DEMO_ID, 3000, None);
                }
                2 => shelf::add(hwnd, &[]),
                3 => {
                    let inst = install::installed_exe_path();
                    let exe = if inst.exists() { inst } else { std::env::current_exe().unwrap_or_default() };
                    if let Err(e) = install::set_startup_enabled(!install::is_startup_enabled(), &exe) {
                        eprintln!("startup toggle failed: {e}");
                    }
                }
                4 => {
                    if let Err(e) = install::install_app(false) {
                        eprintln!("install failed: {e}");
                    }
                }
                5 => PostQuitMessage(0),
                _ => {}
            }
            0
        }
        WM_TIMER => {
            let (open, bloom, demo_over) = W.with(|c| {
                let g = c.borrow();
                let Some(w) = g.as_ref() else { return (false, false, false) };
                (w.open, w.t0.elapsed().as_secs_f64() * 1000.0 < BLOOM_MS + 32.0, w.demo_until.is_some_and(|d| Instant::now() >= d))
            });
            if open && bloom {
                present_hwnd(hwnd);
            }
            if demo_over || (GetAsyncKeyState(VK_ESCAPE as i32) as u16 & 0x8000 != 0 && W.with(|c| c.borrow().as_ref().is_some_and(|w| w.demo_until.is_some()))) {
                close_wheel(hwnd);
                PostQuitMessage(0);
            }
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, m, wp, lp),
    }
}

fn create_window() -> HWND {
    unsafe {
        OleInitialize(std::ptr::null());
        W.with(|c| {
            *c.borrow_mut() = Some(Wheel {
                progress: None,
                open: false,
                demo_until: None,
                mode: WheelMode::Conversions,
                ext: "png".into(),
                paths: vec![],
                ids: vec![],
                labels: vec![],
                selected: None,
                center: "2.2 MB".into(),
                base: vec![],
                frame: vec![],
                cx: 0,
                cy: 0,
                t0: Instant::now(),
            })
        });
        let hi = GetModuleHandleW(std::ptr::null());
        let class = w16("ClementineWheel");
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hi,
            hIcon: load_icon(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            class.as_ptr(),
            WS_POPUP,
            0, 0, S, S,
            std::ptr::null_mut(), std::ptr::null_mut(), hi, std::ptr::null(),
        )
    }
}

fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_APP_TRAY;
        for (d, c) in nid.szTip.iter_mut().zip("Clementine".encode_utf16()) {
            *d = c;
        }
        nid
    }
}

/// 16x16 orange radial icon, generated in code.
fn tray_icon() -> HICON {
    let mut xor = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16 {
        for x in 0..16 {
            let r = ((x as f32 + 0.5 - 8.0).hypot(y as f32 + 0.5 - 8.0) - 7.0).clamp(-0.5, 0.5);
            let a = (0.5 - r) as f32;
            let hole = (x as f32 + 0.5 - 8.0).hypot(y as f32 + 0.5 - 8.0) < 2.5;
            let a = if hole { 0.0 } else { a };
            xor.extend([0u8, (82.0 * a) as u8, (255.0 * a) as u8, (255.0 * a) as u8]);
        }
    }
    let and = [0u8; 32];
    unsafe { CreateIcon(std::ptr::null_mut(), 16, 16, 1, 32, and.as_ptr(), xor.as_ptr()) }
}

/// Icon embedded by build.rs (resource ID 1), falling back to the generated one.
fn load_icon() -> HICON {
    let h = unsafe { LoadIconW(GetModuleHandleW(std::ptr::null()), 1 as *const u16) };
    if h.is_null() { tray_icon() } else { h }
}

const TOUR: [(WheelMode, [usize; 4]); 3] = [
    (WheelMode::Conversions, [0, 1, 2, 6]),
    (WheelMode::Tools, [0, 2, 4, 5]),
    (WheelMode::Actions, [0, 1, 2, 5]),
];

/// One 350ms step of `--live-Tour`: highlight a petal, switching wheel every 4 steps, quit after the last.
fn tour_tick(hwnd: HWND) {
    let step = TOUR_STEP.with(|s| s.replace(s.get() + 1));
    if step >= 12 {
        unsafe {
            KillTimer(hwnd, TIMER_TOUR_ID);
            PostQuitMessage(0);
        }
        close_wheel(hwnd);
        return;
    }
    let (mode, petals) = TOUR[step / 4];
    let target_pt = W.with(|c| {
        let mut g = c.borrow_mut();
        let w = g.as_mut().unwrap();
        if w.mode != mode {
            w.mode = mode;
            refresh_targets(w);
        }
        if w.labels.is_empty() {
            return None;
        }
        let idx = petals[step % 4].min(w.labels.len() - 1);
        w.selected = Some(idx);
        let lp = clementine_core::label_position(idx, w.labels.len());
        rerender(w);
        Some((w.cx + lp.x.round() as i32, w.cy + lp.y.round() as i32))
    });
    if let Some((tx, ty)) = target_pt {
        present_hwnd(hwnd);
        unsafe {
            SetCursorPos(tx, ty);
            PlaySoundW(CLICK_WAV.as_ptr() as _, std::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
        }
    }
}

fn create_stage_window(cx: i32, cy: i32) -> HWND {
    const SW: usize = 800;
    const SH: usize = 540;
    unsafe {
        let hi = GetModuleHandleW(std::ptr::null());
        let class = w16("ClementineStage");
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(DefWindowProcW),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hi,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            class.as_ptr(),
            WS_POPUP,
            cx - (SW as i32) / 2,
            cy - (SH as i32) / 2,
            SW as i32,
            SH as i32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hi,
            std::ptr::null(),
        );
        let mut d = Dib::sized(SW, SH);
        let buf = d.slice();
        for y in 0..SH {
            let v = y as f32 / (SH - 1) as f32;
            for x in 0..SW {
                let u = x as f32 / (SW - 1) as f32;
                // Warm sunset gradient matching Tangerine press kit (#F3A528 -> #F45B3E -> #9782C6)
                let w1 = (1.0 - u) * (1.0 - v);
                let w2 = (1.0 - u) * v;
                let w3 = u * (1.0 - v);
                let w4 = u * v;
                let mut r = (246.0 * w1 + 247.0 * w2 + 214.0 * w3 + 158.0 * w4).clamp(0.0, 255.0);
                let mut g = (176.0 * w1 + 92.0 * w2 + 128.0 * w3 + 132.0 * w4).clamp(0.0, 255.0);
                let mut b = (38.0 * w1 + 66.0 * w2 + 64.0 * w3 + 202.0 * w4).clamp(0.0, 255.0);
                // Subtle center file card (64x64 rounded square) peeking inside the wheel hole
                let dx = (x as f32 - (SW as f32 * 0.5 - 10.0)).abs();
                let dy = (y as f32 - (SH as f32 * 0.5)).abs();
                if dx < 30.0 && dy < 34.0 {
                    r = 255.0;
                    g = 246.0;
                    b = 238.0;
                    if dx < 26.0 && dy < 30.0 {
                        r = 255.0;
                        g = 118.0;
                        b = 38.0;
                    }
                }
                buf[y * SW + x] = (255u32 << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
            }
        }
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        let pt = POINT { x: cx - (SW as i32) / 2, y: cy - (SH as i32) / 2 };
        let sz = SIZE { cx: SW as i32, cy: SH as i32 };
        let src = POINT { x: 0, y: 0 };
        UpdateLayeredWindow(hwnd, std::ptr::null_mut(), &pt, &sz, d.dc, &src, 0, &blend, ULW_ALPHA);
        ShowWindow(hwnd, SW_SHOWNA);
        hwnd
    }
}

fn live_tour() {
    let h = create_window();
    unsafe {
        let (cx, cy) = (GetSystemMetrics(SM_CXSCREEN) / 2, GetSystemMetrics(SM_CYSCREEN) / 2);
        let stage = create_stage_window(cx, cy);
        SetCursorPos(cx, cy);
        open_wheel(h, WheelMode::Conversions, false);
        SetTimer(h, TIMER_TOUR_ID, 350, None);
        message_loop();
        DestroyWindow(stage);
    }
}

fn add_tray(hwnd: HWND) {
    let mut nid = tray_data(hwnd);
    nid.hIcon = load_icon();
    unsafe { Shell_NotifyIconW(NIM_ADD, &nid) };
}

fn remove_tray(hwnd: HWND) {
    let nid = tray_data(hwnd);
    unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };
}

fn message_loop() {
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn parse_mode(s: Option<&String>) -> WheelMode {
    match s.map(|s| s.as_str()) {
        Some("tools") => WheelMode::Tools,
        Some("actions") => WheelMode::Actions,
        _ => WheelMode::Conversions,
    }
}

fn working_set_mb() -> f64 {
    unsafe {
        let mut pmc: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        K32GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb);
        pmc.WorkingSetSize as f64 / 1_048_576.0
    }
}

fn render_state(mode: WheelMode, ext: &str, sel: Option<usize>, center: &str) -> Vec<u32> {
    let (ids, labels) = targets(mode, ext, 1);
    let base = make_base(Some(&synthetic_backdrop()));
    let c = match sel {
        Some(i) if i < labels.len() => labels[i].as_str(),
        _ => center,
    };
    let icons = (mode != WheelMode::Conversions).then_some(ids.as_slice());
    render(&base, &labels, icons, sel, c)
}

fn self_test() -> std::result::Result<(), String> {
    let px = render_state(WheelMode::Conversions, "svg", Some(2), "2.2 MB");
    let img = to_image(&px);
    std::fs::create_dir_all("target").map_err(|e| e.to_string())?;
    img.save("target/self-test-wheel.png").map_err(|e| e.to_string())?;
    if px.iter().all(|p| p >> 24 == 0) {
        return Err("no opaque pixels".into());
    }
    if px[100 * SIZE + 136] >> 24 != 0 {
        return Err("center hole not transparent".into());
    }
    if !img.pixels().any(|p| p[3] == 255 && p[0] >= 253 && (80..=84).contains(&p[1]) && p[2] <= 2) {
        return Err("no #FF5200 highlight pixels".into());
    }
    let mb = working_set_mb();
    println!("WorkingSet: {mb:.2} MB");
    if mb >= 15.0 {
        return Err(format!("working set {mb:.2} MB >= 15 MB"));
    }
    let hud = render_progress_hud(&make_base(Some(&synthetic_backdrop())), "CONVERTING TO WEBP", "photo No Metadata.jpg  \u{b7}  78%", 0.78, false);
    to_image(&hud).save("target/test-progress-hud.png").map_err(|e| e.to_string())?;
    if hud[156 * SIZE + 60] >> 24 == 0 {
        return Err("progress HUD bar missing".into());
    }
    println!("self-test ok: target/self-test-wheel.png");
    Ok(())
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(|s| s.as_str()) {
        Some("--self-test") => {
            if let Err(e) = self_test() {
                eprintln!("self-test FAILED: {e}");
                std::process::exit(1);
            }
        }
        Some("--render-wheel") => {
            let out = a.get(2).expect("usage: --render-wheel <out.png> [conversions|tools|actions] [ext] [highlight_idx]");
            let ext = a.get(4).map(|s| s.as_str()).unwrap_or("svg");
            let sel = a.get(5).and_then(|s| s.parse().ok());
            let px = render_state(parse_mode(a.get(3)), ext, sel, "2.2 MB");
            to_image(&px).save(out).expect("save png");
        }
        Some("--render-showcase") => {
            let out = a.get(2).expect("usage: --render-showcase <out_dir>");
            if let Err(e) = showcase::render_showcase_frames(Path::new(out)) {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        Some("--convert") => {
            let (Some(p), Some(t)) = (a.get(2), a.get(3)) else { return eprintln!("usage: --convert <input_path> <target_ext>") };
            match execute_conversion(Path::new(p), t) {
                Ok(o) => println!("{}", o.display()),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
        Some("--demo") => {
            let h = create_window();
            drop_target::register(h);
            open_wheel(h, parse_mode(a.get(2)), true);
            message_loop();
        }
        Some("--install") | Some("--install-quiet") => {
            let quiet = a[1] == "--install-quiet";
            if let Err(e) = install::install_app(quiet) {
                eprintln!("install failed: {e}");
                std::process::exit(1);
            }
        }
        Some("--uninstall") => {
            if let Err(e) = install::uninstall_app(false) {
                eprintln!("uninstall failed: {e}");
                std::process::exit(1);
            }
        }
        Some("--live-Tour") | Some("--live-tour") => live_tour(),
        None if std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()))
            .is_some_and(|n| n.contains("setup") || n.contains("install")) =>
        {
            match install::install_app(false) {
                Ok(exe) => {
                    let _ = std::process::Command::new(exe).arg("--startup").spawn();
                }
                Err(e) => eprintln!("install failed: {e}"),
            }
        }
        _ => {
            let h = create_window();
            drop_target::register(h);
            hook::install(h as usize);
            add_tray(h);
            message_loop();
            remove_tray(h);
        }
    }
}
