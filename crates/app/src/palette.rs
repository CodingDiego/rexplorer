//! Ctrl+P fuzzy folder palette: a borderless rounded popup with an Edit on top and up to 10 self-drawn result rows.
#![allow(dead_code)] // until main.rs wires it in

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::Once;

use rexplorer_core::fuzzy::{Candidate, rank};
use rexplorer_core::settings::Theme;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{NMHDR, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, Result, w};

use crate::theme;

pub const PN_PICK: u32 = 0x9021;
pub const PN_CLOSE: u32 = 0x9022;

#[repr(C)]
pub struct NmPalette {
    pub hdr: NMHDR,
}

const MAX_ROWS: usize = 10;
const WIDTH: i32 = 560;
const TOP_GAP: i32 = 80;
const EDIT_AREA: i32 = 44;
const EDIT_H: i32 = 28;
const ROW_H: i32 = 32;
const PAD: i32 = 12;
const RADIUS: i32 = 12;
/// Left edge of the typed text and of the result names.
const TEXT_X: i32 = 20;

const WM_APP_KEY: u32 = WM_APP + 1;
const WM_APP_BLUR: u32 = WM_APP + 2;
const ID_EDIT: usize = 1;

// ---- pure model ----

pub struct Model {
    cands: Vec<Candidate>,
    results: Vec<usize>,
    sel: usize,
}

impl Model {
    pub fn new(cands: Vec<Candidate>) -> Model {
        let mut m = Model { cands, results: Vec::new(), sel: 0 };
        m.set_query("");
        m
    }

    pub fn set_query(&mut self, q: &str) {
        let q = q.trim();
        let results = if q.is_empty() {
            (0..self.cands.len().min(MAX_ROWS)).collect()
        } else {
            rank(&self.cands, q, MAX_ROWS)
                .iter()
                .filter_map(|(c, _)| self.cands.iter().position(|x| std::ptr::eq(x, *c)))
                .collect()
        };
        self.results = results;
        self.sel = 0;
    }

    pub fn results(&self) -> &[usize] {
        &self.results
    }

    pub fn selected(&self) -> usize {
        self.sel
    }

    pub fn move_sel(&mut self, delta: i32) {
        let max = self.results.len().saturating_sub(1) as i32;
        self.sel = (self.sel as i32 + delta).clamp(0, max) as usize;
    }

    pub fn pick(&self) -> Option<&str> {
        self.results.get(self.sel).map(|&i| self.cands[i].target.as_str())
    }
}

fn sc(px: i32, dpi: u32) -> i32 {
    px * dpi as i32 / 96
}

fn height_for(rows: usize, dpi: u32) -> i32 {
    sc(EDIT_AREA, dpi) + if rows > 0 { rows as i32 * sc(ROW_H, dpi) + sc(PAD, dpi) / 2 } else { 0 }
}

/// Result row under client-space `y`, if any.
fn row_at(y: i32, rows: usize, dpi: u32) -> Option<usize> {
    let rel = y - sc(EDIT_AREA, dpi);
    if rel < 0 {
        return None;
    }
    let i = (rel / sc(ROW_H, dpi)) as usize;
    (i < rows).then_some(i)
}

// ---- window ----

struct State {
    model: Model,
    edit: HWND,
    font: HFONT,
    colors: [u32; 4], // surface, text, secondary, selection
    brush: HBRUSH,
    id: usize,
    dpi: u32,
    picked: Option<String>,
    tracking: bool,
    closing: bool,
}

static REGISTER: Once = Once::new();

