//! Floating shelf warm-glass mini-window (`260x164` layered topmost card).
use crate::renderer::{blit_mask_sized, fit_mask, rounded_box_sdf, Dib};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::Mutex;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

static FILES: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
const SW: usize = 260;
const SH: usize = 164;

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn c8(r: u8, g: u8, b: u8) -> [f32; 3] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}

fn over(d: &mut [f32; 4], rgb: [f32; 3], a: f32) {
    let a = a.clamp(0.0, 1.0);
    let k = 1.0 - a;
    for i in 0..3 {
        d[i] = rgb[i] * a + d[i] * k;
    }
    d[3] = a + d[3] * k;
}

fn render_shelf(files: &[PathBuf]) -> Vec<u32> {
    let mut buf = vec![[0f32; 4]; SW * SH];
    let orange = c8(255, 82, 0);
    for y in 0..SH {
        let v = y as f32 / (SH - 1) as f32;
        for x in 0..SW {
            let d = rounded_box_sdf(
                x as f64 + 0.5 - SW as f64 * 0.5,
                y as f64 + 0.5 - SH as f64 * 0.5,
                SW as f64 * 0.5 - 2.0,
                SH as f64 * 0.5 - 2.0,
                20.0,
            );
            let c = (0.5 - d).clamp(0.0, 1.0) as f32;
            if c <= 0.0 {
                continue;
            }
            let i = y * SW + x;
            let col = [1.0 - 0.03 * v, 0.96 - 0.04 * v, 0.93 - 0.03 * v];
            over(&mut buf[i], col, 0.94 * c);
            let inner = (0.5 - (d + 1.5)).clamp(0.0, 1.0) as f32;
            over(&mut buf[i], orange, 0.45 * (c - inner).max(0.0));
        }
    }
    blit_mask_sized(&mut buf, SW, SH, &fit_mask("FLOAT SHELF", 11, 700, 140), 66.0, 22.0, orange);
    blit_mask_sized(&mut buf, SW, SH, &fit_mask("Copy All", 10, 700, 68), 182.0, 22.0, orange);
    blit_mask_sized(&mut buf, SW, SH, &fit_mask("Clear", 10, 600, 50), 232.0, 22.0, c8(110, 96, 86));

    for (idx, p) in files.iter().take(3).enumerate() {
        let ry = 54.0 + idx as f64 * 34.0;
        for y in (ry as usize - 13)..=(ry as usize + 13) {
            for x in 16..(SW - 16) {
                let d = rounded_box_sdf(x as f64 + 0.5 - SW as f64 * 0.5, y as f64 + 0.5 - ry, SW as f64 * 0.5 - 18.0, 13.0, 8.0);
                let c = (0.5 - d).clamp(0.0, 1.0) as f32;
                if c > 0.0 {
                    over(&mut buf[y * SW + x], [1.0; 3], 0.85 * c);
                }
            }
        }
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        blit_mask_sized(&mut buf, SW, SH, &fit_mask(&name, 11, 600, 210), SW as f64 * 0.5, ry, c8(36, 32, 28));
    }

    buf.iter()
        .map(|p| {
            let q = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u32;
            (q(p[3]) << 24) | (q(p[0]) << 16) | (q(p[1]) << 8) | q(p[2])
        })
        .collect()
}

fn present_shelf(h: HWND) {
    let files = FILES.lock().unwrap().clone();
    let px = render_shelf(&files);
    let mut d = Dib::sized(SW, SH);
    d.slice().copy_from_slice(&px);
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetWindowRect(h, &mut rc);
        let pt = POINT { x: rc.left, y: rc.top };
        let sz = SIZE { cx: SW as i32, cy: SH as i32 };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        UpdateLayeredWindow(h, null_mut(), &pt, &sz, d.dc, &src, 0, &blend, ULW_ALPHA);
    }
}

unsafe extern "system" fn proc(h: HWND, m: u32, _wp: WPARAM, lp: LPARAM) -> LRESULT {
    match m {
        WM_LBUTTONDOWN => {
            let x = (lp as i16) as i32;
            let y = ((lp >> 16) as i16) as i32;
            let files = FILES.lock().unwrap().clone();
            if y < 36 {
                if (150..=208).contains(&x) {
                    crate::actions::copy_files(h, &files);
                    return 0;
                }
                if x >= 210 {
                    FILES.lock().unwrap().clear();
                    ShowWindow(h, SW_HIDE);
                    return 0;
                }
                SendMessageW(h, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0);
                return 0;
            }
            if y >= 40 {
                let idx = ((y - 40) / 34).max(0) as usize;
                if let Some(p) = files.get(idx) {
                    crate::actions::copy_files(h, std::slice::from_ref(p));
                    let op = w("open");
                    let f = w(&p.display().to_string());
                    ShellExecuteW(h, op.as_ptr(), f.as_ptr(), null(), null(), SW_SHOW);
                }
            }
            0
        }
        WM_RBUTTONDOWN => {
            FILES.lock().unwrap().clear();
            ShowWindow(h, SW_HIDE);
            0
        }
        _ => DefWindowProcW(h, m, _wp, lp),
    }
}

/// Adds files and shows the warm-glass floating shelf on the primary screen edge.
pub fn add(_owner: HWND, paths: &[PathBuf]) {
    {
        let mut f = FILES.lock().unwrap();
        for p in paths {
            if !f.contains(p) {
                f.push(p.clone());
            }
        }
    }
    unsafe {
        let class = w("ClementineShelf");
        let hi = GetModuleHandleW(null());
        let mut h = FindWindowW(class.as_ptr(), null());
        if h.is_null() {
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hi,
                hIcon: null_mut(),
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                hbrBackground: null_mut(),
                lpszMenuName: null(),
                lpszClassName: class.as_ptr(),
            };
            RegisterClassW(&wc);
            let sx = GetSystemMetrics(SM_CXSCREEN) / 2 + 165;
            let sy = GetSystemMetrics(SM_CYSCREEN) / 2 - SH as i32 / 2;
            h = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                class.as_ptr(),
                WS_POPUP,
                sx,
                sy,
                SW as i32,
                SH as i32,
                null_mut(),
                null_mut(),
                hi,
                null(),
            );
        }
        present_shelf(h);
        ShowWindow(h, SW_SHOWNA);
    }
}
