//! The preview pane (class "RexPreview"): text in a read-only Edit, preview handlers hosted in place,
//! everything else as a shell thumbnail with a name/size caption.
#![allow(dead_code)]
use std::ffi::c_void;
use std::io::Read;
use std::mem::size_of;
use std::path::PathBuf;
use std::sync::Once;

use rexplorer_core::com::Pidl;
use rexplorer_core::preview::{self, Kind, Preview};
use rexplorer_core::{images, settings::Theme};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, Result, w};

/// Reserved for the parent's notifications; the pane currently sends none.
pub const PVN_FIRST: u32 = 0x9000;

const MAX_TEXT: u64 = 256 * 1024;

pub struct Subject {
    pub path: Option<PathBuf>,
    pub abs_pidl: Pidl,
    pub name: String,
    pub is_folder: bool,
    pub size: Option<u64>,
    pub type_text: String,
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
    bitmap: Option<(HBITMAP, i32, i32)>,
    name: String,
    detail: String,
    preview: Option<Preview>,
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
    format!("{v:.1} {}", U[i])
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

fn caption_height(hwnd: HWND) -> i32 {
    scale(hwnd, 44)
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
                Ok(e) => (*s).edit = e,
                Err(_) => return LRESULT(-1),
            }
            LRESULT(0)
        }
        WM_SIZE => {
            relayout(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
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
        WM_DESTROY => {
            (*s).preview = None;
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
    if let Some((b, _, _)) = (*s).bitmap.take() {
        let _ = DeleteObject(b.into());
    }
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

        if let Some((hb, bw, bh)) = (*s).bitmap {
            let c = content_rect(hwnd);
            let cap = caption_height(hwnd);
            let area_h = (c.bottom - c.top - cap).max(1);
            // The shell may hand back a larger thumbnail than asked for: shrink it into the card.
            let fit = ((c.right - c.left) as f64 / bw.max(1) as f64).min(area_h as f64 / bh.max(1) as f64).min(1.0);
            let (dw, dh) = (((bw as f64 * fit) as i32).max(1), ((bh as f64 * fit) as i32).max(1));
            let x = c.left + ((c.right - c.left - dw) / 2).max(0);
            let y = c.top + ((area_h - dh) / 2).max(0);
            let src = CreateCompatibleDC(Some(hdc));
            let prev = SelectObject(src, hb.into());
            let bf = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = AlphaBlend(mem, x, y, dw, dh, src, 0, 0, bw, bh, bf);
            SelectObject(src, prev);
            let _ = DeleteDC(src);

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
            SetTextColor(mem, COLORREF((*s).text));
            let mut t: Vec<u16> = (*s).name.encode_utf16().collect();
            DrawTextW(mem, &mut t, &mut r1, fmt);
            let mut r2 = RECT { left: c.left, top: top + half, right: c.right, bottom: top + cap };
            SetTextColor(mem, COLORREF((*s).secondary));
            let mut t: Vec<u16> = (*s).detail.encode_utf16().collect();
            DrawTextW(mem, &mut t, &mut r2, fmt);
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
        (*s).preview = None;
        clear_bitmap(s);
        (*s).name.clear();
        (*s).detail.clear();
        let _ = ShowWindow((*s).edit, SW_HIDE);
        let _ = SetWindowTextW((*s).edit, w!(""));
        if let Some(sub) = subject {
            present(hwnd, s, &sub);
        }
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

unsafe fn present(hwnd: HWND, s: *mut State, sub: &Subject) {
    let kind = match (&sub.path, sub.is_folder) {
        (Some(p), false) => preview::kind(p.extension().and_then(|e| e.to_str()).unwrap_or("")),
        _ => Kind::None,
    };
    let path = sub.path.as_deref().unwrap_or(std::path::Path::new(""));
    if kind == Kind::Text {
        if let Ok(f) = std::fs::File::open(path) {
            let mut buf = Vec::new();
            if f.take(MAX_TEXT).read_to_end(&mut buf).is_ok() {
                let _ = SetWindowTextW((*s).edit, &HSTRING::from(decode_text(&buf)));
                let _ = ShowWindow((*s).edit, SW_SHOW);
                return;
            }
        }
    } else if kind == Kind::Handler {
        if let Ok(p) = preview::open(path, hwnd, content_rect(hwnd)) {
            (*s).preview = Some(p);
            return;
        }
    }
    // Image, None, and every failure above: thumbnail plus caption.
    let rc = content_rect(hwnd);
    let size = thumb_size(rc.right - rc.left, rc.bottom - rc.top, caption_height(hwnd));
    if let Ok(b) = images::thumbnail(&sub.abs_pidl, size) {
        let mut bm = BITMAP::default();
        GetObjectW(b.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void));
        (*s).bitmap = Some((b, bm.bmWidth, bm.bmHeight));
    }
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
pub fn has_bitmap(hwnd: HWND) -> bool {
    unsafe { (*state(hwnd)).bitmap.is_some() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rexplorer_core::com::ComInit;
    use windows::Win32::UI::Shell::SHParseDisplayName;

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
    fn pure_helpers() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536 * 1024), "1.5 MB");
        assert_eq!(decode_text(b"\xef\xbb\xbfa\nb\r\nc\rd"), "a\r\nb\r\nc\r\nd");
        assert_eq!(thumb_size(300, 200, 44), 156);
        assert_eq!(thumb_size(100, 10, 44), 1);
        assert_eq!(detail_text(None, "Folder"), "Folder");
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
            pump();
            assert_eq!(edit_text(pane), "hola\r\nmundo");

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
