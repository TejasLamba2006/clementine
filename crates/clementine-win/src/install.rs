use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Clementine";
const SETTINGS_KEY: &str = r"Software\Clementine";
const APP: &str = "Clementine";
pub const CURRENT_VERSION: &str = "0.2.1";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DETACHED_PROCESS: u32 = 0x0000_0008;

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn env_dir(var: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(var).unwrap_or_default())
}

pub fn install_dir() -> PathBuf {
    env_dir("LOCALAPPDATA").join("Programs").join("Clementine")
}

pub fn installed_exe_path() -> PathBuf {
    install_dir().join("Clementine.exe")
}

fn shortcut_path() -> PathBuf {
    env_dir("APPDATA").join(r"Microsoft\Windows\Start Menu\Programs\Clementine.lnk")
}

fn run_command(exe: &Path) -> String {
    format!("\"{}\" --startup", exe.display())
}

fn check(code: u32, what: &str) -> Result<(), String> {
    if code == 0 { Ok(()) } else { Err(format!("{what} failed (error {code})")) }
}

fn open_key(path: &str, access: u32) -> Option<HKEY> {
    let mut k: HKEY = null_mut();
    (unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, w(path).as_ptr(), 0, access, &mut k) } == 0).then_some(k)
}

fn create_key(path: &str) -> Result<HKEY, String> {
    let mut k: HKEY = null_mut();
    let r = unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, w(path).as_ptr(), 0, null(), 0, KEY_WRITE, null(), &mut k, null_mut()) };
    check(r, "RegCreateKeyExW").map(|_| k)
}

fn set_str(k: HKEY, name: &str, val: &str) -> Result<(), String> {
    let v = w(val);
    let r = unsafe { RegSetValueExW(k, w(name).as_ptr(), 0, REG_SZ, v.as_ptr() as *const u8, (v.len() * 2) as u32) };
    check(r, "RegSetValueExW")
}

fn set_dword(k: HKEY, name: &str, val: u32) -> Result<(), String> {
    let r = unsafe { RegSetValueExW(k, w(name).as_ptr(), 0, REG_DWORD, &val as *const u32 as *const u8, 4) };
    check(r, "RegSetValueExW")
}

fn run_value_exists(name: &str) -> bool {
    let Some(k) = open_key(RUN_KEY, KEY_READ) else { return false };
    let r = unsafe { RegQueryValueExW(k, w(name).as_ptr(), null(), null_mut(), null_mut(), null_mut()) };
    unsafe { RegCloseKey(k) };
    r == 0
}

fn set_run_value(name: &str, enable: bool, exe_path: &Path) -> Result<(), String> {
    if enable {
        let k = create_key(RUN_KEY)?;
        let r = set_str(k, name, &run_command(exe_path));
        unsafe { RegCloseKey(k) };
        r
    } else {
        let Some(k) = open_key(RUN_KEY, KEY_WRITE) else { return Ok(()) };
        let r = unsafe { RegDeleteValueW(k, w(name).as_ptr()) };
        unsafe { RegCloseKey(k) };
        if r == 0 || r == 2 { Ok(()) } else { check(r, "RegDeleteValueW") } // 2 = already absent
    }
}

pub fn is_startup_enabled() -> bool {
    run_value_exists(APP)
}

pub fn set_startup_enabled(enable: bool, exe_path: &Path) -> Result<(), String> {
    set_run_value(APP, enable, exe_path)
}

pub fn is_auto_update_enabled() -> bool {
    let Some(k) = open_key(SETTINGS_KEY, KEY_READ) else { return true };
    let mut val: u32 = 1;
    let mut sz: u32 = 4;
    let r = unsafe {
        RegQueryValueExW(
            k,
            w("AutoUpdate").as_ptr(),
            null(),
            null_mut(),
            &mut val as *mut u32 as *mut u8,
            &mut sz,
        )
    };
    unsafe { RegCloseKey(k) };
    if r == 0 { val != 0 } else { true }
}

pub fn set_auto_update_enabled(enable: bool) {
    if let Ok(k) = create_key(SETTINGS_KEY) {
        let _ = set_dword(k, "AutoUpdate", u32::from(enable));
        unsafe { RegCloseKey(k) };
    }
}

pub fn parse_semver(s: &str) -> (u32, u32, u32) {
    let clean = s.trim().trim_start_matches(['v', 'V']);
    let mut it = clean.split(['.', '-']);
    let a = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let b = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let c = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    (a, b, c)
}

pub fn is_newer_version(remote: &str, local: &str) -> bool {
    parse_semver(remote) > parse_semver(local)
}

