//! Custom-drawn tab strip of pill-shaped tabs, window class "RexTabs".
//! The parent receives WM_NOTIFY with an `NmTab` (TN_SELECT / TN_CLOSE / TN_NEW); it owns adding and removing tabs.

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::Once;

use rexplorer_core::settings::Theme;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::NMHDR;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, Result, w};

use crate::theme;

pub const TN_SELECT: u32 = 0x9001;
pub const TN_CLOSE: u32 = 0x9002;
pub const TN_NEW: u32 = 0x9003;
/// A tab was dragged from `index` to `to`; the strip has already moved its title.
pub const TN_MOVE: u32 = 0x9004;
/// Right-click on tab `index`.
pub const TN_CONTEXT: u32 = 0x9005;
/// Tab `index` was released outside the window.
pub const TN_TEAR: u32 = 0x9006;

#[repr(C)]
pub struct NmTab {
    pub hdr: NMHDR,
    pub index: usize,
    pub to: usize,
}

const WM_MOUSELEAVE: u32 = 0x02A3;
const HOVER_TIMER: usize = 1;
/// A drag resting on a tab this long switches to it.
const HOVER_MS: u32 = 700;
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

/// Where index `active` ends up after the tab at `from` moves to `to`.
pub fn moved(active: usize, from: usize, to: usize) -> usize {
    if active == from {
        to
    } else if from < to && active > from && active <= to {
        active - 1
    } else if to < from && active >= to && active < from {
        active + 1
    } else {
        active
    }
}

/// A tab released at screen point (x, y) left window rect `win`.
fn torn_out(win: RECT, x: i32, y: i32) -> bool {
    x < win.left || x >= win.right || y < win.top || y >= win.bottom
}

impl TabLayout {
    /// The slot under client x, clamped to the tabs.
    pub fn slot(&self, x: i32) -> usize {
        if self.count == 0 || self.tab_w == 0 { 0 } else { (x.max(0) / self.tab_w).min(self.count as i32 - 1) as usize }
    }

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

// ---- window ---------------------------------------------------------------------------------

struct State {
    titles: Vec<String>,
    active: usize,
    hover: Hit,
    tracking: bool,
    /// The tab a drag rests on, while HOVER_TIMER runs.
    drag_hover: Option<usize>,
    /// The tab under the left button (and the client point of the press), until release or capture loss.
    press: Option<(usize, i32, i32)>,
    dragging: bool,
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
            drag_hover: None,
            press: None,
            dragging: false,
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
        s.press = None;
        s.dragging = false;
    });
}

