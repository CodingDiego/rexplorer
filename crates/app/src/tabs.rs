//! Custom-drawn tab strip of pill-shaped tabs, window class "RexTabs".
//! The parent receives WM_NOTIFY with an `NmTab` (TN_SELECT / TN_CLOSE / TN_NEW); it owns adding and removing tabs.

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::Once;

use rexplorer_core::access;
use rexplorer_core::i18n::tr;
use rexplorer_core::settings::Theme;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{ROLE_SYSTEM_PAGETAB, ROLE_SYSTEM_PAGETABLIST, ROLE_SYSTEM_PUSHBUTTON};
use windows::Win32::UI::Controls::NMHDR;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetFocus, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VIRTUAL_KEY, VK_END, VK_ESCAPE, VK_HOME, VK_LBUTTON,
    VK_LEFT, VK_RBUTTON, VK_RETURN, VK_RIGHT,
};
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
/// A context menu asked by keyboard (Shift+F10, Menu key) for the active tab `index`; place it with `menu_point`.
pub const TN_CONTEXT_KEY: u32 = 0x9007;

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
    focused: bool,
    theme: Theme,
    font: HFONT,
    acc: Option<access::Server>,
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
            focused: false,
            theme: Theme::light(),
            font: HFONT::default(),
            acc: None,
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
    access::event(EVENT_OBJECT_REORDER, hwnd, None);
    n
}

pub fn remove(hwnd: HWND, index: usize) {
    let mut removed = false;
    with(hwnd, |s| {
        if index >= s.titles.len() {
            return;
        }
        removed = true;
        s.titles.remove(index);
        if s.active > index {
            s.active -= 1;
        }
        s.active = s.active.min(s.titles.len().saturating_sub(1));
        s.hover = Hit::None;
        s.press = None;
        s.dragging = false;
    });
    if removed {
        access::event(EVENT_OBJECT_REORDER, hwnd, None);
    }
}

/// Moves the title at `from` to `to`, the active index following; the parent moves its tab on TN_MOVE.
pub fn move_tab(hwnd: HWND, from: usize, to: usize) {
    let mut done = false;
    with(hwnd, |s| {
        if from >= s.titles.len() || to >= s.titles.len() || from == to {
            return;
        }
        done = true;
        let t = s.titles.remove(from);
        s.titles.insert(to, t);
        s.active = moved(s.active, from, to);
        s.hover = Hit::None;
    });
    if done {
        access::event(EVENT_OBJECT_REORDER, hwnd, None);
    }
}

pub fn set_title(hwnd: HWND, index: usize, title: &str) {
    let mut done = false;
    with(hwnd, |s| {
        if let Some(t) = s.titles.get_mut(index) {
            *t = title.to_string();
            done = true;
        }
    });
    if done {
        access::event(EVENT_OBJECT_NAMECHANGE, hwnd, Some(index));
    }
}

