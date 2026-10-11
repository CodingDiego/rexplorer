//! Single-instance plumbing (named mutex + WM_COPYDATA hand-off) and external launchers.

use std::path::Path;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_TIMEOUT, E_FAIL, GetLastError, HANDLE, HWND, LPARAM, WPARAM,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetPropW, IsHungAppWindow, SMTO_ABORTIFHUNG, SMTO_NORMAL, SetPropW, SW_SHOWNORMAL, SendMessageTimeoutW,
    SetForegroundWindow, WM_COPYDATA,
};
use windows::core::{Error, HSTRING, PCWSTR, Result, w};

pub const COPYDATA_MAGIC: usize = 0x5245584;

pub struct Guard(HANDLE);

impl Drop for Guard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Instances are single per settings folder: a launch with another %APPDATA% (the smoke test's scratch profiles)
/// runs beside the user's window instead of handing off to it.
fn group() -> usize {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    rexplorer_core::settings::dir().to_string_lossy().to_lowercase().hash(&mut h);
    h.finish() as usize | 1
}

const GROUP_PROP: PCWSTR = w!("RexplorerGroup");

/// Some(guard) if this is the first instance; None if another already holds the mutex.
pub fn claim() -> Option<Guard> {
    unsafe {
        let name = HSTRING::from(format!("Local\\rexplorer-single-instance-{:x}", group()));
        let h = CreateMutexW(None, false, &name).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(h);
            None
        } else {
            Some(Guard(h))
        }
    }
}

/// Tags the main window with this instance's group, for `find_main_window`.
pub fn mark(hwnd: HWND) {
    unsafe {
        let _ = SetPropW(hwnd, GROUP_PROP, Some(HANDLE(group() as *mut _)));
    }
}

/// The main window of this settings folder's instance.
pub fn find_main_window() -> Option<HWND> {
    let mut after = None;
    while let Ok(h) = unsafe { FindWindowExW(None, after, w!("RexplorerMain"), PCWSTR::null()) } {
        if unsafe { GetPropW(h, GROUP_PROP) }.0 as usize == group() {
            return Some(h);
        }
        after = Some(h);
    }
    None
}

pub fn send(hwnd: HWND, payload: &str) -> bool {
    let wide: Vec<u16> = payload.encode_utf16().chain(Some(0)).collect();
    let cds = COPYDATASTRUCT {
        dwData: COPYDATA_MAGIC,
        cbData: (wide.len() * 2) as u32,
        lpData: wide.as_ptr() as *mut _,
    };
    unsafe {
        let call = |flags, ms| SendMessageTimeoutW(hwnd, WM_COPYDATA, WPARAM(0), LPARAM(&cds as *const _ as isize), flags, ms, None);
        let mut r = call(SMTO_ABORTIFHUNG, 3000);
        // A busy UI thread is not a dead one: wait longer before this launch becomes a second window. Read before
        // anything else can clobber the error; other failures (access denied) and a hung window fall through unchanged.
        if r.0 == 0 && GetLastError() == ERROR_TIMEOUT && !IsHungAppWindow(hwnd).as_bool() {
            r = call(SMTO_NORMAL, 10_000);
        }
        let _ = SetForegroundWindow(hwnd);
        r.0 != 0
    }
}

/// `lparam` must be the LPARAM of a WM_COPYDATA message (or a valid COPYDATASTRUCT pointer).
pub fn receive(lparam: LPARAM) -> Option<String> {
    unsafe {
        let cds = (lparam.0 as *const COPYDATASTRUCT).as_ref()?;
        if cds.dwData != COPYDATA_MAGIC || cds.lpData.is_null() {
            return None;
        }
        let words = std::slice::from_raw_parts(cds.lpData as *const u16, cds.cbData as usize / 2);
        let end = words.iter().position(|&c| c == 0).unwrap_or(words.len());
        Some(String::from_utf16_lossy(&words[..end]))
    }
}

fn exec(file: &HSTRING, params: &HSTRING, dir: &HSTRING) -> bool {
    unsafe { ShellExecuteW(None, w!("open"), file, params, dir, SW_SHOWNORMAL).0 as isize > 32 }
}

/// `dir` quoted for a command line: a trailing backslash would escape the closing quote (`"C:\"` reads as `C:"`).
fn quoted_dir(dir: &Path) -> String {
    let d = dir.display().to_string();
    let bs = if d.ends_with(BS) { BS } else { "" };
    format!("\"{d}{bs}\"")
}

