//! Crash recovery: on an unhandled exception (typically a misbehaving in-process shell extension)
//! the process writes a minidump and a text report to `%LOCALAPPDATA%\rexplorer\crashes` (local
//! only, nothing is sent) and relaunches
//! itself with `--recover`. A panic writes its report at once; an SEH crash writes a fixed-size `.seh` record that
//! the next start turns into the report. The crash path opens that one prepared file and nothing else: each `--recover` start
//! counts itself in `crash-count.txt` (beside the state file) instead, and the second one within the guard window
//! hands the folder to Windows Explorer rather than run (a crash loop ends there). A start without `--recover`
//! resets the count.

use std::cell::UnsafeCell;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::{CloseHandle, FILETIME, GENERIC_WRITE, HANDLE, HMODULE, SYSTEMTIME};
use windows::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_BEGIN, FILE_SHARE_MODE, SetFilePointerEx, WriteFile,
};
use windows::Win32::System::Diagnostics::Debug::{
    EXCEPTION_EXECUTE_HANDLER, EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MiniDumpNormal, MiniDumpWriteDump,
    SetUnhandledExceptionFilter,
};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
    GetModuleHandleExW,
};
use windows::Win32::System::Time::FileTimeToSystemTime;
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

/// What the exception filter knows about an SEH crash, written as raw bytes: formatting text there would need the
/// heap, which may be the thing that is broken.
#[repr(C)]
struct SehRecord {
    magic: [u8; 4],
    code: u32,
    address: u64,
    module_base: u64,
    thread: u32,
    _pad: u32,
    time: u64,
    version: [u8; 16],
    module: [u16; 260],
}

const SEH_MAGIC: [u8; 4] = *b"RXS1";

const fn version_bytes() -> [u8; 16] {
    let v = env!("CARGO_PKG_VERSION").as_bytes();
    let mut out = [0u8; 16];
    let mut i = 0;
    while i < v.len() && i < out.len() {
        out[i] = v[i];
        i += 1;
    }
    out
}

const SEH_EMPTY: SehRecord = SehRecord {
    magic: SEH_MAGIC,
    code: 0,
    address: 0,
    module_base: 0,
    thread: 0,
    _pad: 0,
    time: 0,
    version: version_bytes(),
    module: [0; 260],
};

/// The record the filter fills, static so a stack overflow does not have to find room for it.
struct SehBuf(UnsafeCell<SehRecord>);
// SAFETY: only the crash path touches it, and the DUMPED flag lets one thread through.
unsafe impl Sync for SehBuf {}
static SEH: SehBuf = SehBuf(UnsafeCell::new(SEH_EMPTY));
/// This process's `.seh` file, opened by `install` (no sharing, so another start leaves it alone while we live).
static SEH_FILE: OnceLock<isize> = OnceLock::new();

fn seh_bytes(r: &SehRecord) -> &[u8] {
    // SAFETY: plain-data repr(C) struct with explicit padding, read as bytes for its own size.
    unsafe { std::slice::from_raw_parts((r as *const SehRecord).cast::<u8>(), size_of::<SehRecord>()) }
}

fn parse_seh(bytes: &[u8]) -> Option<SehRecord> {
    (bytes.len() == size_of::<SehRecord>() && bytes[..4] == SEH_MAGIC)
        // SAFETY: the length matches and every bit pattern is a valid SehRecord.
        .then(|| unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<SehRecord>()) })
}

fn code_name(code: u32) -> Option<&'static str> {
    Some(match code {
        0xC000_0005 => "access violation",
        0xC000_0006 => "in-page error",
        0xC000_001D => "illegal instruction",
        0xC000_0094 => "integer divide by zero",
        0xC000_0096 => "privileged instruction",
        0xC000_00FD => "stack overflow",
        0xC000_0374 => "heap corruption",
        0xC000_0409 => "stack buffer overrun",
        0x8000_0003 => "breakpoint",
        0xE06D_7363 => "C++ exception",
        _ => return None,
    })
}

