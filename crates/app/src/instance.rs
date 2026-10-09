//! Single-instance plumbing (named mutex + WM_COPYDATA hand-off) and external launchers.

use std::path::Path;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, E_FAIL, GetLastError, HANDLE, HWND, LPARAM, WPARAM,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SMTO_ABORTIFHUNG, SW_SHOWNORMAL, SendMessageTimeoutW, SetForegroundWindow,
    WM_COPYDATA,
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

/// Some(guard) if this is the first instance; None if another already holds the mutex.
pub fn claim() -> Option<Guard> {
    unsafe {
        let h = CreateMutexW(None, false, w!("Local\\rexplorer-single-instance")).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(h);
            None
        } else {
            Some(Guard(h))
        }
    }
}

pub fn find_main_window() -> Option<HWND> {
    unsafe { FindWindowW(w!("RexplorerMain"), PCWSTR::null()).ok() }
}

pub fn send(hwnd: HWND, payload: &str) -> bool {
    let wide: Vec<u16> = payload.encode_utf16().chain(Some(0)).collect();
    let cds = COPYDATASTRUCT {
        dwData: COPYDATA_MAGIC,
        cbData: (wide.len() * 2) as u32,
        lpData: wide.as_ptr() as *mut _,
    };
    unsafe {
        let r = SendMessageTimeoutW(
            hwnd,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&cds as *const _ as isize),
            SMTO_ABORTIFHUNG,
            3000,
            None,
        );
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

fn exec(file: PCWSTR, params: &HSTRING, dir: &HSTRING) -> bool {
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
    let wt = HSTRING::from(format!("-d {}", quoted_dir(dir)));
    if exec(w!("wt.exe"), &wt, &d) {
        return Ok(());
    }
    let cmd = HSTRING::from(format!("/K cd /d {}", quoted_dir(dir)));
    if exec(w!("cmd.exe"), &cmd, &d) {
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
    fn find_main_window_no_panic() {
        let _ = find_main_window();
    }
}
