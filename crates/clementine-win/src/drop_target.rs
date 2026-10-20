//! Hand-rolled COM `IDropTarget`. Callbacks stash paths and post messages to the wheel window.
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::Mutex;
use windows_sys::core::{GUID, HRESULT};
use windows_sys::Win32::Foundation::{HWND, POINTL, S_OK};
use windows_sys::Win32::System::Com::{FORMATETC, STGMEDIUM};
use windows_sys::Win32::System::Ole::*;
use windows_sys::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_APP_ENTER: u32 = WM_APP + 10;
pub const WM_APP_OVER: u32 = WM_APP + 11;
pub const WM_APP_DROP: u32 = WM_APP + 12;
pub const WM_APP_LEAVE: u32 = WM_APP + 13;

pub static PATHS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static TARGET: AtomicUsize = AtomicUsize::new(0);

type Fp = unsafe extern "system" fn();
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
        0x80004002u32 as i32 // E_NOINTERFACE
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

/// `IDataObject::GetData` (vtable slot 3) for `CF_HDROP`.
unsafe fn read_paths(data: *mut c_void) -> Vec<PathBuf> {
    let fmt = FORMATETC { cfFormat: 15, ptd: std::ptr::null_mut(), dwAspect: 1, lindex: -1, tymed: 1 };
    let mut stg: STGMEDIUM = std::mem::zeroed();
    let vt = *(data as *const *const usize);
    let get: unsafe extern "system" fn(*mut c_void, *const FORMATETC, *mut STGMEDIUM) -> HRESULT =
        std::mem::transmute(*vt.add(3));
    let mut out = vec![];
    if get(data, &fmt, &mut stg) == S_OK {
        let h = stg.u.hGlobal as HDROP;
        let n = DragQueryFileW(h, u32::MAX, std::ptr::null_mut(), 0);
        for i in 0..n {
            let len = DragQueryFileW(h, i, std::ptr::null_mut(), 0) as usize;
            let mut b = vec![0u16; len + 1];
            DragQueryFileW(h, i, b.as_mut_ptr(), b.len() as u32);
            out.push(PathBuf::from(String::from_utf16_lossy(&b[..len])));
        }
        ReleaseStgMedium(&mut stg);
    }
    out
}

unsafe extern "system" fn enter(_: *mut c_void, data: *mut c_void, _: u32, pt: POINTL, eff: *mut u32) -> HRESULT {
    *PATHS.lock().unwrap() = read_paths(data);
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

#[allow(dead_code)]
type _Unused = Fp;
