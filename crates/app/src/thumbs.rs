//! Asynchronous thumbnail/icon loading: a worker thread renders bitmaps and posts them to the window.
#![allow(dead_code)]

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::thread::{self, JoinHandle};

use rexplorer_core::com::{ComInit, Pidl};
use rexplorer_core::images;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{DeleteObject, HBITMAP, HGDIOBJ};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

struct Request {
    gen_: u64,
    id: usize,
    abs: Pidl,
    size: i32,
}

pub struct Thumbs {
    tx: Option<Sender<Request>>,
    gen_: Arc<AtomicU64>,
    worker: Option<JoinHandle<()>>,
}

/// Decoded completion message; the receiver owns `bitmap` and must DeleteObject it.
pub struct Done {
    pub id: usize,
    pub bitmap: HBITMAP,
}

/// Spawns the worker; each finished bitmap is posted to `hwnd` as `msg` (wparam = id, lparam = HBITMAP).
pub fn start(hwnd: HWND, msg: u32) -> Thumbs {
    let (tx, rx) = channel::<Request>();
    let gen_ = Arc::new(AtomicU64::new(0));
    let latest = gen_.clone();
    let hwnd = hwnd.0 as isize;
    let worker = thread::spawn(move || {
        let _com = ComInit::mta();
        // recv fails once the sender is dropped.
        while let Ok(r) = rx.recv() {
            if r.gen_ < latest.load(Ordering::SeqCst) {
                continue;
            }
            let bmp = images::thumbnail(&r.abs, r.size);
            let Ok(bmp) = bmp else { continue };
            let posted = unsafe {
                PostMessageW(Some(HWND(hwnd as *mut _)), msg, WPARAM(r.id), LPARAM(bmp.0 as isize))
            };
            if posted.is_err() {
                // Window is gone: nobody will own the bitmap.
                let _ = unsafe { DeleteObject(HGDIOBJ(bmp.0)) };
            }
        }
    });
    Thumbs { tx: Some(tx), gen_, worker: Some(worker) }
}

impl Thumbs {
    pub fn request(&self, id: usize, abs: Pidl, size: i32) {
        self.send(self.generation(), id, abs, size);
    }

    #[cfg(test)]
    pub fn request_with_generation(&self, gen_: u64, id: usize, abs: Pidl, size: i32) {
        self.send(gen_, id, abs, size);
    }

    fn send(&self, gen_: u64, id: usize, abs: Pidl, size: i32) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Request { gen_, id, abs, size });
        }
    }

    /// Navigation happened: queued requests from older generations are skipped.
    pub fn invalidate(&self) {
        self.gen_.fetch_add(1, Ordering::SeqCst);
    }

    pub fn generation(&self) -> u64 {
        self.gen_.load(Ordering::SeqCst)
    }
}

impl Drop for Thumbs {
    fn drop(&mut self) {
        self.tx = None;
        // Not joined: the worker may be inside a slow shell call and must not stall the UI thread.
        self.worker = None;
    }
}

pub fn done(wparam: WPARAM, lparam: LPARAM) -> Done {
    Done { id: wparam.0, bitmap: HBITMAP(lparam.0 as *mut _) }
}

/// Items the parent should request: `top - lookahead .. top + per_page + lookahead`, clamped to `0..count`.
pub fn visible_range(top: i32, per_page: i32, count: usize, lookahead: i32) -> Range<usize> {
    let end = ((top + per_page + lookahead).max(0) as usize).min(count);
    let start = ((top - lookahead).max(0) as usize).min(end);
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use windows::Win32::Graphics::Gdi::{BITMAP, GetObjectW};
    use windows::Win32::UI::Shell::SHParseDisplayName;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::{HSTRING, PCWSTR, w};

    const MSG_ID: u32 = WM_APP + 77;

    #[test]
    fn visible_range_table() {
        assert_eq!(visible_range(0, 10, 100, 5), 0..15);
        assert_eq!(visible_range(50, 10, 100, 5), 45..65);
        assert_eq!(visible_range(95, 10, 100, 5), 90..100);
        assert_eq!(visible_range(0, 10, 3, 5), 0..3);
        assert_eq!(visible_range(0, 10, 0, 5), 0..0);
        assert_eq!(visible_range(200, 10, 100, 5), 100..100);
        assert_eq!(visible_range(-20, 10, 100, 5), 0..0);
    }

    fn temp_pidl() -> Pidl {
        let mut p = std::ptr::null_mut();
        unsafe { SHParseDisplayName(&HSTRING::from(std::env::temp_dir().as_os_str()), None, &mut p, 0, None) }.unwrap();
        unsafe { Pidl::from_raw(p) }
    }

    /// Waits up to `ms` for MSG_ID; None on timeout.
    fn wait_msg(ms: u64) -> Option<MSG> {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            let mut m = MSG::default();
            if unsafe { PeekMessageW(&mut m, None, MSG_ID, MSG_ID, PM_REMOVE) }.as_bool() {
                return Some(m);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    #[test]
    fn delivers_bitmap_and_drops_stale_generation() {
        let _com = ComInit::sta();
        let hwnd = unsafe {
            CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 800, 600, None, None, None, None)
        }
        .unwrap();
        let t = start(hwnd, MSG_ID);
        t.request(7, temp_pidl(), 64);
        let m = wait_msg(5000).expect("no thumbnail within 5s");
        let d = done(m.wParam, m.lParam);
        assert_eq!(d.id, 7);
        let mut bm = BITMAP::default();
        let n = unsafe { GetObjectW(HGDIOBJ(d.bitmap.0), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) };
        assert!(n > 0 && bm.bmWidth > 0);
        let _ = unsafe { DeleteObject(HGDIOBJ(d.bitmap.0)) };

        let old = t.generation();
        t.invalidate();
        assert_eq!(t.generation(), old + 1);
        t.request_with_generation(old, 8, temp_pidl(), 64);
        assert!(wait_msg(1000).is_none(), "stale request was delivered");
        // A current-generation request still works after invalidate.
        t.request(9, temp_pidl(), 64);
        let m = wait_msg(5000).expect("no icon within 5s");
        let _ = unsafe { DeleteObject(HGDIOBJ(done(m.wParam, m.lParam).bitmap.0)) };

        drop(t);
        let _ = unsafe { DestroyWindow(hwnd) };
    }
}
