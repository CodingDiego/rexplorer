//! More > Folder options...: a modal themed tabbed dialog (General, View, Search) for the profile's keys.
//! The tab strip, group headers, labels, check boxes and radios are painted by hand; edits, the start-folder combo and the
//! advanced list (window class "RexOptList") are real controls on the dialog.

use std::ffi::c_void;
use std::sync::Once;
use std::sync::atomic::Ordering;

use rexplorer_core::settings::{HOME_START, Theme};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_COMBOBOXEDIT, ODS_DISABLED, ODS_FOCUS, ODS_SELECTED, ODT_COMBOBOX, SetScrollInfo, SetWindowTheme};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

use crate::*;

const DM_GETDEFID: u32 = 0x0400; // WM_USER
const DC_HASDEFID: isize = 0x534B;
const THIS_PC: &str = "shell:MyComputerFolder";

const ID_APPLY: i32 = 100;
const ID_START: i32 = 101;
const ID_SINGLE: i32 = 102;
const ID_DOUBLE: i32 = 103;
const ID_UL_ALWAYS: i32 = 104;
const ID_UL_HOVER: i32 = 105;
const ID_RECENT: i32 = 106;
const ID_FREQ: i32 = 107;
const ID_CLEAR: i32 = 108;
const ID_RESTORE: i32 = 110; // + page
const ID_LIST: i32 = 120;
const ID_HIDDEN_SHOW: i32 = 121;
const ID_HIDDEN_HIDE: i32 = 122;
const ID_EXT: i32 = 123;
const ID_STATUS: i32 = 124;
const ID_PREVIEW: i32 = 125;
const ID_CHECKS: i32 = 126;
const ID_SEARCH_DELAY: i32 = 127;
const ID_HOVER_DELAY: i32 = 128;
const ID_SYSTEM: i32 = 130;
const ID_ARCHIVES: i32 = 131;

const PAGES: [&str; 3] = ["General", "View", "Search"];
const TAB: (i32, i32, i32, i32) = (12, 12, 72, 28);
/// Client size and page rectangle (x, y, w, h) in 96-DPI pixels; the controls are placed relative to the page.
const CLIENT: (i32, i32) = (480, 480);
const PAGE: (i32, i32, i32, i32) = (12, 40, 456, 390);
const BUTTON: (i32, i32) = (84, 26);
const ROW: i32 = 24;
const DROP_ROWS: i32 = 7;
const COMBO_ROW: i32 = 28;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Header,
    Label,
    Hint,
    Check,
    Radio(u8),
    Push,
    Combo,
    Edit,
    List,
}

struct Ctl {
    page: usize,
    kind: Kind,
    id: i32,
    text: &'static str,
    r: (i32, i32, i32, i32),
}

const fn c(page: usize, kind: Kind, id: i32, text: &'static str, x: i32, y: i32, w: i32, h: i32) -> Ctl {
    Ctl { page, kind, id, text, r: (x, y, w, h) }
}

const RESTORE: (i32, i32, i32, i32) = (324, 352, 120, 26);

const CTLS: [Ctl; 28] = [
    c(0, Kind::Label, -1, "Open rexplorer to:", 12, 12, 180, 26),
    c(0, Kind::Combo, ID_START, "", 200, 12, 232, 26),
    c(0, Kind::Header, -1, "Click items as follows", 12, 52, 432, 20),
    c(0, Kind::Radio(1), ID_SINGLE, "Single-click to open an item (point to select)", 24, 76, 408, ROW),
    c(0, Kind::Radio(2), ID_UL_ALWAYS, "Always underline item names", 48, 100, 384, ROW),
    c(0, Kind::Radio(2), ID_UL_HOVER, "Underline item names only when I point at them", 48, 124, 384, ROW),
    c(0, Kind::Radio(1), ID_DOUBLE, "Double-click to open an item (single-click to select)", 24, 148, 408, ROW),
    c(0, Kind::Header, -1, "Privacy", 12, 188, 432, 20),
    c(0, Kind::Check, ID_RECENT, "Show recently used files on Home", 24, 212, 408, ROW),
    c(0, Kind::Check, ID_FREQ, "Show frequently used folders on Home", 24, 236, 408, ROW),
    c(0, Kind::Label, -1, "Clear the recent folders list (Ctrl+P)", 24, 268, 300, 26),
    c(0, Kind::Push, ID_CLEAR, "Clear", 344, 268, 88, 26),
    c(0, Kind::Push, ID_RESTORE, "Restore defaults", RESTORE.0, RESTORE.1, RESTORE.2, RESTORE.3),
    c(1, Kind::Header, -1, "Advanced settings", 12, 12, 432, 20),
    c(1, Kind::List, ID_LIST, "", 12, 36, 432, 200),
    c(1, Kind::Header, -1, "Timing", 12, 252, 432, 20),
    c(1, Kind::Label, -1, "Search delay (ms)", 24, 280, 200, 26),
    c(1, Kind::Edit, ID_SEARCH_DELAY, "", 240, 280, 72, 26),
    c(1, Kind::Hint, -1, "50 to 5000", 324, 280, 120, 26),
    c(1, Kind::Label, -1, "Hover preview delay (ms)", 24, 314, 200, 26),
    c(1, Kind::Edit, ID_HOVER_DELAY, "", 240, 314, 72, 26),
    c(1, Kind::Hint, -1, "200 to 10000", 324, 314, 120, 26),
    c(1, Kind::Push, ID_RESTORE + 1, "Restore defaults", RESTORE.0, RESTORE.1, RESTORE.2, RESTORE.3),
    c(2, Kind::Header, -1, "When searching non-indexed locations", 12, 12, 432, 20),
    c(2, Kind::Check, ID_SYSTEM, "Include system folders", 24, 36, 408, ROW),
    c(2, Kind::Check, ID_ARCHIVES, "Include compressed files (ZIP, CAB...)", 24, 60, 408, ROW),
    c(2, Kind::Push, ID_RESTORE + 2, "Restore defaults", RESTORE.0, RESTORE.1, RESTORE.2, RESTORE.3),
    c(2, Kind::Hint, -1, "Searches of a whole drive from rexplorer --elevated use the drive's index instead.", 24, 96, 408, 40),
];

fn at(id: i32) -> usize {
    CTLS.iter().position(|c| c.id == id).unwrap_or(0)
}

fn pos(r: (i32, i32, i32, i32), dpi: i32) -> RECT {
    let s = |v: i32| v * dpi / 96;
    RECT { left: s(r.0), top: s(r.1), right: s(r.0 + r.2), bottom: s(r.1 + r.3) }
}

