//! Breadcrumb path bar, window class "RexCrumbs": a borderless rounded field of pill-shaped crumbs.
//!
//! The parent owns the text Edit and overlays it when it receives `CN_EDIT`.

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::Once;

use rexplorer_core::settings::Theme;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Result, w};

use crate::theme::fill_round;

/// The user clicked crumb `index`.
pub const CN_CRUMB: u32 = 0x9011;
/// The user clicked the empty area and wants to type a path.
pub const CN_EDIT: u32 = 0x9012;
/// The user clicked the chevron after crumb `index` (the "..." crumb reports index 0).
pub const CN_DROPDOWN: u32 = 0x9013;

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
    font: HFONT,
    icon: Option<HICON>,
    crumbs: Vec<String>,
    layout: CrumbLayout,
    origin: i32,
    hover: Hit,
    tracking: bool,
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
            font: HFONT::default(),
            icon: None,
            crumbs: Vec::new(),
            layout: CrumbLayout { first_visible: 0, items: Vec::new() },
            origin: 0,
            hover: Hit::Empty,
            tracking: false,
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
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

pub fn set_crumbs(hwnd: HWND, crumbs: &[String]) {
    unsafe {
        if let Some(s) = state(hwnd) {
            s.crumbs = crumbs.to_vec();
            s.hover = Hit::Empty;
            let _ = InvalidateRect(Some(hwnd), None, false);
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

unsafe fn notify(hwnd: HWND, code: u32, index: usize) {
    unsafe {
        let id = GetWindowLongPtrW(hwnd, GWLP_ID) as usize;
        let nm = NmCrumb { hdr: NMHDR { hwndFrom: hwnd, idFrom: id, code }, index };
        if let Ok(parent) = GetParent(hwnd) {
            SendMessageW(parent, WM_NOTIFY, Some(WPARAM(id)), Some(LPARAM(&nm as *const _ as isize)));
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
            WM_SETFONT => {
                st.font = HFONT(wparam.0 as *mut c_void);
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            WM_PAINT => paint(hwnd, st),
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

        // Each crumb is its own 28 px pill: hover fill, the current folder (last crumb) in the selection color.
        let top = (ch - s(28)) / 2;
        let (bottom, origin) = (top + s(28), st.origin);
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
        }

        let flags = DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX;
        let chevron_color = blend(st.secondary, st.background, 0.6);
        if first > 0 {
            if let Some((c, _)) = st.layout.items.first() {
                let mut r = at(&RECT { left: 0, top: 0, right: c.left, bottom: 0 });
                SelectObject(mem, text_font.into());
                SetTextColor(mem, COLORREF(st.text));
                DrawTextW(mem, &mut wide("\u{2026}"), &mut r, flags);
            }
        }
        for (k, (c, chev)) in st.layout.items.iter().enumerate() {
            let (mut cr, mut kr) = (at(c), at(chev));
            SelectObject(mem, text_font.into());
            SetTextColor(mem, COLORREF(st.text));
            DrawTextW(mem, &mut wide(&st.crumbs[first + k]), &mut cr, flags);
            SelectObject(mem, glyph.into());
            SetTextColor(mem, COLORREF(chevron_color));
            DrawTextW(mem, &mut wide("\u{E76C}"), &mut kr, flags);
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
}
