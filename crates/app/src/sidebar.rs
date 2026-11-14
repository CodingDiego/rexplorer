//! Places sidebar, window class "RexSidebar": titled sections of rows (small icon + label), the current place
//! marked with a pill. A click sends WM_NOTIFY SN_PICK with an `NmSidebar` to the parent, which reads the row's
//! target with `target`; a right-click sends SN_CONTEXT the same way. Rows with an empty section title have no
//! header. Rows (and their icons) are built by the caller, typically on a worker thread.

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::Once;

use rexplorer_core::access;
use rexplorer_core::i18n::tr;
use rexplorer_core::settings::Theme;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{ROLE_SYSTEM_LIST, ROLE_SYSTEM_LISTITEM};
use windows::Win32::UI::Controls::{NMHDR, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VIRTUAL_KEY, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_NEXT, VK_PRIOR, VK_RETURN, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, Result, w};

use crate::theme;

pub const SN_PICK: u32 = 0x9031;
pub const SN_CONTEXT: u32 = 0x9032;
/// A context menu asked by keyboard (Shift+F10, Menu key) for the focused row; place it with `menu_point`.
pub const SN_CONTEXT_KEY: u32 = 0x9033;
/// Test hook (the smoke test clicks rows): returns the client-space top of row `wparam`, or -1.
pub const SM_ROW_TOP: u32 = WM_APP + 0x40;
/// Test hook: sets the window text to row `wparam`'s target (WM_GETTEXT reads it from another process) and
/// returns its length, or -1 when there is no such row.
pub const SM_ROW_TARGET: u32 = WM_APP + 0x41;

#[repr(C)]
pub struct NmSidebar {
    pub hdr: NMHDR,
    pub index: usize,
}

pub struct Row {
    /// Section title; consecutive rows with the same title form a section.
    pub section: &'static str,
    pub label: String,
    /// Parsing name of the place (navigable, and matched against the current folder).
    pub target: String,
    /// Owned by the sidebar once passed to `set_rows`.
    pub icon: Option<HICON>,
    /// Drawn with a pin at its end, as Explorer's navigation pane marks what Quick Access has pinned.
    pub pinned: bool,
}

// ---- pure model -----------------------------------------------------------------------------

const TOP: i32 = 8;
const HEADER: i32 = 28;
const ROW: i32 = 28;
const GAP: i32 = 8;
const INSET: i32 = 8;

fn scale(px: i32, dpi: u32) -> i32 {
    px * dpi as i32 / 96
}

/// The DPI the rows' heights (`layout`, scrolling) use: they hold text, so the Windows text size grows them.
fn row_dpi(hwnd: HWND) -> u32 {
    theme::text_dpi(unsafe { GetDpiForWindow(hwnd) })
}

/// Vertical extent of each row in content coordinates (left and right are 0: rows span the width).
/// Each section starts with a header; sections are `GAP` apart.
pub fn layout(rows: &[Row], dpi: u32) -> Vec<RECT> {
    let mut y = scale(TOP, dpi);
    let mut out = Vec::with_capacity(rows.len());
    for (i, r) in rows.iter().enumerate() {
        if i == 0 || rows[i - 1].section != r.section {
            y += if i > 0 { scale(GAP, dpi) } else { 0 } + if r.section.is_empty() { 0 } else { scale(HEADER, dpi) };
        }
        out.push(RECT { left: 0, top: y, right: 0, bottom: y + scale(ROW, dpi) });
        y += scale(ROW, dpi);
    }
    out
}

/// The keyboard row after `key`: Up / Down by one, Home / End, PageUp / PageDown by `page` rows, all clamped (no
/// wrap). With no row yet any of them lands on the first. None for other keys or no rows.
pub fn step(n: usize, focus: Option<usize>, key: VIRTUAL_KEY, page: usize) -> Option<usize> {
    if n == 0 || !matches!(key, VK_UP | VK_DOWN | VK_HOME | VK_END | VK_PRIOR | VK_NEXT) {
        return None;
    }
    let Some(f) = focus else { return Some(0) };
    Some(
        match key {
            VK_UP => f.saturating_sub(1),
            VK_DOWN => f + 1,
            VK_HOME => 0,
            VK_END => n - 1,
            VK_PRIOR => f.saturating_sub(page),
            _ => f + page,
        }
        .min(n - 1),
    )
}

/// The scroll that shows rows spanning `top..bottom` in a client `page` px tall, moving as little as it can.
pub fn shown(scroll: i32, top: i32, bottom: i32, page: i32) -> i32 {
    if top < scroll {
        top
    } else if bottom > scroll + page {
        bottom - page
    } else {
        scroll
    }
}