fn solid(dc: HDC, r: &RECT, color: u32) {
    unsafe {
        let b = CreateSolidBrush(COLORREF(color));
        FillRect(dc, r, b);
        let _ = DeleteObject(b.into());
    }
}

/// A 1 px frame with rounded corners: the border color, then the fill inside it.
unsafe fn field(dc: HDC, r: &RECT, border: u32, fill: u32, radius: i32) {
    theme::fill_round(dc, r, border, radius);
    let inner = RECT { left: r.left + 1, top: r.top + 1, right: r.right - 1, bottom: r.bottom - 1 };
    theme::fill_round(dc, &inner, fill, (radius - 1).max(0));
}

unsafe fn text(dc: HDC, font: HFONT, color: u32, s: &str, r: &mut RECT, flags: DRAW_TEXT_FORMAT) {
    let old = SelectObject(dc, font.into());
    SetBkMode(dc, TRANSPARENT);
    SetTextColor(dc, COLORREF(color));
    let mut u: Vec<u16> = s.encode_utf16().collect();
    DrawTextW(dc, &mut u, r, flags | DT_NOPREFIX);
    SelectObject(dc, old);
}

unsafe fn text_width(dc: HDC, font: HFONT, s: &str) -> i32 {
    let old = SelectObject(dc, font.into());
    let u: Vec<u16> = s.encode_utf16().collect();
    let mut sz = SIZE::default();
    let _ = GetTextExtentPoint32W(dc, &u, &mut sz);
    SelectObject(dc, old);
    sz.cx
}

/// Colors of a field and its frame on the page.
fn field_colors(t: &Theme) -> (u32, u32) {
    (theme::blend(t.surface, t.text, 0.3), t.background)
}

fn line_color(t: &Theme) -> u32 {
    theme::blend(t.surface, t.text, 0.14)
}

struct Fonts {
    ui: HFONT,
    glyphs: HFONT,
}

