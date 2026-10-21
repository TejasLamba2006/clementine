//! COM `IDropTarget` (`CF_HDROP` + `CF_UNICODETEXT` `file://` URI support for VS Code / Cursor / IDEs)
//! and active File Explorer selection fallback.
use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::Mutex;
use windows_sys::core::{GUID, HRESULT};
use windows_sys::Win32::Foundation::{HWND, POINT, POINTL, S_OK};
use windows_sys::Win32::System::Com::{FORMATETC, STGMEDIUM};
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Ole::*;
use windows_sys::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClassNameW, GetCursorPos, GetForegroundWindow, GetWindowTextW, PostMessageW,
    WindowFromPoint, GA_ROOT, WM_APP,
};

pub const WM_APP_ENTER: u32 = WM_APP + 10;
pub const WM_APP_OVER: u32 = WM_APP + 11;
pub const WM_APP_DROP: u32 = WM_APP + 12;
pub const WM_APP_LEAVE: u32 = WM_APP + 13;

pub static PATHS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static TARGET: AtomicUsize = AtomicUsize::new(0);

#[repr(C)]
struct Vtbl {
    qi: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    enter: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, POINTL, *mut u32) -> HRESULT,
    over: unsafe extern "system" fn(*mut c_void, u32, POINTL, *mut u32) -> HRESULT,
    leave: unsafe extern "system" fn(*mut c_void) -> HRESULT,
    drop: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, POINTL, *mut u32) -> HRESULT,
}
#[repr(C)]
struct Obj(*const Vtbl);
unsafe impl Sync for Obj {}

const IID_UNKNOWN: GUID = GUID { data1: 0, data2: 0, data3: 0, data4: [0xC0, 0, 0, 0, 0, 0, 0, 0x46] };
const IID_DROPTARGET: GUID = GUID { data1: 0x122, data2: 0, data3: 0, data4: [0xC0, 0, 0, 0, 0, 0, 0, 0x46] };

fn same(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

static VTBL: Vtbl = Vtbl { qi, add_ref, release, enter, over, leave, drop };
static OBJ: Obj = Obj(&VTBL);

unsafe extern "system" fn qi(this: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    let g = &*iid;
    if same(g, &IID_UNKNOWN) || same(g, &IID_DROPTARGET) {
        *out = this;
        S_OK
    } else {
        *out = std::ptr::null_mut();
        0x80004002u32 as i32
    }
}
unsafe extern "system" fn add_ref(_: *mut c_void) -> u32 {
    1
}
unsafe extern "system" fn release(_: *mut c_void) -> u32 {
    1
}

fn post(msg: u32, p: POINTL) {
    unsafe { PostMessageW(TARGET.load(Relaxed) as HWND, msg, p.x as isize as usize, p.y as isize) };
}

/// Parses `"file:///c%3A/dir/a%20b.png"` or quoted/plain path lines from VS Code / Cursor / IDEs.
pub fn parse_dropped_line(line: &str) -> Option<PathBuf> {
    let s = line.trim().trim_matches(|c| c == '"' || c == '\'');
    if s.is_empty() || s.starts_with('#') {
        return None;
    }
    let decoded = if let Some(rest) = s.strip_prefix("file:///").or_else(|| s.strip_prefix("FILE:///")) {
        let mut out = String::new();
        let bytes = rest.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                    if let Ok(b) = u8::from_str_radix(hex, 16) {
                        out.push(b as char);
                        i += 3;
                        continue;
                    }
                }
            }
            out.push(if bytes[i] == b'/' { '\\' } else { bytes[i] as char });
            i += 1;
        }
        out
    } else {
        s.to_string()
    };
    let p = PathBuf::from(&decoded);
    (p.is_absolute() && p.exists()).then_some(p)
}

/// `IDataObject::GetData` (vtable slot 3) for `CF_HDROP` (15) and `CF_UNICODETEXT` (13).
unsafe fn read_paths(data: *mut c_void) -> Vec<PathBuf> {
    if data.is_null() {
        return vec![];
    }
    let vt = *(data as *const *const usize);
    let get: unsafe extern "system" fn(*mut c_void, *const FORMATETC, *mut STGMEDIUM) -> HRESULT =
        std::mem::transmute(*vt.add(3));
    let mut out = vec![];

    // 1. Try CF_HDROP (15) - standard File Explorer & Desktop drag
    let fmt_hdrop = FORMATETC { cfFormat: 15, ptd: std::ptr::null_mut(), dwAspect: 1, lindex: -1, tymed: 1 };
    let mut stg: STGMEDIUM = std::mem::zeroed();
    if get(data, &fmt_hdrop, &mut stg) == S_OK {
        let h = stg.u.hGlobal as HDROP;
        if !h.is_null() {
            let n = DragQueryFileW(h, u32::MAX, std::ptr::null_mut(), 0);
            for i in 0..n {
                let len = DragQueryFileW(h, i, std::ptr::null_mut(), 0) as usize;
                let mut b = vec![0u16; len + 1];
                DragQueryFileW(h, i, b.as_mut_ptr(), b.len() as u32);
                let p = PathBuf::from(String::from_utf16_lossy(&b[..len]));
                if p.exists() && !out.contains(&p) {
                    out.push(p);
                }
            }
        }
        ReleaseStgMedium(&mut stg);
        if !out.is_empty() {
            return out;
        }
    }

    // 2. Try CF_UNICODETEXT (13) - VS Code, Cursor, Windsurf, JetBrains IDE file tree drag
    let fmt_text = FORMATETC { cfFormat: 13, ptd: std::ptr::null_mut(), dwAspect: 1, lindex: -1, tymed: 1 };
    let mut stg2: STGMEDIUM = std::mem::zeroed();
    if get(data, &fmt_text, &mut stg2) == S_OK {
        let h = stg2.u.hGlobal;
        if !h.is_null() {
            let ptr = GlobalLock(h) as *const u16;
            if !ptr.is_null() {
                let mut len = 0usize;
                while len < 16384 && *ptr.add(len) != 0 {
                    len += 1;
                }
                let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
                GlobalUnlock(h);
                for line in text.lines() {
                    if let Some(p) = parse_dropped_line(line) {
                        if !out.contains(&p) {
                            out.push(p);
                        }
                    }
                }
            }
        }
        ReleaseStgMedium(&mut stg2);
    }
    out
}

