//! Payloads that workers hand to a window. A message carries no pointer, since any program on the desktop can post
//! one: the payload waits here and the message (lParam 0) only says that one did. A forged message can at most deliver
//! early what a worker already queued.

use std::sync::Mutex;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// The last entry number given, and the entries: (window, number, payload).
type Queue<T> = (u64, Vec<(isize, u64, T)>);

/// One queue per message, keyed by the window it was posted to, oldest first.
pub(crate) struct Mailbox<T>(Mutex<Queue<T>>);

// SAFETY: a payload is moved to the UI thread, never shared: what the old Box-in-lParam did, minus the pointer. The
// payloads are pidls, strings and shell objects made for the UI thread; nothing that counts references without atomics.
unsafe impl<T> Sync for Mailbox<T> {}

impl<T> Mailbox<T> {
    pub(crate) const fn new() -> Self {
        Mailbox(Mutex::new((0, Vec::new())))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Queue<T>> {
        // A panic elsewhere while holding the lock leaves the queue usable.
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queues `v` for `hwnd` and posts it `msg`; gives `v` back, with nothing left queued, when the post fails (the
    /// window is gone or its queue full).
    pub(crate) fn post(&self, hwnd: isize, msg: u32, v: T) -> Result<(), T> {
        self.post_while(hwnd, msg, v, || true)
    }

    /// `post`, also giving `v` back when `wanted` is false once it is queued. The receiver makes `wanted` false before
    /// its last drain (`forget` or a `take` loop), so a payload queued after that drain is not left behind.
    pub(crate) fn post_while(&self, hwnd: isize, msg: u32, v: T, wanted: impl Fn() -> bool) -> Result<(), T> {
        // Pushed before the post, and posted outside the lock: the message never arrives before its payload, and the
        // UI thread never waits in `take` on a worker that runs at background priority.
        let n = {
            let mut q = self.lock();
            q.0 += 1;
            let n = q.0;
            q.1.push((hwnd, n, v));
            n
        };
        if unsafe { PostMessageW(Some(HWND(hwnd as *mut _)), msg, WPARAM(0), LPARAM(0)) }.is_err() || !wanted() {
            let mut q = self.lock();
            // Not there any more: a forged message already delivered it.
            return match q.1.iter().position(|e| e.1 == n) {
                Some(i) => Err(q.1.remove(i).2),
                None => Ok(()),
            };
        }
        Ok(())
    }

    /// The oldest payload queued for `hwnd`: one per message.
    pub(crate) fn take(&self, hwnd: HWND) -> Option<T> {
        let mut q = self.lock();
        let i = q.1.iter().position(|e| e.0 == hwnd.0 as isize)?;
        Some(q.1.remove(i).2)
    }

    /// Drops what is still queued for `hwnd`, which is being destroyed.
    pub(crate) fn forget(&self, hwnd: HWND) {
        self.lock().1.retain(|e| e.0 != hwnd.0 as isize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, MSG, PM_REMOVE, PeekMessageW, WINDOW_EX_STYLE, WM_APP, WS_OVERLAPPEDWINDOW,
    };
    use windows::core::{PCWSTR, w};

    unsafe fn window() -> HWND {
        CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 10, 10, None, None, None, None).unwrap()
    }

    #[test]
    fn payloads_arrive_one_per_message_in_order() {
        static Q: Mailbox<String> = Mailbox::new();
        unsafe {
            let h = window();
            for s in ["a", "b", "c"] {
                assert!(Q.post(h.0 as isize, WM_APP + 1, s.to_string()).is_ok());
            }
            let mut got = Vec::new();
            let mut m = MSG::default();
            while PeekMessageW(&mut m, Some(h), WM_APP + 1, WM_APP + 1, PM_REMOVE).as_bool() {
                assert_eq!(m.lParam.0, 0, "no pointer in the message");
                got.extend(Q.take(h));
            }
            assert_eq!(got, ["a", "b", "c"]);
            let _ = DestroyWindow(h);
        }
    }

    #[test]
    fn a_bare_message_on_an_empty_queue_does_nothing() {
        static Q: Mailbox<Box<u32>> = Mailbox::new();
        unsafe {
            let h = window();
            assert!(Q.take(h).is_none());
            let _ = DestroyWindow(h);
        }
    }

    #[test]
    fn another_windows_payload_is_not_delivered_and_a_gone_window_keeps_none() {
        static Q: Mailbox<u32> = Mailbox::new();
        unsafe {
            let (a, b) = (window(), window());
            assert!(Q.post(a.0 as isize, WM_APP + 2, 1).is_ok());
            assert!(Q.post(b.0 as isize, WM_APP + 2, 2).is_ok());
            assert_eq!(Q.take(b), Some(2));
            assert_eq!(Q.take(b), None, "a's payload stays a's");
            Q.forget(a);
            assert_eq!(Q.take(a), None);
            let _ = DestroyWindow(a);
            let _ = DestroyWindow(b);
            // A post to a window that is gone fails and leaves nothing queued.
            assert_eq!(Q.post(a.0 as isize, WM_APP + 2, 3), Err(3), "the payload comes back");
            assert!(Q.lock().1.is_empty());
        }
    }

    #[test]
    fn a_payload_no_longer_wanted_is_not_left_queued() {
        static Q: Mailbox<u32> = Mailbox::new();
        unsafe {
            let h = window();
            // The receiver gave up (and drained) while the worker was producing: it gets its payload back.
            assert_eq!(Q.post_while(h.0 as isize, WM_APP + 3, 1, || false), Err(1));
            assert!(Q.lock().1.is_empty());
            assert_eq!(Q.post_while(h.0 as isize, WM_APP + 3, 2, || true), Ok(()));
            assert_eq!(Q.take(h), Some(2));
            let _ = DestroyWindow(h);
        }
    }
}