/// A check box or radio and its label inside `r`, on a `bg` backdrop.
unsafe fn draw_option(dc: HDC, r: &RECT, kind: Kind, on: bool, enabled: bool, focus: bool, label: &str, t: &Theme, f: &Fonts, dpi: i32, bg: u32) {
    let g = 16 * dpi / 96;
    let gy = r.top + (r.bottom - r.top - g) / 2;
    let fade = |c: u32| if enabled { c } else { theme::blend(c, bg, 0.55) };
    let b = RECT { left: r.left, top: gy, right: r.left + g, bottom: gy + g };
    if let Kind::Radio(_) = kind {
        let glyph = if on { "\u{ECCB}" } else { "\u{ECCA}" };
        let mut gr = b;
        text(dc, f.glyphs, fade(if on { t.accent } else { t.secondary_text }), glyph, &mut gr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    } else if on {
        theme::fill_round(dc, &b, fade(t.accent), 4 * dpi / 96);
        let mut gr = b;
        text(dc, f.glyphs, 0x00FF_FFFF, "\u{E73E}", &mut gr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    } else {
        field(dc, &b, fade(t.secondary_text), bg, 4 * dpi / 96);
    }
    let mut tr = RECT { left: b.right + 8 * dpi / 96, top: r.top, right: r.right, bottom: r.bottom };
    let color = fade(t.text);
    let mut calc = tr;
    let old = SelectObject(dc, f.ui.into());
    let mut u: Vec<u16> = label.encode_utf16().collect();
    DrawTextW(dc, &mut u, &mut calc, DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX);
    SelectObject(dc, old);
    text(dc, f.ui, color, label, &mut tr, DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
    if focus {
        let fr = RECT { left: tr.left - 3 * dpi / 96, top: tr.top + 2, right: (tr.left + calc.right - calc.left + 3 * dpi / 96).min(r.right), bottom: tr.bottom - 2 };
        SetTextColor(dc, COLORREF(t.text));
        SetBkColor(dc, COLORREF(bg));
        let _ = DrawFocusRect(dc, &fr);
    }
}

// ---- the advanced list ----------------------------------------------------------------------------------

struct Row {
    kind: Kind,
    level: i32,
    id: i32,
    text: &'static str,
    on: bool,
}

const fn row(kind: Kind, level: i32, id: i32, text: &'static str) -> Row {
    Row { kind, level, id, text, on: false }
}

struct ListState {
    theme: Theme,
    fonts: Fonts,
    rows: Vec<Row>,
    top: i32,
    focus: usize,
    parent: HWND,
}

fn list_rows() -> Vec<Row> {
    vec![
        row(Kind::Header, 0, -1, "Files and folders"),
        row(Kind::Header, 1, -1, "Hidden files and folders"),
        row(Kind::Radio(1), 2, ID_HIDDEN_SHOW, "Show hidden files, folders and drives"),
        row(Kind::Radio(1), 2, ID_HIDDEN_HIDE, "Don't show hidden files, folders or drives"),
        row(Kind::Check, 1, ID_EXT, "Hide extensions for known file types"),
        row(Kind::Check, 1, ID_STATUS, "Show status bar"),
        row(Kind::Check, 1, ID_PREVIEW, "Show preview handlers in preview pane"),
        row(Kind::Check, 1, ID_CHECKS, "Use check boxes to select items"),
    ]
}

unsafe fn list_state(h: HWND) -> *mut ListState {
    GetWindowLongPtrW(h, GWLP_USERDATA) as *mut ListState
}

fn row_h(dpi: i32) -> i32 {
    ROW * dpi / 96
}

unsafe fn list_scroll(h: HWND, ls: &mut ListState) {
    let mut rc = RECT::default();
    let _ = GetClientRect(h, &mut rc);
    let dpi = GetDpiForWindow(h) as i32;
    let total = ls.rows.len() as i32 * row_h(dpi);
    let page = rc.bottom - 2;
    ls.top = ls.top.clamp(0, (total - page).max(0));
    let si = SCROLLINFO {
        cbSize: size_of::<SCROLLINFO>() as u32,
        fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
        nMin: 0,
        nMax: total - 1,
        nPage: page.max(0) as u32,
        nPos: ls.top,
        nTrackPos: 0,
    };
    SetScrollInfo(h, SB_VERT, &si, true);
}

unsafe fn list_paint(h: HWND, ls: &ListState) {
    let mut ps = PAINTSTRUCT::default();
    let wdc = BeginPaint(h, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(h, &mut rc);
    let dc = CreateCompatibleDC(Some(wdc));
    let bmp = CreateCompatibleBitmap(wdc, rc.right, rc.bottom);
    let old = SelectObject(dc, bmp.into());
    let (t, dpi) = (&ls.theme, GetDpiForWindow(h) as i32);
    let (border, fill) = field_colors(t);
    solid(dc, &rc, t.surface);
    field(dc, &rc, border, fill, 4 * dpi / 96);
    let (rh, focused) = (row_h(dpi), GetFocus() == h);
    for (i, r) in ls.rows.iter().enumerate() {
        let top = 1 + i as i32 * rh - ls.top;
        if top + rh < 1 || top > rc.bottom {
            continue;
        }
        let x = 8 * dpi / 96 + r.level * 18 * dpi / 96;
        let right = rc.right - 8 * dpi / 96;
        let rr = RECT { left: x, top, right, bottom: top + rh };
        if focused && i == ls.focus && !matches!(r.kind, Kind::Header) {
            let pill = RECT { left: 4 * dpi / 96, top: top + 1, right: rc.right - 4 * dpi / 96, bottom: top + rh - 1 };
            theme::fill_round(dc, &pill, t.selection, 4 * dpi / 96);
        }
        if r.kind == Kind::Header {
            let mut tr = rr;
            text(dc, ls.fonts.ui, if r.level == 0 { t.text } else { t.secondary_text }, r.text, &mut tr, DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
        } else {
            draw_option(dc, &rr, r.kind, r.on, true, false, r.text, t, &ls.fonts, dpi, fill);
        }
    }
    let _ = BitBlt(wdc, 0, 0, rc.right, rc.bottom, Some(dc), 0, 0, SRCCOPY);
    SelectObject(dc, old);
    let _ = DeleteObject(bmp.into());
    let _ = DeleteDC(dc);
    let _ = EndPaint(h, &ps);
}

/// Toggles a check or selects a radio (clearing its group) and tells the dialog.
unsafe fn list_set(h: HWND, ls: &mut ListState, i: usize) {
    match ls.rows[i].kind {
        Kind::Check => ls.rows[i].on = !ls.rows[i].on,
        Kind::Radio(g) => {
            for r in ls.rows.iter_mut().filter(|r| r.kind == Kind::Radio(g)) {
                r.on = false;
            }
            ls.rows[i].on = true;
        }
        _ => return,
    }
    let _ = InvalidateRect(Some(h), None, false);
    SendMessageW(ls.parent, WM_COMMAND, Some(WPARAM(ID_LIST as usize)), Some(LPARAM(h.0 as isize)));
}

unsafe fn list_focus(h: HWND, ls: &mut ListState, i: usize) {
    ls.focus = i;
    let mut rc = RECT::default();
    let _ = GetClientRect(h, &mut rc);
    let rh = row_h(GetDpiForWindow(h) as i32);
    ls.top = ls.top.min(i as i32 * rh).max((i as i32 + 1) * rh - (rc.bottom - 2));
    list_scroll(h, ls);
    let _ = InvalidateRect(Some(h), None, false);
}

unsafe extern "system" fn list_proc(h: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lparam.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(h, GWLP_USERDATA, cs.lpCreateParams as isize);
        return DefWindowProcW(h, msg, wparam, lparam);
    }
    let ls = list_state(h);
    if ls.is_null() {
        return DefWindowProcW(h, msg, wparam, lparam);
    }
    let ls = &mut *ls;
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            list_paint(h, ls);
            LRESULT(0)
        }
        WM_SIZE => {
            list_scroll(h, ls);
            LRESULT(0)
        }
        WM_GETDLGCODE => LRESULT(DLGC_WANTARROWS as isize),
        WM_SETFOCUS | WM_KILLFOCUS => {
            let _ = InvalidateRect(Some(h), None, false);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let _ = SetFocus(Some(h));
            let y = (lparam.0 >> 16) as i16 as i32;
            let i = (y - 1 + ls.top).max(0) / row_h(GetDpiForWindow(h) as i32);
            if (i as usize) < ls.rows.len() && ls.rows[i as usize].kind != Kind::Header {
                list_focus(h, ls, i as usize);
                list_set(h, ls, i as usize);
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let lines = (wparam.0 >> 16) as i16 as i32 / 120;
            ls.top -= lines * 3 * row_h(GetDpiForWindow(h) as i32);
            list_scroll(h, ls);
            let _ = InvalidateRect(Some(h), None, false);
            LRESULT(0)
        }
        WM_VSCROLL => {
            let mut si = SCROLLINFO { cbSize: size_of::<SCROLLINFO>() as u32, fMask: SIF_ALL, ..Default::default() };
            let _ = GetScrollInfo(h, SB_VERT, &mut si);
            let rh = row_h(GetDpiForWindow(h) as i32);
            ls.top = match SCROLLBAR_COMMAND((wparam.0 & 0xFFFF) as i32) {
                SB_LINEUP => ls.top - rh,
                SB_LINEDOWN => ls.top + rh,
                SB_PAGEUP => ls.top - si.nPage as i32,
                SB_PAGEDOWN => ls.top + si.nPage as i32,
                SB_THUMBTRACK | SB_THUMBPOSITION => si.nTrackPos,
                _ => ls.top,
            };
            list_scroll(h, ls);
            let _ = InvalidateRect(Some(h), None, false);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = VIRTUAL_KEY(wparam.0 as u16);
            let n = ls.rows.len();
            let step = |from: usize, down: bool| -> Option<usize> {
                let order: Vec<usize> = if down { (from + 1..n).collect() } else { (0..from).rev().collect() };
                order.into_iter().find(|&i| ls.rows[i].kind != Kind::Header)
            };
            let target = match vk {
                VK_DOWN | VK_RIGHT => step(ls.focus, true),
                VK_UP | VK_LEFT => step(ls.focus, false),
                VK_HOME => (0..n).find(|&i| ls.rows[i].kind != Kind::Header),
                VK_END => step(n, false),
                VK_SPACE => {
                    list_set(h, ls, ls.focus);
                    return LRESULT(0);
                }
                _ => None,
            };
            if let Some(i) = target {
                let radio_move = matches!((ls.rows[ls.focus].kind, ls.rows[i].kind), (Kind::Radio(a), Kind::Radio(b)) if a == b);
                list_focus(h, ls, i);
                if radio_move {
                    list_set(h, ls, i);
                }
            }
            LRESULT(0)
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(h, GWLP_USERDATA, 0);
            drop(Box::from_raw(ls as *mut ListState));
            DefWindowProcW(h, msg, wparam, lparam)
        }
        _ => DefWindowProcW(h, msg, wparam, lparam),
    }
}

// ---- the dialog -----------------------------------------------------------------------------------------

struct State {
    app: *mut App,
    theme: Theme,
    fonts: Fonts,
    bg: HBRUSH,
    field: HBRUSH,
    hw: Vec<HWND>,
    on: Vec<bool>,
    list: *mut ListState,
    /// The start entry Home and This PC leave out: the profile's own folder.
    start_other: Option<String>,
    page: usize,
    ok: HWND,
    cancel: HWND,
    apply: HWND,
    dirty: bool,
    ready: bool,
    done: bool,
}

unsafe fn page_origin(dpi: i32) -> (i32, i32) {
    (PAGE.0 * dpi / 96, PAGE.1 * dpi / 96)
}

unsafe fn place(hwnd: HWND, st: &State) {
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let dpi = GetDpiForWindow(hwnd) as i32;
    let s = |v: i32| v * dpi / 96;
    let (ox, oy) = page_origin(dpi);
    let eh = st.theme.font_size * dpi / 72 * 4 / 3 + s(4);
    for (i, ctl) in CTLS.iter().enumerate() {
        let h = st.hw[i];
        if h.0.is_null() {
            continue;
        }
        let r = pos(ctl.r, dpi);
        let (x, y, w, hh) = (ox + r.left, oy + r.top, r.right - r.left, r.bottom - r.top);
        let _ = match ctl.kind {
            Kind::Edit => MoveWindow(h, x + s(8), y + (hh - eh) / 2, w - s(16), eh, true),
            Kind::Combo => MoveWindow(h, x, y, w, hh + DROP_ROWS * s(COMBO_ROW), true),
            _ => MoveWindow(h, x, y, w, hh, true),
        };
    }
    let (bw, bh) = (s(BUTTON.0), s(BUTTON.1));
    let by = rc.bottom - s(12) - bh;
    let _ = MoveWindow(st.apply, rc.right - s(12) - bw, by, bw, bh, true);
    let _ = MoveWindow(st.cancel, rc.right - s(12) - 2 * bw - s(8), by, bw, bh, true);
    let _ = MoveWindow(st.ok, rc.right - s(12) - 3 * bw - s(16), by, bw, bh, true);
}

unsafe fn create_children(hwnd: HWND, st: &mut State) {
    let hinst = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
    let dark = st.theme.dark;
    let mk = |class: PCWSTR, text: &str, style: u32, id: i32, param: *const c_void| {
        let h = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            &HSTRING::from(text),
            WS_CHILD | WINDOW_STYLE(style),
            0,
            0,
            0,
            0,
            Some(hwnd),
            Some(HMENU(id as usize as *mut c_void)),
            Some(hinst),
            if param.is_null() { None } else { Some(param) },
        )
        .unwrap_or_default();
        SendMessageW(h, WM_SETFONT, Some(WPARAM(st.fonts.ui.0 as usize)), Some(LPARAM(1)));
        h
    };
    let tab = WS_TABSTOP.0;
    for (i, ctl) in CTLS.iter().enumerate() {
        st.hw[i] = match ctl.kind {
            Kind::Check | Kind::Radio(_) | Kind::Push => mk(w!("BUTTON"), ctl.text, tab | BS_OWNERDRAW as u32, ctl.id, std::ptr::null()),
            Kind::Edit => {
                let h = mk(w!("EDIT"), "", tab | ES_AUTOHSCROLL as u32 | ES_NUMBER as u32, ctl.id, std::ptr::null());
                crate::edit::attach(h);
                h
            }
            Kind::Combo => {
                let style = WS_VSCROLL.0 | (CBS_DROPDOWNLIST | CBS_OWNERDRAWFIXED | CBS_HASSTRINGS) as u32;
                let h = mk(w!("COMBOBOX"), "", tab | style, ctl.id, std::ptr::null());
                if dark {
                    let _ = SetWindowTheme(h, w!("DarkMode_CFD"), w!("Combobox"));
                }
                h
            }
            Kind::List => {
                let ls = Box::new(ListState {
                    theme: st.theme.clone(),
                    fonts: Fonts { ui: st.fonts.ui, glyphs: st.fonts.glyphs },
                    rows: list_rows(),
                    top: 0,
                    focus: 2,
                    parent: hwnd,
                });
                let p = Box::into_raw(ls);
                st.list = p;
                let h = mk(w!("RexOptList"), "", tab | WS_VSCROLL.0, ctl.id, p as *const c_void);
                if dark {
                    let _ = SetWindowTheme(h, w!("DarkMode_Explorer"), PCWSTR::null());
                }
                h
            }
            _ => HWND::default(),
        };
    }
    st.ok = mk(w!("BUTTON"), "OK", tab | BS_OWNERDRAW as u32, IDOK.0, std::ptr::null());
    st.cancel = mk(w!("BUTTON"), "Cancel", tab | BS_OWNERDRAW as u32, IDCANCEL.0, std::ptr::null());
    st.apply = mk(w!("BUTTON"), "Apply", tab | BS_OWNERDRAW as u32 | WS_DISABLED.0, ID_APPLY, std::ptr::null());
    for h in [st.ok, st.cancel, st.apply] {
        let _ = ShowWindow(h, SW_SHOW);
    }
}

unsafe fn show_page(st: &mut State, page: usize) {
    st.page = page;
    for (i, ctl) in CTLS.iter().enumerate() {
        if !st.hw[i].0.is_null() {
            let _ = ShowWindow(st.hw[i], if ctl.page == page { SW_SHOW } else { SW_HIDE });
        }
    }
    let parent = GetParent(st.ok).unwrap_or_default();
    let _ = InvalidateRect(Some(parent), None, true);
    if let Some(i) = CTLS.iter().position(|c| c.page == page && !matches!(c.kind, Kind::Header | Kind::Label | Kind::Hint) && (c.kind != Kind::Push)) {
        let _ = SetFocus(Some(st.hw[i]));
    }
}

unsafe fn get_on(st: &State, id: i32) -> bool {
    match st.list.as_ref().and_then(|l| l.rows.iter().find(|r| r.id == id)) {
        Some(r) => r.on,
        None => st.on[at(id)],
    }
}

unsafe fn set_on(st: &mut State, id: i32, on: bool) {
    if let Some(r) = st.list.as_mut().and_then(|l| l.rows.iter_mut().find(|r| r.id == id)) {
        r.on = on;
        return;
    }
    st.on[at(id)] = on;
}

unsafe fn number(h: HWND, old: u32) -> u32 {
    let mut buf = vec![0u16; GetWindowTextLengthW(h) as usize + 1];
    let n = GetWindowTextW(h, &mut buf) as usize;
    String::from_utf16_lossy(&buf[..n]).trim().parse().unwrap_or(old)
}

/// Sets the controls of `page` (every page for None) from `p`.
unsafe fn load(st: &mut State, p: &Profile, page: Option<usize>) {
    let want = |pg: usize| page.is_none_or(|x| x == pg);
    if want(0) {
        let combo = st.hw[at(ID_START)];
        SendMessageW(combo, CB_RESETCONTENT, None, None);
        let mut entries = vec!["Home".to_string(), "This PC".to_string()];
        if let Some(o) = &st.start_other {
            entries.push(o.clone());
        }
        for e in &entries {
            SendMessageW(combo, CB_ADDSTRING, None, Some(LPARAM(HSTRING::from(e.as_str()).as_ptr() as isize)));
        }
        let sel = if p.start.eq_ignore_ascii_case(HOME_START) {
            0
        } else if p.start.eq_ignore_ascii_case(THIS_PC) {
            1
        } else {
            entries.len() - 1
        };
        SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(sel)), None);
        let dpi = GetDpiForWindow(combo) as isize;
        SendMessageW(combo, CB_SETITEMHEIGHT, Some(WPARAM(usize::MAX)), Some(LPARAM(20 * dpi / 96)));
        SendMessageW(combo, CB_SETITEMHEIGHT, Some(WPARAM(0)), Some(LPARAM(COMBO_ROW as isize * dpi / 96)));
        let always = p.underline_titles == "always";
        for (id, on) in [
            (ID_SINGLE, p.single_click),
            (ID_DOUBLE, !p.single_click),
            (ID_UL_ALWAYS, always),
            (ID_UL_HOVER, !always),
            (ID_RECENT, p.show_recent),
            (ID_FREQ, p.show_frequent),
        ] {
            set_on(st, id, on);
        }
    }
    if want(1) {
        for (id, on) in [
            (ID_HIDDEN_SHOW, p.show_hidden),
            (ID_HIDDEN_HIDE, !p.show_hidden),
            (ID_EXT, !p.show_extensions),
            (ID_STATUS, p.show_status),
            (ID_PREVIEW, p.preview_handlers),
            (ID_CHECKS, p.item_checkboxes),
        ] {
            set_on(st, id, on);
        }
        if let Some(l) = st.list.as_mut() {
            let checked = if p.show_hidden { ID_HIDDEN_SHOW } else { ID_HIDDEN_HIDE };
            l.focus = l.rows.iter().position(|r| r.id == checked).unwrap_or(l.focus);
        }
        for (id, ms) in [(ID_SEARCH_DELAY, p.search_delay_ms), (ID_HOVER_DELAY, p.hover_preview_ms)] {
            let _ = SetWindowTextW(st.hw[at(id)], &HSTRING::from(ms.to_string()));
        }
    }
    if want(2) {
        set_on(st, ID_SYSTEM, p.search_system);
        set_on(st, ID_ARCHIVES, p.search_archives);
    }
}

