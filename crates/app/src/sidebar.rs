//! Places sidebar, window class "RexSidebar": titled sections of rows (small icon + label), the current place
//! marked with a pill. A click sends WM_NOTIFY SN_PICK with an `NmSidebar` to the parent, which reads the row's
//! target with `target`; a right-click sends SN_CONTEXT the same way. Rows with an empty section title have no
//! header. Rows (and their icons) are built by the caller, typically on a worker thread.

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::Once;

use rexplorer_core::settings::Theme;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{NMHDR, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, Result, w};

use crate::theme;

pub const SN_PICK: u32 = 0x9031;
pub const SN_CONTEXT: u32 = 0x9032;
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
    hover: Option<usize>,
    pressed: Option<usize>,
    tracking: bool,
    scroll: i32,
    theme: Theme,
    font: HFONT,
    header_font: HFONT,
    header_dpi: u32,
}

impl State {
    unsafe fn sync(&mut self, hwnd: HWND) {
        let dpi = GetDpiForWindow(hwnd);
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
            hover: None,
            pressed: None,
            tracking: false,
            scroll: 0,
            theme: Theme::light(),
            font: HFONT::default(),
            header_font: HFONT::default(),
            header_dpi: 0,
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

/// Replaces the rows and takes ownership of their icons. Rows equal to the current ones (same sections, labels
/// and targets) change nothing: their new icons are destroyed and the sidebar does not repaint.
pub fn set_rows(hwnd: HWND, rows: Vec<Row>) {
    unsafe {
        let Some(s) = state(hwnd) else { return destroy_icons(&rows) };
        let key = |r: &Row| (r.section, r.label.clone(), r.target.clone());
        if s.rows.len() == rows.len() && s.rows.iter().zip(&rows).all(|(a, b)| key(a) == key(b)) {
            return destroy_icons(&rows);
        }
        destroy_icons(&s.rows);
        s.rows = rows;
        s.rects.clear();
        s.hover = None;
        s.pressed = None;
        s.reselect();
        let _ = InvalidateRect(Some(hwnd), None, false);
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
            }
        }
    }
}

/// The parsing name behind row `index`, empty when there is no such row.
pub fn target(hwnd: HWND, index: usize) -> String {
    unsafe { state(hwnd).and_then(|s| s.rows.get(index)).map(|r| r.target.clone()).unwrap_or_default() }
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
            if !s.header_font.is_invalid() {
                let _ = DeleteObject(s.header_font.into());
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
            s.scroll -= delta * 3 * scale(ROW, GetDpiForWindow(hwnd)) / 120;
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
        WM_LBUTTONDOWN => s.pressed = row_at(hwnd, s, lparam),
        WM_LBUTTONUP | WM_RBUTTONUP => {
            let (picked, code) = if msg == WM_LBUTTONUP {
                (s.pressed.take().filter(|&i| row_at(hwnd, s, lparam) == Some(i)), SN_PICK)
            } else {
                (row_at(hwnd, s, lparam), SN_CONTEXT)
            };
            // No state reference is used after notifying: the parent may do anything, including destroying us.
            if let (Some(index), Ok(parent)) = (picked, GetParent(hwnd)) {
                let id = GetDlgCtrlID(hwnd) as usize;
                let nm = NmSidebar { hdr: NMHDR { hwndFrom: hwnd, idFrom: id, code }, index };
                SendMessageW(parent, WM_NOTIFY, Some(WPARAM(id)), Some(LPARAM(&nm as *const _ as isize)));
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
    let dpi = GetDpiForWindow(hwnd);
    if s.header_dpi != dpi {
        if !s.header_font.is_invalid() {
            let _ = DeleteObject(s.header_font.into());
        }
        s.header_font = theme::header_font(&s.theme, dpi, 8);
        s.header_dpi = dpi;
    }
    let dc = CreateCompatibleDC(Some(wdc));
    let bmp = CreateCompatibleBitmap(wdc, cw, ch);
    let old_bmp = SelectObject(dc, bmp.into());
    let t = &s.theme;
    let bg = CreateSolidBrush(COLORREF(t.background));
    FillRect(dc, &rc, bg);
    let _ = DeleteObject(bg.into());
    SetBkMode(dc, TRANSPARENT);
    let font = if s.font.is_invalid() { HFONT(GetStockObject(DEFAULT_GUI_FONT).0) } else { s.font };
    let old_font = SelectObject(dc, font.into());
    let sc = |v: i32| scale(v, dpi);
    let (inset, icon) = (sc(INSET), sc(16));
    for (i, row) in s.rows.iter().enumerate() {
        let Some(r) = s.rects.get(i) else { break };
        let (top, bottom) = (r.top - s.scroll, r.bottom - s.scroll);
        if bottom < 0 || top - sc(HEADER) > ch {
            continue;
        }
        if (i == 0 || s.rows[i - 1].section != row.section) && !row.section.is_empty() {
            SelectObject(dc, s.header_font.into());
            SetTextColor(dc, COLORREF(t.secondary_text));
            let mut hr = RECT { left: 2 * inset, top: top - sc(HEADER), right: cw - inset, bottom: top - sc(4) };
            let mut text: Vec<u16> = row.section.encode_utf16().collect();
            DrawTextW(dc, &mut text, &mut hr, DT_SINGLELINE | DT_BOTTOM | DT_NOPREFIX | DT_END_ELLIPSIS);
            SelectObject(dc, font.into());
        }
        let pill = RECT { left: inset, top, right: cw - inset, bottom };
        if s.selected == Some(i) {
            theme::fill_round(dc, &pill, t.selection, sc(6));
        } else if s.hover == Some(i) {
            theme::fill_round(dc, &pill, theme::hover(t), sc(6));
        }
        if let Some(h) = row.icon {
            let _ = DrawIconEx(dc, pill.left + inset, top + (bottom - top - icon) / 2, h, icon, icon, 0, None, DI_NORMAL);
        }
        SetTextColor(dc, COLORREF(t.text));
        let mut lr = RECT { left: pill.left + inset + icon + inset, top, right: pill.right - inset, bottom };
        let mut text: Vec<u16> = row.label.encode_utf16().collect();
        DrawTextW(dc, &mut text, &mut lr, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
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

    fn row(section: &'static str, target: &str) -> Row {
        Row { section, label: target.to_string(), target: target.to_string(), icon: None }
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
            let dpi = GetDpiForWindow(sb);
            assert_eq!(SendMessageW(sb, SM_ROW_TOP, Some(WPARAM(0)), None).0 as i32, layout(&state(sb).unwrap().rows, dpi)[0].top);
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
}