/// Row under content-space `y`.
pub fn hit(rects: &[RECT], y: i32) -> Option<usize> {
    rects.iter().position(|r| y >= r.top && y < r.bottom)
}

/// The row whose target is `current`, else the one that is its nearest ancestor (longest parsing-name prefix
/// ending at a path separator), compared case-insensitively.
pub fn best_match<'a>(targets: impl Iterator<Item = &'a str>, current: &str) -> Option<usize> {
    let cur = current.to_lowercase();
    targets
        .enumerate()
        .map(|(i, t)| (i, t.to_lowercase()))
        .filter(|(_, t)| {
            !t.is_empty() && (cur == *t || (cur.starts_with(t.as_str()) && (t.ends_with('\\') || cur[t.len()..].starts_with('\\'))))
        })
        .max_by_key(|(_, t)| t.len())
        .map(|(i, _)| i)
}

// ---- window ---------------------------------------------------------------------------------

struct State {
    rows: Vec<Row>,
    rects: Vec<RECT>,
    rects_dpi: u32,
    current: String,
    selected: Option<usize>,
    /// The keyboard row (the dotted outline), while the sidebar has the focus.
    focus: Option<usize>,
    typed: String,
    typed_at: u32,
    hover: Option<usize>,
    pressed: Option<usize>,
    tracking: bool,
    scroll: i32,
    theme: Theme,
    font: HFONT,
    header_font: HFONT,
    header_dpi: u32,
    pin_font: HFONT,
    acc: Option<access::Server>,
}

impl State {
    unsafe fn sync(&mut self, hwnd: HWND) {
        let dpi = row_dpi(hwnd);
        if self.rects_dpi != dpi || self.rects.len() != self.rows.len() {
            self.rects = layout(&self.rows, dpi);
            self.rects_dpi = dpi;
        }
        // Keep the scroll inside the content.
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let content = self.rects.last().map_or(0, |r| r.bottom + scale(TOP, dpi));
        self.scroll = self.scroll.clamp(0, (content - rc.bottom).max(0));
    }

    fn reselect(&mut self) {
        self.selected = best_match(self.rows.iter().map(|r| r.target.as_str()), &self.current);
    }
}

fn destroy_icons(rows: &[Row]) {
    for i in rows.iter().filter_map(|r| r.icon) {
        let _ = unsafe { DestroyIcon(i) };
    }
}

unsafe fn state<'a>(hwnd: HWND) -> Option<&'a mut State> {
    (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State).as_mut()
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
                lpszClassName: w!("RexSidebar"),
                ..Default::default()
            };
            RegisterClassW(&wc);
        });
        // Freed by WM_NCDESTROY.
        let st = Box::into_raw(Box::new(State {
            rows: Vec::new(),
            rects: Vec::new(),
            rects_dpi: 0,
            current: String::new(),
            selected: None,
            focus: None,
            typed: String::new(),
            typed_at: 0,
            hover: None,
            pressed: None,
            tracking: false,
            scroll: 0,
            theme: Theme::light(),
            font: HFONT::default(),
            header_font: HFONT::default(),
            header_dpi: 0,
            pin_font: HFONT::default(),
            acc: None,
        }));
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("RexSidebar"),
            PCWSTR::null(),
            WS_CHILD | WS_CLIPSIBLINGS,
            0,
            0,
            0,
            0,
            Some(parent),
            Some(HMENU(id as *mut c_void)),
            Some(hinst),
            Some(st as *const c_void),
        )
    }
}