pub fn open(parent: HWND, id: usize, candidates: Vec<Candidate>, t: &Theme, font: HFONT) -> Result<HWND> {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = w!("RexPalette");
        REGISTER.call_once(|| {
            let wc = WNDCLASSW {
                hInstance: hinst,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpfnWndProc: Some(wndproc),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassW(&wc);
        });
        let dpi = match GetDpiForWindow(parent) {
            0 => 96,
            d => d,
        };
        let model = Model::new(candidates);
        let (wd, ht) = (sc(WIDTH, dpi), height_for(model.results().len(), dpi));
        let mut pr = RECT::default();
        GetWindowRect(parent, &mut pr)?;
        let x = pr.left + (pr.right - pr.left - wd) / 2;
        let y = pr.top + sc(TOP_GAP, dpi);
        // shortcut: if CreateWindowExW fails before WM_NCCREATE this Box leaks; negligible.
        let st = Box::into_raw(Box::new(State {
            model,
            edit: HWND::default(),
            font,
            colors: [t.surface, t.text, t.secondary_text, t.selection],
            brush: CreateSolidBrush(COLORREF(t.surface)),
            id,
            dpi,
            picked: None,
            tracking: false,
            closing: false,
        }));
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class,
            PCWSTR::null(),
            WS_POPUP,
            x,
            y,
            wd,
            ht,
            Some(parent),
            None,
            Some(hinst),
            Some(st as *const c_void),
        )?;
        theme::round_corners(hwnd, sc(RADIUS, dpi));
        let _ = ShowWindow(hwnd, SW_SHOWNA);
        let _ = SetFocus(Some((*st).edit));
        Ok(hwnd)
    }
}

/// Null unless `hwnd` is a live palette.
unsafe fn state_of(hwnd: HWND) -> *mut State {
    if !IsWindow(Some(hwnd)).as_bool() || GetWindowLongPtrW(hwnd, GWLP_WNDPROC) as usize != wndproc as *const () as usize {
        return std::ptr::null_mut();
    }
    GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State
}

pub fn picked(hwnd: HWND) -> Option<String> {
    unsafe {
        let st = state_of(hwnd);
        if st.is_null() { None } else { (*st).picked.clone() }
    }
}

pub fn close(hwnd: HWND) {
    unsafe {
        let st = state_of(hwnd);
        if !st.is_null() {
            (*st).closing = true;
            let _ = DestroyWindow(hwnd);
        }
    }
}

unsafe fn notify(hwnd: HWND, id: usize, code: u32) {
    let nm = NmPalette { hdr: NMHDR { hwndFrom: hwnd, idFrom: id, code } };
    let parent = GetParent(hwnd).unwrap_or_default();
    SendMessageW(parent, WM_NOTIFY, Some(WPARAM(id)), Some(LPARAM(&nm as *const _ as isize)));
}

/// The parent may close us from inside the handler, so never touch `st` after notifying.
unsafe fn finish(hwnd: HWND, st: *mut State, code: u32) {
    if (*st).closing {
        return;
    }
    (*st).closing = true;
    let id = (*st).id;
    notify(hwnd, id, code);
    if IsWindow(Some(hwnd)).as_bool() {
        let _ = DestroyWindow(hwnd);
    }
}

unsafe fn confirm(hwnd: HWND, st: *mut State) {
    let Some(t) = (*st).model.pick().map(String::from) else { return };
    if (*st).closing {
        return;
    }
    (*st).picked = Some(t);
    finish(hwnd, st, PN_PICK);
}

unsafe fn refresh(hwnd: HWND, st: *mut State) {
    let h = height_for((*st).model.results().len(), (*st).dpi);
    let _ = SetWindowPos(hwnd, None, 0, 0, sc(WIDTH, (*st).dpi), h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    theme::round_corners(hwnd, sc(RADIUS, (*st).dpi));
    let _ = InvalidateRect(Some(hwnd), None, false);
}

unsafe fn on_create(hwnd: HWND, st: *mut State) -> bool {
    let dpi = (*st).dpi;
    let pad = sc(TEXT_X, dpi);
    let Ok(edit) = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("EDIT"),
        PCWSTR::null(),
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        pad,
        sc((EDIT_AREA - EDIT_H) / 2, dpi),
        sc(WIDTH, dpi) - 2 * pad,
        sc(EDIT_H, dpi),
        Some(hwnd),
        Some(HMENU(ID_EDIT as *mut c_void)),
        None,
        None,
    ) else {
        return false;
    };
    (*st).edit = edit;
    if !(*st).font.is_invalid() {
        SendMessageW(edit, WM_SETFONT, Some(WPARAM((*st).font.0 as usize)), Some(LPARAM(1)));
    }
    let _ = SetWindowSubclass(edit, Some(edit_proc), 1, 0);
    crate::edit::attach(edit);
    true
}

