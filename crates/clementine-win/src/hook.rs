//! Low-level mouse/keyboard hooks. Callbacks only touch atomics and `PostMessageW`.
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering::Relaxed};
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const WM_APP_WHEEL_KEY: u32 = WM_APP + 1;
pub const WM_APP_DRAG_END: u32 = WM_APP + 2;
pub const WM_APP_ESC: u32 = WM_APP + 3;

static TARGET: AtomicUsize = AtomicUsize::new(0);
static LDOWN: AtomicBool = AtomicBool::new(false);
static DRAGGING: AtomicBool = AtomicBool::new(false);
static SX: AtomicI32 = AtomicI32::new(0);
static SY: AtomicI32 = AtomicI32::new(0);

fn post(msg: u32, wp: usize) {
    unsafe { PostMessageW(TARGET.load(Relaxed) as _, msg, wp, 0) };
}

unsafe extern "system" fn mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        let ms = &*(lp as *const MSLLHOOKSTRUCT);
        match wp as u32 {
            WM_LBUTTONDOWN => {
                LDOWN.store(true, Relaxed);
                DRAGGING.store(false, Relaxed);
                SX.store(ms.pt.x, Relaxed);
                SY.store(ms.pt.y, Relaxed);
            }
            WM_MOUSEMOVE => {
                if LDOWN.load(Relaxed) && !DRAGGING.load(Relaxed) {
                    let (dx, dy) = (ms.pt.x - SX.load(Relaxed), ms.pt.y - SY.load(Relaxed));
                    if dx * dx + dy * dy > 16 {
                        DRAGGING.store(true, Relaxed);
                    }
                }
            }
            WM_LBUTTONUP => {
                LDOWN.store(false, Relaxed);
                if DRAGGING.swap(false, Relaxed) {
                    post(WM_APP_DRAG_END, 0);
                }
            }
            _ => {}
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wp, lp)
}

unsafe extern "system" fn key_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 && matches!(wp as u32, WM_KEYDOWN | WM_SYSKEYDOWN) {
        let vk = (*(lp as *const KBDLLHOOKSTRUCT)).vkCode as u16;
        if DRAGGING.load(Relaxed) {
            match vk {
                VK_SHIFT | VK_LSHIFT | VK_RSHIFT => post(WM_APP_WHEEL_KEY, 0),
                VK_MENU | VK_LMENU | VK_RMENU => post(WM_APP_WHEEL_KEY, 1),
                VK_CONTROL | VK_LCONTROL | VK_RCONTROL => post(WM_APP_WHEEL_KEY, 2),
                _ => {}
            }
        }
        if vk == VK_ESCAPE {
            post(WM_APP_ESC, 0);
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wp, lp)
}

pub fn install(target: usize) {
    TARGET.store(target, Relaxed);
    unsafe {
        let h = GetModuleHandleW(std::ptr::null());
        SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), h, 0);
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_proc), h, 0);
    }
}