pub fn set_theme(hwnd: HWND, t: &Theme) {
    unsafe {
        if let Some(s) = state(hwnd) {
            s.theme = t.clone();
            s.header_dpi = 0; // the header font follows the theme's face
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

/// Replaces the rows and takes ownership of their icons. Rows equal to the current ones (same sections, labels,
/// targets and pins) change nothing: their new icons are destroyed and the sidebar does not repaint.
pub fn set_rows(hwnd: HWND, rows: Vec<Row>) {
    unsafe {
        let Some(s) = state(hwnd) else { return destroy_icons(&rows) };
        let key = |r: &Row| (r.section, r.label.clone(), r.target.clone(), r.pinned);
        if s.rows.len() == rows.len() && s.rows.iter().zip(&rows).all(|(a, b)| key(a) == key(b)) {
            return destroy_icons(&rows);
        }
        destroy_icons(&s.rows);
        s.rows = rows;
        s.rects.clear();
        s.focus = s.focus.filter(|&f| f < s.rows.len());
        s.hover = None;
        s.pressed = None;
        s.reselect();
        let _ = InvalidateRect(Some(hwnd), None, false);
        access::event(EVENT_OBJECT_REORDER, hwnd, None);
    }
}

/// Marks the row for `current` (a parsing name) or its nearest ancestor.
pub fn select(hwnd: HWND, current: &str) {
    unsafe {
        if let Some(s) = state(hwnd) {
            s.current = current.to_string();
            let before = s.selected;
            s.reselect();
            if s.selected != before {
                let _ = InvalidateRect(Some(hwnd), None, false);
                if let Some(i) = s.selected {
                    access::event(EVENT_OBJECT_SELECTION, hwnd, Some(i));
                }
            }
        }
    }
}

/// The parsing name behind row `index`, empty when there is no such row.
pub fn target(hwnd: HWND, index: usize) -> String {
    unsafe { state(hwnd).and_then(|s| s.rows.get(index)).map(|r| r.target.clone()).unwrap_or_default() }
}

/// Row under screen point (x, y).
pub fn row_at_screen(hwnd: HWND, x: i32, y: i32) -> Option<usize> {
    unsafe {
        let s = state(hwnd)?;
        let mut p = POINT { x, y };
        let _ = ScreenToClient(hwnd, &mut p);
        s.sync(hwnd);
        hit(&s.rects, p.y + s.scroll)
    }
}

/// Row `index`'s place on screen for a keyboard-opened menu: its bottom-left, or the client's top-left when it is
/// scrolled out.
pub fn menu_point(hwnd: HWND, index: usize) -> POINT {
    unsafe {
        let mut p = POINT::default();
        if let Some(s) = state(hwnd) {
            s.sync(hwnd);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            if let Some(r) = s.rects.get(index).filter(|r| r.bottom - s.scroll > 0 && r.top - s.scroll < rc.bottom) {
                p = POINT { x: scale(INSET, GetDpiForWindow(hwnd)), y: r.bottom - s.scroll };
            }
        }
        let _ = ClientToScreen(hwnd, &mut p);
        p
    }
}

/// Tells the parent about row `index`. Nothing may use the sidebar's state afterwards: the parent can destroy us.
unsafe fn notify(hwnd: HWND, code: u32, index: usize) {
    if let Ok(parent) = GetParent(hwnd) {
        let id = GetDlgCtrlID(hwnd) as usize;
        let nm = NmSidebar { hdr: NMHDR { hwndFrom: hwnd, idFrom: id, code }, index };
        access::send_own(parent, WM_NOTIFY, WPARAM(id), LPARAM(&nm as *const _ as isize));
    }
}

/// Scrolls the keyboard row into view.
unsafe fn reveal(hwnd: HWND, s: &mut State) {
    s.sync(hwnd);
    let Some((i, r)) = s.focus.and_then(|i| s.rects.get(i).map(|r| (i, *r))) else { return };
    let dpi = row_dpi(hwnd);
    let starts_section = (i == 0 || s.rows[i - 1].section != s.rows[i].section) && !s.rows[i].section.is_empty();
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    s.scroll = shown(s.scroll, r.top - if starts_section { scale(HEADER, dpi) } else { 0 }, r.bottom, rc.bottom);
    s.sync(hwnd);
}

/// Reads only: a client may ask while the sidebar is in the middle of anything, so no `sync`.
unsafe fn acc_tree(hwnd: HWND, s: &State, t: &mut access::Tree) {
    let dpi = GetDpiForWindow(hwnd);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    t.name = tr("side.name").into();
    t.role = ROLE_SYSTEM_LIST;
    t.focusable = true;
    t.focus = s.focus;
    for (r, row) in layout(&s.rows, row_dpi(hwnd)).iter().zip(&s.rows) {
        let i = t.items.len();
        t.items.push(access::Item {
            name: row.label.clone(),
            description: if row.pinned { format!("{}, {}", row.section, tr("side.pinned")) } else { row.section.to_string() },
            role: ROLE_SYSTEM_LISTITEM,
            state: STATE_SYSTEM_SELECTABLE | if s.selected == Some(i) { STATE_SYSTEM_SELECTED } else { 0 },
            rect: RECT { left: scale(INSET, dpi), top: r.top - s.scroll, right: rc.right - scale(INSET, dpi), bottom: r.bottom - s.scroll },
            action: tr("tb.action_open"),
            ..Default::default()
        });
    }
}

unsafe fn row_at(hwnd: HWND, s: &mut State, lparam: LPARAM) -> Option<usize> {
    s.sync(hwnd);
    hit(&s.rects, (lparam.0 >> 16) as i16 as i32 + s.scroll)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lparam.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    if msg == WM_NCDESTROY {
        let p = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut State;
        if !p.is_null() {
            let s = Box::from_raw(p);
            destroy_icons(&s.rows);
            for f in [s.header_font, s.pin_font] {
                if !f.is_invalid() {
                    let _ = DeleteObject(f.into());
                }
            }
        }
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let Some(s) = state(hwnd) else { return DefWindowProcW(hwnd, msg, wparam, lparam) };
    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        WM_SETFONT => {
            s.font = HFONT(wparam.0 as *mut c_void);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        WM_PAINT => paint(hwnd, s),
        WM_GETOBJECT if access::is_client(lparam) => return s.acc.get_or_insert_with(|| access::Server::new(hwnd)).answer(wparam),
        access::WM_ACC_ITEMS if access::own_send(hwnd, msg, lparam) => {
            acc_tree(hwnd, s, &mut *(lparam.0 as *mut access::Tree));
            return LRESULT(1);
        }
        access::WM_ACC_INVOKE => {
            if wparam.0 < s.rows.len() {
                notify(hwnd, SN_PICK, wparam.0);
            }
        }
        WM_SIZE => {
            s.sync(hwnd);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        SM_ROW_TARGET => {
            let Some(r) = s.rows.get(wparam.0) else { return LRESULT(-1) };
            let t = windows::core::HSTRING::from(r.target.as_str());
            let _ = SetWindowTextW(hwnd, PCWSTR(t.as_ptr()));
            return LRESULT(t.len() as isize);
        }
        SM_ROW_TOP => {
            s.sync(hwnd);
            return LRESULT(s.rects.get(wparam.0).map_or(-1, |r| (r.top - s.scroll) as isize));
        }
        WM_MOUSEWHEEL => {
            let delta = (wparam.0 >> 16) as i16 as i32;
            s.scroll -= delta * 3 * scale(ROW, row_dpi(hwnd)) / 120;
            s.sync(hwnd);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        WM_MOUSEMOVE => {
            if !s.tracking {
                let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                s.tracking = TrackMouseEvent(&mut tme).is_ok();
            }
            let h = row_at(hwnd, s, lparam);
            if s.hover != h {
                s.hover = h;
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
        WM_MOUSELEAVE => {
            s.tracking = false;
            s.hover = None;
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        WM_LBUTTONDOWN => {
            // As Explorer's navigation pane: a click takes the keyboard focus, onto the row clicked.
            s.pressed = row_at(hwnd, s, lparam);
            if s.pressed.is_some() {
                s.focus = s.pressed;
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
            let _ = SetFocus(Some(hwnd));
        }
        WM_LBUTTONUP | WM_RBUTTONUP => {
            let (picked, code) = if msg == WM_LBUTTONUP {
                (s.pressed.take().filter(|&i| row_at(hwnd, s, lparam) == Some(i)), SN_PICK)
            } else {
                (row_at(hwnd, s, lparam), SN_CONTEXT)
            };
            if let Some(index) = picked {
                notify(hwnd, code, index);
            }
        }
        WM_SETFOCUS => {
            s.focus = s.focus.or(s.selected).or(if s.rows.is_empty() { None } else { Some(0) });
            let _ = InvalidateRect(Some(hwnd), None, false);
            if let Some(i) = s.focus {
                access::event(EVENT_OBJECT_FOCUS, hwnd, Some(i));
            }
        }
        WM_KILLFOCUS => {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        WM_KEYDOWN => {
            let key = VIRTUAL_KEY(wparam.0 as u16);
            if key == VK_RETURN {
                if let Some(i) = s.focus {
                    notify(hwnd, SN_PICK, i);
                }
            } else if key == VK_ESCAPE {
                // The main window hands the focus on to the view.
                if let Ok(parent) = GetParent(hwnd) {
                    let _ = SetFocus(Some(parent));
                }
            } else {
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let page = (rc.bottom / scale(ROW, row_dpi(hwnd)).max(1)).max(1) as usize;
                // Keys it does not handle (Shift+F10 among them) go on to DefWindowProc.
                let Some(f) = step(s.rows.len(), s.focus, key, page) else { return DefWindowProcW(hwnd, msg, wparam, lparam) };
                s.focus = Some(f);
                reveal(hwnd, s);
                let _ = InvalidateRect(Some(hwnd), None, false);
                access::event(EVENT_OBJECT_FOCUS, hwnd, Some(f));
            }
        }
        // The keyboard's menu comes with (-1, -1) in the low 32 bits only.
        WM_CONTEXTMENU if lparam.0 as i32 == -1 && s.focus.is_some() => {
            if let Some(i) = s.focus {
                notify(hwnd, SN_CONTEXT_KEY, i);
            }
        }
        WM_CHAR => {
            let now = GetMessageTime() as u32;
            let (typed, needle) = crate::list::type_ahead(&s.typed, s.typed_at, now, char::from_u32(wparam.0 as u32).unwrap_or('\0'));
            if !needle.is_empty() {
                s.typed = typed;
                s.typed_at = now;
                let start = if needle.chars().count() == 1 { s.focus.map_or(0, |f| f + 1) } else { s.focus.unwrap_or(0) };
                if let Some(i) = crate::list::find_prefix(s.rows.len(), start, true, &needle, |i| s.rows[i].label.clone()) {
                    s.focus = Some(i);
                    reveal(hwnd, s);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    access::event(EVENT_OBJECT_FOCUS, hwnd, Some(i));
                }
            }
        }
        _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
    }
    LRESULT(0)
}

unsafe fn paint(hwnd: HWND, s: &mut State) {
    let mut ps = PAINTSTRUCT::default();
    let wdc = BeginPaint(hwnd, &mut ps);
    s.sync(hwnd);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let (cw, ch) = (rc.right.max(1), rc.bottom.max(1));
    let (dpi, tdpi) = (GetDpiForWindow(hwnd), row_dpi(hwnd));
    // Keyed on the text DPI so a text size change rebuilds the font too.
    if s.header_dpi != tdpi {
        for f in [s.header_font, s.pin_font] {
            if !f.is_invalid() {
                let _ = DeleteObject(f.into());
            }
        }
        s.header_font = theme::header_font(&s.theme, dpi, 8);
        s.pin_font = theme::icon_font(tdpi, 10);
        s.header_dpi = tdpi;
    }
    let dc = CreateCompatibleDC(Some(wdc));
    let bmp = CreateCompatibleBitmap(wdc, cw, ch);
    let old_bmp = SelectObject(dc, bmp.into());
    crate::rtl::match_dc(dc, wdc);
    let t = &s.theme;
    let bg = CreateSolidBrush(COLORREF(t.background));
    FillRect(dc, &rc, bg);
    let _ = DeleteObject(bg.into());
    SetBkMode(dc, TRANSPARENT);
    let font = if s.font.is_invalid() { HFONT(GetStockObject(DEFAULT_GUI_FONT).0) } else { s.font };
    let old_font = SelectObject(dc, font.into());
    let sc = |v: i32| scale(v, dpi);
    let (inset, icon) = (sc(INSET), sc(16));
    let focused = GetFocus() == hwnd;
    for (i, row) in s.rows.iter().enumerate() {
        let Some(r) = s.rects.get(i) else { break };
        let (top, bottom) = (r.top - s.scroll, r.bottom - s.scroll);
        if bottom < 0 || top - scale(HEADER, tdpi) > ch {
            continue;
        }
        if (i == 0 || s.rows[i - 1].section != row.section) && !row.section.is_empty() {
            SelectObject(dc, s.header_font.into());
            SetTextColor(dc, COLORREF(t.secondary_text));
            let mut hr = RECT { left: 2 * inset, top: top - scale(HEADER, tdpi), right: cw - inset, bottom: top - scale(4, tdpi) };
            let mut text: Vec<u16> = row.section.encode_utf16().collect();
            DrawTextW(dc, &mut text, &mut hr, crate::rtl::text(DT_SINGLELINE | DT_BOTTOM | DT_NOPREFIX | DT_END_ELLIPSIS));
            SelectObject(dc, font.into());
        }
        let pill = RECT { left: inset, top, right: cw - inset, bottom };
        if s.selected == Some(i) {
            theme::fill_round(dc, &pill, t.selection, sc(6));
        } else if s.hover == Some(i) {
            theme::fill_round(dc, &pill, theme::hover(t), sc(6));
            theme::hot_outline(dc, &pill, t, sc(6), dpi);
        }
        if let Some(h) = row.icon {
            let _ = DrawIconEx(dc, pill.left + inset, top + (bottom - top - icon) / 2, h, icon, icon, 0, None, DI_NORMAL | DI_NOMIRROR);
        }
        let mut lr = RECT { left: pill.left + inset + icon + inset, top, right: pill.right - inset, bottom };
        if row.pinned {
            let mut pr = RECT { left: lr.right - icon, ..lr };
            lr.right = pr.left - inset / 2;
            SelectObject(dc, s.pin_font.into());
            SetTextColor(dc, COLORREF(if s.selected == Some(i) { theme::on_selection(t, t.secondary_text) } else { t.secondary_text }));
            DrawTextW(dc, &mut [0xE718], &mut pr, DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX);
            SelectObject(dc, font.into());
        }
        SetTextColor(dc, COLORREF(if s.selected == Some(i) { theme::on_selection(t, t.text) } else { t.text }));
        theme::draw_text(dc, &row.label, &mut lr, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
        if focused && s.focus == Some(i) {
            // The system's XOR rectangle shows on any fill, in a contrast theme too (as the folder options list does).
            SetTextColor(dc, COLORREF(t.text));
            SetBkColor(dc, COLORREF(t.background));
            let _ = DrawFocusRect(dc, &RECT { left: pill.left + 1, top: pill.top + 1, right: pill.right - 1, bottom: pill.bottom - 1 });
        }
    }
    SelectObject(dc, old_font);
    let _ = BitBlt(wdc, 0, 0, cw, ch, Some(dc), 0, 0, SRCCOPY);
    SelectObject(dc, old_bmp);
    let _ = DeleteObject(bmp.into());
    let _ = DeleteDC(dc);
    let _ = EndPaint(hwnd, &ps);
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_A;

    #[test]
    fn row_at_screen_maps_screen_points() {
        use windows::Win32::Graphics::Gdi::ClientToScreen;
        unsafe {
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 800, 600, None, None, None, None).unwrap();
            let sb = create(parent, 7).unwrap();
            let _ = MoveWindow(sb, 0, 0, 220, 400, false);
            set_rows(sb, vec![row("Favorites", "C:/Users"), row("This PC", "C:\\")]);
            let r = layout(&state(sb).unwrap().rows, row_dpi(sb));
            let screen = |x: i32, y: i32| {
                let mut p = windows::Win32::Foundation::POINT { x, y };
                let _ = ClientToScreen(sb, &mut p);
                (p.x, p.y)
            };
            let (x, y) = screen(10, r[1].top + 2);
            assert_eq!(row_at_screen(sb, x, y), Some(1));
            let (x, y) = screen(10, 2);
            assert_eq!(row_at_screen(sb, x, y), None, "the first header");
            let _ = DestroyWindow(parent);
        }
    }

    fn row(section: &'static str, target: &str) -> Row {
        Row { section, label: target.to_string(), target: target.to_string(), icon: None, pinned: false }
    }

    #[test]
    fn step_rules() {
        let k = |n, f, key: u16, page| step(n, f, VIRTUAL_KEY(key), page);
        assert_eq!(k(5, None, VK_DOWN.0, 3), Some(0));
        assert_eq!(k(5, Some(0), VK_UP.0, 3), Some(0));
        assert_eq!(k(5, Some(4), VK_DOWN.0, 3), Some(4));
        assert_eq!(k(5, Some(2), VK_HOME.0, 3), Some(0));
        assert_eq!(k(5, Some(2), VK_END.0, 3), Some(4));
        assert_eq!(k(5, Some(1), VK_NEXT.0, 3), Some(4));
        assert_eq!(k(5, Some(4), VK_PRIOR.0, 3), Some(1));
        assert_eq!(k(0, None, VK_DOWN.0, 3), None);
        assert_eq!(k(5, Some(1), VK_A.0, 3), None);
    }

    #[test]
    fn shown_rules() {
        assert_eq!(shown(0, 100, 128, 80), 48);
        assert_eq!(shown(200, 100, 128, 80), 100);
        assert_eq!(shown(90, 100, 128, 80), 90);
    }

    thread_local!(static NOTES: std::cell::RefCell<Vec<(u32, usize)>> = Default::default());

    unsafe extern "system" fn record(h: HWND, m: u32, w: WPARAM, l: LPARAM, _: usize, _: usize) -> LRESULT {
        if m == WM_NOTIFY {
            NOTES.with_borrow_mut(|c| c.push(((*(l.0 as *const NMHDR)).code, (*(l.0 as *const NmSidebar)).index)));
        }
        windows::Win32::UI::Shell::DefSubclassProc(h, m, w, l)
    }

    #[test]
    fn keyboard_moves_picks_and_menus() {
        unsafe {
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 800, 600, None, None, None, None).unwrap();
            let sb = create(parent, 7).unwrap();
            let _ = MoveWindow(sb, 0, 0, 220, 400, false);
            let _ = windows::Win32::UI::Shell::SetWindowSubclass(parent, Some(record), 1, 0);
            set_rows(sb, vec![row("", "Alpha"), row("", "Beta"), row("", "Bravo")]);
            let key = |k: VIRTUAL_KEY| SendMessageW(sb, WM_KEYDOWN, Some(WPARAM(k.0 as usize)), None);
            key(VK_DOWN);
            key(VK_DOWN);
            assert_eq!(state(sb).unwrap().focus, Some(1));
            key(VK_RETURN);
            SendMessageW(sb, WM_CONTEXTMENU, Some(WPARAM(0)), Some(LPARAM(-1)));
            SendMessageW(sb, WM_CONTEXTMENU, Some(WPARAM(0)), Some(LPARAM(((5i32 as isize) << 16) | 5)));
            assert_eq!(NOTES.with_borrow(|c| c.clone()), vec![(SN_PICK, 1), (SN_CONTEXT_KEY, 1)]);
            // MAKELPARAM(-1, -1) zero-extended, as a 64-bit lparam can carry it, is the keyboard's too.
            SendMessageW(sb, WM_CONTEXTMENU, Some(WPARAM(0)), Some(LPARAM(0xFFFF_FFFF)));
            assert_eq!(NOTES.with_borrow(|c| c.len()), 3, "(-1, -1) in the low 32 bits is a keyboard menu");
            SendMessageW(sb, WM_CHAR, Some(WPARAM('b' as usize)), None);
            assert_eq!(state(sb).unwrap().focus, Some(2));
            SendMessageW(sb, WM_CHAR, Some(WPARAM('b' as usize)), None);
            assert_eq!(state(sb).unwrap().focus, Some(1));
            set_rows(sb, vec![row("", "Alpha")]);
            assert_eq!(state(sb).unwrap().focus, None);
            let _ = DestroyWindow(parent);
        }
    }

    #[test]
    fn layout_sections_and_hit() {
        let rows = [row("Favorites", "a"), row("Favorites", "b"), row("This PC", "C:\\")];
        let r = layout(&rows, 96);
        // 8 top + 28 header, then rows of 28; the next section adds 8 + 28.
        assert_eq!((r[0].top, r[0].bottom), (36, 64));
        assert_eq!(r[1].top, 64);
        assert_eq!(r[2].top, 92 + 8 + 28);
        assert_eq!(layout(&rows, 192)[0].top, 72);
        assert_eq!(hit(&r, 36), Some(0));
        assert_eq!(hit(&r, 91), Some(1));
        assert_eq!(hit(&r, 100), None, "a section header is no row");
        assert_eq!(hit(&r, 10), None);
        let top = [row("", "home"), row("Favorites", "a")];
        let r = layout(&top, 96);
        assert_eq!((r[0].top, r[1].top), (8, 36 + 8 + 28), "an untitled first row has no header");
    }

    #[test]
    fn current_folder_or_nearest_ancestor() {
        let t = ["C:\\Users\\v\\Desktop", "C:\\", "D:\\", "C:\\Users\\v\\Desk", "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"];
        let m = |cur: &str| best_match(t.iter().copied(), cur);
        assert_eq!(m("c:\\users\\V\\desktop"), Some(0));
        assert_eq!(m("C:\\Users\\v\\Desktop\\x\\y"), Some(0));
        assert_eq!(m("C:\\Users\\v\\Desktop2"), Some(1), "a name prefix is no ancestor");
        assert_eq!(m("C:\\Windows"), Some(1));
        assert_eq!(m("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"), Some(4));
        assert_eq!(m("\\\\server\\share"), None);
    }

    #[test]
    fn window_rows_select_and_row_tops() {
        unsafe {
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 800, 600, None, None, None, None).unwrap();
            let sb = create(parent, 7).unwrap();
            let _ = MoveWindow(sb, 0, 0, 220, 400, false);
            set_theme(sb, &Theme::dark());
            select(sb, "C:\\Windows");
            set_rows(sb, vec![row("Favorites", "C:\\Users"), row("This PC", "C:\\")]);
            assert_eq!(state(sb).unwrap().selected, Some(1), "selection follows the rows");
            select(sb, "C:\\Users\\x");
            assert_eq!(state(sb).unwrap().selected, Some(0));
            assert_eq!(target(sb, 1), "C:\\");
            assert_eq!(target(sb, 9), "");
            assert_eq!(SendMessageW(sb, SM_ROW_TOP, Some(WPARAM(0)), None).0 as i32, layout(&state(sb).unwrap().rows, row_dpi(sb))[0].top);
            assert_eq!(SendMessageW(sb, SM_ROW_TOP, Some(WPARAM(5)), None).0, -1);
            assert_eq!(SendMessageW(sb, SM_ROW_TARGET, Some(WPARAM(1)), None).0, 3);
            let mut buf = [0u16; 8];
            let n = GetWindowTextW(sb, &mut buf) as usize;
            assert_eq!(String::from_utf16_lossy(&buf[..n]), "C:\\");
            assert_eq!(SendMessageW(sb, SM_ROW_TARGET, Some(WPARAM(5)), None).0, -1);
            set_rows(sb, vec![row("Favorites", "C:\\Users"), row("This PC", "C:\\")]);
            assert_eq!(state(sb).unwrap().rows.len(), 2);
            let _ = DestroyWindow(parent);
        }
    }

    thread_local! {
        static ACC_NOTES: std::cell::RefCell<Vec<(u32, usize)>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    unsafe extern "system" fn rec_parent(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_NOTIFY {
            let nm = &*(lparam.0 as *const NmSidebar);
            ACC_NOTES.with(|n| n.borrow_mut().push((nm.hdr.code, nm.index)));
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    #[test]
    fn accessible_rows() {
        use crate::acc_test::*;
        use rexplorer_core::com::ComInit;
        let _com = ComInit::sta();
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap().into();
            let wc = WNDCLASSW { lpfnWndProc: Some(rec_parent), hInstance: hinst, lpszClassName: w!("RexSidebarAccParent"), ..Default::default() };
            RegisterClassW(&wc);
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("RexSidebarAccParent"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 800, 600, None, None, Some(hinst), None).unwrap();
            let sb = create(parent, 7).unwrap();
            let _ = MoveWindow(sb, 0, 0, 220, 400, false);
            set_rows(sb, vec![row("Favorites", "C:\\Users"), row("This PC", "C:\\")]);
            select(sb, "C:\\Users\\x");
            let acc = object(sb);
            let k = kids(&acc);
            assert_eq!(k.iter().map(|k| k.name.as_str()).collect::<Vec<_>>(), ["C:\\Users", "C:\\"]);
            assert_eq!(k.iter().map(|k| k.description.as_str()).collect::<Vec<_>>(), ["Favorites", "This PC"]);
            assert!(k.iter().all(|k| k.role == ROLE_SYSTEM_LISTITEM));
            assert_ne!(k[0].state & STATE_SYSTEM_SELECTED, 0);
            assert_eq!(k[1].state & STATE_SYSTEM_SELECTED, 0);
            let dpi = GetDpiForWindow(sb);
            let r = layout(&state(sb).unwrap().rows, row_dpi(sb));
            let want = screen(sb, RECT { left: scale(INSET, dpi), top: r[1].top, right: 220 - scale(INSET, dpi), bottom: r[1].bottom });
            assert_eq!((k[1].rect.left, k[1].rect.top, k[1].rect.right, k[1].rect.bottom), (want.left, want.top, want.right, want.bottom));
            let r0 = root(&acc);
            assert_eq!((r0.name.as_str(), r0.role), ("Navigation pane", ROLE_SYSTEM_LIST));
            ACC_NOTES.with(|n| n.borrow_mut().clear());
            invoke(&acc, 2).unwrap();
            assert_eq!(ACC_NOTES.with(|n| n.borrow().clone()), [(SN_PICK, 1)]);
            ACC_NOTES.with(|n| n.borrow_mut().clear());
            let y = SendMessageW(sb, SM_ROW_TOP, Some(WPARAM(1)), None).0 as isize + 2;
            SendMessageW(sb, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(LPARAM(10 | (y << 16))));
            SendMessageW(sb, WM_LBUTTONUP, Some(WPARAM(0)), Some(LPARAM(10 | (y << 16))));
            assert_eq!(ACC_NOTES.with(|n| n.borrow().clone()), [(SN_PICK, 1)], "the same record as a click");
            assert_eq!(state(sb).unwrap().focus, Some(1), "a click moves the keyboard to the row clicked");
            let hk = hook();
            select(sb, "C:\\");
            assert!(hk.take().contains(&(EVENT_OBJECT_SELECTION, sb, 2)));
            drop(hk);
            let _ = DestroyWindow(parent);
            assert_eq!(acc.accChildCount().unwrap_err().code(), windows::Win32::Foundation::CO_E_OBJNOTCONNECTED);
        }
    }
}
