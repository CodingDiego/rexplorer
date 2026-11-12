//! Crash recovery: on an unhandled exception (typically a misbehaving in-process shell extension)
//! the process writes a minidump (and, for a panic, a text report) to `%LOCALAPPDATA%\rexplorer\crashes` (local
//! only, nothing is sent) and relaunches
//! itself with `--recover`. The crash path opens that one prepared file and nothing else: each `--recover` start
//! counts itself in `crash-count.txt` (beside the state file) instead, and the second one within the guard window
//! hands the folder to Windows Explorer rather than run (a crash loop ends there). A start without `--recover`
//! resets the count.

use std::cell::UnsafeCell;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE};
use windows::Win32::Storage::FileSystem::{CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_MODE};
use windows::Win32::System::Diagnostics::Debug::{
    EXCEPTION_EXECUTE_HANDLER, EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MiniDumpNormal, MiniDumpWriteDump,
    SetUnhandledExceptionFilter,
};
use windows::Win32::System::Threading::{
    CreateProcessW, GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, PROCESS_CREATION_FLAGS,
    PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

/// Prebuilt command line. `CreateProcessW` may write into it, so it is mutable; the crash path
/// is single-shot (the process is dying), so unsynchronized access is acceptable.
struct CmdLine(UnsafeCell<Vec<u16>>);
// SAFETY: written once in `install` before the filter is set, then only touched on the crash path.
unsafe impl Sync for CmdLine {}

static CMDLINE: OnceLock<CmdLine> = OnceLock::new();

/// One argument double-quoted so `CommandLineToArgvW` gives it back unchanged: backslashes before a quote, and
/// before the closing quote (`C:\` would otherwise escape it), are doubled.
fn quote_arg(s: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in s.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(slashes * 2 + 1));
                out.push('"');
                slashes = 0;
            }
            c => {
                out.push_str(&"\\".repeat(slashes));
                out.push(c);
                slashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(slashes * 2));
    out.push('"');
    out
}

/// Builds a command line: the exe quoted as is (argv[0] has no escapes), every argument by `quote_arg`, NUL-terminated.
pub fn command_line(exe: &Path, args: &[String]) -> Vec<u16> {
    let mut parts = vec![format!("\"{}\"", exe.to_string_lossy())];
    parts.extend(args.iter().map(|a| quote_arg(a)));
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

/// This process's dump file, named by `install` so the crash path builds nothing.
static DUMP: OnceLock<HSTRING> = OnceLock::new();
static DUMPED: AtomicBool = AtomicBool::new(false);
static RELAUNCHED: AtomicBool = AtomicBool::new(false);
/// Dumps kept in the crashes folder; `install` deletes the oldest beyond this.
const KEEP_DUMPS: usize = 10;

/// `%LOCALAPPDATA%\rexplorer\crashes`, falling back to the temp dir.
pub fn crash_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("rexplorer").join("crashes")
}

/// Whether the crashes folder holds a dump, for the offer after a recovery.
pub fn has_dumps() -> bool {
    !dumps(&crash_dir()).is_empty()
}

/// Our own `.dmp` files in `dir`, oldest first: names start with a zero-padded Unix time. Only these are pruned.
fn dumps(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dmp")))
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("rexplorer-")))
        .collect();
    v.sort();
    v
}

/// The oldest of `sorted` beyond the newest `keep`.
fn excess(sorted: &[PathBuf], keep: usize) -> &[PathBuf] {
    &sorted[..sorted.len().saturating_sub(keep)]
}

/// The crash path: one relaunch, then one minidump. The relaunch goes first because an in-process dump can
/// deadlock (heap or loader lock held by another thread) or fault again, and recovery must not depend on it.
fn on_crash(info: *const EXCEPTION_POINTERS) {
    if !RELAUNCHED.swap(true, Ordering::SeqCst) {
        relaunch();
    }
    if !DUMPED.swap(true, Ordering::SeqCst) {
        write_dump(info);
    }
}

/// A small dump (thread stacks, no heap) of this process; `info` is null for a panic.
fn write_dump(info: *const EXCEPTION_POINTERS) {
    let Some(path) = DUMP.get() else {
        return;
    };
    // SAFETY: `info` is the filter's own exception record (or null), valid for this call; the handle is closed here.
    unsafe {
        let Ok(file) = CreateFileW(path, GENERIC_WRITE.0, FILE_SHARE_MODE(0), None, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, None) else {
            return;
        };
        let ex = MINIDUMP_EXCEPTION_INFORMATION {
            ThreadId: GetCurrentThreadId(),
            ExceptionPointers: info as *mut _,
            ClientPointers: false.into(),
        };
        let ex = (!info.is_null()).then_some(&ex as *const _);
        let _ = MiniDumpWriteDump(GetCurrentProcess(), GetCurrentProcessId(), file, MiniDumpNormal, ex, None, None);
        let _ = CloseHandle(file);
    }
}

unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
    on_crash(info);
    EXCEPTION_EXECUTE_HANDLER
}

/// Pre-builds the relaunch command line and the dump path (pruning old dumps), and installs the
/// unhandled-exception filter and panic hook.
pub fn install(relaunch_args: &[String]) {
    let exe = std::env::current_exe().unwrap_or_default();
    let _ = CMDLINE.set(CmdLine(UnsafeCell::new(command_line(&exe, relaunch_args))));
    let dir = crash_dir();
    let _ = std::fs::create_dir_all(&dir);
    for old in excess(&dumps(&dir), KEEP_DUMPS - 1) {
        let _ = std::fs::remove_file(old);
        let _ = std::fs::remove_file(old.with_extension("txt"));
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let _ = DUMP.set(HSTRING::from(dir.join(format!("rexplorer-{now:011}-{}.dmp", std::process::id())).as_os_str()));
    std::panic::set_hook(Box::new(|info| {
        // The dump of a panic carries no exception record: the message and stack go in a text report beside it.
        if let Some(dump) = DUMP.get() {
            let report = format!("rexplorer {}\n{info}\n\n{}", env!("CARGO_PKG_VERSION"), std::backtrace::Backtrace::force_capture());
            let _ = std::fs::write(PathBuf::from(dump.to_os_string()).with_extension("txt"), report);
        }
        log(&format!("panic: {info}"));
        on_crash(std::ptr::null());
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

/// Appends `line` to `rexplorer.log` beside the state file, for what a later run must explain (a drag that dropped
/// nothing); errors are ignored.
pub fn log(line: &str) {
    log_to(&state_path().with_file_name("rexplorer.log"), line);
}

/// Past this size the log starts again, so it never grows without bound.
const LOG_MAX: u64 = 64 * 1024;

fn log_to(path: &Path, line: &str) {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let full = std::fs::metadata(path).is_ok_and(|m| m.len() > LOG_MAX);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let file = std::fs::OpenOptions::new().create(true).write(true).append(!full).truncate(full).open(path);
    if let Ok(mut f) = file {
        let _ = writeln!(f, "{now} {line}");
    }
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
    fn command_line_round_trips_through_argv() {
        let args: Vec<String> = [r"C:\", r"D:\a b\", r#"x"y"#, r#"a\"b"#, r"a\\b", "", r"\\server\share\"].map(String::from).into();
        let v = command_line(Path::new(r"C:\a b\x.exe"), &args);
        let mut n = 0;
        let argv = unsafe { windows::Win32::UI::Shell::CommandLineToArgvW(PCWSTR(v.as_ptr()), &mut n) };
        let got: Vec<String> = (0..n as usize).map(|i| unsafe { (*argv.add(i)).to_string().unwrap() }).collect();
        unsafe { windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(argv as _))) };
        assert_eq!(got[0], r"C:\a b\x.exe");
        assert_eq!(&got[1..], &args[..]);
    }

    #[test]
    fn only_the_newest_dumps_are_kept() {
        let names: Vec<PathBuf> =
            ["rexplorer-00000000100-7.dmp", "rexplorer-00000000200-3.dmp", "rexplorer-00000000300-9.dmp"].map(PathBuf::from).into();
        assert_eq!(excess(&names, 2), &names[..1]);
        assert!(excess(&names, 3).is_empty());
        assert!(excess(&names[..1], 9).is_empty());
    }

    #[test]
    fn only_our_own_dumps_are_listed() {
        let dir = std::env::temp_dir().join(format!("rexplorer-dumps-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        for name in ["rexplorer-00000000200-1.dmp", "rexplorer-00000000100-1.DMP", "rexplorer-00000000100-1.txt", "other.dmp"] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        let names: Vec<_> = dumps(&dir).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["rexplorer-00000000100-1.DMP", "rexplorer-00000000200-1.dmp"]);
        let _ = std::fs::remove_dir_all(&dir);
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
    fn log_appends_lines_and_restarts_when_full() {
        let dir = std::env::temp_dir().join(format!("rexplorer-log-{}", std::process::id()));
        let p = dir.join("rexplorer.log");
        log_to(&p, "one");
        log_to(&p, "two");
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text.lines().map(|l| l.split_once(' ').unwrap().1).collect::<Vec<_>>(), ["one", "two"]);
        std::fs::write(&p, vec![b'x'; LOG_MAX as usize + 1]).unwrap();
        log_to(&p, "three");
        assert!(std::fs::read_to_string(&p).unwrap().ends_with(" three\n"));
        assert!(std::fs::metadata(&p).unwrap().len() < 100, "a full log starts again");
        let _ = std::fs::remove_dir_all(&dir);
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