/// The profile the controls describe, on top of the current one.
unsafe fn profile_of(st: &State, base: &Profile) -> Profile {
    let sel = SendMessageW(st.hw[at(ID_START)], CB_GETCURSEL, None, None).0;
    let start = match sel {
        0 => HOME_START.to_string(),
        1 => THIS_PC.to_string(),
        _ => st.start_other.clone().unwrap_or_else(|| base.start.clone()),
    };
    let delay = |id: i32, old: u32, lo: u32, hi: u32| {
        let ms = number(st.hw[at(id)], old).clamp(lo, hi);
        let _ = SetWindowTextW(st.hw[at(id)], &HSTRING::from(ms.to_string()));
        ms
    };
    Profile {
        start,
        single_click: get_on(st, ID_SINGLE),
        underline_titles: if get_on(st, ID_UL_ALWAYS) { "always" } else { "hover" }.into(),
        show_recent: get_on(st, ID_RECENT),
        show_frequent: get_on(st, ID_FREQ),
        show_hidden: get_on(st, ID_HIDDEN_SHOW),
        show_extensions: !get_on(st, ID_EXT),
        show_status: get_on(st, ID_STATUS),
        preview_handlers: get_on(st, ID_PREVIEW),
        item_checkboxes: get_on(st, ID_CHECKS),
        search_delay_ms: delay(ID_SEARCH_DELAY, base.search_delay_ms, 50, 5000),
        hover_preview_ms: delay(ID_HOVER_DELAY, base.hover_preview_ms, 200, 10000),
        search_system: get_on(st, ID_SYSTEM),
        search_archives: get_on(st, ID_ARCHIVES),
        ..base.clone()
    }
}

