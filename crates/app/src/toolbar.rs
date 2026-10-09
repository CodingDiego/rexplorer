//! Row of glyph buttons (Segoe Fluent Icons), window class "RexToolbar".
//! A click sends WM_COMMAND(cmd) to the parent; each button has a tooltip. Buttons put in a group with
//! `set_group` share one pill-shaped segment; a gap separates a group from its neighbours.

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::Once;

use rexplorer_core::settings::Theme;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, PWSTR, Result, w};

use crate::theme;

const CLASS: PCWSTR = w!("RexToolbar");

struct Button {
    glyph: u32,
    cmd: u16,
    tip: Vec<u16>,
    enabled: bool,
    /// Drawn filled with the selection color (the active view).
    checked: bool,
    group: Option<usize>,
}

struct State {
    buttons: Vec<Button>,
    groups: usize,
    hover: Option<usize>,
    pressed: Option<usize>,
    tracking: bool,
    theme: Theme,
    font: HFONT,
    font_dpi: u32,
    tip: HWND,
}

fn scale(px: i32, dpi: u32) -> i32 {
    px * dpi as i32 / 96
}

pub fn button_width(dpi: u32) -> i32 {
    scale(36, dpi)
}

/// Left edge of each button and the total width: fixed-width buttons, `gap` wherever a group starts or ends.
fn edges(groups: &[Option<usize>], bw: i32, gap: i32) -> (Vec<i32>, i32) {
    let mut x = 0;
    let mut lefts = Vec::with_capacity(groups.len());
    for (i, g) in groups.iter().enumerate() {
        if i > 0 && groups[i - 1] != *g && (g.is_some() || groups[i - 1].is_some()) {
            x += gap;
        }
        lefts.push(x);
        x += bw;
    }
    (lefts, x)
}

/// Button under client-space `x`.
fn hit(x: i32, lefts: &[i32], bw: i32) -> Option<usize> {
    lefts.iter().position(|&l| x >= l && x < l + bw)
}

unsafe fn lefts(hwnd: HWND, s: &State) -> (Vec<i32>, i32) {
    let dpi = GetDpiForWindow(hwnd);
    let groups: Vec<Option<usize>> = s.buttons.iter().map(|b| b.group).collect();
    edges(&groups, button_width(dpi), scale(8, dpi))
}

unsafe fn state<'a>(hwnd: HWND) -> Option<&'a mut State> {
    (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State).as_mut()
}

pub fn create(parent: HWND, id: usize, buttons: &[(u32, u16, &str)]) -> Result<HWND> {
    static REGISTER: Once = Once::new();
    unsafe {
        let hinst = GetModuleHandleW(None)?.into();
        REGISTER.call_once(|| {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinst,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: CLASS,
                ..Default::default()
            };
            RegisterClassW(&wc);
        });
        // Freed by WM_NCDESTROY.
        let st = Box::into_raw(Box::new(State {
            buttons: buttons
                .iter()
                .map(|&(glyph, cmd, tip)| Button {
                    glyph,
                    cmd,
                    tip: tip.encode_utf16().chain(Some(0)).collect(),
                    enabled: true,
                    checked: false,
                    group: None,
                })
                .collect(),
            groups: 0,
            hover: None,
            pressed: None,
            tracking: false,
            theme: Theme::light(),
            font: HFONT::default(),
            font_dpi: 0,
            tip: HWND::default(),
        }));
        let hwnd = CreateWindowExW(
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
            Some(st as *const c_void),
        )?;
        add_tooltips(hwnd);
        Ok(hwnd)
    }
}

/// Width that shows every button.
pub fn width(hwnd: HWND) -> i32 {
    unsafe { state(hwnd).map_or(0, |s| lefts(hwnd, s).1) }
}

