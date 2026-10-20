//! Floating shelf mini-window: left click a row opens the file, right click clears the shelf.
use std::path::PathBuf;
use std::sync::Mutex;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

static FILES: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
const ROW: i32 = 22;

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

unsafe extern "system" fn proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match m {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(h, &mut ps);
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(h, &mut rc);
            FillRect(dc, &rc, GetStockObject(WHITE_BRUSH) as _);
            SetBkMode(dc, TRANSPARENT as i32);
            let files = FILES.lock().unwrap();
            for (i, p) in files.iter().enumerate() {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let t: Vec<u16> = name.encode_utf16().collect();
                TextOutW(dc, 8, 6 + i as i32 * ROW, t.as_ptr(), t.len() as i32);
            }
            EndPaint(h, &ps);
            0
        }
        WM_LBUTTONDOWN => {
            let y = ((lp >> 16) & 0xFFFF) as i32;
            let files = FILES.lock().unwrap().clone();
            if let Some(p) = files.get(((y - 6) / ROW).max(0) as usize) {
                let op = w("open");
                let f = w(&p.display().to_string());
                ShellExecuteW(h, op.as_ptr(), f.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOW);
            }
            0
        }
        WM_RBUTTONDOWN => {
            FILES.lock().unwrap().clear();
            ShowWindow(h, SW_HIDE);
            0
        }
        _ => DefWindowProcW(h, m, wp, lp),
    }
}

/// Adds files and shows the shelf. Must run on the UI thread.
pub fn add(_owner: HWND, paths: &[PathBuf]) {
    let n = {
        let mut f = FILES.lock().unwrap();
        f.extend(paths.iter().cloned());
        f.len() as i32
    };
    unsafe {
        let class = w("ClementineShelf");
        let hi = GetModuleHandleW(std::ptr::null());
        let mut h = FindWindowW(class.as_ptr(), std::ptr::null());
        if h.is_null() {
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(proc),
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
            let title = w("Clementine Shelf (right click clears)");
            h = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
                80, 80, 300, 120, std::ptr::null_mut(), std::ptr::null_mut(), hi, std::ptr::null(),
            );
        }
        SetWindowPos(h, HWND_TOPMOST, 0, 0, 300, 60 + n.max(1) * ROW, SWP_NOMOVE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        InvalidateRect(h, std::ptr::null(), 1);
    }
}
