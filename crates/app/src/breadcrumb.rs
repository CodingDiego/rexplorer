//! Breadcrumb path bar, window class "RexCrumbs": a borderless rounded field of pill-shaped crumbs.
//!
//! The parent owns the text Edit and overlays it when it receives `CN_EDIT`.

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::Once;

use rexplorer_core::access;
use rexplorer_core::i18n::tr;
use rexplorer_core::settings::{Contrast, Theme};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{ROLE_SYSTEM_PUSHBUTTON, ROLE_SYSTEM_TOOLBAR};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VIRTUAL_KEY, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Result, w};

use crate::theme::fill_round;

/// The user clicked crumb `index`.
pub const CN_CRUMB: u32 = 0x9011;
/// The user clicked the empty area and wants to type a path.
pub const CN_EDIT: u32 = 0x9012;
/// The user clicked the chevron after crumb `index` (the "..." crumb reports index 0).
pub const CN_DROPDOWN: u32 = 0x9013;

/// A dropdown asked by keyboard for crumb `index`; place it with `menu_point`.
pub const CN_DROPDOWN_KEY: u32 = 0x9014;

#[repr(C)]
pub struct NmCrumb {
    pub hdr: NMHDR,
    pub index: usize,
}

pub fn height(dpi: u32) -> i32 {
    32 * dpi as i32 / 96
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    Crumb(usize),
    Chevron(usize),
    Overflow,
    Empty,
}

/// Rects are horizontal only (top and bottom are 0) and start at x = 0; the painter supplies the vertical extent.
pub struct CrumbLayout {
    pub first_visible: usize,
    pub items: Vec<(RECT, RECT)>,
}

impl CrumbLayout {
    pub fn hit(&self, x: i32) -> Hit {
        let inside = |r: &RECT| x >= r.left && x < r.right;
        if self.first_visible > 0 && x >= 0 && self.items.first().is_some_and(|(c, _)| x < c.left) {
            return Hit::Overflow;
        }
        for (k, (c, ch)) in self.items.iter().enumerate() {
            if inside(c) {
                return Hit::Crumb(self.first_visible + k);
            }
            if inside(ch) {
                return Hit::Chevron(self.first_visible + k);
            }
        }
        Hit::Empty
    }
}

/// `widths` are measured text widths. A crumb is its text plus `pad` on both sides, followed by a chevron slot of
/// `chevron + pad`. When not everything fits, crumbs drop from the left and an overflow slot of `chevron + 2*pad` leads.
pub fn layout(widths: &[i32], chevron: i32, pad: i32, avail: i32) -> CrumbLayout {
    let n = widths.len();
    let crumb_w = |i: usize| widths[i] + 2 * pad;
    let step = |i: usize| crumb_w(i) + chevron + pad;
    let slot = chevron + 2 * pad;
    let mut first = n.saturating_sub(1);
    for s in 0..n {
        let need: i32 = (s..n).map(step).sum::<i32>() + if s > 0 { slot } else { 0 };
        if need <= avail {
            first = s;
            break;
        }
    }
    let mut x = if first > 0 { slot } else { 0 };
    let mut items = Vec::new();
    for i in first..n {
        let c = RECT { left: x, top: 0, right: x + crumb_w(i), bottom: 0 };
        let ch = RECT { left: c.right, top: 0, right: c.right + chevron + pad, bottom: 0 };
        x = ch.right;
        items.push((c, ch));
    }
    CrumbLayout { first_visible: first, items }
}

fn blend(a: u32, b: u32, t: f32) -> u32 {
    let mix = |s: u32| ((((a >> s) & 0xFF) as f32) * (1.0 - t) + (((b >> s) & 0xFF) as f32) * t).round() as u32;
    mix(0) | (mix(8) << 8) | (mix(16) << 16)
}

