//! Crash recovery: on an unhandled exception (typically a misbehaving in-process shell extension)
//! the process relaunches itself with `--recover`, guarded against crash loops.

use std::cell::UnsafeCell;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::Debug::{
    EXCEPTION_EXECUTE_HANDLER, EXCEPTION_POINTERS, SetUnhandledExceptionFilter,
};
use windows::Win32::System::Threading::{
    CreateProcessW, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::core::{PCWSTR, PWSTR};

/// Prebuilt command line. `CreateProcessW` may write into it, so it is mutable; the crash path
/// is single-shot (the process is dying), so unsynchronized access is acceptable.
struct CmdLine(UnsafeCell<Vec<u16>>);
// SAFETY: written once in `install` before the filter is set, then only touched on the crash path.
unsafe impl Sync for CmdLine {}

static CMDLINE: OnceLock<CmdLine> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();
static GUARD_SECS: AtomicU64 = AtomicU64::new(0);
static STARTED_WITH_RECOVER: AtomicBool = AtomicBool::new(false);

/// Builds a command line: every part double-quoted, embedded quotes escaped as `\"`, NUL-terminated.
pub fn command_line(exe: &Path, args: &[String]) -> Vec<u16> {
    let quote = |s: &str| format!("\"{}\"", s.replace('"', "\\\""));
    let mut parts = vec![quote(&exe.to_string_lossy())];
    parts.extend(args.iter().map(|a| quote(a)));
    let mut v: Vec<u16> = parts.join(" ").encode_utf16().collect();
    v.push(0);
    v
}

/// Crash-loop rule: refuse only when this process was itself a recovery launch and is still young.
pub fn should_relaunch(started_with_recover: bool, alive: Duration, guard: Duration) -> bool {
    !(started_with_recover && alive < guard)
}

unsafe extern "system" fn filter(_info: *const EXCEPTION_POINTERS) -> i32 {
    relaunch();
    EXCEPTION_EXECUTE_HANDLER
}

/// Pre-builds the relaunch command line and installs the unhandled-exception filter and panic hook.
pub fn install(relaunch_args: &[String], guard_secs: u64) {
    let exe = std::env::current_exe().unwrap_or_default();
    let _ = CMDLINE.set(CmdLine(UnsafeCell::new(command_line(&exe, relaunch_args))));
    let _ = START.set(Instant::now());
    GUARD_SECS.store(guard_secs, Ordering::Relaxed);
    STARTED_WITH_RECOVER.store(std::env::args().any(|a| a == "--recover"), Ordering::Relaxed);
    std::panic::set_hook(Box::new(|info| {
        eprintln!("{info}");
        relaunch();
        std::process::abort();
    }));
    unsafe { SetUnhandledExceptionFilter(Some(filter)) };
}

/// Starts a fresh copy of the process; false if refused by the crash-loop guard or on failure.
pub fn relaunch() -> bool {
    let (Some(cmd), Some(start)) = (CMDLINE.get(), START.get()) else {
        return false;
    };
    let guard = Duration::from_secs(GUARD_SECS.load(Ordering::Relaxed));
    if !should_relaunch(STARTED_WITH_RECOVER.load(Ordering::Relaxed), start.elapsed(), guard) {
        return false;
    }
    let si = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    let mut pi = PROCESS_INFORMATION::default();
    // SAFETY: the buffer is NUL-terminated and owned by a static; CreateProcessW may modify it.
    unsafe {
        let buf = &mut *cmd.0.get();
        if CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(buf.as_mut_ptr())),
            None,
            None,
            false,
            PROCESS_CREATION_FLAGS(0),
            None,
            PCWSTR::null(),
            &si,
            &mut pi,
        )
        .is_err()
        {
            return false;
        }
        let _ = CloseHandle(pi.hProcess);
        let _ = CloseHandle(pi.hThread);
    }
    true
}

/// `%APPDATA%\rexplorer\last-folder.txt`, falling back to the temp dir.
pub fn state_path() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("rexplorer")
        .join("last-folder.txt")
}

fn save_state_to(path: &Path, text: &str) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, text);
}

fn read_state_from(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Records the current folder for the recovery launch; errors are ignored.
pub fn save_state(text: &str) {
    save_state_to(&state_path(), text);
}

/// Reads the folder recorded by `save_state`.
pub fn read_state() -> Option<String> {
    read_state_from(&state_path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_quotes_parts() {
        let v = command_line(Path::new(r"C:\a b\x.exe"), &["--recover".into(), r"C:\p q".into()]);
        assert_eq!(*v.last().unwrap(), 0);
        let s = String::from_utf16(&v[..v.len() - 1]).unwrap();
        assert_eq!(s, r#""C:\a b\x.exe" "--recover" "C:\p q""#);
    }

    #[test]
    fn state_round_trips() {
        let dir = std::env::temp_dir().join(format!("rexplorer-crash-{}", std::process::id()));
        let p = dir.join("sub").join("last-folder.txt");
        save_state_to(&p, "shell:MyComputerFolder");
        assert_eq!(read_state_from(&p).as_deref(), Some("shell:MyComputerFolder"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn relaunch_refused_during_guard() {
        let (young, old, g) = (Duration::from_secs(1), Duration::from_secs(100), Duration::from_secs(60));
        assert!(!should_relaunch(true, young, g));
        assert!(should_relaunch(true, old, g));
        assert!(should_relaunch(false, young, g));
        assert!(should_relaunch(false, old, g));
    }
}
