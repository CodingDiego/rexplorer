//! Single-instance plumbing (named mutex + WM_COPYDATA hand-off) and external launchers.

use std::path::Path;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_TIMEOUT, E_FAIL, GetLastError, HANDLE, HWND, LPARAM, WPARAM,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GA_ROOT, GetAncestor, GetClassNameW, GetPropW, IsHungAppWindow, SMTO_ABORTIFHUNG, SMTO_NORMAL, SetPropW, SW_SHOWNORMAL,
    SendMessageTimeoutW, SetForegroundWindow, WM_COPYDATA,
};
use windows::core::{Error, HSTRING, PCWSTR, Result, w};

/// Arguments for a separate window on `folder`. A command line is readable by the user's other processes, so a
/// name that carries a password (an FTP URL) opens the start place instead.
pub fn new_window_args(folder: &str) -> Vec<String> {
    let mut a = vec!["--new-window".to_string()];
    if !folder.is_empty() && !folder.starts_with(['-', '/']) && crate::connect::redact(folder) == folder {
        a.push(folder.to_string());
    }
    a
}

/// Ctrl+N: starts another process that owns its own window.
pub fn open_new_window(folder: &str) -> bool {
    std::env::current_exe().and_then(|exe| std::process::Command::new(exe).args(new_window_args(folder)).spawn()).is_ok()
}

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

// Signals for scripts/budgets.ps1 (#48), read from outside with GetProp: the tick count (ms since boot) of the first
// paint, how many folder loads have finished, the tick count of the last one, and the slowest UI-thread message.
const PAINTED_PROP: PCWSTR = w!("rexplorer.painted");
const LOADS_PROP: PCWSTR = w!("rexplorer.loads");
const LOAD_TICK_PROP: PCWSTR = w!("rexplorer.loadtick");
const SLOWEST_PROP: PCWSTR = w!("rexplorer.slowest");
const SLOWEST_MSG_PROP: PCWSTR = w!("rexplorer.slowestmsg");

fn set_prop(hwnd: HWND, name: PCWSTR, v: usize) {
    unsafe {
        let _ = SetPropW(hwnd, name, Some(HANDLE(v as *mut _)));
    }
}

fn tick() -> usize {
    // Never 0: a property of 0 reads the same as no property.
    (unsafe { windows::Win32::System::SystemInformation::GetTickCount64() } as usize).max(1)
}

/// Called on every paint; the property keeps the first one.
pub fn signal_painted(hwnd: HWND) {
    if unsafe { GetPropW(hwnd, PAINTED_PROP) }.0.is_null() {
        set_prop(hwnd, PAINTED_PROP, tick());
    }
}

/// A folder load finished and its items are on screen.
pub fn signal_loaded(hwnd: HWND) {
    set_prop(hwnd, LOADS_PROP, unsafe { GetPropW(hwnd, LOADS_PROP) }.0 as usize + 1);
    set_prop(hwnd, LOAD_TICK_PROP, tick());
}

