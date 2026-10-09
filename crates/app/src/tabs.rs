//! Custom-drawn browser-style tab strip, window class "RexTabs".
//! The parent receives WM_NOTIFY with an `NmTab` (TN_SELECT / TN_CLOSE / TN_NEW); it owns adding and removing tabs.

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::Once;

use rexplorer_core::settings::Theme;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::NMHDR;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, Result, w};

pub const TN_SELECT: u32 = 0x9001;
pub const TN_CLOSE: u32 = 0x9002;
pub const TN_NEW: u32 = 0x9003;

#[repr(C)]
pub struct NmTab {
    pub hdr: NMHDR,
    pub index: usize,
}

const WM_MOUSELEAVE: u32 = 0x02A3;
const CLASS: PCWSTR = w!("RexTabs");

// ---- pure model -----------------------------------------------------------------------------

fn scale(px: i32, dpi: u32) -> i32 {
    (px as i64 * dpi as i64 / 96) as i32
}

pub fn height(dpi: u32) -> i32 {
    scale(36, dpi)
}

#[derive(Debug, PartialEq)]
pub enum Hit {
    Tab(usize),
    Close(usize),
    New,
    None,
}

pub struct TabLayout {
    count: usize,
    tab_w: i32,
    h: i32,
    close: i32,
    pad: i32,
    new_w: i32,
}

pub fn layout(count: usize, width: i32, dpi: u32) -> TabLayout {
    let new_w = scale(36, dpi);
    let (max_w, min_w) = (scale(180, dpi), scale(80, dpi));
    // Beyond min width the strip just clips.
    let tab_w = if count == 0 { max_w } else { ((width - new_w) / count as i32).clamp(min_w, max_w) };
    TabLayout { count, tab_w, h: height(dpi), close: scale(20, dpi), pad: scale(6, dpi), new_w }
}

impl TabLayout {
    pub fn rect(&self, i: usize) -> RECT {
        let left = i as i32 * self.tab_w;
        RECT { left, top: 0, right: left + self.tab_w, bottom: self.h }
    }

    pub fn close_rect(&self, i: usize) -> RECT {
        let r = self.rect(i);
        let top = (self.h - self.close) / 2;
        RECT { left: r.right - self.pad - self.close, top, right: r.right - self.pad, bottom: top + self.close }
    }

    pub fn new_rect(&self) -> RECT {
        let left = self.count as i32 * self.tab_w;
        RECT { left, top: 0, right: left + self.new_w, bottom: self.h }
    }

    pub fn hit(&self, x: i32, y: i32) -> Hit {
        let inside = |r: RECT| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
        if inside(self.new_rect()) {
            return Hit::New;
        }
        if y < 0 || y >= self.h || x < 0 || self.tab_w == 0 {
            return Hit::None;
        }
        let i = (x / self.tab_w) as usize;
        if i >= self.count {
            Hit::None
        } else if inside(self.close_rect(i)) {
            Hit::Close(i)
        } else {
            Hit::Tab(i)
        }
    }
}

/// Mix `a` toward `b` by `pct` percent, per 0x00BBGGRR channel.
fn blend(a: u32, b: u32, pct: u32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = ((a >> s) & 0xFF, (b >> s) & 0xFF);
        ((x * (100 - pct) + y * pct) / 100) << s
    };
    ch(0) | ch(8) | ch(16)
}

// ---- window ---------------------------------------------------------------------------------

struct State {
    titles: Vec<String>,
    active: usize,
    hover: Hit,
    tracking: bool,
    theme: Theme,
    font: HFONT,
}

