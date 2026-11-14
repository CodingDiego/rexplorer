//! Hang watchdog: a thread posts `WM_HEARTBEAT` to the main window every second and, when the UI thread has not
//! answered for `HANG_AFTER`, appends what it was doing and for how long to `rexplorer.log`. It only observes:
//! nothing is killed or interrupted. Modal loops (menus, dialogs, the shell's progress UI) dispatch the
//! heartbeat too, so an open dialog is not a hang.
//! Every message the main pump handles (dispatched, or a key the accelerator table turns into a command) is timed:
//! the longest so far goes on the main window as properties for scripts/budgets.ps1 (#48), and debug builds log
//! each one over `SLOW` with its message, target window class and the operation it was in (#49).

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::time::{Duration, Instant};

use rexplorer_core::crash;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;

pub(crate) const WM_HEARTBEAT: u32 = WM_APP + 31;
const TICK: Duration = Duration::from_secs(1);
const HANG_AFTER: Duration = Duration::from_secs(5);
#[cfg(debug_assertions)]
const SLOW: Duration = Duration::from_millis(50);

static BEATS: AtomicU64 = AtomicU64::new(0);
/// The message the main pump is dispatching: (message << 32) | low 32 bits of wParam; 0 between messages.
static MSG: AtomicU64 = AtomicU64::new(0);
static OP: Mutex<&str> = Mutex::new("");
/// The last operation entered during the message being timed (OP is back to its old value once the handler returns).
static LAST_OP: Mutex<&str> = Mutex::new("");
/// The main window, which carries the slowest-message properties, and the slowest message so far in ms.
static MAIN: AtomicIsize = AtomicIsize::new(0);
static SLOWEST: AtomicUsize = AtomicUsize::new(0);
/// When the message being timed began a message loop of its own, if it did.
static MODAL: Mutex<Option<Instant>> = Mutex::new(None);

/// The message being handled begins a message loop of its own here (a window move or resize, a menu, a dialog, a
/// drag): from now on it lasts as long as the user takes, so `timed` counts it only up to this call. Call it right
/// before the loop; what the handler does after the loop is not counted either.
pub(crate) fn modal() {
    MODAL.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Instant::now);
}

/// The main window's answer to the heartbeat.
pub(crate) fn beat() {
    BEATS.fetch_add(1, Relaxed);
}

/// Names the slow operation the UI thread is in until the guard drops.
pub(crate) struct Op(&'static str);

pub(crate) fn op(label: &'static str) -> Op {
    *LAST_OP.lock().unwrap_or_else(|e| e.into_inner()) = label;
    Op(std::mem::replace(&mut *OP.lock().unwrap_or_else(|e| e.into_inner()), label))
}

impl Drop for Op {
    fn drop(&mut self) {
        *OP.lock().unwrap_or_else(|e| e.into_inner()) = self.0;
    }
}

/// Runs one message of the main loop through `f` (`DispatchMessageW`, or `TranslateAcceleratorW`, which sends the
/// command itself), recording it for a hang report and timing it. Cost per message: two `Instant::now` reads (QPC,
/// tens of ns), one uncontended lock and a compare; the window properties are written only when a message beats the
/// slowest so far. A message whose handler ran a message loop of its own counts up to where that loop began
/// (`modal`). A loop nobody reported (a menu a child control owns) is timed as one message for as long as it runs:
/// read such a line with its operation label.
pub(crate) fn timed<R>(msg: &MSG, f: impl FnOnce() -> R) -> R {
    MSG.store(((msg.message as u64) << 32) | (msg.wParam.0 as u64 & 0xFFFF_FFFF), Relaxed);
    *LAST_OP.lock().unwrap_or_else(|e| e.into_inner()) = "";
    // Read before the handler runs: WM_CLOSE and friends destroy the window.
    #[cfg(debug_assertions)]
    let class = class_of(msg.hwnd);
    *MODAL.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let start = Instant::now();
    let r = f();
    let looped = MODAL.lock().unwrap_or_else(|e| e.into_inner()).take();
    let took = looped.map_or_else(|| start.elapsed(), |at| at.duration_since(start));
    let ms = took.as_millis() as usize;
    let main = MAIN.load(Relaxed);
    if new_slowest(ms) && main != 0 {
        crate::instance::signal_slowest(HWND(main as *mut c_void), ms, MSG.load(Relaxed));
    }
    #[cfg(debug_assertions)]
    {
        let op = *LAST_OP.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(line) = slow_line(msg.message, msg.wParam.0, took, move || class, op) {
            crash::log(&line);
        }
    }
    MSG.store(0, Relaxed);
    r
}

/// Whether a message of `ms` is the slowest so far (and records it).
fn new_slowest(ms: usize) -> bool {
    ms > SLOWEST.fetch_max(ms, Relaxed)
}

/// Starts the watchdog thread for `hwnd`; it ends when the window is gone.
pub(crate) fn spawn(hwnd: HWND) {
    let h = hwnd.0 as isize;
    MAIN.store(h, Relaxed);
    std::thread::spawn(move || {
        let hwnd = HWND(h as *mut c_void);
        let (mut stall, mut last) = (Stall::default(), Instant::now());
        loop {
            // During a hang these queue up (one a second) and drain at once when the thread answers again.
            let _ = unsafe { PostMessageW(Some(hwnd), WM_HEARTBEAT, WPARAM(0), LPARAM(0)) };
            std::thread::sleep(TICK);
            if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
                return;
            }
            let now = Instant::now();
            match stall.check(BEATS.load(Relaxed), now - last, HANG_AFTER) {
                Some(Hang::Started(d)) => crash::log(&format!("hang: UI thread blocked {} s in {}", d.as_secs(), doing())),
                Some(Hang::Ended(d)) => crash::log(&format!("hang: UI thread answered again after {} s", d.as_secs())),
                None => {}
            }
            last = now;
        }
    });
}

