//! Dedicated-thread low-level mouse & keyboard hooks (`WH_MOUSE_LL`, `WH_KEYBOARD_LL`).
//! Prevents UI rendering or modal dialogs from stalling OS input hooks, blocks key auto-repeat,
//! and forwards live cursor moves while the radial wheel is open.
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU16, AtomicUsize, Ordering::Relaxed};
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const WM_APP_WHEEL_KEY: u32 = WM_APP + 1;
pub const WM_APP_DRAG_END: u32 = WM_APP + 2;
pub const WM_APP_ESC: u32 = WM_APP + 3;
pub const WM_APP_HOOK_MOVE: u32 = WM_APP + 4;

static TARGET: AtomicUsize = AtomicUsize::new(0);
static WHEEL_OPEN: AtomicBool = AtomicBool::new(false);
static LDOWN: AtomicBool = AtomicBool::new(false);
static LAST_VK: AtomicU16 = AtomicU16::new(0);
static LAST_MX: AtomicI32 = AtomicI32::new(0);
static LAST_MY: AtomicI32 = AtomicI32::new(0);

// Default bindings: 0 = Shift (Convert), 1 = Alt (Tools), 2 = Ctrl (Actions)
static KEY_CONVERT: AtomicU16 = AtomicU16::new(VK_SHIFT);
static KEY_TOOLS: AtomicU16 = AtomicU16::new(VK_MENU);
static KEY_ACTIONS: AtomicU16 = AtomicU16::new(VK_CONTROL);

pub fn set_wheel_open(open: bool) {
    WHEEL_OPEN.store(open, Relaxed);
    if !open {
        LAST_VK.store(0, Relaxed);
    }
}

pub fn set_key_bindings(convert_vk: u16, tools_vk: u16, actions_vk: u16) {
    KEY_CONVERT.store(convert_vk, Relaxed);
    KEY_TOOLS.store(tools_vk, Relaxed);
    KEY_ACTIONS.store(actions_vk, Relaxed);
}

pub fn get_key_bindings() -> (u16, u16, u16) {
    (KEY_CONVERT.load(Relaxed), KEY_TOOLS.load(Relaxed), KEY_ACTIONS.load(Relaxed))
}

fn post(msg: u32, wp: usize, lp: isize) {
    let hwnd = TARGET.load(Relaxed);
    if hwnd != 0 {
        unsafe { PostMessageW(hwnd as _, msg, wp, lp) };
    }
}

fn canonical_vk(vk: u16) -> u16 {
    match vk {
        VK_SHIFT | VK_LSHIFT | VK_RSHIFT => VK_SHIFT,
        VK_MENU | VK_LMENU | VK_RMENU => VK_MENU,
        VK_CONTROL | VK_LCONTROL | VK_RCONTROL => VK_CONTROL,
        VK_LWIN | VK_RWIN => VK_LWIN,
        _ => vk,
    }
}

fn mode_for_vk(canon: u16) -> Option<usize> {
    if canon == KEY_CONVERT.load(Relaxed) {
        Some(0)
    } else if canon == KEY_TOOLS.load(Relaxed) {
        Some(1)
    } else if canon == KEY_ACTIONS.load(Relaxed) {
        Some(2)
    } else {
        None
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        let ms = &*(lp as *const MSLLHOOKSTRUCT);
        match wp as u32 {
            WM_LBUTTONDOWN => {
                LDOWN.store(true, Relaxed);
            }
            WM_MOUSEMOVE => {
                if WHEEL_OPEN.load(Relaxed) {
                    let (px, py) = (LAST_MX.swap(ms.pt.x, Relaxed), LAST_MY.swap(ms.pt.y, Relaxed));
                    if px != ms.pt.x || py != ms.pt.y {
                        post(WM_APP_HOOK_MOVE, ms.pt.x as isize as usize, ms.pt.y as isize);
                    }
                }
            }
            WM_LBUTTONUP => {
                LDOWN.store(false, Relaxed);
                LAST_VK.store(0, Relaxed);
                if WHEEL_OPEN.load(Relaxed) {
                    post(WM_APP_DRAG_END, ms.pt.x as isize as usize, ms.pt.y as isize);
                }
            }
            _ => {}
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wp, lp)
}

unsafe extern "system" fn key_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        let raw_vk = (*(lp as *const KBDLLHOOKSTRUCT)).vkCode as u16;
        let canon = canonical_vk(raw_vk);
        match wp as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                if raw_vk == VK_ESCAPE && WHEEL_OPEN.load(Relaxed) {
                    LAST_VK.store(0, Relaxed);
                    post(WM_APP_ESC, 0, 0);
                } else if let Some(mode) = mode_for_vk(canon) {
                    // Ignore key auto-repeat while modifier is held down
                    if LAST_VK.swap(canon, Relaxed) != canon {
                        let lbutton_down = LDOWN.load(Relaxed)
                            || ((GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000) != 0);
                        if lbutton_down || WHEEL_OPEN.load(Relaxed) {
                            post(WM_APP_WHEEL_KEY, mode, 0);
                        }
                    }
                }
            }
            WM_KEYUP | WM_SYSKEYUP => {
                if LAST_VK.load(Relaxed) == canon {
                    LAST_VK.store(0, Relaxed);
                }
            }
            _ => {}
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wp, lp)
}

/// Spawns a dedicated background thread with its own Win32 message loop for the low-level hooks.
pub fn install(target: usize) {
    TARGET.store(target, Relaxed);
    std::thread::Builder::new()
        .name("clementine-hooks".into())
        .spawn(|| unsafe {
            let h = GetModuleHandleW(std::ptr::null());
            let hk_m = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), h, 0);
            let hk_k = SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_proc), h, 0);
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if !hk_m.is_null() {
                UnhookWindowsHookEx(hk_m);
            }
            if !hk_k.is_null() {
                UnhookWindowsHookEx(hk_k);
            }
        })
        .expect("spawn hook thread");
}