struct State {
    /// Behind the field's rounded corners (the header).
    background: u32,
    /// The field itself.
    surface: u32,
    text: u32,
    secondary: u32,
    selection: u32,
    hover_fill: u32,
    accent: u32,
    /// Some while a Windows contrast theme is on.
    contrast: Option<Contrast>,
    font: HFONT,
    icon: Option<HICON>,
    crumbs: Vec<String>,
    layout: CrumbLayout,
    origin: i32,
    hover: Hit,
    tracking: bool,
    /// The crumb the keyboard is on, while the control has the focus.
    focus: usize,
    focused: bool,
    acc: Option<access::Server>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

unsafe fn state<'a>(hwnd: HWND) -> Option<&'a mut State> {
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State).as_mut() }
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
                lpszClassName: w!("RexCrumbs"),
                ..Default::default()
            };
            RegisterClassW(&wc);
        });
        let t = Theme::light();
        let st = Box::into_raw(Box::new(State {
            background: t.background,
            surface: t.surface,
            text: t.text,
            secondary: t.secondary_text,
            selection: t.selection,
            hover_fill: crate::theme::hover(&t),
            accent: t.accent,
            contrast: t.contrast,
            font: HFONT::default(),
            icon: None,
            crumbs: Vec::new(),
            layout: CrumbLayout { first_visible: 0, items: Vec::new() },
            origin: 0,
            hover: Hit::Empty,
            tracking: false,
            focus: 0,
            focused: false,
            acc: None,
        }));
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("RexCrumbs"),
            None,
            WS_CHILD | WS_VISIBLE,
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
            s.background = t.background;
            s.surface = t.surface;
            s.text = t.text;
            s.secondary = t.secondary_text;
            s.selection = t.selection;
            s.hover_fill = crate::theme::hover(t);
            s.accent = t.accent;
            s.contrast = t.contrast;
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

pub fn set_crumbs(hwnd: HWND, crumbs: &[String]) {
    unsafe {
        if let Some(s) = state(hwnd) {
            s.crumbs = crumbs.to_vec();
            s.focus = s.focus.min(s.crumbs.len().saturating_sub(1));
            s.hover = Hit::Empty;
            let _ = InvalidateRect(Some(hwnd), None, false);
            access::event(EVENT_OBJECT_REORDER, hwnd, None);
        }
    }
}

