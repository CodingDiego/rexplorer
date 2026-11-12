//! The preview pane (class "RexPreview"): text in a read-only Edit, preview handlers hosted in place,
//! everything else as a shell thumbnail with a name/size caption.
#![allow(dead_code)]
use std::ffi::c_void;
use std::io::Read;
use rexplorer_core::i18n::tr;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::SystemTime;

use rexplorer_core::access;
use rexplorer_core::com::{ComInit, Pidl};
use rexplorer_core::preview::{self, Kind, Preview};
use rexplorer_core::{images, settings::Theme};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{ROLE_SYSTEM_PANE, ROLE_SYSTEM_STATICTEXT};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::UI::Shell::{IShellItem2, SHCreateItemFromParsingName};
use windows::core::{GUID, HSTRING, PCWSTR, Result, w};

/// Reserved for the parent's notifications; the pane currently sends none.
pub const PVN_FIRST: u32 = 0x9000;
/// Posted to the pane by a worker that filled the cache for a file (lparam = Box<PathBuf> of it); the pane shows the
/// subject again from the cache if it is still that file.
const PV_READY: u32 = WM_APP + 0x51;

/// Posted to the pane by the Dimensions worker (lparam = Box<(PathBuf, String)>: the file and its "2 x 3" text).
const PV_DIMS: u32 = WM_APP + 0x52;

// The Edit word-wraps all of its text on every set and resize, so keep what goes in small.
const MAX_TEXT: u64 = 64 * 1024;

/// An owned GDI bitmap, shared between the cache and the pane; the last holder deletes it.
struct Bmp(HBITMAP);
unsafe impl Send for Bmp {}
unsafe impl Sync for Bmp {}
impl Drop for Bmp {
    fn drop(&mut self) {
        let _ = unsafe { DeleteObject(self.0.into()) };
    }
}

const CACHE_MAX: usize = 16;

type Stamp = (SystemTime, u64);
type Thumb = (Arc<Bmp>, i32, i32);

/// What was prepared for one file version; `thumb` remembers the size it was requested at.
struct Entry {
    path: PathBuf,
    stamp: Stamp,
    text: Option<String>,
    thumb: Option<(i32, Thumb)>,
}

/// Oldest first. Written by the UI thread and the hover prefetch thread.
static CACHE: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

fn stamp(p: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

fn lookup<T>(path: &Path, st: Stamp, pick: impl FnOnce(&Entry) -> Option<T>) -> Option<T> {
    let c = CACHE.lock().ok()?;
    c.iter().find(|e| e.path == path && e.stamp == st).and_then(pick)
}

fn store(path: &Path, st: Stamp, set: impl FnOnce(&mut Entry)) {
    let Ok(mut c) = CACHE.lock() else { return };
    c.retain(|e| e.path != path || e.stamp == st);
    let i = c.iter().position(|e| e.path == path).unwrap_or_else(|| {
        if c.len() >= CACHE_MAX {
            c.remove(0);
        }
        c.push(Entry { path: path.to_path_buf(), stamp: st, text: None, thumb: None });
        c.len() - 1
    });
    set(&mut c[i]);
}

/// The file's text (first 64 KB, decoded for the Edit), from the cache when its mtime and size still match.
fn cached_text(path: &Path) -> Option<String> {
    let st = stamp(path);
    if let Some(t) = st.and_then(|st| lookup(path, st, |e| e.text.clone())) {
        return Some(t);
    }
    let mut buf = Vec::new();
    std::fs::File::open(path).ok()?.take(MAX_TEXT).read_to_end(&mut buf).ok()?;
    let t = decode_text(&buf);
    if let Some(st) = st {
        store(path, st, |e| e.text = Some(t.clone()));
    }
    Some(t)
}

/// The premultiplied shell thumbnail; `key` is the file to cache it under (none for folders and non-filesystem items).
fn cached_thumb(key: Option<&Path>, abs: &Pidl, size: i32) -> Option<Thumb> {
    let key = key.and_then(|p| Some((p, stamp(p)?)));
    if let Some((p, st)) = key
        && let Some(t) = lookup(p, st, |e| e.thumb.as_ref().filter(|(sz, _)| *sz == size).map(|(_, t)| t.clone())) {
            return Some(t);
        }
    let b = images::thumbnail(abs, size).ok()?;
    // Straight alpha from the shell; AlphaBlend expects premultiplied or the edges halo.
    images::premultiply(b);
    let mut bm = BITMAP::default();
    unsafe { GetObjectW(b.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void)) };
    let t = (Arc::new(Bmp(b)), bm.bmWidth, bm.bmHeight);
    if let Some((p, st)) = key {
        store(p, st, |e| e.thumb = Some((size, t.clone())));
    }
    Some(t)
}

/// Whether files with a registered shell preview handler use it; off, they get the thumbnail and caption.
pub(crate) static HANDLERS: AtomicBool = AtomicBool::new(true);