unsafe fn on_change(hwnd: HWND, st: *mut State) {
    let n = GetWindowTextLengthW((*st).edit).max(0) as usize;
    let mut buf = vec![0u16; n + 1];
    let got = GetWindowTextW((*st).edit, &mut buf).max(0) as usize;
    (*st).model.set_query(&String::from_utf16_lossy(&buf[..got]));
    refresh(hwnd, st);
}

/// Result row under the mouse position packed in `lparam`.
unsafe fn row_under(st: *mut State, lparam: LPARAM) -> Option<usize> {
    let y = (lparam.0 >> 16) as i16 as i32;
    row_at(y, (*st).model.results().len(), (*st).dpi)
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
    match msg {
        WM_CREATE => return LRESULT(if on_create(hwnd, st) { 0 } else { -1 }),
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            let s = Box::from_raw(st);
            let _ = DeleteObject(s.brush.into());
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => paint(hwnd, st),
        WM_CTLCOLOREDIT => {
            let hdc = HDC(wparam.0 as *mut c_void);
            SetTextColor(hdc, COLORREF((*st).colors[1]));
            SetBkColor(hdc, COLORREF((*st).colors[0]));
            return LRESULT((*st).brush.0 as isize);
        }
        WM_COMMAND if (wparam.0 >> 16) as u32 == EN_CHANGE => on_change(hwnd, st),
        WM_APP_KEY => {
            match VIRTUAL_KEY(wparam.0 as u16) {
                VK_UP | VK_DOWN => {
                    (*st).model.move_sel(if wparam.0 as u16 == VK_UP.0 { -1 } else { 1 });
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                VK_RETURN => confirm(hwnd, st),
                _ => finish(hwnd, st, PN_CLOSE),
            }
            return LRESULT(0);
        }
        WM_APP_BLUR => {
            let next = HWND(wparam.0 as *mut c_void);
            if next != hwnd && next != (*st).edit {
                finish(hwnd, st, PN_CLOSE);
            }
        }
        WM_ACTIVATE if wparam.0 & 0xFFFF == 0 => finish(hwnd, st, PN_CLOSE),
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            if !(*st).tracking {
                let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                (*st).tracking = TrackMouseEvent(&mut tme).is_ok();
            }
            if let Some(row) = row_under(st, lparam) {
                let delta = row as i32 - (*st).model.selected() as i32;
                if delta != 0 {
                    (*st).model.move_sel(delta);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
        }
        WM_MOUSELEAVE => (*st).tracking = false,
        WM_LBUTTONDOWN => {
            if let Some(row) = row_under(st, lparam) {
                let delta = row as i32 - (*st).model.selected() as i32;
                (*st).model.move_sel(delta);
                confirm(hwnd, st);
            }
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe extern "system" fn edit_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _ref: usize) -> LRESULT {
    let parent = GetParent(hwnd).unwrap_or_default();
    match msg {
        WM_KEYDOWN if matches!(VIRTUAL_KEY(wparam.0 as u16), VK_UP | VK_DOWN | VK_RETURN | VK_ESCAPE) => {
            SendMessageW(parent, WM_APP_KEY, Some(wparam), Some(LPARAM(0)));
            return LRESULT(0);
        }
        WM_CHAR if wparam.0 == 13 || wparam.0 == 27 => return LRESULT(0), // no beep
        WM_KILLFOCUS => {
            let r = DefSubclassProc(hwnd, msg, wparam, lparam);
            SendMessageW(parent, WM_APP_BLUR, Some(wparam), Some(LPARAM(0)));
            return r;
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

unsafe fn draw_text(hdc: HDC, s: &str, rc: &mut RECT, flags: DRAW_TEXT_FORMAT) {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    DrawTextW(hdc, &mut v, rc, flags);
}

unsafe fn fill(hdc: HDC, rc: &RECT, color: u32) {
    let b = CreateSolidBrush(COLORREF(color));
    FillRect(hdc, rc, b);
    let _ = DeleteObject(b.into());
}

unsafe fn paint(hwnd: HWND, st: *mut State) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let (cw, ch) = (rc.right, rc.bottom);
    let mem = CreateCompatibleDC(Some(hdc));
    let bmp = CreateCompatibleBitmap(hdc, cw, ch);
    let old_bmp = SelectObject(mem, bmp.into());
    let s = &*st;
    let [bg, text, secondary, sel] = s.colors;
    let font = if s.font.is_invalid() { GetStockObject(DEFAULT_GUI_FONT) } else { s.font.into() };
    let old_font = SelectObject(mem, font);
    SetBkMode(mem, TRANSPARENT);

    fill(mem, &rc, bg);
    // Rows are pills inset like the Edit's text, so names line up with what is typed.
    let (pad, row_h, m) = (sc(TEXT_X, s.dpi), sc(ROW_H, s.dpi), sc(PAD - 4, s.dpi));
    for (i, &ci) in s.model.results().iter().enumerate() {
        let top = sc(EDIT_AREA, s.dpi) + i as i32 * row_h;
        let row = RECT { left: m, top, right: cw - m, bottom: top + row_h };
        if i == s.model.selected() {
            theme::fill_round(mem, &row, sel, sc(6, s.dpi));
        }
        let c = &s.model.cands[ci];
        let flags = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX;
        let mut name = RECT { left: pad, right: cw - pad, ..row };
        SetTextColor(mem, COLORREF(text));
        let mut measure = name;
        draw_text(mem, &c.text, &mut measure, flags | DT_CALCRECT);
        draw_text(mem, &c.text, &mut name, flags | DT_END_ELLIPSIS);
        let left = pad + (measure.right - measure.left) + pad;
        if c.target != c.text && left < cw - pad {
            let mut tr = RECT { left, right: cw - pad, ..row };
            SetTextColor(mem, COLORREF(secondary));
            draw_text(mem, &c.target, &mut tr, flags | DT_RIGHT | DT_PATH_ELLIPSIS);
        }
    }
    let _ = BitBlt(hdc, 0, 0, cw, ch, Some(mem), 0, 0, SRCCOPY);
    SelectObject(mem, old_font);
    SelectObject(mem, old_bmp);
    let _ = DeleteObject(bmp.into());
    let _ = DeleteDC(mem);
    let _ = EndPaint(hwnd, &ps);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(t: &str) -> Candidate {
        Candidate { text: t.into(), target: format!("C:\\{t}") }
    }

    #[test]
    fn model_ranks_clamps_and_picks() {
        let mut m = Model::new(vec![c("Documents"), c("Downloads"), c("Desktop")]);
        assert_eq!(m.results(), [0, 1, 2]); // empty query keeps given order
        m.set_query("dow");
        assert_eq!(m.results()[0], 1);
        assert_eq!(m.pick(), Some("C:\\Downloads"));
        m.set_query("d");
        m.move_sel(-5);
        assert_eq!(m.selected(), 0);
        m.move_sel(99);
        assert_eq!(m.selected(), m.results().len() - 1);
        assert_eq!(m.pick(), Some(&*m.cands[m.results()[m.selected()]].target));
        m.set_query("zzz");
        assert!(m.results().is_empty() && m.pick().is_none());
        let many = (0..30).map(|i| c(&format!("f{i}"))).collect();
        assert_eq!(Model::new(many).results().len(), 10);
    }

    #[test]
    fn hit_test_scales() {
        assert_eq!(row_at(43, 3, 96), None);
        assert_eq!(row_at(44, 3, 96), Some(0));
        assert_eq!(row_at(44 + 64, 3, 96), Some(2));
        assert_eq!(row_at(44 + 96, 3, 96), None);
        assert_eq!(row_at(88 + 128, 3, 192), Some(2));
    }

    #[test]
    fn open_under_hidden_parent() {
        unsafe {
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 800, 600, None, None, None, None).unwrap();
            let hwnd = open(parent, 7, vec![c("Documents"), c("Downloads")], &Theme::light(), HFONT::default()).unwrap();
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            assert_eq!(picked(hwnd), None);
            close(hwnd);
            assert!(!IsWindow(Some(hwnd)).as_bool());
            let _ = DestroyWindow(parent);
        }
    }
}
