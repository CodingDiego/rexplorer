//! Asynchronous thumbnail/icon loading: a worker thread renders bitmaps and posts them to the window.
#![allow(dead_code)]

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, TryRecvError, channel};
use std::thread::{self, JoinHandle};

use rexplorer_core::archive::Entry;
use rexplorer_core::com::{ComInit, Pidl};
use rexplorer_core::images;
use crate::mailbox::Mailbox;
use crate::{archive_view, views};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{DeleteObject, HBITMAP, HGDIOBJ};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_NULL};

struct Request {
    gen_: u64,
    id: usize,
    abs: Pidl,
    size: i32,
    icon_only: bool,
    /// An archive member to extract and thumbnail; `abs` (its type's icon) when that fails.
    member: Option<(PathBuf, Entry)>,
}

/// `window` before the first scroll: every item.
const ALL: u64 = u32::MAX as u64;

pub struct Thumbs {
    tx: Option<Sender<Request>>,
    gen_: Arc<AtomicU64>,
    /// Items worth rendering, `start << 32 | end`: the worker skips requests outside it.
    window: Arc<AtomicU64>,
    /// The worker's thread id once its message queue exists (0 before), for the wake-up posts.
    thread: Arc<AtomicU32>,
    worker: Option<JoinHandle<()>>,
}

/// A finished bitmap; the receiver owns `bitmap` (already fitted to the request's size) and must DeleteObject it.
pub struct Done {
    pub id: usize,
    pub bitmap: HBITMAP,
}

/// The finished bitmaps, one per `msg` the worker posts (the message carries no handle).
pub(crate) static DONE: Mailbox<Done> = Mailbox::new();

/// Deletes the bitmaps still queued for `hwnd`, which is being destroyed.
pub(crate) fn forget(hwnd: HWND) {
    while let Some(d) = DONE.take(hwnd) {
        let _ = unsafe { DeleteObject(HGDIOBJ(d.bitmap.0)) };
    }
}

/// Spawns the worker; each finished bitmap is queued in `DONE` for `hwnd`, which gets `msg`.
pub fn start(hwnd: HWND, msg: u32) -> Thumbs {
    let (tx, rx) = channel::<Request>();
    let gen_ = Arc::new(AtomicU64::new(0));
    let latest = gen_.clone();
    let window = Arc::new(AtomicU64::new(ALL));
    let win = window.clone();
    let thread_id = Arc::new(AtomicU32::new(0));
    let tid = thread_id.clone();
    let hwnd = hwnd.0 as isize;
    let worker = thread::spawn(move || {
        // An STA: thumbnails of a library's files asked from an MTA crash the process inside thumbcache's own worker
        // (an access violation in Windows.Storage.Search); from an STA they work.
        let _com = ComInit::sta();
        // The queue exists before the id is published, so the first wake-up post cannot fail.
        let mut m = MSG::default();
        let _ = unsafe { PeekMessageW(&mut m, None, 0, 0, PM_NOREMOVE) };
        tid.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
        // An STA must pump between requests: COM calls into it and broadcasts sent to it wait on its messages. The
        // sender posts a wake-up after each request and at shutdown; the requests themselves stay in the channel.
        loop {
            let r = match rx.try_recv() {
                Ok(r) => r,
                Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {
                    if unsafe { GetMessageW(&mut m, None, 0, 0) }.0 <= 0 {
                        break;
                    }
                    unsafe { DispatchMessageW(&m) };
                    continue;
                }
            };
            // Stale, or scrolled out of the window while queued.
            if r.gen_ < latest.load(Ordering::SeqCst) || !in_window(&win, r.id & 0xFFFF_FFFF) {
                continue;
            }
            let extracted = r.member.as_ref().and_then(|(a, e)| archive_view::extract_into(a, e, &archive_view::show_dir(a)).ok()).and_then(|p| archive_view::path_pidl(&p));
            let bmp = match &extracted {
                Some(abs) => images::thumbnail(abs, r.size).or_else(|_| images::icon(&r.abs, r.size)),
                None if r.icon_only => images::icon(&r.abs, r.size),
                None => images::thumbnail(&r.abs, r.size),
            };
            let Ok(bmp) = bmp else { continue };
            // Scaled here: the UI thread only adds the image to its list.
            let fitted = unsafe { views::fit(bmp, r.size) };
            let _ = unsafe { DeleteObject(HGDIOBJ(bmp.0)) };
            let Some(bmp) = fitted else { continue };
            // The shell's overlay (shortcut arrow, sync state) goes on the tile as it does on the list's icons.
            if let Some(o) = rexplorer_core::icons::overlay_bitmap(&r.abs) {
                unsafe {
                    views::blend_over(bmp, r.size, o);
                    let _ = DeleteObject(HGDIOBJ(o.0));
                }
            }
            // Dropping `Thumbs` moves the generation on before the window's `forget`: a bitmap queued after it comes back.
            if let Err(d) = DONE.post_while(hwnd, msg, Done { id: r.id, bitmap: bmp }, || latest.load(Ordering::SeqCst) == r.gen_) {
                // Window is gone or the listing changed: nobody will own the bitmap.
                let _ = unsafe { DeleteObject(HGDIOBJ(d.bitmap.0)) };
            }
        }
    });
    Thumbs { tx: Some(tx), gen_, window, thread: thread_id, worker: Some(worker) }
}