fn class_name(hwnd: HWND) -> String {
    if hwnd.is_null() {
        return String::new();
    }
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) as usize };
    String::from_utf16_lossy(&buf[..n])
}

fn window_title(hwnd: HWND) -> String {
    if hwnd.is_null() {
        return String::new();
    }
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) as usize };
    String::from_utf16_lossy(&buf[..n])
}

/// Returns `true` when the user is dragging inside the Clementine web playground
/// in a browser so the native desktop wheel does not double-open on top of the web demo.
pub fn is_cursor_over_clementine_web_demo() -> bool {
    let fg = unsafe { GetForegroundWindow() };
    let title = window_title(fg).to_ascii_lowercase();
    title.contains("clementine |")
        || title.contains("zero-click offline file converter")
        || title.contains("tejaslamba2006.github.io/clementine")
}

/// Returns `true` when the cursor or active foreground window is Windows File Explorer or the Desktop.
pub fn is_explorer_or_desktop_active() -> bool {
    let is_shell_cls = |cls: &str| {
        matches!(
            cls,
            "CabinetWClass"
                | "ExploreWClass"
                | "Progman"
                | "WorkerW"
                | "DirectUIHWND"
                | "SHELLDLL_DefView"
                | "SysListView32"
        )
    };
    let fg = unsafe { GetForegroundWindow() };
    if is_shell_cls(&class_name(fg)) {
        return true;
    }
    let mut pt = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut pt) };
    let at = unsafe { WindowFromPoint(pt) };
    if is_shell_cls(&class_name(at)) {
        return true;
    }
    let root = unsafe { GetAncestor(at, GA_ROOT) };
    is_shell_cls(&class_name(root))
}

/// Queries the currently selected/focused item(s) in the active File Explorer window via Shell.Application COM.
pub fn query_active_explorer_selection() -> Vec<PathBuf> {
    if !is_explorer_or_desktop_active() {
        return vec![];
    }
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let fg = unsafe { GetForegroundWindow() } as usize;
    let script = format!(
        r#"$fg = {}; $sh = New-Object -ComObject Shell.Application; foreach ($w in $sh.Windows()) {{ try {{ if ([uintptr]$w.HWND -eq [uintptr]$fg) {{ $sel = $w.Document.SelectedItems(); if ($sel -and $sel.Count -gt 0) {{ foreach ($i in $sel) {{ Write-Output $i.Path }}; exit }} $f = $w.Document.FocusedItem; if ($f -and $f.Path) {{ Write-Output $f.Path; exit }} }} }} catch {{}} }}; foreach ($w in $sh.Windows()) {{ try {{ $sel = $w.Document.SelectedItems(); if ($sel -and $sel.Count -gt 0) {{ foreach ($i in $sel) {{ Write-Output $i.Path }}; exit }} }} catch {{}} }}"#,
        fg
    );
    let Ok(out) = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return vec![];
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| PathBuf::from(l.trim()))
        .filter(|p| p.exists())
        .collect()
}

unsafe extern "system" fn enter(_: *mut c_void, data: *mut c_void, _: u32, pt: POINTL, eff: *mut u32) -> HRESULT {
    let p = read_paths(data);
    if !p.is_empty() {
        *PATHS.lock().unwrap() = p;
    }
    *eff = DROPEFFECT_COPY;
    post(WM_APP_ENTER, pt);
    S_OK
}
unsafe extern "system" fn over(_: *mut c_void, _: u32, pt: POINTL, eff: *mut u32) -> HRESULT {
    *eff = DROPEFFECT_COPY;
    post(WM_APP_OVER, pt);
    S_OK
}
unsafe extern "system" fn leave(_: *mut c_void) -> HRESULT {
    post(WM_APP_LEAVE, POINTL { x: 0, y: 0 });
    S_OK
}
unsafe extern "system" fn drop(_: *mut c_void, data: *mut c_void, _: u32, pt: POINTL, eff: *mut u32) -> HRESULT {
    let p = read_paths(data);
    if !p.is_empty() {
        *PATHS.lock().unwrap() = p;
    }
    *eff = DROPEFFECT_COPY;
    post(WM_APP_DROP, pt);
    S_OK
}

pub fn register(hwnd: HWND) {
    TARGET.store(hwnd as usize, Relaxed);
    unsafe {
        RegisterDragDrop(hwnd, &OBJ as *const Obj as *mut c_void);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vscode_file_uri_and_plain_path() {
        let exe = std::env::current_exe().unwrap();
        let s = exe.display().to_string();
        assert_eq!(parse_dropped_line(&s), Some(exe.clone()));
        let uri = format!("file:///{}", s.replace('\\', "/").replace(':', "%3A"));
        assert_eq!(parse_dropped_line(&uri), Some(exe));
    }
}