pub fn create(parent: HWND, id: usize) -> Result<HWND> {
    static REGISTER: Once = Once::new();
    unsafe {
        let hinst = GetModuleHandleW(None)?.into();
        REGISTER.call_once(|| {
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wndproc),
                hInstance: hinst,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: CLASS,
                ..Default::default()
            };
            RegisterClassW(&wc);
        });
        // Freed by WM_NCDESTROY (leaks only if creation fails before WM_NCCREATE).
        let state = Box::into_raw(Box::new(State {
            titles: Vec::new(),
            active: 0,
            hover: Hit::None,
            tracking: false,
            theme: Theme::light(),
            font: HFONT::default(),
        }));
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
            0,
            0,
            0,
            0,
            Some(parent),
            Some(HMENU(id as *mut c_void)),
            Some(hinst),
            Some(state as *const c_void),
        )
    }
}

unsafe fn state(hwnd: HWND) -> *mut State {
    GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State
}

/// Run `f` on the state and repaint; no-op for a foreign window.
fn with(hwnd: HWND, f: impl FnOnce(&mut State)) {
    unsafe {
        let s = state(hwnd);
        if !s.is_null() {
            f(&mut *s);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

pub fn set_theme(hwnd: HWND, t: &Theme) {
    with(hwnd, |s| s.theme = t.clone());
}

pub fn add(hwnd: HWND, title: &str) -> usize {
    let mut n = 0;
    with(hwnd, |s| {
        s.titles.push(title.to_string());
        n = s.titles.len() - 1;
    });
    n
}

pub fn remove(hwnd: HWND, index: usize) {
    with(hwnd, |s| {
        if index >= s.titles.len() {
            return;
        }
        s.titles.remove(index);
        if s.active > index {
            s.active -= 1;
        }
        s.active = s.active.min(s.titles.len().saturating_sub(1));
        s.hover = Hit::None;
    });
}

pub fn set_title(hwnd: HWND, index: usize, title: &str) {
    with(hwnd, |s| {
        if let Some(t) = s.titles.get_mut(index) {
            *t = title.to_string();
        }
    });
}

pub fn set_active(hwnd: HWND, index: usize) {
    with(hwnd, |s| {
        if index < s.titles.len() {
            s.active = index;
        }
    });
}

pub fn active(hwnd: HWND) -> usize {
    unsafe { state(hwnd).as_ref().map_or(0, |s| s.active) }
}

pub fn count(hwnd: HWND) -> usize {
    unsafe { state(hwnd).as_ref().map_or(0, |s| s.titles.len()) }
}

unsafe fn notify(hwnd: HWND, code: u32, index: usize) {
    let id = GetDlgCtrlID(hwnd) as usize;
    let nm = NmTab { hdr: NMHDR { hwndFrom: hwnd, idFrom: id, code }, index };
    if let Ok(parent) = GetParent(hwnd) {
        SendMessageW(parent, WM_NOTIFY, Some(WPARAM(id)), Some(LPARAM(&nm as *const _ as isize)));
    }
}

unsafe fn hit_at(hwnd: HWND, lparam: LPARAM) -> Hit {
    let (x, y) = (lparam.0 as i16 as i32, (lparam.0 >> 16) as i16 as i32);
    let s = state(hwnd);
    let mut rc = RECT::default();
    if s.is_null() || GetClientRect(hwnd, &mut rc).is_err() {
        return Hit::None;
    }
    layout((*s).titles.len(), rc.right, GetDpiForWindow(hwnd)).hit(x, y)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        }
        WM_NCDESTROY => {
            let s = state(hwnd);
            if !s.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(s));
            }
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_SETFONT => with(hwnd, |s| s.font = HFONT(wparam.0 as *mut c_void)),
        WM_PAINT => {
            paint(hwnd);
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            let h = hit_at(hwnd, lparam);
            let s = state(hwnd);
            if !s.is_null() {
                if !(*s).tracking {
                    let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    (*s).tracking = TrackMouseEvent(&mut tme).is_ok();
                }
                if (*s).hover != h {
                    (*s).hover = h;
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
        }
        WM_MOUSELEAVE => with(hwnd, |s| {
            s.tracking = false;
            s.hover = Hit::None;
        }),
        // The parent may remove tabs from inside the notification, so no state reference is held across it.
        WM_LBUTTONDOWN => match hit_at(hwnd, lparam) {
            Hit::Tab(i) => {
                set_active(hwnd, i);
                notify(hwnd, TN_SELECT, i);
            }
            Hit::Close(i) => notify(hwnd, TN_CLOSE, i),
            Hit::New => notify(hwnd, TN_NEW, 0),
            Hit::None => {}
        },
        WM_MBUTTONUP => {
            if let Hit::Tab(i) | Hit::Close(i) = hit_at(hwnd, lparam) {
                notify(hwnd, TN_CLOSE, i);
            }
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Segoe Fluent Icons, else Segoe MDL2 Assets (same code points).
unsafe fn icon_font(hdc: HDC, height: i32) -> HFONT {
    let mk = |face: PCWSTR| {
        CreateFontW(-height, 0, 0, 0, 400, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, 0, face)
    };
    let fluent = mk(w!("Segoe Fluent Icons"));
    // GDI silently substitutes a missing face, so ask which one was actually selected.
    let old = SelectObject(hdc, fluent.into());
    let mut buf = [0u16; 64];
    let n = GetTextFaceW(hdc, Some(&mut buf)).clamp(0, 63) as usize;
    SelectObject(hdc, old);
    if String::from_utf16_lossy(&buf[..n]).eq_ignore_ascii_case("Segoe Fluent Icons") {
        return fluent;
    }
    let _ = DeleteObject(fluent.into());
    mk(w!("Segoe MDL2 Assets"))
}

unsafe fn text(hdc: HDC, s: &str, rc: &mut RECT, fmt: DRAW_TEXT_FORMAT) {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    DrawTextW(hdc, &mut v, rc, fmt);
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let wdc = BeginPaint(hwnd, &mut ps);
    let s = state(hwnd);
    let mut rc = RECT::default();
    if !s.is_null() && GetClientRect(hwnd, &mut rc).is_ok() {
        let s = &*s;
        let dpi = GetDpiForWindow(hwnd);
        let (cw, ch) = (rc.right.max(1), rc.bottom.max(1));
        let dc = CreateCompatibleDC(Some(wdc));
        let bmp = CreateCompatibleBitmap(wdc, cw, ch);
        let old_bmp = SelectObject(dc, bmp.into());
        draw(dc, s, &layout(s.titles.len(), cw, dpi), dpi, cw, ch);
        let _ = BitBlt(wdc, 0, 0, cw, ch, Some(dc), 0, 0, SRCCOPY);
        SelectObject(dc, old_bmp);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(dc);
    }
    let _ = EndPaint(hwnd, &ps);
}

unsafe fn fill(dc: HDC, r: &RECT, color: u32) {
    let b = CreateSolidBrush(COLORREF(color));
    FillRect(dc, r, b);
    let _ = DeleteObject(b.into());
}

unsafe fn draw(dc: HDC, s: &State, lay: &TabLayout, dpi: u32, cw: i32, ch: i32) {
    let t = &s.theme;
    let strip = blend(t.background, t.text, 6);
    let hover_fill = blend(strip, t.text, 10);
    fill(dc, &RECT { left: 0, top: 0, right: cw, bottom: ch }, strip);
    SetBkMode(dc, TRANSPARENT);
    let font = if s.font.is_invalid() { HFONT(GetStockObject(DEFAULT_GUI_FONT).0) } else { s.font };
    let glyphs = icon_font(dc, scale(12, dpi));
    let pad = scale(10, dpi);
    let radius = scale(8, dpi);
    let top = scale(4, dpi);
    let fmt = DT_END_ELLIPSIS | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX;
    let glyph_fmt = DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX;

    for (i, title) in s.titles.iter().enumerate() {
        let r = lay.rect(i);
        let is_active = i == s.active;
        let hovered = matches!(s.hover, Hit::Tab(j) | Hit::Close(j) if j == i);
        if is_active {
            // Bottom corners fall below the client area so only the top is rounded.
            let pen = CreatePen(PS_SOLID, 1, COLORREF(t.border));
            let brush = CreateSolidBrush(COLORREF(t.background));
            let (op, ob) = (SelectObject(dc, pen.into()), SelectObject(dc, brush.into()));
            let _ = RoundRect(dc, r.left, r.top + top, r.right, r.bottom + radius, radius, radius);
            SelectObject(dc, op);
            SelectObject(dc, ob);
            let _ = DeleteObject(pen.into());
            let _ = DeleteObject(brush.into());
        } else if hovered {
            fill(dc, &RECT { top: r.top + top, ..r }, hover_fill);
        }
        SetTextColor(dc, COLORREF(if is_active { t.text } else { t.secondary_text }));
        let c = lay.close_rect(i);
        let old = SelectObject(dc, font.into());
        let mut tr = RECT { left: r.left + pad, top: r.top, right: c.left - scale(2, dpi), bottom: r.bottom };
        text(dc, title, &mut tr, fmt);
        if is_active || hovered {
            SelectObject(dc, glyphs.into());
            let mut cr = c;
            text(dc, "\u{E8BB}", &mut cr, glyph_fmt);
        }
        SelectObject(dc, old);
    }

    let n = lay.new_rect();
    if s.hover == Hit::New {
        let m = scale(6, dpi);
        fill(dc, &RECT { left: n.left + scale(4, dpi), top: n.top + m, right: n.right - scale(4, dpi), bottom: n.bottom - m }, hover_fill);
    }
    let old = SelectObject(dc, glyphs.into());
    SetTextColor(dc, COLORREF(t.secondary_text));
    let mut nr = n;
    text(dc, "\u{E710}", &mut nr, glyph_fmt);
    SelectObject(dc, old);
    let _ = DeleteObject(glyphs.into());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pump() {
        unsafe {
            let mut m = MSG::default();
            while PeekMessageW(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&m);
                DispatchMessageW(&m);
            }
        }
    }

    #[test]
    fn layout_three_tabs() {
        let l = layout(3, 1000, 96);
        let r0 = l.rect(0);
        assert_eq!((r0.left, r0.right - r0.left), (0, 180));
        assert_eq!(l.new_rect().left, l.rect(2).right);
        assert_eq!(height(96), 36);
        assert_eq!(height(192), 72);
    }

    #[test]
    fn layout_many_tabs_min_width() {
        let l = layout(20, 1000, 96);
        let r = l.rect(0);
        assert!(r.right - r.left >= 80);
    }

    #[test]
    fn hit_testing() {
        let l = layout(3, 1000, 96);
        let c = l.close_rect(1);
        assert_eq!(l.hit(c.left + 2, c.top + 2), Hit::Close(1));
        assert_eq!(l.hit(l.rect(1).left + 5, 5), Hit::Tab(1));
        let n = l.new_rect();
        assert_eq!(l.hit(n.left + 2, 5), Hit::New);
        assert_eq!(l.hit(n.right + 50, 5), Hit::None);
        assert_eq!(layout(0, 1000, 96).hit(100, 5), Hit::None);
    }

    #[test]
    fn blend_mixes_channels() {
        assert_eq!(blend(0x000000, 0xFFFFFF, 50), 0x7F7F7F);
        assert_eq!(blend(0x102030, 0x102030, 40), 0x102030);
    }

    #[test]
    fn create_add_remove() {
        unsafe {
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 800, 600, None, None, None, None).unwrap();
            let tabs = create(parent, 7).unwrap();
            assert_eq!(add(tabs, "a"), 0);
            assert_eq!(add(tabs, "b"), 1);
            assert_eq!(count(tabs), 2);
            set_active(tabs, 1);
            assert_eq!(active(tabs), 1);
            set_title(tabs, 0, "renamed");
            set_theme(tabs, &Theme::dark());
            pump();
            remove(tabs, 0);
            assert_eq!((active(tabs), count(tabs)), (0, 1));
            pump();
            let _ = DestroyWindow(parent);
        }
    }
}