fn kind_of(path: &Path) -> Kind {
    match preview::kind(path.extension().and_then(|e| e.to_str()).unwrap_or("")) {
        Kind::Handler if !HANDLERS.load(Ordering::Relaxed) => Kind::None,
        k => k,
    }
}

/// Fills the cache for a hovered file on a worker thread; only text and images: shell-handler previews need the UI
/// thread, and PDFs, Office files and the like load only once selected.
pub fn prefetch(path: PathBuf, abs: Pidl, size: i32) {
    let kind = kind_of(&path);
    if matches!(kind, Kind::Text | Kind::Image) {
        fetch(path, abs, size, kind, 0);
    }
}

/// Reads the file (text) or asks the shell (thumbnail) off the UI thread, so a stalled network file never hangs the
/// window; `notify` is the pane to post PV_READY (with the file's path) to, or 0 for a speculative hover fetch.
fn fetch(path: PathBuf, abs: Pidl, size: i32, kind: Kind, notify: isize) {
    std::thread::spawn(move || {
        if notify == 0 {
            crate::ui::background_priority();
        }
        let _com = ComInit::mta();
        if kind == Kind::Text {
            let _ = cached_text(&path);
        } else {
            let _ = cached_thumb(Some(&path), &abs, size);
        }
        if notify != 0 {
            // shortcut: one still queued when the pane is destroyed leaks its path (only at exit today); drain
            // PV_READY in WM_DESTROY if panes ever close earlier.
            let p = Box::into_raw(Box::new(path));
            if unsafe { PostMessageW(Some(HWND(notify as *mut c_void)), PV_READY, WPARAM(0), LPARAM(p as isize)) }.is_err() {
                drop(unsafe { Box::from_raw(p) });
            }
        }
    });
}

/// The thumbnail edge `present` asks the shell for at the pane's current size.
pub fn request_size(hwnd: HWND) -> i32 {
    unsafe {
        let rc = content_rect(hwnd);
        thumb_size(rc.right - rc.left, rc.bottom - rc.top, caption_height(hwnd))
    }
}

pub struct Subject {
    pub path: Option<PathBuf>,
    pub abs_pidl: Pidl,
    pub name: String,
    pub is_folder: bool,
    pub size: Option<u64>,
    pub type_text: String,
}

/// What the pane shows in details mode besides the picture: a heading, a line under it and label/value rows.
pub struct DetailsText {
    pub heading: String,
    pub sub: String,
    pub rows: Vec<(&'static str, String)>,
}

struct State {
    bg: u32,
    /// The frame color around the card, for its painted corners.
    frame: u32,
    text: u32,
    secondary: u32,
    brush: HBRUSH,
    ui_font: HFONT,
    edit: HWND,
    bitmap: Option<(Arc<Bmp>, i32, i32)>,
    name: String,
    detail: String,
    preview: Option<Preview>,
    /// What is shown, kept for PV_READY.
    subject: Option<Subject>,
    /// The shown file's mtime and size when its handler opened or its text was set: both are kept while they still match.
    stamp: Option<Stamp>,
    /// Details mode (Alt+Shift+P): drawn instead of the caption, never with the Edit or a handler.
    details: Option<DetailsText>,
    acc: Option<access::Server>,
    /// The lines last announced to screen readers (see `announce`).
    said: Vec<String>,
}

/// Formats a byte count like "1.5 MB" (binary units, one decimal above 1 KB).
fn format_size(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{} {}", format!("{v:.1}").replace('.', rexplorer_core::i18n::tr("num.decimal")), U[i])
}

/// UTF-8 lossy, BOM stripped, every line break normalised to CRLF for the Edit control.
fn decode_text(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let s = s.strip_prefix('\u{feff}').unwrap_or(&s);
    s.replace("\r\n", "\n").replace('\r', "\n").replace('\n', "\r\n")
}

/// Square thumbnail request: fits the width and the height left over by the caption.
fn thumb_size(w: i32, h: i32, caption: i32) -> i32 {
    w.min(h - caption).max(1)
}

fn detail_text(size: Option<u64>, type_text: &str) -> String {
    match size {
        Some(s) => format!("{} \u{b7} {}", format_size(s), type_text),
        None => type_text.to_string(),
    }
}

fn scale(hwnd: HWND, px: i32) -> i32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    px * if dpi == 0 { 96 } else { dpi as i32 } / 96
}

/// `px` at the window's DPI grown by the Windows text size: for the lines that hold text.
fn text_scale(hwnd: HWND, px: i32) -> i32 {
    scale(hwnd, px) * crate::theme::text_scale() as i32 / 100
}

fn caption_height(hwnd: HWND) -> i32 {
    text_scale(hwnd, 44)
}

unsafe fn state(hwnd: HWND) -> *mut State {
    GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State
}

unsafe fn client(hwnd: HWND) -> RECT {
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    rc
}

