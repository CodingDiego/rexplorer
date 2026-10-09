//! Windows Explorer, by full path: once rexplorer is installed, `explorer.exe` by bare name (ShellExecute consults
//! App Paths) launches rexplorer itself.
use std::path::PathBuf;

use windows::Win32::Foundation::{CloseHandle, E_FAIL};
use windows::Win32::System::Threading::{CreateProcessW, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW};
use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{Error, HSTRING, PCWSTR, PWSTR, Result};

/// `%SystemRoot%\explorer.exe`.
pub fn explorer_exe() -> PathBuf {
    PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into())).join("explorer.exe")
}

/// Opens a folder in Windows Explorer. Accepts paths and `::{CLSID}` parsing names alike.
pub fn open_in_explorer(parsing_name: &str) -> Result<()> {
    let file = HSTRING::from(explorer_exe().as_os_str());
    let params = HSTRING::from(format!("\"{parsing_name}\""));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC,
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }
}

/// Hands this process's own arguments, verbatim, to Windows Explorer.
pub fn forward_command_line() -> Result<()> {
    let cmdline = unsafe { windows::Win32::System::Environment::GetCommandLineW().to_string() }.unwrap_or_default();
    run_explorer(crate::cli::tail(&cmdline))
}

/// Starts `explorer.exe <params>` with CreateProcess, `params` passed verbatim (never ShellExecute, which would
/// loop back through App Paths).
pub fn run_explorer(params: &str) -> Result<()> {
    let exe = explorer_exe();
    let mut cmd: Vec<u16> = format!("\"{}\" {params}", exe.display()).encode_utf16().chain(Some(0)).collect();
    let app = HSTRING::from(exe.as_os_str());
    let si = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    let mut pi = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessW(
            &app,
            Some(PWSTR(cmd.as_mut_ptr())),
            None,
            None,
            false,
            PROCESS_CREATION_FLAGS(0),
            None,
            PCWSTR::null(),
            &si,
            &mut pi,
        )
        .map_err(|e| Error::new(E_FAIL, format!("cannot start {}: {e}", exe.display())))?;
        let _ = CloseHandle(pi.hProcess);
        let _ = CloseHandle(pi.hThread);
    }
    Ok(())
}