/// The UI thread's slowest message so far (`watchdog::timed`): `rexplorer.slowest` in ms, `rexplorer.slowestmsg` as
/// (message << 32) | low 32 bits of wParam.
pub fn signal_slowest(hwnd: HWND, ms: usize, msg: u64) {
    set_prop(hwnd, SLOWEST_PROP, ms.max(1));
    set_prop(hwnd, SLOWEST_MSG_PROP, msg as usize);
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

/// Another rexplorer window, of any profile, whose part is `at` (a child or the window itself); not `own`.
pub fn other_main_window(at: HWND, own: HWND) -> Option<HWND> {
    unsafe {
        let root = GetAncestor(at, GA_ROOT);
        let mut class = [0u16; 16];
        let n = GetClassNameW(root, &mut class).max(0) as usize;
        (root != own && !root.is_invalid() && String::from_utf16_lossy(&class[..n]) == "RexplorerMain").then_some(root)
    }
}

/// A tab released at `pt` lands on a window only inside the frame a person sees (`frame`: DWM's extended frame
/// bounds, without the invisible resize border and the shadow) of a window that is `shown` (visible, not
/// minimized, not cloaked on another virtual desktop).
fn lands_on(pt: (i32, i32), frame: windows::Win32::Foundation::RECT, shown: bool) -> bool {
    shown && pt.0 >= frame.left && pt.0 < frame.right && pt.1 >= frame.top && pt.1 < frame.bottom
}

/// `lands_on` for window `w`. WindowFromPoint also answers a top-level window for the ~8 px invisible resize border
/// around its visible edge, so a release just beside it counted as over it.
pub fn release_lands_on(w: HWND, pt: (i32, i32)) -> bool {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsIconic, IsWindowVisible};
    unsafe {
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(w, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4);
        let mut frame = RECT::default();
        if DwmGetWindowAttribute(w, DWMWA_EXTENDED_FRAME_BOUNDS, (&mut frame as *mut RECT).cast(), size_of::<RECT>() as u32).is_err() {
            let _ = GetWindowRect(w, &mut frame);
        }
        lands_on(pt, frame, IsWindowVisible(w).as_bool() && !IsIconic(w).as_bool() && cloaked == 0)
    }
}

/// The hand-off that opens `place` in a new tab of the receiving window.
pub fn tab_payload(place: &str) -> String {
    rexplorer_core::cli::payload(&rexplorer_core::cli::Args { paths: vec![place.to_string()], new_tab: true, ..Default::default() })
}