impl Thumbs {
    pub fn request(&self, id: usize, abs: Pidl, size: i32, icon_only: bool) {
        self.send(self.generation(), id, abs, size, icon_only, None);
    }

    /// Archive member `e` of `archive`: extracted under `archive_view::show_dir` and thumbnailed, else `abs`'s icon.
    pub fn request_member(&self, id: usize, abs: Pidl, size: i32, archive: PathBuf, e: Entry) {
        self.send(self.generation(), id, abs, size, true, Some((archive, e)));
    }

    #[cfg(test)]
    pub fn request_with_generation(&self, gen_: u64, id: usize, abs: Pidl, size: i32, icon_only: bool) {
        self.send(gen_, id, abs, size, icon_only, None);
    }

    fn send(&self, gen_: u64, id: usize, abs: Pidl, size: i32, icon_only: bool, member: Option<(PathBuf, Entry)>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Request { gen_, id, abs, size, icon_only, member });
            self.wake();
        }
    }

    /// A request before the worker published its id needs no wake-up: it reads the channel before it first waits.
    fn wake(&self) {
        let tid = self.thread.load(Ordering::SeqCst);
        if tid != 0 {
            let _ = unsafe { PostThreadMessageW(tid, WM_NULL, WPARAM(0), LPARAM(0)) };
        }
    }

    /// Navigation happened: queued requests from older generations are skipped, and every item is in the window again.
    pub fn invalidate(&self) {
        self.gen_.fetch_add(1, Ordering::SeqCst);
        self.window.store(ALL, Ordering::SeqCst);
    }

    /// The items on screen plus a lookahead: queued requests for others are skipped.
    pub fn set_window(&self, items: Range<usize>) {
        self.window.store((items.start as u64) << 32 | items.end as u64, Ordering::SeqCst);
    }

    pub fn generation(&self) -> u64 {
        self.gen_.load(Ordering::SeqCst)
    }

    /// Whether a request for item `i` would be rendered now rather than skipped.
    pub fn in_window(&self, i: usize) -> bool {
        in_window(&self.window, i)
    }
}

fn in_window(window: &AtomicU64, i: usize) -> bool {
    let w = window.load(Ordering::SeqCst);
    (w >> 32..w & ALL).contains(&(i as u64))
}

impl Drop for Thumbs {
    fn drop(&mut self) {
        self.gen_.fetch_add(1, Ordering::SeqCst);
        self.tx = None;
        // The worker sees the channel closed once it wakes.
        self.wake();
        // Not joined: the worker may be inside a slow shell call and must not stall the UI thread.
        self.worker = None;
    }
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

    /// The payload a MSG_ID message announced, which carries no handle itself.
    fn taken(m: &MSG) -> Done {
        assert_eq!((m.wParam.0, m.lParam.0), (0, 0), "no handle in the message");
        DONE.take(m.hwnd).expect("a payload per message")
    }