const BS: &str = "\\";

pub fn open_terminal(dir: &Path) -> Result<()> {
    let d = HSTRING::from(dir.display().to_string());
    // Absolute paths: ShellExecute resolves a bare name against the working directory (the browsed folder) first.
    // The Store alias (a reparse point `is_file` cannot follow), then wt.exe in PATH's absolute folders (scoop, zip).
    let alias = std::env::var_os("LOCALAPPDATA").map(|l| Path::new(&l).join(r"Microsoft\WindowsApps\wt.exe")).filter(|a| a.symlink_metadata().is_ok());
    let path = std::env::var_os("PATH").unwrap_or_default();
    let on_path = std::env::split_paths(&path).filter(|p| p.is_absolute()).map(|p| p.join("wt.exe")).filter(|f| f.is_file());
    let wt = HSTRING::from(format!("-d {}", quoted_dir(dir)));
    if alias.into_iter().chain(on_path).any(|exe| exec(&HSTRING::from(exe.as_os_str()), &wt, &d)) {
        return Ok(());
    }
    let cmd_exe = Path::new(&std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into())).join(r"System32\cmd.exe");
    let cmd = HSTRING::from(format!("/K cd /d {}", quoted_dir(dir)));
    if exec(&HSTRING::from(cmd_exe.as_os_str()), &cmd, &d) {
        Ok(())
    } else {
        Err(Error::from(E_FAIL))
    }
}

pub fn open_default_app(path: &Path) -> Result<()> {
    let p = HSTRING::from(path.display().to_string());
    let r = unsafe { ShellExecuteW(None, w!("open"), &p, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
    if r.0 as isize > 32 { Ok(()) } else { Err(Error::from(E_FAIL)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_dirs_keep_their_closing_quote() {
        assert_eq!(quoted_dir(Path::new("C:\\")), "\"C:\\\\\"");
        assert_eq!(quoted_dir(Path::new("C:\\Users\\x")), "\"C:\\Users\\x\"");
    }

    #[test]
    fn claim_is_exclusive() {
        let a = claim();
        assert!(a.is_some());
        assert!(claim().is_none());
        drop(a);
        assert!(claim().is_some());
    }

    #[test]
    fn receive_round_trip() {
        let s = "path=C:\\x\nnewtab=1\n";
        let wide: Vec<u16> = s.encode_utf16().chain(Some(0)).collect();
        let mut cds = COPYDATASTRUCT {
            dwData: COPYDATA_MAGIC,
            cbData: (wide.len() * 2) as u32,
            lpData: wide.as_ptr() as *mut _,
        };
        let lp = LPARAM(&cds as *const _ as isize);
        assert_eq!(receive(lp).as_deref(), Some(s));
        cds.dwData = 1;
        assert_eq!(receive(LPARAM(&cds as *const _ as isize)), None);
    }

    #[test]
    fn send_waits_out_a_busy_receiver() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use windows::Win32::Foundation::LRESULT;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG, PostMessageW,
            PostQuitMessage, RegisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_DESTROY, WNDCLASSW,
        };
        static FIRST: AtomicBool = AtomicBool::new(true);
        unsafe extern "system" fn busy(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            match m {
                // Longer than the first send waits, so only the retry gets an answer.
                WM_COPYDATA if FIRST.swap(false, Ordering::Relaxed) => {
                    std::thread::sleep(std::time::Duration::from_secs(4));
                    LRESULT(1)
                }
                WM_COPYDATA => LRESULT(1),
                WM_DESTROY => {
                    unsafe { PostQuitMessage(0) };
                    LRESULT(0)
                }
                _ => unsafe { DefWindowProcW(h, m, w, l) },
            }
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || unsafe {
            let class = w!("RexplorerBusyReceiverTest");
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(busy), lpszClassName: class, ..Default::default() });
            let h = CreateWindowExW(
                WINDOW_EX_STYLE(0), class, PCWSTR::null(), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, None, None,
            )
            .unwrap();
            tx.send(h.0 as isize).unwrap();
            let mut m = MSG::default();
            while GetMessageW(&mut m, None, 0, 0).as_bool() {
                DispatchMessageW(&m);
            }
        });
        let h = HWND(rx.recv().unwrap() as *mut _);
        assert!(send(h, "x"));
        unsafe { PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0)).unwrap() };
        thread.join().unwrap();
    }

    #[test]
    fn find_main_window_no_panic() {
        let _ = find_main_window();
    }
}