/// Draws the buttons that send `cmds` as one segmented pill (Back / Forward, the view switch).
pub fn set_group(hwnd: HWND, cmds: &[u16]) {
    unsafe {
        if let Some(s) = state(hwnd) {
            let g = s.groups;
            s.groups += 1;
            for b in s.buttons.iter_mut().filter(|b| cmds.contains(&b.cmd)) {
                b.group = Some(g);
            }
            update_tool_rects(hwnd, s);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

pub fn set_checked(hwnd: HWND, cmd: u16, checked: bool) {
    unsafe {
        if let Some(s) = state(hwnd) {
            for b in s.buttons.iter_mut().filter(|b| b.cmd == cmd && b.checked != checked) {
                b.checked = checked;
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
    }
}

pub fn set_theme(hwnd: HWND, t: &Theme) {
    unsafe {
        if let Some(s) = state(hwnd) {
            s.theme = t.clone();
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

pub fn set_enabled(hwnd: HWND, cmd: u16, enabled: bool) {
    unsafe {
        if let Some(s) = state(hwnd) {
            for b in s.buttons.iter_mut().filter(|b| b.cmd == cmd && b.enabled != enabled) {
                b.enabled = enabled;
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
    }
}

/// Screen rectangle of the button that sends `cmd` (to anchor a popup menu under it).
pub fn button_rect(hwnd: HWND, cmd: u16) -> Option<RECT> {
    unsafe {
        let s = state(hwnd)?;
        let i = s.buttons.iter().position(|b| b.cmd == cmd)?;
        let r = rect(hwnd, s, i);
        let mut pts = [POINT { x: r.left, y: r.top }, POINT { x: r.right, y: r.bottom }];
        MapWindowPoints(Some(hwnd), None, &mut pts);
        Some(RECT { left: pts[0].x, top: pts[0].y, right: pts[1].x, bottom: pts[1].y })
    }
}

unsafe fn rect(hwnd: HWND, s: &State, i: usize) -> RECT {
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let bw = button_width(GetDpiForWindow(hwnd));
    let left = lefts(hwnd, s).0[i];
    RECT { left, top: 0, right: left + bw, bottom: rc.bottom }
}

unsafe fn add_tooltips(hwnd: HWND) {
    let Some(s) = state(hwnd) else { return };
    let style = WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX);
    let Ok(tip) = CreateWindowExW(WS_EX_TOPMOST, TOOLTIPS_CLASSW, PCWSTR::null(), style, 0, 0, 0, 0, Some(hwnd), None, None, None) else {
        return;
    };
    s.tip = tip;
    for (i, b) in s.buttons.iter_mut().enumerate() {
        let ti = TTTOOLINFOW {
            cbSize: size_of::<TTTOOLINFOW>() as u32,
            uFlags: TTF_SUBCLASS,
            hwnd,
            uId: i,
            lpszText: PWSTR(b.tip.as_mut_ptr()),
            ..Default::default()
        };
        SendMessageW(tip, TTM_ADDTOOLW, None, Some(LPARAM(&ti as *const _ as isize)));
    }
}

unsafe fn update_tool_rects(hwnd: HWND, s: &State) {
    for i in 0..s.buttons.len() {
        let ti = TTTOOLINFOW { cbSize: size_of::<TTTOOLINFOW>() as u32, hwnd, uId: i, rect: rect(hwnd, s, i), ..Default::default() };
        SendMessageW(s.tip, TTM_NEWTOOLRECTW, None, Some(LPARAM(&ti as *const _ as isize)));
    }
}

unsafe fn paint(hwnd: HWND, s: &mut State) {
    let mut ps = PAINTSTRUCT::default();
    let wdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let (cw, ch) = (rc.right.max(1), rc.bottom.max(1));
    let dpi = GetDpiForWindow(hwnd);
    if s.font_dpi != dpi {
        if !s.font.is_invalid() {
            let _ = DeleteObject(s.font.into());
        }
        s.font = theme::icon_font(dpi, 16);
        s.font_dpi = dpi;
    }
    let dc = CreateCompatibleDC(Some(wdc));
    let bmp = CreateCompatibleBitmap(wdc, cw, ch);
    let old_bmp = SelectObject(dc, bmp.into());
    let t = &s.theme;
    let bg = CreateSolidBrush(COLORREF(t.background));
    FillRect(dc, &RECT { left: 0, top: 0, right: cw, bottom: ch }, bg);
    let _ = DeleteObject(bg.into());
    SetBkMode(dc, TRANSPARENT);
    let old_font = SelectObject(dc, s.font.into());
    let (radius, inner, m) = (scale(8, dpi), scale(6, dpi), scale(2, dpi));
    // Each group's segment: one surface pill around its buttons.
    for g in 0..s.groups {
        let members: Vec<RECT> = (0..s.buttons.len()).filter(|&i| s.buttons[i].group == Some(g)).map(|i| rect(hwnd, s, i)).collect();
        if let (Some(a), Some(z)) = (members.first(), members.last()) {
            theme::fill_round(dc, &RECT { left: a.left, top: 0, right: z.right, bottom: ch }, t.surface, radius);
        }
    }
    for (i, b) in s.buttons.iter().enumerate() {
        let r = rect(hwnd, s, i);
        let fill = match (b.enabled, s.pressed == Some(i), s.hover == Some(i)) {
            (true, true, true) => Some(theme::pressed(t)),
            (true, _, true) => Some(theme::hover(t)),
            _ if b.checked => Some(t.selection),
            _ => None,
        };
        if let Some(c) = fill {
            // Inside a segment the fill is inset on every side; a lone button's fill is the whole pill.
            let (my, rad) = if b.group.is_some() { (m, inner) } else { (0, radius) };
            theme::fill_round(dc, &RECT { left: r.left + m, top: r.top + my, right: r.right - m, bottom: r.bottom - my }, c, rad);
        }
        SetTextColor(dc, COLORREF(if b.enabled { t.text } else { theme::blend(t.background, t.secondary_text, 0.5) }));
        let mut g: Vec<u16> = char::from_u32(b.glyph).map(|c| c.to_string()).unwrap_or_default().encode_utf16().collect();
        let mut tr = r;
        DrawTextW(dc, &mut g, &mut tr, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    }
    SelectObject(dc, old_font);
    let _ = BitBlt(wdc, 0, 0, cw, ch, Some(dc), 0, 0, SRCCOPY);
    SelectObject(dc, old_bmp);
    let _ = DeleteObject(bmp.into());
    let _ = DeleteDC(dc);
    let _ = EndPaint(hwnd, &ps);
}

unsafe fn hit_at(hwnd: HWND, s: &State, lparam: LPARAM) -> Option<usize> {
    let x = lparam.0 as i16 as i32;
    hit(x, &lefts(hwnd, s).0, button_width(GetDpiForWindow(hwnd)))
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
            if !s.font.is_invalid() {
                let _ = DeleteObject(s.font.into());
            }
            if !s.tip.is_invalid() {
                let _ = DestroyWindow(s.tip);
            }
        }
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let Some(s) = state(hwnd) else { return DefWindowProcW(hwnd, msg, wparam, lparam) };
    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => paint(hwnd, s),
        WM_SIZE => {
            update_tool_rects(hwnd, s);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        WM_MOUSEMOVE => {
            if !s.tracking {
                let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                s.tracking = TrackMouseEvent(&mut tme).is_ok();
            }
            let h = hit_at(hwnd, s, lparam);
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
            s.pressed = hit_at(hwnd, s, lparam).filter(|&i| s.buttons[i].enabled);
            if s.pressed.is_some() {
                SetCapture(hwnd);
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        WM_LBUTTONUP => {
            let pressed = s.pressed.take();
            let over = hit_at(hwnd, s, lparam);
            let cmd = pressed.filter(|&i| over == Some(i)).map(|i| s.buttons[i].cmd);
            let _ = ReleaseCapture();
            let _ = InvalidateRect(Some(hwnd), None, false);
            // No state reference is used after this: the parent may do anything, including destroying us.
            if let (Some(cmd), Ok(parent)) = (cmd, GetParent(hwnd)) {
                SendMessageW(parent, WM_COMMAND, Some(WPARAM(cmd as usize)), Some(LPARAM(hwnd.0 as isize)));
            }
        }
        WM_CAPTURECHANGED => {
            if s.pressed.take().is_some() {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
        _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
    }
    LRESULT(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_maps_x_to_button() {
        let (lefts, w) = edges(&[None; 3], 40, 8);
        assert_eq!((lefts.as_slice(), w), (&[0, 40, 80][..], 120));
        assert_eq!(hit(0, &lefts, 40), Some(0));
        assert_eq!(hit(39, &lefts, 40), Some(0));
        assert_eq!(hit(40, &lefts, 40), Some(1));
        assert_eq!(hit(119, &lefts, 40), Some(2));
        assert_eq!(hit(120, &lefts, 40), None);
        assert_eq!(hit(-1, &lefts, 40), None);
    }

    #[test]
    fn groups_get_gaps_around_them() {
        // [Back Forward] Up: a gap after the segment only.
        let (lefts, w) = edges(&[Some(0), Some(0), None], 36, 8);
        assert_eq!((lefts.as_slice(), w), (&[0, 36, 80][..], 116));
        // [List Grid] Terminal Connect More.
        let (lefts, w) = edges(&[Some(0), Some(0), None, None, None], 36, 8);
        assert_eq!((lefts.as_slice(), w), (&[0, 36, 80, 116, 152][..], 188));
        // Two segments side by side are kept apart.
        assert_eq!(edges(&[Some(0), Some(1)], 36, 8).0, [0, 44]);
        assert_eq!(hit(40, &[0, 44], 36), None, "the gap is no button");
    }

    static LAST_CMD: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    unsafe extern "system" fn parent_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_COMMAND {
            LAST_CMD.store(wparam.0, std::sync::atomic::Ordering::SeqCst);
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    #[test]
    fn click_sends_command_to_parent() {
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap().into();
            let wc = WNDCLASSW { lpfnWndProc: Some(parent_proc), hInstance: hinst, lpszClassName: w!("RexToolbarTestParent"), ..Default::default() };
            RegisterClassW(&wc);
            let parent = CreateWindowExW(
                WINDOW_EX_STYLE(0), w!("RexToolbarTestParent"), PCWSTR::null(), WS_OVERLAPPEDWINDOW,
                0, 0, 400, 200, None, None, Some(hinst), None,
            )
            .unwrap();
            let tb = create(parent, 7, &[(0xE72B, 11, "Back"), (0xE72A, 12, "Forward")]).unwrap();
            let _ = MoveWindow(tb, 0, 0, 200, 32, false);
            let bw = button_width(GetDpiForWindow(tb)) as isize;
            let at = LPARAM(bw + bw / 2 + (10 << 16));
            SendMessageW(tb, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(at));
            SendMessageW(tb, WM_LBUTTONUP, Some(WPARAM(0)), Some(at));
            assert_eq!(LAST_CMD.load(std::sync::atomic::Ordering::SeqCst), 12);
            set_enabled(tb, 11, false);
            LAST_CMD.store(0, std::sync::atomic::Ordering::SeqCst);
            let first = LPARAM(bw / 2 + (10 << 16));
            SendMessageW(tb, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(first));
            SendMessageW(tb, WM_LBUTTONUP, Some(WPARAM(0)), Some(first));
            assert_eq!(LAST_CMD.load(std::sync::atomic::Ordering::SeqCst), 0, "disabled button must not fire");
            DestroyWindow(parent).unwrap();
        }
    }

    #[test]
    fn create_enable_and_rect() {
        unsafe {
            let parent = CreateWindowExW(
                WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW,
                0, 0, 400, 200, None, None, None, None,
            )
            .unwrap();
            let tb = create(parent, 7, &[(0xE72B, 1, "Back"), (0xE72A, 2, "Forward")]).unwrap();
            let _ = MoveWindow(tb, 0, 0, 200, 32, false);
            let bw = button_width(GetDpiForWindow(tb));
            assert_eq!(width(tb), 2 * bw);
            set_enabled(tb, 2, false);
            assert!(!state(tb).unwrap().buttons[1].enabled);
            set_group(tb, &[1, 2]);
            set_checked(tb, 1, true);
            assert!(state(tb).unwrap().buttons[0].checked);
            assert_eq!(width(tb), 2 * bw, "a group alone adds no gap");
            set_theme(tb, &Theme::dark());
            let r = button_rect(tb, 2).unwrap();
            assert_eq!(r.right - r.left, bw);
            assert!(button_rect(tb, 9).is_none());
            DestroyWindow(parent).unwrap();
        }
    }
}