/// Not owned: the caller keeps the icon alive while it is set.
pub fn set_icon(hwnd: HWND, icon: Option<HICON>) {
    unsafe {
        if let Some(s) = state(hwnd) {
            s.icon = icon;
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

/// Where the keyboard focus goes from crumb `cur` among the crumbs `first..first + shown` the layout shows; hidden
/// crumbs are skipped, and nothing wraps.
fn step(cur: usize, first: usize, shown: usize, key: VIRTUAL_KEY) -> usize {
    if shown == 0 {
        return cur;
    }
    let last = first + shown - 1;
    let cur = cur.clamp(first, last);
    match key {
        VK_LEFT => cur.saturating_sub(1).max(first),
        VK_RIGHT => (cur + 1).min(last),
        VK_HOME => first,
        VK_END => last,
        _ => cur,
    }
}

/// Screen point under crumb `index` (its left, bottom edge) for a keyboard-opened dropdown.
pub fn menu_point(hwnd: HWND, index: usize) -> POINT {
    unsafe {
        let mut p = POINT::default();
        if let Some(st) = state(hwnd) {
            let k = index.checked_sub(st.layout.first_visible).and_then(|k| st.layout.items.get(k));
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            p = POINT { x: st.origin + k.map_or(0, |(c, _)| c.left), y: rc.bottom };
        }
        let _ = ClientToScreen(hwnd, &mut p);
        p
    }
}

/// Crumb under screen point (x, y); not a chevron, the overflow or the empty field.
pub fn crumb_at_screen(hwnd: HWND, x: i32, y: i32) -> Option<usize> {
    unsafe {
        let st = state(hwnd)?;
        let mut p = POINT { x, y };
        let _ = ScreenToClient(hwnd, &mut p);
        match st.layout.hit(p.x - st.origin) {
            Hit::Crumb(i) => Some(i),
            _ => None,
        }
    }
}

unsafe fn notify(hwnd: HWND, code: u32, index: usize) {
    unsafe {
        let id = GetWindowLongPtrW(hwnd, GWLP_ID) as usize;
        let nm = NmCrumb { hdr: NMHDR { hwndFrom: hwnd, idFrom: id, code }, index };
        if let Ok(parent) = GetParent(hwnd) {
            SendMessageW(parent, WM_NOTIFY, Some(WPARAM(id)), Some(LPARAM(&nm as *const _ as isize)));
        }
    }
}

unsafe fn acc_tree(hwnd: HWND, st: &State, t: &mut access::Tree) {
    unsafe {
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        t.name = tr("tb.address_name").into();
        t.role = ROLE_SYSTEM_TOOLBAR;
        t.focusable = true;
        t.focus = (st.focused && !st.crumbs.is_empty()).then_some(st.focus);
        for (i, text) in st.crumbs.iter().enumerate() {
            // The layout is only computed in paint and can be stale: a crumb it does not cover is reported hidden.
            let shown = i.checked_sub(st.layout.first_visible).and_then(|k| st.layout.items.get(k));
            let (state, rect) = match shown {
                Some((c, _)) => (0, RECT { left: st.origin + c.left, top: 0, right: st.origin + c.right, bottom: rc.bottom }),
                None => (STATE_SYSTEM_INVISIBLE.0, RECT::default()),
            };
            t.items.push(access::Item { name: text.clone(), role: ROLE_SYSTEM_PUSHBUTTON, state, rect, action: tr("tb.action_open"), ..Default::default() });
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            return LRESULT(1);
        }
        if msg == WM_NCDESTROY {
            let p = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut State;
            if !p.is_null() {
                drop(Box::from_raw(p));
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let Some(st) = state(hwnd) else {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        match msg {
            WM_ERASEBKGND => return LRESULT(1),
            WM_GETOBJECT if access::is_client(lparam) => return st.acc.get_or_insert_with(|| access::Server::new(hwnd)).answer(wparam),
            access::WM_ACC_ITEMS => {
                acc_tree(hwnd, st, &mut *(lparam.0 as *mut access::Tree));
                return LRESULT(1);
            }
            access::WM_ACC_INVOKE => {
                if wparam.0 < st.crumbs.len() {
                    notify(hwnd, CN_CRUMB, wparam.0);
                }
            }
            WM_SETFONT => {
                st.font = HFONT(wparam.0 as *mut c_void);
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            WM_PAINT => paint(hwnd, st),
            WM_SETFOCUS | WM_KILLFOCUS => {
                st.focused = msg == WM_SETFOCUS;
                if st.focused {
                    st.focus = st.crumbs.len().saturating_sub(1);
                    if !st.crumbs.is_empty() {
                        access::event(EVENT_OBJECT_FOCUS, hwnd, Some(st.focus));
                    }
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let key = VIRTUAL_KEY(wparam.0 as u16);
                match key {
                    // Left past the first shown crumb, with crumbs hidden behind the "...", opens that overflow menu.
                    VK_LEFT if msg == WM_KEYDOWN && st.layout.first_visible > 0 && st.focus <= st.layout.first_visible => {
                        notify(hwnd, CN_DROPDOWN_KEY, 0)
                    }
                    VK_LEFT | VK_RIGHT | VK_HOME | VK_END if msg == WM_KEYDOWN => {
                        // The layout is from the last paint: never step past the crumbs there are now.
                        let shown = st.layout.items.len().min(st.crumbs.len().saturating_sub(st.layout.first_visible));
                        let to = step(st.focus, st.layout.first_visible, shown, key);
                        if to != st.focus {
                            st.focus = to;
                            let _ = InvalidateRect(Some(hwnd), None, false);
                            access::event(EVENT_OBJECT_FOCUS, hwnd, Some(to));
                        }
                    }
                    VK_RETURN if msg == WM_KEYDOWN && st.focus < st.crumbs.len() => notify(hwnd, CN_CRUMB, st.focus),
                    // Down and Alt+Down (a system key), and Space; Alt+Space and Alt+F4 stay the window's.
                    VK_DOWN if st.focus < st.crumbs.len() => notify(hwnd, CN_DROPDOWN_KEY, st.focus),
                    VK_SPACE if msg == WM_KEYDOWN && st.focus < st.crumbs.len() => notify(hwnd, CN_DROPDOWN_KEY, st.focus),
                    VK_ESCAPE if msg == WM_KEYDOWN => {
                        // The main window hands the focus on to the view.
                        if let Ok(parent) = GetParent(hwnd) {
                            let _ = SetFocus(Some(parent));
                        }
                    }
                    _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
                }
            }
            WM_MOUSEMOVE => {
                if !st.tracking {
                    let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    st.tracking = TrackMouseEvent(&mut tme).is_ok();
                }
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                set_hover(hwnd, st, st.layout.hit(x - st.origin));
            }
            WM_MOUSELEAVE => {
                st.tracking = false;
                set_hover(hwnd, st, Hit::Empty);
            }
            WM_LBUTTONDOWN => {
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                match st.layout.hit(x - st.origin) {
                    Hit::Crumb(i) => notify(hwnd, CN_CRUMB, i),
                    Hit::Chevron(i) => notify(hwnd, CN_DROPDOWN, i),
                    Hit::Overflow => notify(hwnd, CN_DROPDOWN, 0),
                    Hit::Empty => notify(hwnd, CN_EDIT, 0),
                }
            }
            _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
        }
        LRESULT(0)
    }
}

unsafe fn set_hover(hwnd: HWND, st: &mut State, h: Hit) {
    if st.hover != h {
        st.hover = h;
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

/// Glyph font at `height` pixels: Segoe Fluent Icons, or Segoe MDL2 Assets when that face is missing.
unsafe fn glyph_font(hdc: HDC, height: i32) -> HFONT {
    unsafe {
        let make = |face| {
            CreateFontW(-height, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, DEFAULT_PITCH.0 as u32, face)
        };
        let f = make(w!("Segoe Fluent Icons"));
        let old = SelectObject(hdc, f.into());
        let mut buf = [0u16; 64];
        let n = GetTextFaceW(hdc, Some(&mut buf)).clamp(0, 63) as usize;
        SelectObject(hdc, old);
        if String::from_utf16_lossy(&buf[..n]).trim_end_matches('\0') == "Segoe Fluent Icons" {
            return f;
        }
        let _ = DeleteObject(f.into());
        make(w!("Segoe MDL2 Assets"))
    }
}

unsafe fn text_width(hdc: HDC, s: &[u16]) -> i32 {
    let mut sz = SIZE::default();
    unsafe {
        let _ = GetTextExtentPoint32W(hdc, s, &mut sz);
    }
    sz.cx
}

unsafe fn paint(hwnd: HWND, st: &mut State) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let (cw, ch) = (rc.right, rc.bottom);
        let dpi = GetDpiForWindow(hwnd).max(96) as i32;
        let s = |v: i32| v * dpi / 96;

        let mem = CreateCompatibleDC(Some(hdc));
        let bmp = CreateCompatibleBitmap(hdc, cw.max(1), ch.max(1));
        let old_bmp = SelectObject(mem, bmp.into());

        // The field: a borderless surface pill on the header.
        let bg = CreateSolidBrush(COLORREF(st.background));
        FillRect(mem, &rc, bg);
        fill_round(mem, &rc, st.surface, s(8));
        if st.contrast.is_some() {
            // Surface and header share a color: the field needs an edge.
            // outline_round includes right and bottom: one pixel in keeps them inside the client area.
            crate::theme::outline_round(mem, &RECT { right: cw - 1, bottom: ch - 1, ..rc }, st.text, s(8), 1);
        }

        let text_font = if st.font.is_invalid() { HFONT(GetStockObject(DEFAULT_GUI_FONT).0) } else { st.font };
        let glyph = glyph_font(mem, s(12));
        SetBkMode(mem, TRANSPARENT);

        let (ix, iy) = (s(8), (ch - s(16)) / 2);
        match st.icon {
            Some(icon) => {
                let _ = DrawIconEx(mem, ix, iy, icon, s(16), s(16), 0, None, DI_NORMAL);
            }
            None => {
                let accent = CreateSolidBrush(COLORREF(st.accent));
                FillRect(mem, &RECT { left: ix, top: iy, right: ix + s(16), bottom: iy + s(16) }, accent);
                let _ = DeleteObject(accent.into());
            }
        }
        st.origin = ix + s(16) + s(4);

        let old_font = SelectObject(mem, text_font.into());
        let widths: Vec<i32> = st.crumbs.iter().map(|c| text_width(mem, &wide(c))).collect();
        SelectObject(mem, glyph.into());
        let chevron_w = text_width(mem, &wide("\u{E76C}"));
        st.layout = layout(&widths, chevron_w, s(8), cw - st.origin - s(4));
        if !st.layout.items.is_empty() {
            st.focus = st.focus.clamp(st.layout.first_visible, st.layout.first_visible + st.layout.items.len() - 1);
        }

        // Each crumb is its own pill, 28 px in the 32 px field (both grow with the text size): hover fill, the
        // current folder (last crumb) in the selection color.
        let top = (ch - ch * 28 / 32) / 2;
        let (bottom, origin) = (top + ch * 28 / 32, st.origin);
        let at = |r: &RECT| RECT { left: r.left + origin, top, right: r.right + origin, bottom };
        let first = st.layout.first_visible;
        let last = st.crumbs.len().checked_sub(1);
        if let Some((c, _)) = st.layout.items.last() {
            fill_round(mem, &at(c), st.selection, s(6));
        }
        let hover_rect = match st.hover {
            Hit::Crumb(i) if i >= first && Some(i) != last => st.layout.items.get(i - first).map(|(c, _)| at(c)),
            Hit::Chevron(i) if i >= first => st.layout.items.get(i - first).map(|(_, k)| at(k)),
            Hit::Overflow => st.layout.items.first().map(|(c, _)| at(&RECT { left: 0, top: 0, right: c.left, bottom: 0 })),
            _ => None,
        };
        if let Some(r) = hover_rect {
            fill_round(mem, &r, st.hover_fill, s(6));
            if let Some(c) = st.contrast {
                crate::theme::outline_round(mem, &r, c.hot, s(6), (dpi / 96).max(1));
            }
        }

        let flags = DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX;
        let chevron_color = if st.contrast.is_some() { st.text } else { blend(st.secondary, st.background, 0.6) };
        if first > 0
            && let Some((c, _)) = st.layout.items.first() {
                let mut r = at(&RECT { left: 0, top: 0, right: c.left, bottom: 0 });
                SelectObject(mem, text_font.into());
                SetTextColor(mem, COLORREF(st.text));
                DrawTextW(mem, &mut wide("\u{2026}"), &mut r, flags);
            }
        for (k, (c, chev)) in st.layout.items.iter().enumerate() {
            let (mut cr, mut kr) = (at(c), at(chev));
            SelectObject(mem, text_font.into());
            let current = st.contrast.filter(|_| k + 1 == st.layout.items.len());
            SetTextColor(mem, COLORREF(current.map_or(st.text, |c| c.highlight_text)));
            DrawTextW(mem, &mut wide(&st.crumbs[first + k]), &mut cr, flags);
            SelectObject(mem, glyph.into());
            SetTextColor(mem, COLORREF(chevron_color));
            DrawTextW(mem, &mut wide("\u{E76C}"), &mut kr, flags);
        }

        if st.focused
            && let Some((c, _)) = st.focus.checked_sub(first).and_then(|k| st.layout.items.get(k)) {
                let r = at(c);
                let i = s(3);
                SetTextColor(mem, COLORREF(st.text));
                SetBkColor(mem, COLORREF(st.background));
                let _ = DrawFocusRect(mem, &RECT { left: r.left + i, top: r.top + i, right: r.right - i, bottom: r.bottom - i });
            }

        SelectObject(mem, old_font);
        let _ = BitBlt(hdc, 0, 0, cw, ch, Some(mem), 0, 0, SRCCOPY);
        SelectObject(mem, old_bmp);
        let _ = DeleteObject(glyph.into());
        let _ = DeleteObject(bg.into());
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        let _ = EndPaint(hwnd, &ps);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crumb_at_screen_only_crumbs() {
        unsafe {
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), windows::core::PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 800, 600, None, None, None, None).unwrap();
            let h = create(parent, 7).unwrap();
            let _ = MoveWindow(h, 0, 0, 600, 32, false);
            let l = layout(&[40, 50], 10, 8, 500);
            let (c1, chevron0) = (l.items[1].0, l.items[0].1);
            state(h).unwrap().layout = l;
            state(h).unwrap().origin = 30;
            let screen = |x: i32| {
                let mut p = windows::Win32::Foundation::POINT { x, y: 5 };
                let _ = ClientToScreen(h, &mut p);
                p
            };
            let p = screen(30 + c1.left + 1);
            assert_eq!(crumb_at_screen(h, p.x, p.y), Some(1));
            let p = screen(30 + chevron0.left + 1);
            assert_eq!(crumb_at_screen(h, p.x, p.y), None, "a chevron");
            let _ = DestroyWindow(parent);
        }
    }

    #[test]
    fn layout_all_fit() {
        let l = layout(&[40, 30, 50], 10, 5, 1000);
        assert_eq!(l.first_visible, 0);
        assert_eq!(l.items.len(), 3);
        assert!(l.items.windows(2).all(|p| p[1].0.left > p[0].0.left));
        assert!(l.items.iter().all(|(c, k)| k.left == c.right && k.right > k.left));
    }

    #[test]
    fn layout_too_narrow_drops_left() {
        let l = layout(&[50; 5], 10, 5, 150);
        assert!(l.first_visible > 0);
        assert!(l.items.len() < 5);
        assert_eq!(l.first_visible + l.items.len(), 5);
        assert_eq!(l.hit(1), Hit::Overflow);
        // Even when nothing fits, the last crumb stays.
        assert_eq!(layout(&[500, 500], 10, 5, 10).items.len(), 1);
    }

    #[test]
    fn hit_regions() {
        let l = layout(&[40, 30], 10, 5, 1000);
        let (c0, k0) = l.items[0];
        assert_eq!(l.hit((c0.left + c0.right) / 2), Hit::Crumb(0));
        assert_eq!(l.hit(k0.left), Hit::Chevron(0));
        assert_eq!(l.hit(l.items[1].0.left + 1), Hit::Crumb(1));
        assert_eq!(l.hit(l.items[1].1.right + 50), Hit::Empty);
        assert_eq!(l.hit(-1), Hit::Empty);
    }

    #[test]
    fn blend_ends() {
        assert_eq!(blend(0, 0xFFFFFF, 0.0), 0);
        assert_eq!(blend(0, 0xFFFFFF, 1.0), 0xFFFFFF);
    }

    fn pump() {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    #[test]
    fn create_and_set_crumbs() {
        unsafe {
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), windows::core::PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 800, 600, None, None, None, None).unwrap();
            let bc = create(parent, 7).unwrap();
            let _ = MoveWindow(bc, 0, 0, 600, height(96), true);
            set_theme(bc, &Theme::light());
            set_crumbs(bc, &["Este equipo".to_string(), "C:".to_string(), "Users".to_string()]);
            set_icon(bc, None);
            let _ = UpdateWindow(bc);
            pump();
            assert_eq!(state(bc).unwrap().crumbs.len(), 3);
            let _ = DestroyWindow(parent);
            pump();
        }
    }

    thread_local! {
        static NOTES: std::cell::RefCell<Vec<(u32, usize)>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    unsafe extern "system" fn rec_parent(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_NOTIFY {
            let nm = &*(lparam.0 as *const NmCrumb);
            NOTES.with(|n| n.borrow_mut().push((nm.hdr.code, nm.index)));
            return LRESULT(0);
        }
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    #[test]
    fn step_skips_hidden_and_clamps() {
        // Crumbs 2..5 shown.
        assert_eq!((step(3, 2, 3, VK_LEFT), step(2, 2, 3, VK_LEFT), step(4, 2, 3, VK_RIGHT)), (2, 2, 4));
        assert_eq!((step(3, 2, 3, VK_HOME), step(3, 2, 3, VK_END), step(0, 2, 3, VK_RIGHT)), (2, 4, 3));
        assert_eq!(step(7, 2, 3, VK_LEFT), 3, "a stale focus is pulled into the shown crumbs first");
        assert_eq!(step(1, 0, 0, VK_RIGHT), 1, "nothing shown: nothing moves");
    }

    #[test]
    fn keyboard_moves_and_navigates() {
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap().into();
            let wc = WNDCLASSW { lpfnWndProc: Some(rec_parent), hInstance: hinst, lpszClassName: w!("RexCrumbsKeyParent"), ..Default::default() };
            RegisterClassW(&wc);
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("RexCrumbsKeyParent"), windows::core::PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 800, 600, None, None, Some(hinst), None).unwrap();
            let bc = create(parent, 7).unwrap();
            let _ = MoveWindow(bc, 0, 0, 600, 32, false);
            set_crumbs(bc, &["This PC".to_string(), "C:".to_string(), "Users".to_string()]);
            state(bc).unwrap().layout = layout(&[40, 50, 60], 10, 8, 1000);
            let key = |k: VIRTUAL_KEY| SendMessageW(bc, WM_KEYDOWN, Some(WPARAM(k.0 as usize)), None);
            let at = || state(bc).unwrap().focus;
            SendMessageW(bc, WM_SETFOCUS, None, None);
            assert_eq!(at(), 2, "the current folder");
            key(VK_LEFT);
            key(VK_LEFT);
            assert_eq!(at(), 0);
            key(VK_LEFT);
            assert_eq!(at(), 0, "no wrap");
            key(VK_RIGHT);
            assert_eq!(at(), 1);
            key(VK_HOME);
            assert_eq!(at(), 0);
            key(VK_END);
            key(VK_RIGHT);
            assert_eq!(at(), 2);
            key(VK_LEFT);
            NOTES.with(|n| n.borrow_mut().clear());
            key(VK_RETURN);
            key(VK_DOWN);
            key(VK_SPACE);
            // Alt+F4 and Alt+Space belong to the window (close, system menu): never a dropdown.
            SendMessageW(bc, WM_SYSKEYDOWN, Some(WPARAM(windows::Win32::UI::Input::KeyboardAndMouse::VK_F4.0 as usize)), None);
            SendMessageW(bc, WM_SYSKEYDOWN, Some(WPARAM(VK_SPACE.0 as usize)), None);
            // Alt+Down arrives as a system key.
            SendMessageW(bc, WM_SYSKEYDOWN, Some(WPARAM(VK_DOWN.0 as usize)), None);
            assert_eq!(NOTES.with(|n| n.borrow().clone()), [(CN_CRUMB, 1), (CN_DROPDOWN_KEY, 1), (CN_DROPDOWN_KEY, 1), (CN_DROPDOWN_KEY, 1)]);
            let click_x = state(bc).unwrap().origin + state(bc).unwrap().layout.items[1].0.left + 1;
            NOTES.with(|n| n.borrow_mut().clear());
            SendMessageW(bc, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(LPARAM(click_x as isize | (5 << 16))));
            assert_eq!(NOTES.with(|n| n.borrow().clone()), [(CN_CRUMB, 1)], "Enter sends what a click on that crumb sends");
            set_crumbs(bc, &["a".to_string()]);
            assert_eq!(at(), 0, "clamped when the crumbs shrink");
            NOTES.with(|n| n.borrow_mut().clear());
            // Crumbs hidden behind the "...": Left from the first shown crumb opens the overflow dropdown (index 0, as a click does).
            set_crumbs(bc, &["a".to_string(), "b".to_string(), "c".to_string()]);
            state(bc).unwrap().layout.first_visible = 1;
            state(bc).unwrap().focus = 1;
            key(VK_LEFT);
            assert_eq!(NOTES.with(|n| n.borrow().clone()), [(CN_DROPDOWN_KEY, 0)]);
            NOTES.with(|n| n.borrow_mut().clear());
            key(VK_ESCAPE);
            assert!(NOTES.with(|n| n.borrow().is_empty()), "Esc hands the focus back, notifying nothing");
            let _ = DestroyWindow(parent);
        }
    }

    #[test]
    fn accessible_crumbs() {
        use crate::acc_test::*;
        use rexplorer_core::com::ComInit;
        let _com = ComInit::sta();
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap().into();
            let wc = WNDCLASSW { lpfnWndProc: Some(rec_parent), hInstance: hinst, lpszClassName: w!("RexCrumbsAccParent"), ..Default::default() };
            RegisterClassW(&wc);
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("RexCrumbsAccParent"), windows::core::PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 800, 600, None, None, Some(hinst), None).unwrap();
            let bc = create(parent, 7).unwrap();
            let _ = MoveWindow(bc, 0, 0, 600, 32, false);
            let names = ["This PC".to_string(), "C:".to_string(), "Users".to_string()];
            set_crumbs(bc, &names);
            // Set before any pump: a paint would recompute the layout.
            state(bc).unwrap().layout = layout(&[40, 50, 60], 10, 8, 210);
            state(bc).unwrap().origin = 30;
            let acc = object(bc);
            let k = kids(&acc);
            assert_eq!(k.iter().map(|k| k.name.as_str()).collect::<Vec<_>>(), ["This PC", "C:", "Users"]);
            assert!(k.iter().all(|k| k.role == ROLE_SYSTEM_PUSHBUTTON));
            assert_ne!(k[0].state & STATE_SYSTEM_INVISIBLE.0, 0);
            let c = state(bc).unwrap().layout.items[0].0;
            let r = screen(bc, RECT { left: 30 + c.left, top: 0, right: 30 + c.right, bottom: 32 });
            assert_eq!((k[1].rect.left, k[1].rect.top, k[1].rect.right, k[1].rect.bottom), (r.left, r.top, r.right, r.bottom));
            let r0 = root(&acc);
            assert_eq!((r0.name.as_str(), r0.role), ("Address", ROLE_SYSTEM_TOOLBAR));
            NOTES.with(|n| n.borrow_mut().clear());
            invoke(&acc, 3).unwrap();
            assert_eq!(NOTES.with(|n| n.borrow().clone()), [(CN_CRUMB, 2)]);
            let click_x = 30 + state(bc).unwrap().layout.items[1].0.left + 1;
            NOTES.with(|n| n.borrow_mut().clear());
            SendMessageW(bc, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(LPARAM(click_x as isize | (5 << 16))));
            assert_eq!(NOTES.with(|n| n.borrow().clone()), [(CN_CRUMB, 2)], "the same record as a click");
            let tree = || {
                let mut t = access::Tree::default();
                acc_tree(bc, state(bc).unwrap(), &mut t);
                (t.focusable, t.focus)
            };
            assert_eq!(tree(), (true, None), "focusable, no focused crumb without the focus");
            let hk = hook();
            SendMessageW(bc, WM_SETFOCUS, None, None);
            assert!(hk.take().contains(&(EVENT_OBJECT_FOCUS, bc, 3)));
            assert_eq!(tree(), (true, Some(2)));
            SendMessageW(bc, WM_KEYDOWN, Some(WPARAM(VK_LEFT.0 as usize)), None);
            assert!(hk.take().contains(&(EVENT_OBJECT_FOCUS, bc, 2)));
            SendMessageW(bc, WM_KILLFOCUS, None, None);
            assert_eq!(tree(), (true, None));
            set_crumbs(bc, &names);
            assert!(hk.take().contains(&(EVENT_OBJECT_REORDER, bc, 0)));
            drop(hk);
            let _ = DestroyWindow(parent);
            assert_eq!(acc.accChildCount().unwrap_err().code(), windows::Win32::Foundation::CO_E_OBJNOTCONNECTED);
        }
    }
}