pub fn check_for_updates(silent: bool) {
    std::thread::spawn(move || {
        crate::dashboard::set_update_status("Checking GitHub...".into());
        let ps = "$ProgressPreference='SilentlyContinue'; \
                  $r = Invoke-RestMethod -Uri 'https://api.github.com/repos/TejasLamba2006/clementine/releases/latest' -Headers @{'User-Agent'='Clementine-Updater'} -TimeoutSec 10; \
                  $a = $r.assets | Where-Object { $_.name -like '*Setup*.exe' -or $_.name -eq 'clementine.exe' } | Select-Object -First 1; \
                  Write-Output \"$($r.tag_name)|$($a.browser_download_url)\"";
        let out = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", ps])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        let Ok(out) = out else {
            crate::dashboard::set_update_status(format!("v{CURRENT_VERSION} (Offline)"));
            if !silent {
                message("Could not reach GitHub Releases to check for updates.");
            }
            return;
        };
        let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let (tag, url) = line.split_once('|').unwrap_or((&line, ""));
        if tag.is_empty() {
            crate::dashboard::set_update_status(format!("Up to date (v{CURRENT_VERSION})"));
            if !silent {
                message(&format!("Clementine v{CURRENT_VERSION} is up to date!"));
            }
            return;
        }
        if is_newer_version(tag, CURRENT_VERSION) && !url.is_empty() {
            crate::dashboard::set_update_status(format!("Updating to {tag}..."));
            let tmp = std::env::temp_dir().join("Clementine-Setup-Update.exe");
            let dl = format!(
                "$ProgressPreference='SilentlyContinue'; Invoke-WebRequest -Uri '{}' -OutFile '{}' -TimeoutSec 30",
                url.replace('\'', "''"),
                tmp.display().to_string().replace('\'', "''")
            );
            let ok = std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &dl])
                .creation_flags(CREATE_NO_WINDOW)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok && tmp.exists() {
                crate::dashboard::set_update_status(format!("Updated to {tag}!"));
                if !silent {
                    message(&format!("Downloaded Clementine {tag} from GitHub Releases.\nApplying update and restarting now..."));
                }
                let _ = std::process::Command::new(&tmp)
                    .arg("--install-quiet")
                    .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
                    .spawn();
                return;
            }
        }
        crate::dashboard::set_update_status(format!("Up to date (v{CURRENT_VERSION})"));
        if !silent {
            message(&format!(
                "Clementine v{CURRENT_VERSION} is up to date!\n\nLatest GitHub Release: {tag}\nAuto-update on startup: {}",
                if is_auto_update_enabled() { "Enabled" } else { "Disabled" }
            ));
        }
    });
}

fn message(text: &str) {
    unsafe { MessageBoxW(null_mut(), w(text).as_ptr(), w(APP).as_ptr(), MB_OK | MB_ICONINFORMATION) };
}

fn make_shortcut(exe: &Path) -> Result<(), String> {
    let q = |p: &Path| p.display().to_string().replace('\'', "''");
    let lnk = shortcut_path();
    let cmd = format!(
        "$s=(New-Object -COM WScript.Shell).CreateShortcut('{}'); $s.TargetPath='{}'; $s.IconLocation='{},0'; $s.Description='Clementine - Zero-Click Offline File Converter'; $s.Save()",
        q(&lnk), q(exe), q(exe)
    );
    let st = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &cmd])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| e.to_string())?;
    if st.success() { Ok(()) } else { Err("shortcut creation failed".into()) }
}

pub fn install_app(quiet: bool) -> Result<PathBuf, String> {
    let target = installed_exe_path();
    std::fs::create_dir_all(install_dir()).map_err(|e| e.to_string())?;
    let cur = std::env::current_exe().map_err(|e| e.to_string())?;
    if cur != target {
        let pid_filter = format!("PID ne {}", std::process::id());
        let _ = std::process::Command::new("taskkill.exe")
            .args(["/F", "/IM", "Clementine.exe", "/FI", &pid_filter])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        std::thread::sleep(std::time::Duration::from_millis(120));
        if let Err(e) = std::fs::copy(&cur, &target) {
            if !target.exists() {
                return Err(format!("copy failed: {e}"));
            }
        }
    }
    set_startup_enabled(true, &target)?;
    let exe = target.display().to_string();
    let k = create_key(UNINSTALL_KEY)?;
    let r = (|| {
        set_str(k, "DisplayName", APP)?;
        set_str(k, "DisplayIcon", &format!("{exe},0"))?;
        set_str(k, "Publisher", "Tejas Lamba")?;
        set_str(k, "DisplayVersion", CURRENT_VERSION)?;
        set_str(k, "UninstallString", &format!("\"{exe}\" --uninstall"))?;
        set_dword(k, "NoModify", 1)?;
        set_dword(k, "NoRepair", 1)
    })();
    unsafe { RegCloseKey(k) };
    r?;
    make_shortcut(&target)?;
    let _ = std::process::Command::new(&target)
        .arg("--startup")
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn();
    if !quiet {
        message("Clementine has been installed!\n\n\u{2022} Added to Start Menu & Installed Apps\n\u{2022} Configured to run automatically at Windows startup\n\u{2022} Running silently in your System Tray (no terminal window)\n\nDrag any file and tap Shift (Convert), Alt (Tools), or Ctrl (Actions).");
    }
    Ok(target)
}

pub fn uninstall_app(quiet: bool) -> Result<(), String> {
    set_startup_enabled(false, Path::new(""))?;
    let r = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, w(UNINSTALL_KEY).as_ptr()) };
    if r != 0 && r != 2 {
        return check(r, "RegDeleteKeyW");
    }
    let _ = std::fs::remove_file(shortcut_path());
    if !quiet {
        message("Clementine has been uninstalled.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_paths_and_command_format() {
        assert!(RUN_KEY.ends_with(r"CurrentVersion\Run"));
        assert!(UNINSTALL_KEY.ends_with(r"Uninstall\Clementine"));
        assert_eq!(run_command(Path::new(r"C:\a\Clementine.exe")), r#""C:\a\Clementine.exe" --startup"#);
        assert!(installed_exe_path().ends_with(r"Programs\Clementine\Clementine.exe"));
    }

    #[test]
    fn startup_toggle_roundtrip() {
        let name = "ClementineUnitTest";
        let exe = Path::new(r"C:\x\Clementine.exe");
        set_run_value(name, true, exe).unwrap();
        assert!(run_value_exists(name));
        set_run_value(name, false, exe).unwrap();
        assert!(!run_value_exists(name));
        assert!(is_newer_version("v0.3.0", "0.2.0"));
        assert!(!is_newer_version("v0.2.0", "0.2.0"));
        assert!(!is_newer_version("v0.1.0", "0.2.0"));
    }
}