pub fn send(hwnd: HWND, payload: &str) -> bool {
    let wide: Vec<u16> = payload.encode_utf16().chain(Some(0)).collect();
    let cds = COPYDATASTRUCT {
        dwData: COPYDATA_MAGIC,
        cbData: (wide.len() * 2) as u32,
        lpData: wide.as_ptr() as *mut _,
    };
    // The receiver's answer: 1 when it took the hand-off, 0 when it refused it (another protocol version).
    let mut answer = 0usize;
    unsafe {
        let mut call = |flags, ms| SendMessageTimeoutW(hwnd, WM_COPYDATA, WPARAM(0), LPARAM(&cds as *const _ as isize), flags, ms, Some(&mut answer as *mut usize));
        let mut r = call(SMTO_ABORTIFHUNG, 3000);
        // A busy UI thread is not a dead one: wait longer before this launch becomes a second window. Read before
        // anything else can clobber the error; other failures (access denied) and a hung window fall through unchanged.
        if r.0 == 0 && GetLastError() == ERROR_TIMEOUT && !IsHungAppWindow(hwnd).as_bool() {
            r = call(SMTO_NORMAL, 10_000);
        }
        let _ = SetForegroundWindow(hwnd);
        r.0 != 0 && answer == 1
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

    /// The budget script reads these three properties (#48): the first paint is kept, each load adds one.
    #[test]
    fn budget_signals_keep_the_first_paint_and_count_loads() {
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
        };
        let h = unsafe {
            CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, None, None)
        }
        .unwrap();
        let read = |name| unsafe { GetPropW(h, name) }.0 as usize;
        assert_eq!((read(PAINTED_PROP), read(LOADS_PROP)), (0, 0));
        signal_painted(h);
        let first = read(PAINTED_PROP);
        assert!(first > 0);
        std::thread::sleep(std::time::Duration::from_millis(30));
        signal_painted(h);
        assert_eq!(read(PAINTED_PROP), first, "a later paint does not move it");
        signal_loaded(h);
        signal_loaded(h);
        assert_eq!(read(LOADS_PROP), 2);
        assert!(read(LOAD_TICK_PROP) >= first);
        signal_slowest(h, 0, 0x0111_0000_0007);
        assert_eq!((read(SLOWEST_PROP), read(SLOWEST_MSG_PROP)), (1, 0x0111_0000_0007), "0 ms reads as 1, not as unset");
        signal_slowest(h, 140, 0x8001_0000_0003);
        assert_eq!((read(SLOWEST_PROP), read(SLOWEST_MSG_PROP)), (140, 0x8001_0000_0003));
        let _ = unsafe { DestroyWindow(h) };
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
    fn new_window_args_keep_secrets_off_the_command_line() {
        assert_eq!(new_window_args("C:\\a b"), ["--new-window", "C:\\a b"]);
        for none in ["", "ftp://bob:pw@h/x", "-x"] {
            assert_eq!(new_window_args(none), ["--new-window"], "{none:?}");
        }
        assert_eq!(new_window_args("ftp://bob@h/x").len(), 2);
        let a = rexplorer_core::cli::parse(new_window_args("C:\\a b")).unwrap();
        assert!(a.new_window && a.paths == ["C:\\a b"]);
    }

    #[test]
    fn a_tab_moves_only_onto_another_rexplorer_window() {
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, WINDOW_EX_STYLE, WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW};
        unsafe {
            unsafe extern "system" fn plain(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> windows::Win32::Foundation::LRESULT {
                unsafe { DefWindowProcW(h, m, w, l) }
            }
            let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap().into();
            let wc = WNDCLASSW { lpfnWndProc: Some(plain), hInstance: hinst, lpszClassName: w!("RexplorerMain"), ..Default::default() };
            RegisterClassW(&wc);
            let other = CreateWindowExW(WINDOW_EX_STYLE(0), w!("RexplorerMain"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 100, 100, None, None, Some(hinst), None).unwrap();
            let child = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_CHILD, 0, 0, 10, 10, Some(other), None, None, None).unwrap();
            let stranger = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 10, 10, None, None, None, None).unwrap();
            assert_eq!(other_main_window(child, stranger), Some(other), "a control of another window: that window");
            assert_eq!(other_main_window(other, stranger), Some(other));
            assert_eq!(other_main_window(child, other), None, "our own window");
            assert_eq!(other_main_window(stranger, other), None, "not rexplorer");
            let _ = DestroyWindow(other);
            let _ = DestroyWindow(stranger);
        }
        // Versioned like a second launch's hand-off; the receiver reads it as one new tab.
        let p = tab_payload("C:\\a b");
        assert!(p.starts_with(&format!("v={}\n", rexplorer_core::cli::PROTOCOL)));
        let a = rexplorer_core::cli::parse_payload(&p).unwrap();
        assert!(a.new_tab && a.paths == ["C:\\a b"]);
    }

    #[test]
    fn a_release_counts_only_inside_the_visible_frame_of_a_shown_window() {
        use windows::Win32::Foundation::RECT;
        let frame = RECT { left: 100, top: 50, right: 900, bottom: 650 };
        assert!(lands_on((100, 50), frame, true) && lands_on((899, 649), frame, true));
        // The invisible resize border WindowFromPoint still answers for: just outside the visible edge.
        for beside in [(95, 300), (904, 300), (500, 45), (500, 656), (900, 300), (500, 650)] {
            assert!(!lands_on(beside, frame, true), "{beside:?}");
        }
        assert!(!lands_on((500, 300), frame, false), "a minimized, hidden or cloaked window takes nothing");
    }

    #[test]
    fn release_lands_on_a_real_window_only_when_shown() {
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, SW_SHOWNOACTIVATE, ShowWindow, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW};
        unsafe {
            let w = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 200, 200, 300, 300, None, None, None, None).unwrap();
            assert!(!release_lands_on(w, (350, 350)), "hidden");
            let _ = ShowWindow(w, SW_SHOWNOACTIVATE);
            let shown = release_lands_on(w, (350, 350));
            let _ = DestroyWindow(w);
            assert!(shown);
        }
    }

    #[test]
    fn find_main_window_no_panic() {
        let _ = find_main_window();
    }
}