pub fn set_active(hwnd: HWND, index: usize) {
    let mut done = false;
    with(hwnd, |s| {
        if index < s.titles.len() && index != s.active {
            s.active = index;
            done = true;
        }
    });
    if done {
        access::event(EVENT_OBJECT_SELECTION, hwnd, Some(index));
    }
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

/// Reads only. The active tab is the keyboard-focused element.
unsafe fn acc_tree(hwnd: HWND, s: &State, t: &mut access::Tree) {
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let lay = layout(s.titles.len(), rc.right, theme::text_dpi(GetDpiForWindow(hwnd)));
    t.name = tr("tabs.name").into();
    t.role = ROLE_SYSTEM_PAGETABLIST;
    t.focusable = true;
    t.focus = (!s.titles.is_empty()).then_some(s.active);
    for (i, title) in s.titles.iter().enumerate() {
        t.items.push(access::Item {
            name: title.clone(),
            role: ROLE_SYSTEM_PAGETAB,
            state: STATE_SYSTEM_SELECTABLE | if i == s.active { STATE_SYSTEM_SELECTED } else { 0 },
            rect: lay.rect(i),
            action: tr("tb.action_switch"),
            ..Default::default()
        });
    }
    t.items.push(access::Item { name: tr("tabs.new_tab").into(), role: ROLE_SYSTEM_PUSHBUTTON, rect: lay.new_rect(), action: tr("tb.action_press"), ..Default::default() });
}

/// Screen point under tab `index` (its left, bottom edge) for a keyboard-opened menu.
pub fn menu_point(hwnd: HWND, index: usize) -> POINT {
    unsafe {
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let r = layout(count(hwnd), rc.right, theme::text_dpi(GetDpiForWindow(hwnd))).rect(index);
        let mut p = POINT { x: r.left, y: r.bottom };
        let _ = ClientToScreen(hwnd, &mut p);
        p
    }
}

unsafe fn hit_at(hwnd: HWND, lparam: LPARAM) -> Hit {
    let (x, y) = (lparam.0 as i16 as i32, (lparam.0 >> 16) as i16 as i32);
    let s = state(hwnd);
    let mut rc = RECT::default();
    if s.is_null() || GetClientRect(hwnd, &mut rc).is_err() {
        return Hit::None;
    }
    layout((*s).titles.len(), rc.right, theme::text_dpi(GetDpiForWindow(hwnd))).hit(x, y)
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
        match layout((*s).titles.len(), rc.right, theme::text_dpi(GetDpiForWindow(hwnd))).hit(p.x, p.y) {
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
        WM_GETOBJECT if access::is_client(lparam) => {
            let s = state(hwnd);
            if !s.is_null() {
                return (*s).acc.get_or_insert_with(|| access::Server::new(hwnd)).answer(wparam);
            }
        }
        access::WM_ACC_ITEMS => {
            let s = state(hwnd);
            if !s.is_null() {
                acc_tree(hwnd, &*s, &mut *(lparam.0 as *mut access::Tree));
                return LRESULT(1);
            }
        }
        // What a click on the tab (or the New button) does.
        access::WM_ACC_INVOKE => {
            let n = count(hwnd);
            if wparam.0 < n {
                set_active(hwnd, wparam.0);
                notify(hwnd, TN_SELECT, wparam.0, 0);
            } else if wparam.0 == n {
                notify(hwnd, TN_NEW, 0, 0);
            }
            return LRESULT(0);
        }
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
                        let to = layout(len, rc.right, theme::text_dpi(GetDpiForWindow(hwnd))).slot(x);
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
                if let Ok(parent) = GetParent(hwnd)
                    && ClientToScreen(hwnd, &mut pt).as_bool() && GetWindowRect(parent, &mut wr).is_ok() && torn_out(wr, pt.x, pt.y) {
                        notify(hwnd, TN_TEAR, i, 0);
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
        WM_SETFOCUS | WM_KILLFOCUS => {
            with(hwnd, |s| s.focused = msg == WM_SETFOCUS);
            if msg == WM_SETFOCUS && count(hwnd) > 0 {
                access::event(EVENT_OBJECT_FOCUS, hwnd, Some(active(hwnd)));
            }
        }
        WM_KEYDOWN => {
            let (n, cur) = (count(hwnd), active(hwnd));
            let to = match crate::rtl::key(VIRTUAL_KEY(wparam.0 as u16)) {
                VK_LEFT => cur.saturating_sub(1),
                VK_RIGHT => (cur + 1).min(n.saturating_sub(1)),
                VK_HOME => 0,
                VK_END => n.saturating_sub(1),
                VK_RETURN | VK_ESCAPE => {
                    // The main window hands the focus on to the view.
                    if let Ok(parent) = GetParent(hwnd) {
                        let _ = SetFocus(Some(parent));
                    }
                    return LRESULT(0);
                }
                _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
            };
            if n > 0 && to != cur {
                set_active(hwnd, to);
                notify(hwnd, TN_SELECT, to, 0);
                // Switching a tab moves the focus to the view; arrow keys keep working from here.
                if GetFocus() != hwnd {
                    let _ = SetFocus(Some(hwnd));
                }
                if count(hwnd) > to {
                    access::event(EVENT_OBJECT_FOCUS, hwnd, Some(to));
                }
            }
            return LRESULT(0);
        }
        // The keyboard's menu comes with (-1, -1) in the low 32 bits only.
        WM_CONTEXTMENU if lparam.0 as i32 == -1 => {
            if count(hwnd) > 0 {
                notify(hwnd, TN_CONTEXT_KEY, active(hwnd), 0);
            }
            return LRESULT(0);
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
    theme::draw_text(hdc, s, rc, fmt);
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
        crate::rtl::match_dc(dc, wdc);
        // The tabs hold titles: their size follows the text size, the glyphs only the DPI.
        draw(dc, s, &layout(s.titles.len(), cw, theme::text_dpi(dpi)), dpi, cw, ch);
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
            // Surface and background are the same color in a contrast theme: the pill needs an edge.
            if t.contrast.is_some() {
                theme::outline_round(dc, &pill(&r), t.text, radius, scale(2, dpi));
            }
        } else if hovered {
            theme::fill_round(dc, &pill(&r), theme::hover(t), radius);
            theme::hot_outline(dc, &pill(&r), t, radius, dpi);
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
    if s.focused && s.active < s.titles.len() {
        let r = lay.rect(s.active);
        let i = scale(3, dpi);
        SetTextColor(dc, COLORREF(t.text));
        SetBkColor(dc, COLORREF(t.background));
        let _ = DrawFocusRect(dc, &RECT { left: r.left + i, top: r.top + i, right: r.right - i, bottom: r.bottom - i });
    }

    let n = lay.new_rect();
    if s.hover == Hit::New {
        theme::fill_round(dc, &RECT { left: n.left + m, top: n.top + m, right: n.right - m, bottom: n.bottom - m }, theme::hover(t), radius);
        theme::hot_outline(dc, &RECT { left: n.left + m, top: n.top + m, right: n.right - m, bottom: n.bottom - m }, t, radius, dpi);
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
            let l = layout(2, 600, theme::text_dpi(GetDpiForWindow(tabs)));
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
        (parent, tabs, layout(3, 600, theme::text_dpi(GetDpiForWindow(tabs))))
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

    thread_local!(static NOTES: std::cell::RefCell<Vec<(u32, usize)>> = Default::default());

    unsafe extern "system" fn record_nm(h: HWND, m: u32, w: WPARAM, l: LPARAM, _: usize, _: usize) -> LRESULT {
        if m == WM_NOTIFY {
            NOTES.with_borrow_mut(|c| c.push(((*(l.0 as *const NMHDR)).code, (*(l.0 as *const NmTab)).index)));
        }
        windows::Win32::UI::Shell::DefSubclassProc(h, m, w, l)
    }

    #[test]
    fn keyboard_switches_and_menus() {
        use windows::Win32::UI::Input::KeyboardAndMouse::{VK_END, VK_HOME, VK_RIGHT};
        unsafe {
            let (parent, tabs, _) = strip3();
            let _ = windows::Win32::UI::Shell::SetWindowSubclass(parent, Some(record_nm), 2, 0);
            let key = |k| SendMessageW(tabs, WM_KEYDOWN, Some(WPARAM(k as usize)), None);
            let seen = || NOTES.with_borrow(|c| c.clone());
            assert_eq!(active(tabs), 0);
            key(VK_RIGHT.0);
            assert_eq!((active(tabs), seen()), (1, vec![(TN_SELECT, 1)]));
            key(VK_RIGHT.0);
            key(VK_RIGHT.0);
            assert_eq!((active(tabs), seen().len()), (2, 2), "the second Right is clamped and silent");
            key(VK_END.0);
            assert_eq!(seen().len(), 2, "End on the last tab notifies nothing");
            key(VK_HOME.0);
            assert_eq!((active(tabs), seen().len()), (0, 3));
            SendMessageW(tabs, WM_CONTEXTMENU, Some(WPARAM(0)), Some(LPARAM(-1)));
            assert_eq!(seen().last(), Some(&(TN_CONTEXT_KEY, 0)));
            SendMessageW(tabs, WM_CONTEXTMENU, Some(WPARAM(0)), Some(lp(5, 5)));
            assert_eq!(seen().len(), 4, "a pointer-placed menu message is the mouse's");
            // MAKELPARAM(-1, -1) zero-extended, as a 64-bit lparam can carry it, is the keyboard's too.
            SendMessageW(tabs, WM_CONTEXTMENU, Some(WPARAM(0)), Some(LPARAM(0xFFFF_FFFF)));
            assert_eq!(seen().len(), 5, "(-1, -1) in the low 32 bits is a keyboard menu");
            let _ = DestroyWindow(parent);
        }
    }

    #[test]
    fn menu_point_under_tab() {
        unsafe {
            let (parent, tabs, l) = strip3();
            let mut p = POINT { x: l.rect(1).left, y: l.rect(1).bottom };
            let _ = ClientToScreen(tabs, &mut p);
            let got = menu_point(tabs, 1);
            assert_eq!((got.x, got.y), (p.x, p.y));
            let _ = DestroyWindow(parent);
        }
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

    #[test]
    fn contrast_active_tab_is_outlined() {
        use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, COLOR_WINDOWTEXT};
        unsafe {
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 800, 600, None, None, None, None).unwrap();
            let tabs = create(parent, 7).unwrap();
            add(tabs, "a");
            add(tabs, "b");
            set_active(tabs, 1);
            let t = theme::effective_theme(&Theme::light(), true, |i| if i == COLOR_WINDOW { 0xFFFFFF } else if i == COLOR_WINDOWTEXT { 0 } else { 0x0000FF });
            set_theme(tabs, &t);
            let h = height(96);
            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: 400, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bmp = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
            let old = SelectObject(dc, bmp.into());
            let lay = layout(2, 400, 96);
            draw(dc, &*state(tabs), &lay, 96, 400, h);
            let _ = GdiFlush();
            let px = std::slice::from_raw_parts(bits as *const u32, (400 * h) as usize);
            let r = lay.rect(1);
            let (left, y) = (r.left + scale(4, 96) / 2, (r.top + r.bottom) / 2);
            let at = |x: i32| px[(y * 400 + x) as usize] & 0xFF_FFFF;
            assert_eq!((at(left), at(left + 1)), (0, 0), "the outline is the text color");
            assert_eq!(at(left + 6), 0xFF_FFFF, "inside the pill");
            SelectObject(dc, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(dc);
            let _ = DestroyWindow(parent);
        }
    }

    #[test]
    fn accessible_tabs_and_new_button() {
        use crate::acc_test::*;
        use rexplorer_core::com::ComInit;
        let _com = ComInit::sta();
        unsafe {
            let (parent, tabs, l) = strip3();
            let _ = windows::Win32::UI::Shell::SetWindowSubclass(parent, Some(record_nm), 2, 0);
            let acc = object(tabs);
            let k = kids(&acc);
            assert_eq!(k.iter().map(|k| k.name.as_str()).collect::<Vec<_>>(), ["a", "b", "c", "New tab"]);
            assert_eq!(k.iter().map(|k| k.role).collect::<Vec<_>>(), [ROLE_SYSTEM_PAGETAB, ROLE_SYSTEM_PAGETAB, ROLE_SYSTEM_PAGETAB, ROLE_SYSTEM_PUSHBUTTON]);
            assert_eq!(k.iter().map(|k| k.state & STATE_SYSTEM_SELECTED != 0).collect::<Vec<_>>(), [true, false, false, false]);
            #[allow(clippy::needless_range_loop)] // i also indexes l.rect(i), not only k
            for i in 0..3 {
                let r = screen(tabs, l.rect(i));
                assert_eq!((k[i].rect.left, k[i].rect.top, k[i].rect.right, k[i].rect.bottom), (r.left, r.top, r.right, r.bottom));
            }
            let r = screen(tabs, l.new_rect());
            assert_eq!((k[3].rect.left, k[3].rect.right), (r.left, r.right));
            let r0 = root(&acc);
            assert_eq!((r0.name.as_str(), r0.role), ("Tabs", ROLE_SYSTEM_PAGETABLIST));
            NOTES.with_borrow_mut(|c| c.clear());
            invoke(&acc, 2).unwrap();
            assert_eq!(NOTES.with_borrow(|c| c.clone()), [(TN_SELECT, 1)]);
            assert_eq!(active(tabs), 1);
            set_active(tabs, 0);
            NOTES.with_borrow_mut(|c| c.clear());
            let (x, y) = center(l.rect(1));
            SendMessageW(tabs, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(lp(x, y)));
            SendMessageW(tabs, WM_LBUTTONUP, Some(WPARAM(0)), Some(lp(x, y)));
            assert_eq!(NOTES.with_borrow(|c| c.clone()), [(TN_SELECT, 1)], "the same record as a click");
            NOTES.with_borrow_mut(|c| c.clear());
            invoke(&acc, 4).unwrap();
            assert_eq!(NOTES.with_borrow(|c| c.clone()), [(TN_NEW, 0)]);
            let hk = hook();
            set_active(tabs, 2);
            assert!(hk.take().contains(&(EVENT_OBJECT_SELECTION, tabs, 3)));
            set_active(tabs, 2);
            assert!(hk.take().is_empty(), "no event when nothing changed");
            set_title(tabs, 0, "x");
            assert!(hk.take().contains(&(EVENT_OBJECT_NAMECHANGE, tabs, 1)));
            add(tabs, "d");
            assert!(hk.take().contains(&(EVENT_OBJECT_REORDER, tabs, 0)));
            drop(hk);
            let _ = DestroyWindow(parent);
            assert_eq!(acc.accChildCount().unwrap_err().code(), windows::Win32::Foundation::CO_E_OBJNOTCONNECTED);
        }
    }
}