/// The underline choices only matter while single click is on.
unsafe fn sync_enabled(st: &State) {
    let single = get_on(st, ID_SINGLE);
    for id in [ID_UL_ALWAYS, ID_UL_HOVER] {
        let h = st.hw[at(id)];
        let _ = EnableWindow(h, single);
        let _ = InvalidateRect(Some(h), None, false);
    }
}

unsafe fn mark_dirty(st: &mut State) {
    if st.ready && !st.dirty {
        st.dirty = true;
        let _ = EnableWindow(st.apply, true);
        let _ = InvalidateRect(Some(st.apply), None, false);
    }
}

unsafe fn apply(st: &mut State) {
    let new = profile_of(st, &(*st.app).profile);
    apply_profile(st.app, new);
    st.dirty = false;
    let _ = EnableWindow(st.apply, false);
    let _ = InvalidateRect(Some(st.apply), None, false);
}

/// Dialog accent pills: OK is the accent, the rest sit on a background-colored field with a frame.
unsafe fn draw_push(st: &State, di: &DRAWITEMSTRUCT) {
    let t = &st.theme;
    let id = di.CtlID as i32;
    let dpi = GetDpiForWindow(di.hwndItem) as i32;
    let primary = id == IDOK.0;
    let disabled = di.itemState.0 & ODS_DISABLED.0 != 0;
    let outside = if CTLS[at(id)].id == id { t.surface } else { t.background };
    let pressed = di.itemState.0 & ODS_SELECTED.0 != 0;
    let (border, fill) = if primary {
        let a = if pressed { theme::blend(t.accent, 0, 0.2) } else { t.accent };
        (a, a)
    } else {
        let f = if pressed { theme::pressed(t) } else { t.background };
        (if di.itemState.0 & ODS_FOCUS.0 != 0 { t.accent } else { theme::blend(t.surface, t.text, 0.3) }, f)
    };
    let dc = di.hDC;
    solid(dc, &di.rcItem, outside);
    field(dc, &di.rcItem, if disabled { line_color(t) } else { border }, fill, 4 * dpi / 96);
    let mut buf = [0u16; 64];
    let n = GetWindowTextW(di.hwndItem, &mut buf) as usize;
    let label = String::from_utf16_lossy(&buf[..n]);
    let color = if primary && !disabled { 0x00FF_FFFF } else if disabled { theme::blend(t.surface, t.text, 0.4) } else { t.text };
    let mut r = di.rcItem;
    text(dc, st.fonts.ui, color, &label, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
}

/// The start combo's field and its drop-down rows; a long custom path is shortened in the middle.
unsafe fn draw_combo(st: &State, di: &DRAWITEMSTRUCT) {
    let t = &st.theme;
    let dpi = GetDpiForWindow(di.hwndItem) as i32;
    let row = di.itemState.0 & ODS_COMBOBOXEDIT.0 == 0;
    let hot = row && di.itemState.0 & ODS_SELECTED.0 != 0;
    let focused = !row && di.itemState.0 & ODS_FOCUS.0 != 0;
    solid(di.hDC, &di.rcItem, if hot || focused { t.selection } else { t.background });
    if di.itemID == u32::MAX {
        return;
    }
    let n = SendMessageW(di.hwndItem, CB_GETLBTEXTLEN, Some(WPARAM(di.itemID as usize)), None).0.max(0) as usize;
    let mut buf = vec![0u16; n + 1];
    SendMessageW(di.hwndItem, CB_GETLBTEXT, Some(WPARAM(di.itemID as usize)), Some(LPARAM(buf.as_mut_ptr() as isize)));
    let label = String::from_utf16_lossy(&buf[..n]);
    let pad = if row { 10 } else { 4 } * dpi / 96;
    let mut r = RECT { left: di.rcItem.left + pad, right: di.rcItem.right - 4 * dpi / 96, ..di.rcItem };
    text(di.hDC, st.fonts.ui, t.text, &label, &mut r, DT_VCENTER | DT_SINGLELINE | DT_PATH_ELLIPSIS);
}

unsafe fn draw_item(st: &State, di: &DRAWITEMSTRUCT) {
    if di.CtlType == ODT_COMBOBOX {
        draw_combo(st, di);
        return;
    }
    let id = di.CtlID as i32;
    let ctl = &CTLS[at(id)];
    if ctl.id != id || ctl.kind == Kind::Push {
        draw_push(st, di);
        return;
    }
    let dpi = GetDpiForWindow(di.hwndItem) as i32;
    solid(di.hDC, &di.rcItem, st.theme.surface);
    let enabled = di.itemState.0 & ODS_DISABLED.0 == 0;
    let mut buf = [0u16; 128];
    let n = GetWindowTextW(di.hwndItem, &mut buf) as usize;
    let label = String::from_utf16_lossy(&buf[..n]);
    let (kind, on) = (ctl.kind, st.on[at(id)]);
    draw_option(di.hDC, &di.rcItem, kind, on, enabled, di.itemState.0 & ODS_FOCUS.0 != 0, &label, &st.theme, &st.fonts, dpi, st.theme.surface);
}

fn tab_rect(i: usize, dpi: i32) -> RECT {
    pos((TAB.0 + i as i32 * TAB.2, TAB.1, TAB.2, TAB.3), dpi)
}

unsafe fn paint_background(hwnd: HWND, dc: HDC, st: &State) {
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let (t, dpi) = (&st.theme, GetDpiForWindow(hwnd) as i32);
    solid(dc, &rc, t.background);
    let line = line_color(t);
    let page = pos(PAGE, dpi);
    solid(dc, &page, line);
    solid(dc, &RECT { left: page.left + 1, top: page.top + 1, right: page.right - 1, bottom: page.bottom - 1 }, t.surface);
    for (i, name) in PAGES.iter().enumerate() {
        let r = tab_rect(i, dpi);
        let mut tr = r;
        if i == st.page {
            let body = RECT { bottom: r.bottom + 1, ..r };
            solid(dc, &body, line);
            solid(dc, &RECT { left: r.left + 1, top: r.top + 1, right: r.right - 1, bottom: r.bottom + 1 }, t.surface);
            solid(dc, &RECT { left: r.left, top: r.top, right: r.right, bottom: r.top + 2 * dpi / 96 }, t.accent);
            text(dc, st.fonts.ui, t.text, name, &mut tr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        } else {
            text(dc, st.fonts.ui, t.secondary_text, name, &mut tr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        }
    }
    let (ox, oy) = page_origin(dpi);
    for ctl in CTLS.iter().filter(|c| c.page == st.page) {
        let mut r = pos(ctl.r, dpi);
        r = RECT { left: r.left + ox, top: r.top + oy, right: r.right + ox, bottom: r.bottom + oy };
        match ctl.kind {
            Kind::Header => {
                let mut tr = r;
                text(dc, st.fonts.ui, t.text, ctl.text, &mut tr, DT_VCENTER | DT_SINGLELINE);
                let x = r.left + text_width(dc, st.fonts.ui, ctl.text) + 8 * dpi / 96;
                solid(dc, &RECT { left: x, top: (r.top + r.bottom) / 2, right: r.right, bottom: (r.top + r.bottom) / 2 + 1 }, line);
            }
            Kind::Label => {
                let mut tr = r;
                text(dc, st.fonts.ui, t.text, ctl.text, &mut tr, DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
            }
            Kind::Hint => {
                let mut tr = r;
                let flags = if r.bottom - r.top > 30 * dpi / 96 { DT_WORDBREAK } else { DT_VCENTER | DT_SINGLELINE };
                text(dc, st.fonts.ui, t.secondary_text, ctl.text, &mut tr, flags);
            }
            Kind::Edit => {
                let (border, fill) = field_colors(t);
                field(dc, &r, border, fill, 4 * dpi / 96);
            }
            _ => {}
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lparam.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let st = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if st.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let st = &mut *st;
    match msg {
        WM_SIZE => {
            if !st.ok.0.is_null() {
                place(hwnd, st);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => {
            let dc = HDC(wparam.0 as *mut c_void);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let mem = CreateCompatibleDC(Some(dc));
            let bmp = CreateCompatibleBitmap(dc, rc.right, rc.bottom);
            let old = SelectObject(mem, bmp.into());
            paint_background(hwnd, mem, st);
            let _ = BitBlt(dc, 0, 0, rc.right, rc.bottom, Some(mem), 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem);
            LRESULT(1)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = (lparam.0 as i16 as i32, (lparam.0 >> 16) as i16 as i32);
            let dpi = GetDpiForWindow(hwnd) as i32;
            if let Some(i) = (0..PAGES.len()).find(|&i| {
                let r = tab_rect(i, dpi);
                x >= r.left && x < r.right && y >= r.top && y < r.bottom
            }) {
                if i != st.page {
                    show_page(st, i);
                }
            }
            LRESULT(0)
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORSTATIC => {
            let hdc = HDC(wparam.0 as *mut c_void);
            SetTextColor(hdc, COLORREF(st.theme.text));
            SetBkColor(hdc, COLORREF(st.theme.background));
            LRESULT(st.field.0 as isize)
        }
        WM_DRAWITEM => {
            draw_item(st, &*(lparam.0 as *const DRAWITEMSTRUCT));
            LRESULT(1)
        }
        // IsDialogMessageW asks this to find the Enter button; without it Enter does nothing in a plain window.
        DM_GETDEFID => LRESULT((DC_HASDEFID << 16) | IDOK.0 as isize),
        WM_COMMAND => {
            let (id, code) = ((wparam.0 & 0xFFFF) as i32, ((wparam.0 >> 16) & 0xFFFF) as u32);
            let i = at(id);
            match id {
                x if x == IDOK.0 => {
                    if st.dirty {
                        apply(st);
                    }
                    st.done = true;
                }
                x if x == IDCANCEL.0 => st.done = true,
                ID_APPLY => apply(st),
                ID_LIST => {
                    sync_enabled(st);
                    mark_dirty(st);
                }
                ID_CLEAR => {
                    (*st.app).recent.clear();
                    (*st.app).recent.save();
                    // A disabled window cannot keep the focus: hand it on first or the dialog loses its keyboard.
                    // WM_NEXTDLGCTL would do nothing: this is a plain window, not DefDlgProc's.
                    if let Ok(next) = GetNextDlgTabItem(hwnd, Some(st.hw[i]), false) {
                        let _ = SetFocus(Some(next));
                    }
                    let _ = EnableWindow(st.hw[i], false);
                    let _ = InvalidateRect(Some(st.hw[i]), None, false);
                }
                x if (ID_RESTORE..ID_RESTORE + 3).contains(&x) => {
                    load(st, &Profile::default_profile(), Some((x - ID_RESTORE) as usize));
                    sync_enabled(st);
                    mark_dirty(st);
                    let _ = InvalidateRect(Some(hwnd), None, true);
                }
                ID_START if code == 1 /* CBN_SELCHANGE */ => mark_dirty(st),
                ID_SEARCH_DELAY | ID_HOVER_DELAY if code == 0x0300 /* EN_CHANGE */ => mark_dirty(st),
                // Owner-drawn buttons report the second click of a double click as BN_DOUBLECLICKED (5).
                _ if CTLS[i].id == id && (code == 0 || code == 5) => {
                    match CTLS[i].kind {
                        Kind::Check => st.on[i] = !st.on[i],
                        Kind::Radio(g) => {
                            for j in 0..CTLS.len() {
                                if CTLS[j].kind == Kind::Radio(g) {
                                    st.on[j] = j == i;
                                    let _ = InvalidateRect(Some(st.hw[j]), None, false);
                                }
                            }
                        }
                        _ => return LRESULT(0),
                    }
                    let _ = InvalidateRect(Some(st.hw[i]), None, false);
                    sync_enabled(st);
                    mark_dirty(st);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            st.done = true;
            LRESULT(0)
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0); // dialog() owns and drops the State
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn register() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let hinst = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
        let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        // The app icon (resource 1, as the main window's class) for the caption and Alt+Tab.
        let icon = LoadIconW(Some(hinst), PCWSTR(1 as *const u16)).unwrap_or_default();
        let class = |name: PCWSTR, proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT| {
            RegisterClassW(&WNDCLASSW { hInstance: hinst, hCursor: cursor, hIcon: icon, lpfnWndProc: Some(proc), lpszClassName: name, ..Default::default() });
        };
        class(w!("RexFolderOptions"), wndproc);
        class(w!("RexOptList"), list_proc);
    });
}

/// Keys the modal loop handles before IsDialogMessageW: Ctrl+Tab and Ctrl+PageDown/Up switch tabs, arrows move
/// inside a radio group, Enter presses the focused action button, else OK.
unsafe fn pre_translate(hwnd: HWND, st: &mut State, msg: &MSG) -> bool {
    if msg.message != WM_KEYDOWN {
        return false;
    }
    let vk = VIRTUAL_KEY(msg.wParam.0 as u16);
    let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
    let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
    let n = PAGES.len();
    if ctrl && matches!(vk, VK_TAB | VK_NEXT | VK_PRIOR) {
        let back = vk == VK_PRIOR || (vk == VK_TAB && shift);
        show_page(st, if back { (st.page + n - 1) % n } else { (st.page + 1) % n });
        return true;
    }
    let focus = GetFocus();
    let Some(i) = st.hw.iter().position(|&h| h == focus && !h.0.is_null()) else { return false };
    match (CTLS[i].kind, vk) {
        (Kind::Radio(g), VK_UP | VK_DOWN | VK_LEFT | VK_RIGHT) => {
            let group: Vec<usize> = (0..CTLS.len()).filter(|&j| CTLS[j].kind == Kind::Radio(g) && IsWindowEnabled(st.hw[j]).as_bool()).collect();
            let k = group.iter().position(|&j| j == i).unwrap_or(0);
            let next = if matches!(vk, VK_UP | VK_LEFT) { group[(k + group.len() - 1) % group.len()] } else { group[(k + 1) % group.len()] };
            let _ = SetFocus(Some(st.hw[next]));
            SendMessageW(hwnd, WM_COMMAND, Some(WPARAM(CTLS[next].id as usize)), Some(LPARAM(st.hw[next].0 as isize)));
            true
        }
        (Kind::Push, VK_RETURN) => {
            SendMessageW(focus, BM_CLICK, None, None);
            true
        }
        (Kind::Combo, VK_RETURN) if SendMessageW(focus, CB_GETDROPPEDSTATE, None, None).0 != 0 => false,
        (_, VK_RETURN) => {
            SendMessageW(hwnd, WM_COMMAND, Some(WPARAM(IDOK.0 as usize)), None);
            true
        }
        _ => false,
    }
}

/// Shows the dialog over the main window; edits and Apply act on the app as they happen.
unsafe fn dialog(app: *mut App) {
    register();
    let (parent, t, p) = ((*app).hwnd, &(*app).theme, (*app).profile.clone());
    let dpi = GetDpiForWindow(parent) as i32;
    let ui = if (*app).font.is_invalid() { HFONT(GetStockObject(DEFAULT_GUI_FONT).0) } else { (*app).font };
    let glyphs = theme::icon_font(dpi as u32, 16);
    let start_other = (!p.start.eq_ignore_ascii_case(HOME_START) && !p.start.eq_ignore_ascii_case(THIS_PC)).then(|| p.start.clone());
    let mut st = Box::new(State {
        app,
        theme: t.clone(),
        fonts: Fonts { ui, glyphs },
        bg: CreateSolidBrush(COLORREF(t.background)),
        field: CreateSolidBrush(COLORREF(t.background)),
        hw: vec![HWND::default(); CTLS.len()],
        on: vec![false; CTLS.len()],
        list: std::ptr::null_mut(),
        start_other,
        page: 0,
        ok: HWND::default(),
        cancel: HWND::default(),
        apply: HWND::default(),
        dirty: false,
        ready: false,
        done: false,
    });
    let mut wr = RECT { left: 0, top: 0, right: CLIENT.0 * dpi / 96, bottom: CLIENT.1 * dpi / 96 };
    let style = WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN;
    let _ = AdjustWindowRectExForDpi(&mut wr, style, false, WINDOW_EX_STYLE(0), dpi as u32);
    let (w, h) = (wr.right - wr.left, wr.bottom - wr.top);
    let mut pr = RECT::default();
    let _ = GetWindowRect(parent, &mut pr);
    let hinst = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
    let created = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("RexFolderOptions"),
        w!("Folder options"),
        style,
        pr.left + (pr.right - pr.left - w) / 2,
        pr.top + (pr.bottom - pr.top - h) / 2,
        w,
        h,
        Some(parent),
        None,
        Some(hinst),
        Some(&mut *st as *mut State as *const c_void),
    );
    if let Ok(hwnd) = created {
        let dark = t.dark as i32;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const i32 as *const c_void, 4);
        for attr in [DWMWA_CAPTION_COLOR, DWMWA_BORDER_COLOR] {
            let _ = DwmSetWindowAttribute(hwnd, attr, &t.background as *const u32 as *const c_void, 4);
        }
        create_children(hwnd, &mut st);
        load(&mut st, &p, None);
        sync_enabled(&st);
        place(hwnd, &st);
        show_page(&mut st, 0);
        st.ready = true;
        let _ = EnableWindow(parent, false);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let mut msg = MSG::default();
        while !st.done {
            if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                PostQuitMessage(msg.wParam.0 as i32); // hand WM_QUIT back to the main loop
                break;
            }
            if !pre_translate(hwnd, &mut st, &msg) && !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let _ = EnableWindow(parent, true); // before destroy, so activation returns to the parent
        let _ = DestroyWindow(hwnd);
    }
    let _ = DeleteObject(st.bg.into());
    let _ = DeleteObject(st.field.into());
    let _ = DeleteObject(glyphs.into());
}

/// The statics the rest of the app reads for the profile's keys (startup and after the dialog).
pub(crate) fn sync(p: &Profile) {
    source::SHOW_EXT.store(p.show_extensions, Ordering::Relaxed);
    home::SHOW_RECENT.store(p.show_recent, Ordering::Relaxed);
    home::SHOW_FREQUENT.store(p.show_frequent, Ordering::Relaxed);
    preview_pane::HANDLERS.store(p.preview_handlers, Ordering::Relaxed);
}

/// Makes `new` the profile: saves the keys that changed to settings.ini and applies them live.
unsafe fn apply_profile(app: *mut App, new: Profile) {
    let old = std::mem::replace(&mut (*app).profile, new.clone());
    let b = |v: bool| v.to_string();
    let keys = [
        ("start", old.start.clone(), new.start.clone()),
        ("show_hidden", b(old.show_hidden), b(new.show_hidden)),
        ("show_extensions", b(old.show_extensions), b(new.show_extensions)),
        ("single_click", b(old.single_click), b(new.single_click)),
        ("underline_titles", old.underline_titles.clone(), new.underline_titles.clone()),
        ("show_recent", b(old.show_recent), b(new.show_recent)),
        ("show_frequent", b(old.show_frequent), b(new.show_frequent)),
        ("show_status", b(old.show_status), b(new.show_status)),
        ("preview_handlers", b(old.preview_handlers), b(new.preview_handlers)),
        ("item_checkboxes", b(old.item_checkboxes), b(new.item_checkboxes)),
        ("search_system", b(old.search_system), b(new.search_system)),
        ("search_archives", b(old.search_archives), b(new.search_archives)),
        ("search_delay_ms", old.search_delay_ms.to_string(), new.search_delay_ms.to_string()),
        ("hover_preview_ms", old.hover_preview_ms.to_string(), new.hover_preview_ms.to_string()),
    ];
    for (key, a, v) in keys {
        if a != v {
            views::save_profile_key(&new.name, key, &v);
        }
    }
    sync(&new);
    for tab in &mut (*app).tabs {
        // Show hidden files changes what a listing holds: the other tabs reload when shown.
        tab.dirty |= old.show_hidden != new.show_hidden;
        // Show file extensions changes the Name column's text; the listings cached it.
        if old.show_extensions != new.show_extensions {
            if let Some(Source::Shell(l)) = &mut tab.source {
                l.cells.fill(Default::default());
            }
        }
    }
    ui::sync_list_style(app);
    let _ = InvalidateRect(Some((*app).list), None, true);
    ui::layout(app);
    panes::schedule_preview(app);
    nav::refresh(app);
}

/// Flips show_recent (or show_frequent) from Home's menu, saved and applied as the dialog's OK does.
pub(crate) unsafe fn toggle(app: *mut App, recent: bool) {
    let mut p = (*app).profile.clone();
    if recent {
        p.show_recent = !p.show_recent;
    } else {
        p.show_frequent = !p.show_frequent;
    }
    apply_profile(app, p);
}

/// CMD_FOLDER_OPTIONS.
pub(crate) unsafe fn run(app: *mut App) {
    dialog(app);
}
