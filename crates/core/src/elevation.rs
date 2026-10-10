//! Explorer never opens a folder window elevated. An elevated caller (an installer's "Open folder", an admin
//! console) would otherwise get an elevated rexplorer: everything opened from it runs elevated, and later
//! medium-integrity launches cannot reach it (UIPI drops their WM_COPYDATA). So an elevated process starts itself
//! again under the desktop shell's token and exits.
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;

use windows::Win32::Foundation::{CloseHandle, E_FAIL, HANDLE};
use windows::Win32::Security::{
    DuplicateTokenEx, GetTokenInformation, SecurityImpersonation, TOKEN_ADJUST_DEFAULT, TOKEN_ADJUST_SESSIONID,
    TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation, TokenPrimary,
};
use windows::Win32::System::Environment::GetCommandLineW;
use windows::Win32::System::Threading::{
    CREATE_PROCESS_LOGON_FLAGS, CREATE_UNICODE_ENVIRONMENT, CreateProcessWithTokenW, GetCurrentProcess, OpenProcess,
    OpenProcessToken, PROCESS_INFORMATION, PROCESS_QUERY_INFORMATION, STARTUPINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};
use windows::core::{Error, PCWSTR, PWSTR, Result};

unsafe fn token_elevated(tok: HANDLE) -> bool {
    let mut e = TOKEN_ELEVATION::default();
    let mut n = 0u32;
    let size = size_of::<TOKEN_ELEVATION>() as u32;
    unsafe { GetTokenInformation(tok, TokenElevation, Some(&mut e as *mut _ as *mut c_void), size, &mut n) }.is_ok()
        && e.TokenIsElevated != 0
}

/// Whether this process holds an elevated (UAC) token.
pub fn is_elevated() -> bool {
    unsafe {
        let mut tok = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok).is_err() {
            return false;
        }
        let r = token_elevated(tok);
        let _ = CloseHandle(tok);
        r
    }
}

/// Starts this exe again with the same command line under the desktop shell's token; the caller exits on Ok.
/// Err when there is no shell, or its token is elevated too (UAC off): then this process just keeps running.
pub fn relaunch_unelevated() -> Result<()> {
    unsafe {
        let shell = GetShellWindow();
        if shell.is_invalid() {
            return Err(Error::new(E_FAIL, "no desktop shell window"));
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(shell, Some(&mut pid));
        let process = OpenProcess(PROCESS_QUERY_INFORMATION, false, pid)?;
        let mut tok = HANDLE::default();
        let opened = OpenProcessToken(process, TOKEN_DUPLICATE | TOKEN_QUERY, &mut tok);
        let _ = CloseHandle(process);
        opened?;
        let mut primary = HANDLE::default();
        let access = TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT | TOKEN_ADJUST_SESSIONID;
        let dup = DuplicateTokenEx(tok, access, None, SecurityImpersonation, TokenPrimary, &mut primary);
        let _ = CloseHandle(tok);
        dup?;
        if token_elevated(primary) {
            let _ = CloseHandle(primary);
            return Err(Error::new(E_FAIL, "the shell runs elevated too"));
        }
        let mut cmd: Vec<u16> = GetCommandLineW().as_wide().iter().copied().chain(Some(0)).collect();
        // This process's environment, not the shell user's profile: APPDATA and REXPLORER_* overrides carry over.
        let mut env: Vec<u16> = std::env::vars_os()
            .flat_map(|(k, v)| {
                let mut s = k;
                s.push("=");
                s.push(v);
                s.encode_wide().chain(Some(0)).collect::<Vec<u16>>()
            })
            .chain(Some(0))
            .collect();
        let si = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
        let mut pi = PROCESS_INFORMATION::default();
        let started = CreateProcessWithTokenW(
            primary,
            CREATE_PROCESS_LOGON_FLAGS(0),
            PCWSTR::null(),
            Some(PWSTR(cmd.as_mut_ptr())),
            CREATE_UNICODE_ENVIRONMENT,
            Some(env.as_mut_ptr() as *const c_void),
            PCWSTR::null(),
            &si,
            &mut pi,
        );
        let _ = CloseHandle(primary);
        started?;
        let _ = CloseHandle(pi.hThread);
        let _ = CloseHandle(pi.hProcess);
        Ok(())
    }
}