/// `time` (Unix seconds) as UTC, or the bare number when Windows cannot convert it.
fn utc(time: u64) -> String {
    let ticks = time.saturating_add(11_644_473_600).saturating_mul(10_000_000);
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut st = SYSTEMTIME::default();
    // SAFETY: both pointers are to locals.
    match unsafe { FileTimeToSystemTime(&ft, &mut st) } {
        Ok(()) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
        ),
        Err(_) => time.to_string(),
    }
}

/// The text report for an SEH record, in the shape of the panic one (version first).
fn seh_report(r: &SehRecord) -> String {
    let version = String::from_utf8_lossy(&r.version);
    let name = code_name(r.code).map(|n| format!(" ({n})")).unwrap_or_default();
    let module_len = r.module.iter().position(|&c| c == 0).unwrap_or(r.module.len());
    let place = if r.module_base == 0 || module_len == 0 {
        "module: not resolved".to_string()
    } else {
        format!("module: {}+{:#x}", String::from_utf16_lossy(&r.module[..module_len]), r.address.wrapping_sub(r.module_base))
    };
    format!(
        "rexplorer {}\nexception {:#010X}{name} at {:#018X}\n{place}\nthread {}\ntime {}\n",
        version.trim_end_matches('\0'),
        r.code,
        r.address,
        r.thread,
        utc(r.time)
    )
}

/// Turns each finished `.seh` record in `dir` into the `.txt` beside its `.dmp`. An empty record (a run that did
/// not crash) is deleted; one still open by a running process cannot be read and is left alone.
fn convert_seh_records(dir: &Path) {
    let records = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| {
        p.extension().is_some_and(|e| e.eq_ignore_ascii_case("seh"))
            && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("rexplorer-"))
    });
    for path in records {
        // Never read a file of another size whole; a live run's record is locked, so removing it fails.
        if std::fs::metadata(&path).map_or(true, |m| m.len() != size_of::<SehRecord>() as u64) {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let txt = path.with_extension("txt");
        if let Some(r) = parse_seh(&bytes)
            && !txt.exists()
        {
            let _ = std::fs::write(&txt, seh_report(&r));
        }
        let _ = std::fs::remove_file(&path);
    }
}

/// Fills and writes the SEH record; nothing here allocates. `info` is null for a panic, which has its own report.
fn write_record(info: *const EXCEPTION_POINTERS) {
    let Some(&h) = SEH_FILE.get() else {
        return;
    };
    let file = HANDLE(h as *mut std::ffi::c_void);
    // SAFETY: `info` is the filter's own exception pointers (or null); DUMPED lets one thread use the static.
    unsafe {
        let Some(ex) = info.as_ref().and_then(|i| i.ExceptionRecord.as_ref()) else {
            return;
        };
        let rec = &mut *SEH.0.get();
        rec.code = ex.ExceptionCode.0 as u32;
        rec.address = ex.ExceptionAddress as u64;
        rec.thread = GetCurrentThreadId();
        rec.time = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        put_record(file, rec);
        // The module lookup can block on a loader lock the dying process holds: the record above is on disk first.
        let mut module = HMODULE::default();
        let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
        if GetModuleHandleExW(flags, PCWSTR(ex.ExceptionAddress.cast()), &mut module).is_ok() {
            rec.module_base = module.0 as u64;
            GetModuleFileNameW(Some(module), &mut rec.module);
            put_record(file, rec);
        }
        // Unlocks the record so the --recover start, if it scans after this, reports it instead of the start after.
        let _ = CloseHandle(file);
    }
}

fn put_record(file: HANDLE, rec: &SehRecord) {
    let mut written = 0u32;
    // SAFETY: `file` is the handle `install` opened for writing; the buffer outlives the call.
    unsafe {
        let _ = SetFilePointerEx(file, 0, None, FILE_BEGIN);
        let _ = WriteFile(file, Some(seh_bytes(rec)), Some(&mut written), None);
    }
}