/// The operation label and the message being dispatched, for a hang report.
fn doing() -> String {
    let op = *OP.lock().unwrap_or_else(|e| e.into_inner());
    let m = MSG.load(Relaxed);
    let msg = if m == 0 { "no message".to_string() } else { label((m >> 32) as u32, (m & 0xFFFF_FFFF) as usize) };
    format!("{} ({msg})", if op.is_empty() { "-" } else { op })
}

#[derive(Debug, PartialEq)]
enum Hang {
    Started(Duration),
    Ended(Duration),
}

#[derive(Default)]
struct Stall {
    seen: u64,
    blocked: Duration,
    reported: bool,
}

impl Stall {
    /// One check, `step` after the previous one: `beats` is the heartbeat count now. A hang is reported once when
    /// it passes `after`, and its end once. A step is counted as at most two ticks, so a system sleep is no hang.
    fn check(&mut self, beats: u64, step: Duration, after: Duration) -> Option<Hang> {
        if beats != self.seen {
            self.seen = beats;
            let blocked = std::mem::take(&mut self.blocked);
            return std::mem::take(&mut self.reported).then_some(Hang::Ended(blocked));
        }
        self.blocked += step.min(TICK * 2);
        (!self.reported && self.blocked >= after).then(|| {
            self.reported = true;
            Hang::Started(self.blocked)
        })
    }
}

/// A message and its wParam (the command, timer or key id for the messages that carry one).
fn label(message: u32, wparam: usize) -> String {
    let known = [
        (WM_COMMAND, "WM_COMMAND"),
        (WM_NOTIFY, "WM_NOTIFY"),
        (WM_TIMER, "WM_TIMER"),
        (WM_PAINT, "WM_PAINT"),
        (WM_CLOSE, "WM_CLOSE"),
        (WM_KEYDOWN, "WM_KEYDOWN"),
        (WM_KEYUP, "WM_KEYUP"),
        (WM_SYSKEYDOWN, "WM_SYSKEYDOWN"),
        (WM_CHAR, "WM_CHAR"),
        (WM_LBUTTONDOWN, "WM_LBUTTONDOWN"),
        (WM_LBUTTONUP, "WM_LBUTTONUP"),
        (WM_LBUTTONDBLCLK, "WM_LBUTTONDBLCLK"),
        (WM_RBUTTONDOWN, "WM_RBUTTONDOWN"),
        (WM_RBUTTONUP, "WM_RBUTTONUP"),
        (WM_MOUSEMOVE, "WM_MOUSEMOVE"),
        (WM_MOUSEWHEEL, "WM_MOUSEWHEEL"),
        (WM_CONTEXTMENU, "WM_CONTEXTMENU"),
    ];
    let name = match known.iter().find(|k| k.0 == message) {
        Some(k) => k.1.to_string(),
        None if message >= WM_APP => format!("WM_APP+{}", message - WM_APP),
        None => format!("0x{message:04X}"),
    };
    // A command's id in decimal, as the `CMD_*` / `ID_*` constants are written; the notification code only when set.
    match (message, wparam >> 16 & 0xFFFF) {
        (WM_COMMAND, 0) => format!("{name} id {}", wparam & 0xFFFF),
        (WM_COMMAND, code) => format!("{name} id {} code {code:#x}", wparam & 0xFFFF),
        _ => format!("{name} {wparam:#x}"),
    }
}

/// The main window's own `WM_APP` messages by their constant's name (other classes reuse the same numbers).
#[cfg(debug_assertions)]
fn main_app_message(message: u32) -> Option<&'static str> {
    let known = [
        (crate::WM_ENUM_DONE, "WM_ENUM_DONE"),
        (crate::WM_SHELL_CHANGE, "WM_SHELL_CHANGE"),
        (crate::panes::WM_TREE_SELECT, "WM_TREE_SELECT"),
        (crate::source::WM_SEARCH_BATCH, "WM_SEARCH_BATCH"),
        (crate::source::WM_BROWSER_NAV, "WM_BROWSER_NAV"),
        (crate::source::WM_WORKER_ERROR, "WM_WORKER_ERROR"),
        (crate::views::WM_THUMB_DONE, "WM_THUMB_DONE"),
        (crate::panes::WM_SIDEBAR_ROWS, "WM_SIDEBAR_ROWS"),
        (crate::panes::WM_QUICK_ACCESS_DONE, "WM_QUICK_ACCESS_DONE"),
        (crate::views::WM_ICONS_EXPORTED, "WM_ICONS_EXPORTED"),
        (crate::WM_UPDATE_FOUND, "WM_UPDATE_FOUND"),
        (crate::source::WM_ARCHIVE_PASSWORD, "WM_ARCHIVE_PASSWORD"),
        (crate::WM_CRUMB_ITEMS, "WM_CRUMB_ITEMS"),
        (crate::nav::WM_TAB_RETURN, "WM_TAB_RETURN"),
        (WM_HEARTBEAT, "WM_HEARTBEAT"),
    ];
    known.iter().find(|k| k.0 == message).map(|k| k.1)
}