/// Moves the title at `from` to `to`, the active index following; the parent moves its tab on TN_MOVE.
pub fn move_tab(hwnd: HWND, from: usize, to: usize) {
    with(hwnd, |s| {
        if from >= s.titles.len() || to >= s.titles.len() || from == to {
            return;
        }
        let t = s.titles.remove(from);
        s.titles.insert(to, t);
        s.active = moved(s.active, from, to);
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

unsafe fn notify(hwnd: HWND, code: u32, index: usize, to: usize) {
    let id = GetDlgCtrlID(hwnd) as usize;
    let nm = NmTab { hdr: NMHDR { hwndFrom: hwnd, idFrom: id, code }, index, to };
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

/// The tab a drag-hover timer switches to: the one it was armed for, still under the cursor with a button
/// held, and not already active.
fn hover_switch(armed: Option<usize>, under: Option<usize>, active: usize, down: bool) -> Option<usize> {
    armed.filter(|&i| down && under == Some(i) && i != active)
}

/// Tab (or its close button) under screen point (x, y).
pub fn tab_at_screen(hwnd: HWND, x: i32, y: i32) -> Option<usize> {
    unsafe {
        let s = state(hwnd);
        let mut rc = RECT::default();
        let mut p = POINT { x, y };
        if s.is_null() || GetClientRect(hwnd, &mut rc).is_err() || !ScreenToClient(hwnd, &mut p).as_bool() {
            return None;
        }
        match layout((*s).titles.len(), rc.right, GetDpiForWindow(hwnd)).hit(p.x, p.y) {
            Hit::Tab(i) | Hit::Close(i) => Some(i),
            _ => None,
        }
    }
}

/// A drag is over tab `i` (None: over no tab): (re)arms or stops HOVER_TIMER; no repaint.
pub fn drag_hover(hwnd: HWND, i: Option<usize>) {
    unsafe {
        let s = state(hwnd);
        if s.is_null() || (*s).drag_hover == i {
            return;
        }
        (*s).drag_hover = i;
        if i.is_some() {
            SetTimer(Some(hwnd), HOVER_TIMER, HOVER_MS, None);
        } else {
            let _ = KillTimer(Some(hwnd), HOVER_TIMER);
        }
    }
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
                // Gated on the message's button flag and the stored press only, never the real button or cursor.
                if let (true, Some((from, px, py))) = (wparam.0 & 1 != 0, (*s).press) {
                    let (x, y) = (lparam.0 as i16 as i32, (lparam.0 >> 16) as i16 as i32);
                    if !(*s).dragging {
                        (*s).dragging = (x - px).abs() > GetSystemMetrics(SM_CXDRAG) || (y - py).abs() > GetSystemMetrics(SM_CYDRAG);
                    }
                    let mut rc = RECT::default();
                    if (*s).dragging && GetClientRect(hwnd, &mut rc).is_ok() {
                        let len = (*s).titles.len();
                        let to = layout(len, rc.right, GetDpiForWindow(hwnd)).slot(x);
                        if from < len && to != from {
                            move_tab(hwnd, from, to);
                            (*s).press = Some((to, px, py));
                            notify(hwnd, TN_MOVE, from, to);
                        }
                    }
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
                let s = state(hwnd);
                if !s.is_null() {
                    (*s).press = Some((i, lparam.0 as i16 as i32, (lparam.0 >> 16) as i16 as i32));
                    (*s).dragging = false;
                    SetCapture(hwnd);
                }
                set_active(hwnd, i);
                notify(hwnd, TN_SELECT, i, 0);
            }
            Hit::Close(i) => notify(hwnd, TN_CLOSE, i, 0),
            Hit::New => notify(hwnd, TN_NEW, 0, 0),
            Hit::None => {}
        },
        WM_LBUTTONUP => {
            // Taken before ReleaseCapture, whose WM_CAPTURECHANGED clears the press.
            let s = state(hwnd);
            let (p, dragged) = if s.is_null() { (None, false) } else { ((*s).press.take(), std::mem::take(&mut (*s).dragging)) };
            let _ = ReleaseCapture();
            if let (true, Some((i, ..))) = (dragged, p) {
                let mut pt = POINT { x: lparam.0 as i16 as i32, y: (lparam.0 >> 16) as i16 as i32 };
                let mut wr = RECT::default();
                // The parent is the main window, not the strip: a reorder ending just under the strip stays put.
                if let Ok(parent) = GetParent(hwnd) {
                    if ClientToScreen(hwnd, &mut pt).as_bool() && GetWindowRect(parent, &mut wr).is_ok() && torn_out(wr, pt.x, pt.y) {
                        notify(hwnd, TN_TEAR, i, 0);
                    }
                }
            }
        }
        WM_CAPTURECHANGED => with(hwnd, |s| {
            s.press = None;
            s.dragging = false;
        }),
        WM_TIMER if wparam.0 == HOVER_TIMER => {
            let _ = KillTimer(Some(hwnd), HOVER_TIMER);
            let s = state(hwnd);
            if !s.is_null() {
                let armed = (*s).drag_hover.take();
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let down = GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 || GetAsyncKeyState(VK_RBUTTON.0 as i32) < 0;
                if let Some(i) = hover_switch(armed, tab_at_screen(hwnd, pt.x, pt.y), (*s).active, down) {
                    set_active(hwnd, i);
                    notify(hwnd, TN_SELECT, i, 0);
                }
            }
        }
        WM_RBUTTONUP => {
            if let Hit::Tab(i) | Hit::Close(i) = hit_at(hwnd, lparam) {
                notify(hwnd, TN_CONTEXT, i, 0);
            }
        }
        WM_MBUTTONUP => {
            if let Hit::Tab(i) | Hit::Close(i) = hit_at(hwnd, lparam) {
                notify(hwnd, TN_CLOSE, i, 0);
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
    fill(dc, &RECT { left: 0, top: 0, right: cw, bottom: ch }, t.background);
    SetBkMode(dc, TRANSPARENT);
    let font = if s.font.is_invalid() { HFONT(GetStockObject(DEFAULT_GUI_FONT).0) } else { s.font };
    let glyphs = icon_font(dc, scale(12, dpi));
    let pad = scale(12, dpi);
    let radius = scale(8, dpi);
    let m = scale(4, dpi);
    // Each tab is a pill inset in its slot: active = surface, hovered = the hover fill, others transparent.
    let pill = |r: &RECT| RECT { left: r.left + m / 2, top: r.top + m, right: r.right - m / 2, bottom: r.bottom - m };
    let fmt = DT_END_ELLIPSIS | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX;
    let glyph_fmt = DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX;

    for (i, title) in s.titles.iter().enumerate() {
        let r = lay.rect(i);
        let is_active = i == s.active;
        let hovered = matches!(s.hover, Hit::Tab(j) | Hit::Close(j) if j == i);
        if is_active {
            theme::fill_round(dc, &pill(&r), t.surface, radius);
        } else if hovered {
            theme::fill_round(dc, &pill(&r), theme::hover(t), radius);
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
        theme::fill_round(dc, &RECT { left: n.left + m, top: n.top + m, right: n.right - m, bottom: n.bottom - m }, theme::hover(t), radius);
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

    #[test]
    fn hover_switch_rules() {
        assert_eq!(hover_switch(Some(1), Some(1), 0, true), Some(1));
        assert_eq!(hover_switch(Some(1), Some(1), 1, true), None, "already active");
        assert_eq!(hover_switch(Some(1), Some(0), 0, true), None, "moved off");
        assert_eq!(hover_switch(Some(1), Some(1), 0, false), None, "dropped");
        assert_eq!(hover_switch(None, Some(1), 0, true), None);
    }

    #[test]
    fn tab_at_screen_and_drag_hover() {
        unsafe {
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 800, 600, None, None, None, None).unwrap();
            let tabs = create(parent, 7).unwrap();
            add(tabs, "a");
            add(tabs, "b");
            let _ = MoveWindow(tabs, 0, 0, 600, 36, false);
            let l = layout(2, 600, GetDpiForWindow(tabs));
            let center = |r: RECT| {
                let mut p = windows::Win32::Foundation::POINT { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 };
                let _ = ClientToScreen(tabs, &mut p);
                p
            };
            let p = center(l.rect(1));
            assert_eq!(tab_at_screen(tabs, p.x, p.y), Some(1));
            let p = center(l.new_rect());
            assert_eq!(tab_at_screen(tabs, p.x, p.y), None);
            drag_hover(tabs, Some(1));
            assert_eq!((*state(tabs)).drag_hover, Some(1));
            drag_hover(tabs, None);
            assert_eq!((*state(tabs)).drag_hover, None);
            let _ = DestroyWindow(parent);
        }
    }

    #[test]
    fn moved_rules() {
        assert_eq!((moved(0, 0, 2), moved(1, 0, 2), moved(2, 0, 1)), (2, 0, 2));
        assert_eq!((moved(0, 2, 0), moved(3, 2, 0), moved(1, 1, 1)), (1, 3, 1));
    }

    #[test]
    fn torn_out_rules() {
        let win = RECT { left: 100, top: 100, right: 500, bottom: 400 };
        assert!(torn_out(win, 99, 200) && torn_out(win, 500, 200) && torn_out(win, 200, 99) && torn_out(win, 200, 400));
        assert!(!torn_out(win, 100, 100) && !torn_out(win, 499, 399));
    }

    #[test]
    fn slot_clamps() {
        let l = layout(3, 1000, 96);
        assert_eq!((l.slot(-5), l.slot(179), l.slot(180), l.slot(5000)), (0, 0, 1, 2));
        assert_eq!(layout(0, 1000, 96).slot(10), 0);
    }

    fn lp(x: i32, y: i32) -> LPARAM {
        LPARAM(((y as u16 as isize) << 16) | (x as u16 as isize))
    }

    /// A parent with a 600 px strip of tabs "a", "b", "c".
    unsafe fn strip3() -> (HWND, HWND, TabLayout) {
        let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 800, 600, None, None, None, None).unwrap();
        let tabs = create(parent, 7).unwrap();
        for t in ["a", "b", "c"] {
            add(tabs, t);
        }
        let _ = MoveWindow(tabs, 0, 0, 600, 36, false);
        (parent, tabs, layout(3, 600, GetDpiForWindow(tabs)))
    }

    fn center(r: RECT) -> (i32, i32) {
        ((r.left + r.right) / 2, (r.top + r.bottom) / 2)
    }

    #[test]
    fn drag_reorders() {
        unsafe {
            let (parent, tabs, l) = strip3();
            let (x0, y0) = center(l.rect(0));
            let (x2, y2) = center(l.rect(2));
            SendMessageW(tabs, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(lp(x0, y0)));
            SendMessageW(tabs, WM_MOUSEMOVE, Some(WPARAM(1)), Some(lp(x2, y2)));
            SendMessageW(tabs, WM_LBUTTONUP, Some(WPARAM(0)), Some(lp(x2, y2)));
            assert_eq!((*state(tabs)).titles, ["b", "c", "a"]);
            assert_eq!(active(tabs), 2);
            assert!((*state(tabs)).press.is_none());
            let _ = DestroyWindow(parent);
        }
    }

    thread_local!(static CODES: std::cell::RefCell<Vec<u32>> = Default::default());

    unsafe extern "system" fn record(h: HWND, m: u32, w: WPARAM, l: LPARAM, _: usize, _: usize) -> LRESULT {
        if m == WM_NOTIFY {
            CODES.with_borrow_mut(|c| c.push((*(l.0 as *const NMHDR)).code));
        }
        windows::Win32::UI::Shell::DefSubclassProc(h, m, w, l)
    }

    #[test]
    fn only_a_release_outside_the_window_tears() {
        unsafe {
            let (parent, tabs, l) = strip3();
            let _ = windows::Win32::UI::Shell::SetWindowSubclass(parent, Some(record), 1, 0);
            let (x, y) = center(l.rect(0));
            for to in [300, 900] {
                SendMessageW(tabs, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(lp(x, y)));
                SendMessageW(tabs, WM_MOUSEMOVE, Some(WPARAM(1)), Some(lp(x, to)));
                SendMessageW(tabs, WM_LBUTTONUP, Some(WPARAM(0)), Some(lp(x, to)));
            }
            // 300 px under the strip is still in the 600 px window; 900 is below it.
            assert_eq!(CODES.with_borrow(|c| c.iter().filter(|&&c| c == TN_TEAR).count()), 1);
            let _ = DestroyWindow(parent);
        }
    }

    #[test]
    fn click_without_move_keeps_order() {
        unsafe {
            let (parent, tabs, l) = strip3();
            set_active(tabs, 1);
            let (x, y) = center(l.rect(0));
            SendMessageW(tabs, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(lp(x, y)));
            SendMessageW(tabs, WM_LBUTTONUP, Some(WPARAM(0)), Some(lp(x, y)));
            assert_eq!((*state(tabs)).titles, ["a", "b", "c"]);
            assert_eq!(active(tabs), 0);
            let _ = DestroyWindow(parent);
        }
    }

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
            move_tab(tabs, 0, 1);
            assert_eq!(((*state(tabs)).titles.clone(), active(tabs)), (vec!["b".to_string(), "renamed".to_string()], 0));
            pump();
            remove(tabs, 0);
            assert_eq!((active(tabs), count(tabs)), (0, 1));
            pump();
            let _ = DestroyWindow(parent);
        }
    }
}
