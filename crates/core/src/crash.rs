//! Crash recovery: on an unhandled exception (typically a misbehaving in-process shell extension)
//! the process relaunches itself with `--recover`. The crash path does no I/O: each `--recover` start counts
//! itself in `crash-count.txt` (beside the state file) instead, and the second one within the guard window hands
//! the folder to Windows Explorer rather than run (a crash loop ends there). A start without `--recover`
//! resets the count.

use std::cell::UnsafeCell;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

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

/// Builds a command line: every part double-quoted, embedded quotes escaped as `\"`, NUL-terminated.
pub fn command_line(exe: &Path, args: &[String]) -> Vec<u16> {
    let quote = |s: &str| format!("\"{}\"", s.replace('"', "\\\""));
    let mut parts = vec![quote(&exe.to_string_lossy())];
    parts.extend(args.iter().map(|a| quote(a)));
    let mut v: Vec<u16> = parts.join(" ").encode_utf16().collect();
    v.push(0);
    v
}

/// Crash-loop rule for a `--recover` start at `now` (Unix seconds), given the stored (count, time of the last
/// recovery): the count to store, and whether rexplorer may run (false from the second crash within `guard`).
pub fn should_relaunch(prev: Option<(u32, u64)>, now: u64, guard: u64) -> ((u32, u64), bool) {
    let n = match prev {
        Some((n, t)) if now.saturating_sub(t) < guard => n + 1,
        _ => 1,
    };
    ((n, now), n < 2)
}

fn count_path() -> PathBuf {
    state_path().with_file_name("crash-count.txt")
}

fn record_recovery_at(path: &Path, guard_secs: u64) -> bool {
    let prev = read_state_from(path).and_then(|t| {
        let (n, at) = t.trim().split_once(' ')?;
        Some((n.parse().ok()?, at.parse().ok()?))
    });
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let ((n, at), run) = should_relaunch(prev, now, guard_secs);
    save_state_to(path, &format!("{n} {at}"));
    // An unwritable count could not stop a loop: then only the first recovery runs.
    run && read_state_from(path).is_some()
}

/// Called by a `--recover` start before anything else: counts this crash; false means hand off to Explorer.
pub fn record_recovery(guard_secs: u64) -> bool {
    record_recovery_at(&count_path(), guard_secs)
}

/// A clean start: the crash count begins again.
pub fn reset_recoveries() {
    let _ = std::fs::remove_file(count_path());
}

unsafe extern "system" fn filter(_info: *const EXCEPTION_POINTERS) -> i32 {
    relaunch();
    EXCEPTION_EXECUTE_HANDLER
}

/// Pre-builds the relaunch command line and installs the unhandled-exception filter and panic hook.
pub fn install(relaunch_args: &[String]) {
    let exe = std::env::current_exe().unwrap_or_default();
    let _ = CMDLINE.set(CmdLine(UnsafeCell::new(command_line(&exe, relaunch_args))));
    std::panic::set_hook(Box::new(|info| {
        eprintln!("{info}");
        relaunch();
        std::process::abort();
    }));
    unsafe { SetUnhandledExceptionFilter(Some(filter)) };
}

/// Starts a fresh copy of the process (which applies the crash-loop rule itself); false on failure.
pub fn relaunch() -> bool {
    let Some(cmd) = CMDLINE.get() else {
        return false;
    };
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
    fn second_crash_within_the_guard_goes_to_explorer() {
        assert_eq!(should_relaunch(None, 1000, 60), ((1, 1000), true));
        assert_eq!(should_relaunch(Some((1, 1000)), 1030, 60), ((2, 1030), false));
        assert_eq!(should_relaunch(Some((2, 1030)), 1031, 60), ((3, 1031), false));
        // Outside the window the count starts again.
        assert_eq!(should_relaunch(Some((1, 1000)), 1060, 60), ((1, 1060), true));
    }

    #[test]
    fn recoveries_are_counted_in_the_file() {
        let dir = std::env::temp_dir().join(format!("rexplorer-crash-count-{}", std::process::id()));
        let p = dir.join("crash-count.txt");
        assert!(record_recovery_at(&p, 60));
        assert!(!record_recovery_at(&p, 60));
        let _ = std::fs::remove_file(&p);
        assert!(record_recovery_at(&p, 60), "a clean start (file removed) resets the count");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
