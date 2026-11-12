//! Hang watchdog: a thread posts `WM_HEARTBEAT` to the main window every second and, when the UI thread has not
//! answered for `HANG_AFTER`, appends what it was doing and for how long to `rexplorer.log`. It only observes:
//! nothing is killed or interrupted. Modal loops (menus, dialogs, the shell's progress UI) dispatch the
//! heartbeat too, so an open dialog is not a hang.
//! Debug builds also log each message the main pump dispatches that takes over `SLOW`.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
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

/// The main window's answer to the heartbeat.
pub(crate) fn beat() {
    BEATS.fetch_add(1, Relaxed);
}

/// Names the slow operation the UI thread is in until the guard drops.
pub(crate) struct Op(&'static str);

pub(crate) fn op(label: &'static str) -> Op {
    Op(std::mem::replace(&mut *OP.lock().unwrap_or_else(|e| e.into_inner()), label))
}

impl Drop for Op {
    fn drop(&mut self) {
        *OP.lock().unwrap_or_else(|e| e.into_inner()) = self.0;
    }
}

/// `DispatchMessageW` for the main loop, recording the message for a hang report.
pub(crate) unsafe fn dispatch(msg: &MSG) {
    MSG.store(((msg.message as u64) << 32) | (msg.wParam.0 as u64 & 0xFFFF_FFFF), Relaxed);
    #[cfg(debug_assertions)]
    let start = Instant::now();
    DispatchMessageW(msg);
    #[cfg(debug_assertions)]
    if let Some(line) = slow_line(msg.message, msg.wParam.0, start.elapsed(), || class_of(msg.hwnd)) {
        crash::log(&line);
    }
    MSG.store(0, Relaxed);
}

/// Starts the watchdog thread for `hwnd`; it ends when the window is gone.
pub(crate) fn spawn(hwnd: HWND) {
    let h = hwnd.0 as isize;
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
        (WM_KEYDOWN, "WM_KEYDOWN"),
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
    format!("{name} {wparam:#x}")
}

#[cfg(debug_assertions)]
fn slow_line(message: u32, wparam: usize, took: Duration, class: impl FnOnce() -> String) -> Option<String> {
    (took > SLOW).then(|| format!("slow: {} to {} took {} ms", label(message, wparam), class(), took.as_millis()))
}

#[cfg(debug_assertions)]
fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) }.max(0) as usize;
    String::from_utf16_lossy(&buf[..n])
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
        assert_eq!(label(WM_COMMAND, 0x9c41), "WM_COMMAND 0x9c41");
        assert_eq!(label(WM_APP + 2, 0), "WM_APP+2 0x0");
        assert_eq!(label(0x1113, 1), "0x1113 0x1");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn only_messages_over_the_budget_are_logged() {
        assert_eq!(slow_line(WM_TIMER, 1, SLOW, || unreachable!()), None);
        let line = slow_line(WM_TIMER, 1, SLOW + Duration::from_millis(1), || "RexplorerMain".into());
        assert_eq!(line.as_deref(), Some("slow: WM_TIMER 0x1 to RexplorerMain took 51 ms"));
    }
}