/// "slow: <message and wParam> [= <main window's name for it>] to <window class> took <n> ms [in <operation>]".
#[cfg(debug_assertions)]
fn slow_line(message: u32, wparam: usize, took: Duration, class: impl FnOnce() -> String, op: &str) -> Option<String> {
    (took > SLOW).then(|| {
        let class = class();
        let mut msg = label(message, wparam);
        if let Some(name) = main_app_message(message).filter(|_| class == "RexplorerMain") {
            msg = format!("{msg} = {name}");
        }
        let op = if op.is_empty() { String::new() } else { format!(" in {op}") };
        format!("slow: {msg} to {class} took {} ms{op}", took.as_millis())
    })
}

#[cfg(debug_assertions)]
fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) }.max(0) as usize;
    if n == 0 { "(no window)".into() } else { String::from_utf16_lossy(&buf[..n]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hang_is_reported_once_and_its_end_once() {
        let mut s = Stall::default();
        assert_eq!(s.check(1, TICK, HANG_AFTER), None, "answered");
        for _ in 0..4 {
            assert_eq!(s.check(1, TICK, HANG_AFTER), None, "blocked under the threshold");
        }
        assert_eq!(s.check(1, TICK, HANG_AFTER), Some(Hang::Started(HANG_AFTER)));
        assert_eq!(s.check(1, TICK, HANG_AFTER), None, "reported once");
        assert_eq!(s.check(9, TICK, HANG_AFTER), Some(Hang::Ended(HANG_AFTER + TICK)));
        assert_eq!(s.check(10, TICK, HANG_AFTER), None);
    }

    #[test]
    fn a_short_stall_or_a_system_sleep_is_no_hang() {
        let mut s = Stall::default();
        assert_eq!(s.check(0, Duration::from_secs(3600), HANG_AFTER), None, "one long step counts as two ticks");
        assert_eq!(s.check(1, TICK, HANG_AFTER), None, "answered again: nothing to report");
        assert_eq!(s.blocked, Duration::ZERO);
    }

    #[test]
    fn labels_name_known_and_app_messages() {
        assert_eq!(label(WM_COMMAND, 7), "WM_COMMAND id 7", "the id as CMD_ENTER is written");
        assert_eq!(label(WM_COMMAND, 0x0300_0064), "WM_COMMAND id 100 code 0x300", "EN_CHANGE from ID_ADDR");
        assert_eq!(label(WM_APP + 2, 0), "WM_APP+2 0x0");
        assert_eq!(label(0x1113, 1), "0x1113 0x1");
    }

    #[test]
    fn only_a_new_maximum_counts_as_the_slowest() {
        let before = SLOWEST.load(Relaxed);
        assert!(new_slowest(before + 120));
        assert!(!new_slowest(before + 120), "equal is not slower");
        assert!(!new_slowest(before + 5));
        assert_eq!(SLOWEST.load(Relaxed), before + 120);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn only_messages_over_the_budget_are_logged() {
        assert_eq!(slow_line(WM_TIMER, 1, SLOW, || unreachable!(), ""), None, "50 ms is within the budget");
        let line = slow_line(WM_TIMER, 1, SLOW + Duration::from_millis(1), || "RexplorerMain".into(), "");
        assert_eq!(line.as_deref(), Some("slow: WM_TIMER 0x1 to RexplorerMain took 51 ms"));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn a_slow_line_names_the_main_windows_messages_and_the_operation() {
        let took = Duration::from_millis(230);
        let line = slow_line(crate::WM_ENUM_DONE, 3, took, || "RexplorerMain".into(), "navigate");
        assert_eq!(line.as_deref(), Some("slow: WM_APP+1 0x3 = WM_ENUM_DONE to RexplorerMain took 230 ms in navigate"));
        // The palette's WM_APP+1 is its own message, not the main window's.
        let line = slow_line(WM_APP + 1, 0, took, || "RexplorerPalette".into(), "");
        assert_eq!(line.as_deref(), Some("slow: WM_APP+1 0x0 to RexplorerPalette took 230 ms"));
        let line = slow_line(WM_COMMAND, 4, took, || "RexplorerMain".into(), "");
        assert_eq!(line.as_deref(), Some("slow: WM_COMMAND id 4 to RexplorerMain took 230 ms"));
    }
}