/// The crash path: one relaunch, then the SEH record and one minidump. The relaunch goes first because an in-process
/// dump can deadlock (heap or loader lock held by another thread) or fault again, and recovery must not depend on it.
fn on_crash(info: *const EXCEPTION_POINTERS) {
    if !RELAUNCHED.swap(true, Ordering::SeqCst) {
        relaunch();
    }
    if !DUMPED.swap(true, Ordering::SeqCst) {
        write_record(info);
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

/// Pre-builds the relaunch command line and the dump path (pruning old dumps), turns the last SEH crash's record
/// into its text report, opens this run's record file, and installs the unhandled-exception filter and panic hook.
pub fn install(relaunch_args: &[String]) {
    let exe = std::env::current_exe().unwrap_or_default();
    let _ = CMDLINE.set(CmdLine(UnsafeCell::new(command_line(&exe, relaunch_args))));
    let dir = crash_dir();
    let _ = std::fs::create_dir_all(&dir);
    convert_seh_records(&dir);
    for old in excess(&dumps(&dir), KEEP_DUMPS - 1) {
        let _ = std::fs::remove_file(old);
        let _ = std::fs::remove_file(old.with_extension("txt"));
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let stem = dir.join(format!("rexplorer-{now:011}-{}", std::process::id()));
    let _ = DUMP.set(HSTRING::from(stem.with_extension("dmp").as_os_str()));
    let seh = HSTRING::from(stem.with_extension("seh").as_os_str());
    // SAFETY: plain file open; the handle stays open for the life of the process.
    if let Ok(h) = unsafe { CreateFileW(&seh, GENERIC_WRITE.0, FILE_SHARE_MODE(0), None, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, None) } {
        let _ = SEH_FILE.set(h.0 as isize);
    }
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

    fn sample_record() -> SehRecord {
        let mut module = [0u16; 260];
        for (d, s) in module.iter_mut().zip(r"C:\x\bad.dll".encode_utf16()) {
            *d = s;
        }
        SehRecord { code: 0xC000_0005, address: 0x7FF0_0000_1234, module_base: 0x7FF0_0000_0000, thread: 42, time: 0, module, ..SEH_EMPTY }
    }

    #[test]
    fn seh_record_round_trips_into_a_report() {
        let r = parse_seh(seh_bytes(&sample_record())).expect("own bytes parse");
        let text = seh_report(&r);
        assert_eq!(
            text,
            format!(
                "rexplorer {}\nexception 0xC0000005 (access violation) at 0x00007FF000001234\nmodule: C:\\x\\bad.dll+0x1234\nthread 42\ntime 1970-01-01 00:00:00 UTC\n",
                env!("CARGO_PKG_VERSION")
            )
        );
        let unresolved = SehRecord { code: 0x1234, module_base: 0, ..sample_record() };
        assert!(seh_report(&unresolved).contains("exception 0x00001234 at"), "an unknown code has no name");
        assert!(seh_report(&unresolved).contains("module: not resolved"));
        assert!(parse_seh(&[]).is_none(), "an empty record (a run that did not crash) is no report");
        assert_eq!(utc(u64::MAX), u64::MAX.to_string(), "a bad time falls back to the number, no overflow");
        assert!(parse_seh(&seh_bytes(&sample_record())[1..]).is_none());
    }

    #[test]
    fn seh_records_become_reports_and_are_removed() {
        let dir = std::env::temp_dir().join(format!("rexplorer-seh-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("rexplorer-00000000100-1.seh"), seh_bytes(&sample_record())).unwrap();
        std::fs::write(dir.join("rexplorer-00000000200-1.seh"), b"").unwrap();
        std::fs::write(dir.join("rexplorer-00000000300-1.seh"), seh_bytes(&sample_record())).unwrap();
        std::fs::write(dir.join("rexplorer-00000000300-1.txt"), "panic report").unwrap();
        let mut big = seh_bytes(&sample_record()).to_vec();
        big.resize(1 << 20, 0);
        std::fs::write(dir.join("rexplorer-00000000400-1.seh"), big).unwrap();
        convert_seh_records(&dir);
        assert!(!dir.join("rexplorer-00000000400-1.txt").exists(), "a file of another size is no record");
        assert!(std::fs::read_to_string(dir.join("rexplorer-00000000100-1.txt")).unwrap().contains("access violation"));
        assert!(!dir.join("rexplorer-00000000200-1.txt").exists(), "an empty record makes no report");
        assert_eq!(std::fs::read_to_string(dir.join("rexplorer-00000000300-1.txt")).unwrap(), "panic report");
        let left = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.path().extension().is_some_and(|x| x == "seh")).count();
        assert_eq!(left, 0);
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