    #[test]
    fn the_worker_pumps_while_it_waits() {
        // A message sent to the worker's apartment window is answered while no request is queued: blocked in a
        // plain channel read, the send would time out.
        let _com = ComInit::sta();
        let hwnd = unsafe {
            CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 80, 60, None, None, None, None)
        }
        .unwrap();
        let t = start(hwnd, MSG_ID);
        let end = Instant::now() + Duration::from_secs(5);
        while t.thread.load(Ordering::SeqCst) == 0 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
        let tid = t.thread.load(Ordering::SeqCst);
        // COM makes the apartment's window on its first cross-apartment work: a thumbnail does that.
        t.request(1, temp_pidl(), 32, false);
        let m = wait_msg(5000).expect("a first thumbnail");
        let _ = unsafe { DeleteObject(HGDIOBJ(taken(&m).bitmap.0)) };
        let mut ole = None;
        let mut after = None;
        while let Ok(h) = unsafe { FindWindowExW(Some(HWND_MESSAGE), after, w!("OleMainThreadWndClass"), PCWSTR::null()) } {
            if unsafe { GetWindowThreadProcessId(h, None) } == tid {
                ole = Some(h);
                break;
            }
            after = Some(h);
        }
        let ole = ole.expect("the worker's apartment window");
        std::thread::sleep(Duration::from_millis(200));
        let mut out = 0usize;
        let answered = unsafe { SendMessageTimeoutW(ole, WM_NULL, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, 2000, Some(&mut out)) };
        assert_ne!(answered.0, 0, "the idle worker answers a sent message");
        // It still serves requests afterwards, and stops when dropped.
        t.request(1, temp_pidl(), 32, true);
        let m = wait_msg(5000).expect("a request after the pump");
        let _ = unsafe { DeleteObject(HGDIOBJ(taken(&m).bitmap.0)) };
        drop(t);
        let _ = unsafe { DestroyWindow(hwnd) };
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
        t.request(7, temp_pidl(), 64, false);
        let m = wait_msg(5000).expect("no thumbnail within 5s");
        let d = taken(&m);
        assert_eq!(d.id, 7);
        let mut bm = BITMAP::default();
        let n = unsafe { GetObjectW(HGDIOBJ(d.bitmap.0), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) };
        assert!(n > 0 && (bm.bmWidth, bm.bmHeight.abs()) == (64, 64), "fitted to the request's size on the worker");
        let _ = unsafe { DeleteObject(HGDIOBJ(d.bitmap.0)) };

        let old = t.generation();
        t.invalidate();
        assert_eq!(t.generation(), old + 1);
        t.request_with_generation(old, 8, temp_pidl(), 64, false);
        assert!(wait_msg(1000).is_none(), "stale request was delivered");
        // A current-generation request still works after invalidate.
        t.request(9, temp_pidl(), 64, true);
        let m = wait_msg(5000).expect("no icon within 5s");
        let _ = unsafe { DeleteObject(HGDIOBJ(taken(&m).bitmap.0)) };

        drop(t);
        let _ = unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    fn skips_requests_outside_the_window_until_invalidate() {
        let _com = ComInit::sta();
        let hwnd = unsafe {
            CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 800, 600, None, None, None, None)
        }
        .unwrap();
        let t = start(hwnd, MSG_ID);
        t.set_window(0..5);
        assert!(t.in_window(4) && !t.in_window(5) && !t.in_window(9), "what the UI asks before requesting");
        t.request(9, temp_pidl(), 64, true);
        t.request(2, temp_pidl(), 64, true);
        let m = wait_msg(5000).expect("the request inside the window is delivered");
        let d = taken(&m);
        assert_eq!(d.id, 2);
        let _ = unsafe { DeleteObject(HGDIOBJ(d.bitmap.0)) };
        assert!(wait_msg(1000).is_none(), "the request outside the window was rendered");
        // The listing token in the high bits does not matter: only the item index is compared.
        t.request((3 << 32) | 4, temp_pidl(), 64, true);
        let m = wait_msg(5000).expect("item 4 is inside 0..5");
        let _ = unsafe { DeleteObject(HGDIOBJ(taken(&m).bitmap.0)) };
        t.invalidate();
        assert!(t.in_window(9));
        t.request(9, temp_pidl(), 64, true);
        let m = wait_msg(5000).expect("invalidate opens the window again");
        let d = taken(&m);
        assert_eq!(d.id, 9);
        let _ = unsafe { DeleteObject(HGDIOBJ(d.bitmap.0)) };
        drop(t);
        let _ = unsafe { DestroyWindow(hwnd) };
    }
}