pub fn create(parent: HWND, id: usize) -> Result<HWND> {
    static REG: Once = Once::new();
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
        REG.call_once(|| {
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wndproc),
                hInstance: hinst,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: w!("RexPreview"),
                ..Default::default()
            };
            RegisterClassW(&wc);
        });
        let st = Box::into_raw(Box::new(State {
            bg: 0x00FF_FFFF,
            frame: 0x00F5_F5F7,
            text: 0,
            secondary: 0x0080_8080,
            brush: CreateSolidBrush(COLORREF(0x00FF_FFFF)),
            ui_font: HFONT::default(),
            edit: HWND::default(),
            bitmap: None,
            name: String::new(),
            detail: String::new(),
            preview: None,
            subject: None,
            stamp: None,
            details: None,
            acc: None,
            said: Vec::new(),
        }));
        let r = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("RexPreview"),
            PCWSTR::null(),
            WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            0,
            0,
            0,
            0,
            Some(parent),
            Some(HMENU(id as *mut c_void)),
            Some(hinst),
            Some(st as *const c_void),
        );
        // shortcut: if creation fails before WM_NCCREATE the State box leaks; not realistic.
        r
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lparam.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let s = state(hwnd);
    if s.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    match msg {
        WM_CREATE => {
            let hinst: HINSTANCE = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
            let style = WS_CHILD
                | WS_VSCROLL
                | WINDOW_STYLE((ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32);
            match CreateWindowExW(
                WINDOW_EX_STYLE(0), w!("EDIT"), PCWSTR::null(), style, 0, 0, 0, 0,
                Some(hwnd), None, Some(hinst), None,
            ) {
                Ok(e) => {
                    // The text preview's read-only box has no label (#21).
                    let _ = rexplorer_core::access::set_name(e, tr("preview.pane_name"));
                    (*s).edit = e;
                }
                Err(_) => return LRESULT(-1),
            }
            LRESULT(0)
        }
        WM_SIZE => {
            relayout(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_GETOBJECT if access::is_client(lparam) => (*s).acc.get_or_insert_with(|| access::Server::new(hwnd)).answer(wparam),
        access::WM_ACC_ITEMS => {
            acc_tree(hwnd, &*s, &mut *(lparam.0 as *mut access::Tree));
            LRESULT(1)
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => {
            let hdc = HDC(wparam.0 as *mut c_void);
            SetTextColor(hdc, COLORREF((*s).text));
            SetBkColor(hdc, COLORREF((*s).bg));
            LRESULT((*s).brush.0 as isize)
        }
        WM_SETFONT => {
            (*s).ui_font = HFONT(wparam.0 as *mut c_void);
            let _ = InvalidateRect(Some(hwnd), None, false);
            LRESULT(0)
        }
        WM_PAINT => {
            paint(hwnd, s);
            LRESULT(0)
        }
        PV_READY => {
            let ready = Box::from_raw(lparam.0 as *mut PathBuf);
            // Cache only: a file that cannot be cached (vanished) would otherwise spawn a worker per PV_READY.
            // A handler already showing stays (a stale PV_READY must not open a second one), and so does a
            // completion for a file no longer shown (it would re-run a folder thumbnail on the UI thread).
            if let Some(sub) = (*s).subject.take() {
                if (*s).preview.is_none() && sub.path.as_ref() == Some(&*ready) {
                    present(hwnd, s, &sub, false);
                    announce(hwnd, s);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                (*s).subject = Some(sub);
            }
            LRESULT(0)
        }
        PV_DIMS => {
            let (path, text) = *Box::from_raw(lparam.0 as *mut (PathBuf, String));
            let shown = (*s).subject.as_ref().and_then(|sub| sub.path.as_ref()) == Some(&path);
            if let (Some(d), true) = (&mut (*s).details, shown)
                && !d.rows.iter().any(|r| r.0 == tr("preview.dimensions")) {
                    d.rows.push((tr("preview.dimensions"), text));
                    announce(hwnd, s);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            LRESULT(0)
        }
        WM_DESTROY => {
            (*s).preview = None;
            (*s).subject = None;
            clear_bitmap(s);
            LRESULT(0)
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            let st = Box::from_raw(s);
            let _ = DeleteObject(st.brush.into());
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn clear_bitmap(s: *mut State) {
    (*s).bitmap = None;
}

/// The client area inside the card's 12 px padding.
unsafe fn content_rect(hwnd: HWND) -> RECT {
    let rc = client(hwnd);
    let p = scale(hwnd, 12);
    RECT { left: p, top: p, right: (rc.right - p).max(p), bottom: (rc.bottom - p).max(p) }
}

unsafe fn relayout(hwnd: HWND) {
    let s = state(hwnd);
    if s.is_null() {
        return;
    }
    let rc = content_rect(hwnd);
    if !(*s).edit.is_invalid() {
        let _ = MoveWindow((*s).edit, rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top, true);
    }
    if let Some(p) = &(*s).preview {
        let _ = p.set_rect(rc);
    }
    let _ = InvalidateRect(Some(hwnd), None, false);
}

unsafe fn paint(hwnd: HWND, s: *mut State) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let rc = client(hwnd);
    let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
    if w > 0 && h > 0 {
        let mem = CreateCompatibleDC(Some(hdc));
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old = SelectObject(mem, bmp.into());
        FillRect(mem, &rc, (*s).brush);

        if (*s).details.is_some() {
            paint_details(hwnd, s, hdc, mem);
        } else if let Some((hb, bw, bh)) = (*s).bitmap.clone() {
            let c = content_rect(hwnd);
            let cap = caption_height(hwnd);
            let area_h = (c.bottom - c.top - cap).max(1);
            // The shell may hand back a larger thumbnail than asked for: shrink it into the card.
            let fit = ((c.right - c.left) as f64 / bw.max(1) as f64).min(area_h as f64 / bh.max(1) as f64).min(1.0);
            let (dw, dh) = (((bw as f64 * fit) as i32).max(1), ((bh as f64 * fit) as i32).max(1));
            let x = c.left + ((c.right - c.left - dw) / 2).max(0);
            let y = c.top + ((area_h - dh) / 2).max(0);
            blend(hdc, mem, &hb, (x, y, dw, dh), (bw, bh));

            let font = if (*s).ui_font.is_invalid() {
                GetStockObject(DEFAULT_GUI_FONT)
            } else {
                (*s).ui_font.into()
            };
            let of = SelectObject(mem, font);
            SetBkMode(mem, TRANSPARENT);
            let half = cap / 2;
            let fmt = DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX;
            let top = c.top + area_h;
            let mut r1 = RECT { left: c.left, top, right: c.right, bottom: top + half };
            let mut r2 = RECT { left: c.left, top: top + half, right: c.right, bottom: top + cap };
            // An empty Vec<u16> hands DrawTextW the dangling pointer 0x2, which it read and faulted on (a Home entry
            // with no detail line crashed the window, 2026-11-11): empty lines are not drawn.
            for (text, r, color) in [(&(*s).name, &mut r1, (*s).text), (&(*s).detail, &mut r2, (*s).secondary)] {
                if !text.is_empty() {
                    SetTextColor(mem, COLORREF(color));
                    let mut t: Vec<u16> = text.encode_utf16().collect();
                    DrawTextW(mem, &mut t, r, fmt);
                }
            }
            SelectObject(mem, of);
        }

        crate::theme::paint_card_corners(hwnd, hwnd, mem, (*s).frame);
        let _ = BitBlt(hdc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
    }
    let _ = EndPaint(hwnd, &ps);
}

unsafe fn blend(hdc: HDC, mem: HDC, hb: &Bmp, (x, y, dw, dh): (i32, i32, i32, i32), (bw, bh): (i32, i32)) {
    let src = CreateCompatibleDC(Some(hdc));
    let prev = SelectObject(src, hb.0.into());
    let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
    let _ = AlphaBlend(mem, x, y, dw, dh, src, 0, 0, bw, bh, bf);
    SelectObject(src, prev);
    let _ = DeleteDC(src);
}

/// Details mode: the picture in a square at the top, then the heading, the line under it and the label/value rows.
/// Colors come from the theme (text, secondary), so a contrast theme needs nothing here.
/// Where the details pane's values start: past the widest label (longer in Spanish, #37), between 2/5 and 3/5 of
/// the width so the values keep room.
fn label_split(left: i32, right: i32, widest: i32, gap: i32) -> i32 {
    let w = right - left;
    (left + widest + gap).clamp(left + w * 2 / 5, left + w * 3 / 5)
}

unsafe fn paint_details(hwnd: HWND, s: *mut State, hdc: HDC, mem: HDC) {
    let Some(d) = &(*s).details else { return };
    let c = content_rect(hwnd);
    let edge = details_edge(c.right - c.left, c.bottom - c.top, scale(hwnd, 256));
    let mut y = c.top;
    if let Some((hb, bw, bh)) = (*s).bitmap.clone() {
        let fit = (edge as f64 / bw.max(1) as f64).min(edge as f64 / bh.max(1) as f64).min(1.0);
        let (dw, dh) = (((bw as f64 * fit) as i32).max(1), ((bh as f64 * fit) as i32).max(1));
        let x = c.left + ((c.right - c.left - dw) / 2).max(0);
        blend(hdc, mem, &hb, (x, y + ((edge - dh) / 2).max(0), dw, dh), (bw, bh));
        y += edge + scale(hwnd, 8);
    }
    let font = if (*s).ui_font.is_invalid() { GetStockObject(DEFAULT_GUI_FONT) } else { (*s).ui_font.into() };
    let of = SelectObject(mem, font);
    SetBkMode(mem, TRANSPARENT);
    let fmt = DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX;
    let line = |y: i32, left: i32, right: i32, h: i32, color: u32, text: &str| {
        // An empty line (no type text, several items) has nothing to draw, and DrawTextW is not called with no characters.
        if text.is_empty() {
            return;
        }
        let mut r = RECT { left, top: y, right, bottom: y + h };
        SetTextColor(mem, COLORREF(color));
        let mut t: Vec<u16> = text.encode_utf16().collect();
        DrawTextW(mem, &mut t, &mut r, fmt);
    };
    let (h_head, h_row, gap) = (text_scale(hwnd, 24), text_scale(hwnd, 20), scale(hwnd, 8));
    line(y, c.left, c.right, h_head, (*s).text, &d.heading);
    y += h_head;
    line(y, c.left, c.right, h_row, (*s).secondary, &d.sub);
    y += h_row + gap;
    let widest = d.rows.iter().map(|(label, _)| {
        let t: Vec<u16> = label.encode_utf16().collect();
        let mut size = windows::Win32::Foundation::SIZE::default();
        let _ = windows::Win32::Graphics::Gdi::GetTextExtentPoint32W(mem, &t, &mut size);
        size.cx
    });
    let split = label_split(c.left, c.right, widest.max().unwrap_or(0), gap);
    for (label, value) in &d.rows {
        line(y, c.left, split, h_row, (*s).secondary, label);
        line(y, split, c.right, h_row, (*s).text, value);
        y += h_row;
    }
    SelectObject(mem, of);
}

pub fn set_theme(hwnd: HWND, t: &Theme) {
    unsafe {
        let s = state(hwnd);
        if s.is_null() {
            return;
        }
        let _ = DeleteObject((*s).brush.into());
        // A card: surface colored, like the list.
        (*s).brush = CreateSolidBrush(COLORREF(t.surface));
        (*s).bg = t.surface;
        (*s).frame = t.background;
        (*s).text = t.text;
        (*s).secondary = t.secondary_text;
        let _ = InvalidateRect(Some(hwnd), None, true);
        let _ = InvalidateRect(Some((*s).edit), None, true);
    }
}

/// Fonts are borrowed, not owned: `ui` for the caption, `mono` for the text Edit.
pub fn set_fonts(hwnd: HWND, ui: HFONT, mono: HFONT) {
    unsafe {
        let s = state(hwnd);
        if s.is_null() {
            return;
        }
        (*s).ui_font = ui;
        SendMessageW((*s).edit, WM_SETFONT, Some(WPARAM(mono.0 as usize)), Some(LPARAM(1)));
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

pub fn show(hwnd: HWND, subject: Option<Subject>) {
    unsafe {
        let s = state(hwnd);
        if s.is_null() {
            return;
        }
        // The same file version is already live in a handler (re-hosting reloads it out of process on the UI thread)
        // or set as text (setting it again re-wraps all of it): keep it.
        let same = match (&subject, &(*s).subject) {
            (Some(new), Some(old)) => {
                (if (*s).preview.is_some() { HANDLERS.load(Ordering::Relaxed) } else { (*s).stamp.is_some() })
                    && (*s).details.is_none()
                    && new.path.is_some()
                    && new.path == old.path
                    && new.path.as_deref().and_then(stamp) == (*s).stamp
            }
            _ => false,
        };
        if same {
            (*s).subject = subject;
            return;
        }
        reset(s);
        if let Some(sub) = subject {
            present(hwnd, s, &sub, true);
            (*s).subject = Some(sub);
        }
        announce(hwnd, s);
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

/// What a screen reader reads: in details mode the heading, the line under it and "label: value" rows, else the
/// caption's name and detail line.
fn spoken(s: &State) -> Vec<String> {
    let lines: Vec<String> = match &s.details {
        Some(d) => [d.heading.clone(), d.sub.clone()].into_iter().chain(d.rows.iter().map(|(l, v)| format!("{l}: {v}"))).collect(),
        None => vec![s.name.clone(), s.detail.clone()],
    };
    lines.into_iter().filter(|t| !t.is_empty()).collect()
}

/// Raises NAMECHANGE for the pane (named by its first line) and for each line that changed since the last call.
unsafe fn announce(hwnd: HWND, s: *mut State) {
    let st = &mut *s;
    let now = spoken(st);
    if now == st.said {
        return;
    }
    for (i, t) in now.iter().enumerate() {
        if st.said.get(i) != Some(t) {
            access::event(EVENT_OBJECT_NAMECHANGE, hwnd, Some(i));
        }
    }
    st.said = now;
    access::event(EVENT_OBJECT_NAMECHANGE, hwnd, None);
}

unsafe fn acc_tree(hwnd: HWND, s: &State, t: &mut access::Tree) {
    let lines = spoken(s);
    t.name = lines.first().cloned().unwrap_or_else(|| tr("preview.pane_name").into());
    t.role = ROLE_SYSTEM_PANE;
    let rect = client(hwnd);
    for name in lines {
        t.items.push(access::Item { name, role: ROLE_SYSTEM_STATICTEXT, state: STATE_SYSTEM_READONLY, rect, ..Default::default() });
    }
}

/// Back to an empty pane: no handler, subject, picture, details or text.
unsafe fn reset(s: *mut State) {
    (*s).preview = None;
    (*s).subject = None;
    (*s).stamp = None;
    (*s).details = None;
    clear_bitmap(s);
    (*s).name.clear();
    (*s).detail.clear();
    let _ = ShowWindow((*s).edit, SW_HIDE);
    let _ = SetWindowTextW((*s).edit, w!(""));
}

/// Details mode: `text` and, for a subject, its picture (from the cache, fetched by a worker); `dims` also reads an
/// image's dimensions on a worker. The Edit and handlers are never used.
pub fn show_details(hwnd: HWND, subject: Option<Subject>, text: DetailsText, dims: bool) {
    unsafe {
        let s = state(hwnd);
        if s.is_null() {
            return;
        }
        reset(s);
        (*s).details = Some(text);
        if let Some(sub) = subject {
            present(hwnd, s, &sub, true);
            if let (true, Some(path)) = (dims, sub.path.clone()) {
                let notify = hwnd.0 as isize;
                std::thread::spawn(move || {
                    crate::ui::background_priority();
                    let _com = ComInit::mta();
                    let Some(text) = image_dimensions(&path) else { return };
                    let b = Box::into_raw(Box::new((path, text)));
                    if PostMessageW(Some(HWND(notify as *mut c_void)), PV_DIMS, WPARAM(0), LPARAM(b as isize)).is_err() {
                        drop(Box::from_raw(b));
                    }
                });
            }
            (*s).subject = Some(sub);
        }
        announce(hwnd, s);
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

/// The shell wraps dates and "2 x 3" in direction marks; they show as boxes in some fonts and break comparisons.
pub fn strip_marks(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}')).collect()
}

/// Reading a cloud placeholder's properties would download it (RECALL_ON_DATA_ACCESS or OFFLINE).
fn dims_allowed(attrs: u32) -> bool {
    attrs & (0x0040_0000 | 0x1000) == 0
}

/// "2 x 3" for a local image, from the shell's property system; call it off the UI thread.
fn image_dimensions(path: &Path) -> Option<String> {
    use std::os::windows::fs::MetadataExt;
    const PKEY_IMAGE_DIMENSIONS: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0x6444048F_4C8B_11D1_8B70_080036B11A03), pid: 13 };
    if !dims_allowed(std::fs::metadata(path).ok()?.file_attributes()) {
        return None;
    }
    unsafe {
        let item: IShellItem2 = SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None).ok()?;
        let text = strip_marks(&rexplorer_core::com::take_cotaskmem_string(item.GetString(&PKEY_IMAGE_DIMENSIONS).ok()?));
        (!text.is_empty()).then_some(text)
    }
}

/// The picture edge in details mode: at most half the height, so the rows keep their room.
fn details_edge(w: i32, h: i32, max: i32) -> i32 {
    w.min(h / 2).min(max).max(1)
}

/// Shows `sub` from the cache; a file not cached yet gets the caption now and, when `spawn`, a worker that posts
/// PV_READY. Folders and virtual items (no cache key) are asked for synchronously.
unsafe fn present(hwnd: HWND, s: *mut State, sub: &Subject, spawn: bool) {
    let kind = match (&sub.path, sub.is_folder) {
        (Some(p), false) if (*s).details.is_none() => kind_of(p),
        _ => Kind::None,
    };
    let path = sub.path.as_deref().unwrap_or(Path::new(""));
    let key = sub.path.as_deref().filter(|_| !sub.is_folder);
    let size = if (*s).details.is_some() {
        let c = content_rect(hwnd);
        details_edge(c.right - c.left, c.bottom - c.top, scale(hwnd, 256))
    } else {
        request_size(hwnd)
    };
    let start = |kind: Kind| {
        if spawn {
            fetch(path.to_path_buf(), sub.abs_pidl.clone(), size, kind, hwnd.0 as isize);
        }
    };
    if kind == Kind::Text {
        let st = stamp(path);
        match st.and_then(|st| lookup(path, st, |e| e.text.clone())) {
            Some(t) => {
                // PV_READY can repeat for a version already set.
                if (*s).stamp != st {
                    let _ = SetWindowTextW((*s).edit, &HSTRING::from(t));
                    (*s).stamp = st;
                }
                let _ = ShowWindow((*s).edit, SW_SHOW);
                return;
            }
            None => start(Kind::Text),
        }
    } else if kind == Kind::Handler {
        // Stamped before the load: a write during it leaves the stamp behind, so the next show re-hosts.
        let st = stamp(path);
        if let Ok(p) = preview::open(path, hwnd, content_rect(hwnd)) {
            (*s).preview = Some(p);
            (*s).stamp = st;
            return;
        }
    }
    // Image, None, and every failure above: thumbnail plus caption.
    (*s).bitmap = match key.and_then(|p| Some((p, stamp(p)?))) {
        Some((p, st)) => {
            let hit = lookup(p, st, |e| e.thumb.as_ref().filter(|(sz, _)| *sz == size).map(|(_, t)| t.clone()));
            if hit.is_none() && kind != Kind::Text {
                start(Kind::Image);
            }
            hit
        }
        None => cached_thumb(None, &sub.abs_pidl, size),
    };
    (*s).name = sub.name.clone();
    (*s).detail = detail_text(sub.size, &sub.type_text);
}

pub fn set_rect(hwnd: HWND, rect: RECT) {
    unsafe {
        let _ = MoveWindow(hwnd, rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top, true);
        relayout(hwnd);
    }
}

#[cfg(test)]
pub fn details_of(hwnd: HWND) -> Option<(String, Vec<(&'static str, String)>)> {
    unsafe { (*state(hwnd)).details.as_ref().map(|d| (d.heading.clone(), d.rows.clone())) }
}

#[cfg(test)]
pub fn has_bitmap(hwnd: HWND) -> bool {
    unsafe { (*state(hwnd)).bitmap.is_some() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rexplorer_core::com::ComInit;
    use windows::Win32::UI::Shell::SHParseDisplayName;

    #[test]
    fn details_values_start_past_the_widest_label() {
        assert_eq!(label_split(0, 500, 60, 8), 200, "short labels: 2/5");
        assert_eq!(label_split(0, 500, 230, 8), 238);
        assert_eq!(label_split(0, 500, 400, 8), 300, "at most 3/5");
        assert_eq!(label_split(100, 600, 230, 8), 338);
    }

    fn pump() {
        let mut m = MSG::default();
        unsafe {
            while PeekMessageW(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&m);
                DispatchMessageW(&m);
            }
        }
    }

    fn edit_text(hwnd: HWND) -> String {
        unsafe {
            let e = GetWindow(hwnd, GW_CHILD).unwrap();
            let n = GetWindowTextLengthW(e) as usize;
            let mut b = vec![0u16; n + 1];
            let got = GetWindowTextW(e, &mut b) as usize;
            String::from_utf16_lossy(&b[..got])
        }
    }

    #[test]
    fn accessible_pane_and_name_changes() {
        use crate::acc_test::*;
        use windows::Win32::UI::Accessibility::*;
        let _com = ComInit::sta();
        unsafe {
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_POPUP, 0, 0, 800, 600, None, None, None, None).unwrap();
            let pane = create(parent, 7).unwrap();
            set_rect(pane, RECT { left: 0, top: 0, right: 400, bottom: 500 });
            let acc = object(pane);
            assert_eq!(root(&acc).name, "Preview", "never an empty name");
            assert_eq!(root(&acc).role, ROLE_SYSTEM_PANE);
            assert_eq!(acc.accChildCount().unwrap(), 0);
            let hk = hook();
            // Caption mode: the file name and its detail line.
            (*state(pane)).name = "a.txt".into();
            (*state(pane)).detail = "Text, 1 KB".into();
            announce(pane, state(pane));
            let ev = hk.take();
            assert!(ev.contains(&(EVENT_OBJECT_NAMECHANGE, pane, 0)) && ev.contains(&(EVENT_OBJECT_NAMECHANGE, pane, 1)) && ev.contains(&(EVENT_OBJECT_NAMECHANGE, pane, 2)), "{ev:?}");
            assert_eq!(root(&acc).name, "a.txt");
            let k = kids(&acc);
            assert_eq!([k[0].name.as_str(), k[1].name.as_str()], ["a.txt", "Text, 1 KB"]);
            assert!(k.iter().all(|k| k.role == ROLE_SYSTEM_STATICTEXT && k.state & STATE_SYSTEM_READONLY != 0));
            announce(pane, state(pane));
            assert!(hk.take().is_empty(), "nothing changed, nothing raised");
            // Details mode: heading, sub line, rows.
            show_details(pane, None, DetailsText { heading: "3 items selected".into(), sub: String::new(), rows: vec![("Size", "5 KB".into())] }, false);
            assert!(hk.take().contains(&(EVENT_OBJECT_NAMECHANGE, pane, 0)));
            assert_eq!(kids(&acc).iter().map(|k| k.name.clone()).collect::<Vec<_>>(), ["3 items selected", "Size: 5 KB"]);
            show(pane, None);
            assert!(hk.take().contains(&(EVENT_OBJECT_NAMECHANGE, pane, 0)));
            assert_eq!(root(&acc).name, "Preview");
            drop(hk);
            let _ = DestroyWindow(parent);
            pump();
        }
    }

    #[test]
    fn pure_helpers() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536 * 1024), "1.5 MB");
        assert_eq!(decode_text(b"\xef\xbb\xbfa\nb\r\nc\rd"), "a\r\nb\r\nc\r\nd");
        assert_eq!(thumb_size(300, 200, 44), 156);
        assert_eq!(thumb_size(100, 10, 44), 1);
        assert_eq!(detail_text(None, "Folder"), "Folder");
    }

    #[test]
    fn details_pure_helpers() {
        assert_eq!(strip_marks("\u{200e}1/2/2026\u{200f} \u{202a}3:04\u{202c}"), "1/2/2026 3:04");
        assert!(dims_allowed(0x20));
        assert!(!dims_allowed(0x0040_0000));
        assert!(!dims_allowed(0x1000));
        assert!(!dims_allowed(0x0040_0020));
        assert_eq!(details_edge(300, 800, 256), 256);
        assert_eq!(details_edge(200, 800, 256), 200);
        assert_eq!(details_edge(300, 300, 256), 150);
        assert_eq!(details_edge(0, 0, 256), 1);
    }

    #[test]
    fn details_mode_rows_picture_and_dimensions() {
        let _com = ComInit::sta();
        let dir = std::env::temp_dir().join(format!("rexplorer-details-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bmp = dir.join("p.bmp");
        let mut b = b"BM".to_vec();
        for v in [54u32 + 24, 0, 54, 40, 2, 3] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&24u16.to_le_bytes());
        b.extend_from_slice(&[0u8; 24]);
        b.extend_from_slice(&[0x80u8; 24]);
        std::fs::write(&bmp, &b).unwrap();
        let digits = |s: &str| s.chars().filter(char::is_ascii_digit).collect::<String>();
        assert_eq!(digits(&image_dimensions(&bmp).expect("dimensions")), "23");
        let mut pi = std::ptr::null_mut();
        let abs = unsafe {
            SHParseDisplayName(&HSTRING::from(bmp.as_os_str()), None, &mut pi, 0, None).unwrap();
            Pidl::from_raw(pi)
        };
        let text = |h: &str| DetailsText { heading: h.into(), sub: String::new(), rows: vec![] };
        unsafe {
            let parent = CreateWindowExW(
                WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW,
                0, 0, 800, 600, None, None, None, None,
            )
            .unwrap();
            let pane = create(parent, 7).unwrap();
            set_rect(pane, RECT { left: 0, top: 0, right: 400, bottom: 500 });
            pump();

            show_details(pane, None, text("3 items selected"), false);
            assert_eq!(details_of(pane).map(|d| d.0).as_deref(), Some("3 items selected"));
            assert!(!has_bitmap(pane));

            let subject = Subject { path: Some(bmp.clone()), abs_pidl: abs, name: "p.bmp".into(), is_folder: false, size: Some(78), type_text: "BMP".into() };
            show_details(pane, Some(subject), text("p.bmp"), true);
            let mut dims = None;
            for _ in 0..300 {
                pump();
                dims = details_of(pane).and_then(|d| d.1.into_iter().find(|r| r.0 == "Dimensions"));
                if dims.is_some() && has_bitmap(pane) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(has_bitmap(pane), "the picture comes through the cache");
            assert_eq!(digits(&dims.expect("a Dimensions row").1), "23");

            show(pane, None);
            assert!(details_of(pane).is_none());
            assert!(!has_bitmap(pane));
            let _ = DestroyWindow(parent);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn show_text_folder_and_clear() {
        let _com = ComInit::sta();
        let dir = std::env::temp_dir().join(format!("rexplorer-preview_pane-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "hola\nmundo").unwrap();
        let parse = |p: &std::path::Path| {
            let mut pi = std::ptr::null_mut();
            unsafe {
                SHParseDisplayName(&HSTRING::from(p.as_os_str()), None, &mut pi, 0, None).unwrap();
                Pidl::from_raw(pi)
            }
        };
        unsafe {
            let parent = CreateWindowExW(
                WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW,
                0, 0, 800, 600, None, None, None, None,
            )
            .unwrap();
            let pane = create(parent, 7).unwrap();
            set_rect(pane, RECT { left: 0, top: 0, right: 400, bottom: 500 });
            pump();

            show(pane, Some(Subject {
                path: Some(file.clone()), abs_pidl: parse(&file), name: "a.txt".into(),
                is_folder: false, size: Some(10), type_text: "Text".into(),
            }));
            // The text arrives from a worker through PV_READY.
            for _ in 0..200 {
                pump();
                if !edit_text(pane).is_empty() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert_eq!(edit_text(pane), "hola\r\nmundo");

            // The same file version again leaves the Edit's text alone.
            let _ = SetWindowTextW(GetWindow(pane, GW_CHILD).unwrap(), w!("kept"));
            show(pane, Some(Subject {
                path: Some(file.clone()), abs_pidl: parse(&file), name: "a.txt".into(),
                is_folder: false, size: Some(10), type_text: "Text".into(),
            }));
            pump();
            assert_eq!(edit_text(pane), "kept");

            show(pane, Some(Subject {
                path: Some(dir.clone()), abs_pidl: parse(&dir), name: "d".into(),
                is_folder: true, size: None, type_text: "Folder".into(),
            }));
            pump();
            assert!(has_bitmap(pane));
            assert_eq!(edit_text(pane), "");

            show(pane, None);
            assert!(!has_bitmap(pane));
            let _ = DestroyWindow(parent);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
