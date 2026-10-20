//! Native file actions: copy, copy path, terminal, move, share, float.
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::Memory::*;
use windows_sys::Win32::UI::Shell::*;

const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().collect()
}

unsafe fn set_clipboard(hwnd: HWND, fmt: u32, data: &[u8]) {
    if OpenClipboard(hwnd) == 0 {
        return;
    }
    EmptyClipboard();
    let h = GlobalAlloc(GMEM_MOVEABLE, data.len());
    if !h.is_null() {
        let p = GlobalLock(h) as *mut u8;
        std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
        GlobalUnlock(h);
        if SetClipboardData(fmt, h as _).is_null() {
            windows_sys::Win32::Foundation::GlobalFree(h);
        }
    }
    CloseClipboard();
}

fn utf16_bytes(units: &[u16]) -> Vec<u8> {
    units.iter().flat_map(|u| u.to_le_bytes()).collect()
}

/// `CF_HDROP` payload: DROPFILES header + double-NUL terminated wide path list.
pub fn hdrop_bytes(paths: &[PathBuf]) -> Vec<u8> {
    let mut b = vec![0u8; std::mem::size_of::<DROPFILES>()];
    let hdr = DROPFILES { pFiles: b.len() as u32, pt: unsafe { std::mem::zeroed() }, fNC: 0, fWide: 1 };
    unsafe { std::ptr::write_unaligned(b.as_mut_ptr() as *mut DROPFILES, hdr) };
    for p in paths {
        let mut w = wide(p.as_os_str());
        w.push(0);
        b.extend(utf16_bytes(&w));
    }
    b.extend([0, 0]);
    b
}

fn copy(hwnd: HWND, paths: &[PathBuf]) {
    unsafe { set_clipboard(hwnd, CF_HDROP, &hdrop_bytes(paths)) }
}

fn copy_path(hwnd: HWND, paths: &[PathBuf]) {
    let text: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    let mut w: Vec<u16> = text.join("\r\n").encode_utf16().collect();
    w.push(0);
    unsafe { set_clipboard(hwnd, CF_UNICODETEXT, &utf16_bytes(&w)) }
}

fn open_terminal(paths: &[PathBuf]) {
    let Some(first) = paths.first() else { return };
    let dir = if first.is_dir() { first.clone() } else { first.parent().unwrap_or(Path::new(".")).to_path_buf() };
    if Command::new("wt.exe").arg("-d").arg(&dir).spawn().is_err() {
        let cmd = format!("Set-Location '{}'", dir.display().to_string().replace('\'', "''"));
        let _ = Command::new("powershell.exe").args(["-NoExit", "-Command", &cmd]).spawn();
    }
}

fn pick_folder(hwnd: HWND) -> Option<PathBuf> {
    unsafe {
        let bi = BROWSEINFOW {
            hwndOwner: hwnd,
            pidlRoot: std::ptr::null_mut(),
            pszDisplayName: std::ptr::null_mut(),
            lpszTitle: std::ptr::null(),
            ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
            lpfn: None,
            lParam: 0,
            iImage: 0,
        };
        let pidl = SHBrowseForFolderW(&bi);
        if pidl.is_null() {
            return None;
        }
        let mut buf = [0u16; 520];
        let ok = SHGetPathFromIDListW(pidl, buf.as_mut_ptr());
        windows_sys::Win32::System::Com::CoTaskMemFree(pidl as _);
        if ok == 0 {
            return None;
        }
        let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..n])))
    }
}

fn move_files(hwnd: HWND, paths: &[PathBuf]) {
    let Some(dir) = pick_folder(hwnd) else { return };
    for p in paths {
        if let Some(name) = p.file_name() {
            let _ = std::fs::rename(p, dir.join(name));
        }
    }
}

fn share(paths: &[PathBuf]) {
    if let Some(p) = paths.first() {
        let _ = Command::new("explorer.exe").arg(format!("/select,{}", p.display())).spawn();
    }
}

/// Runs action `id` (an `action_targets` id) on `paths`.
pub fn run(hwnd: HWND, id: &str, paths: &[PathBuf]) {
    match id {
        "airDrop" => share(paths),
        "float" => crate::shelf::add(hwnd, paths),
        "copy" => copy(hwnd, paths),
        "move" => move_files(hwnd, paths),
        "copyPath" => copy_path(hwnd, paths),
        "openTerminal" => open_terminal(paths),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn hdrop_layout() {
        let b = super::hdrop_bytes(&[std::path::PathBuf::from("a")]);
        assert_eq!(b.len(), 20 + 4 + 2);
        assert_eq!(&b[16..20], &[1, 0, 0, 0]);
        assert_eq!(&b[b.len() - 4..], &[0, 0, 0, 0]);
    }
}
