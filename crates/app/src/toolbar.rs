//! Row of glyph buttons (Segoe Fluent Icons), window class "RexToolbar".
//! A click sends WM_COMMAND(cmd) to the parent; each button has a tooltip. Buttons put in a group with
//! `set_group` share one pill-shaped segment; a gap separates a group from its neighbours.
//! `set_label` gives a button a text label right of its glyph (in the font set with WM_SETFONT) and optionally a
//! dropdown chevron. The entries `SEPARATOR` (a thin vertical line) and `SPACER` (takes the width left over, so
//! the buttons after it sit at the right edge) are not buttons.

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

/// Entry for `create`: a 1 px vertical line between groups of buttons.
pub const SEPARATOR: (u32, u16, &str) = (0, 0, "");
/// Entry for `create`: empty space taking whatever width is left, pushing the following buttons right.
pub const SPACER: (u32, u16, &str) = (1, 0, "");
/// Test hook (the smoke test reads button states): wparam = cmd; returns -1 when no button sends it, else
/// 1 if enabled plus 2 if checked.
pub const TBM_STATE: u32 = WM_APP + 0x44;
/// Test hook (the smoke test clicks buttons): wparam = cmd; returns the client x of the button's center, or -1.
pub const TBM_CENTER: u32 = WM_APP + 0x45;

struct Button {
    glyph: u32,
    cmd: u16,
    tip: Vec<u16>,
    enabled: bool,
    /// Drawn filled with the selection color (the active view).
    checked: bool,
    group: Option<usize>,
    /// Text right of the glyph; empty for a glyph-only button.
    label: Vec<u16>,
    /// A chevron after the label: the button opens a menu.
    dropdown: bool,
}

impl Button {
    fn is_separator(&self) -> bool {
        self.cmd == 0 && self.glyph == SEPARATOR.0
    }

    fn is_spacer(&self) -> bool {
        self.cmd == 0 && self.glyph == SPACER.0
    }
}

struct State {
    buttons: Vec<Button>,
    groups: usize,
    hover: Option<usize>,
    pressed: Option<usize>,
    tracking: bool,
    theme: Theme,
    font: HFONT,
    /// The chevron's smaller glyph font.
    small: HFONT,
    font_dpi: u32,
    /// The labels' font, set with WM_SETFONT; not owned.
    ui_font: HFONT,
    tip: HWND,
}

fn scale(px: i32, dpi: u32) -> i32 {
    px * dpi as i32 / 96
}

pub fn button_width(dpi: u32) -> i32 {
    scale(36, dpi)
}

/// Width of a button with a label `text_w` pixels wide (0: none) and maybe a chevron, at `dpi`.
fn labelled_width(text_w: i32, dropdown: bool, dpi: u32) -> i32 {
    if text_w == 0 && !dropdown {
        return button_width(dpi);
    }
    let text = if text_w > 0 { scale(8, dpi) + text_w } else { 0 };
    scale(12 + 16, dpi) + text + if dropdown { scale(6 + 10, dpi) } else { 0 } + scale(12, dpi)
}

/// Left edge of each entry and the total width: entries side by side, `gap` wherever a group starts or ends.
fn edges(widths: &[i32], groups: &[Option<usize>], gap: i32) -> (Vec<i32>, i32) {
    let mut x = 0;
    let mut lefts = Vec::with_capacity(groups.len());
    for (i, g) in groups.iter().enumerate() {
        if i > 0 && groups[i - 1] != *g && (g.is_some() || groups[i - 1].is_some()) {
            x += gap;
        }
        lefts.push(x);
        x += widths[i];
    }
    (lefts, x)
}

/// Entry under client-space `x`.
fn hit(x: i32, lefts: &[i32], widths: &[i32]) -> Option<usize> {
    lefts.iter().zip(widths).position(|(&l, &w)| x >= l && x < l + w)
}

/// Left edges, widths and the width the entries need (the spacer at zero). The spacer takes the client width
/// left over.
unsafe fn geometry(hwnd: HWND, s: &State) -> (Vec<i32>, Vec<i32>, i32) {
    let dpi = GetDpiForWindow(hwnd);
    let dc = GetDC(Some(hwnd));
    let font = if s.ui_font.is_invalid() { HFONT(GetStockObject(DEFAULT_GUI_FONT).0) } else { s.ui_font };
    let old = SelectObject(dc, font.into());
    let mut widths: Vec<i32> = s
        .buttons
        .iter()
        .map(|b| {
            if b.is_separator() {
                scale(17, dpi)
            } else if b.is_spacer() {
                0
            } else {
                let mut size = windows::Win32::Foundation::SIZE::default();
                if !b.label.is_empty() {
                    let _ = GetTextExtentPoint32W(dc, &b.label, &mut size);
                }
                labelled_width(size.cx, b.dropdown, dpi)
            }
        })
        .collect();
    SelectObject(dc, old);
    ReleaseDC(Some(hwnd), dc);
    let groups: Vec<Option<usize>> = s.buttons.iter().map(|b| b.group).collect();
    let (_, need) = edges(&widths, &groups, scale(8, dpi));
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    if let Some(i) = s.buttons.iter().position(Button::is_spacer) {
        widths[i] = (rc.right - need).max(0);
    }
    let (lefts, _) = edges(&widths, &groups, scale(8, dpi));
    (lefts, widths, need)
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
                    label: Vec::new(),
                    dropdown: false,
                })
                .collect(),
            groups: 0,
            hover: None,
            pressed: None,
            tracking: false,
            theme: Theme::light(),
            font: HFONT::default(),
            small: HFONT::default(),
            font_dpi: 0,
            ui_font: HFONT::default(),
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
    unsafe { state(hwnd).map_or(0, |s| geometry(hwnd, s).2) }
}

/// Gives the button that sends `cmd` a text label after its glyph, and a dropdown chevron when `dropdown`.
pub fn set_label(hwnd: HWND, cmd: u16, label: &str, dropdown: bool) {
    unsafe {
        if let Some(s) = state(hwnd) {
            for b in s.buttons.iter_mut().filter(|b| b.cmd == cmd) {
                b.label = label.encode_utf16().collect();
                b.dropdown = dropdown;
            }
            update_tool_rects(hwnd, s);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
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
    let (lefts, widths, _) = geometry(hwnd, s);
    RECT { left: lefts[i], top: 0, right: lefts[i] + widths[i], bottom: rc.bottom }
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
        for f in [s.font, s.small] {
            if !f.is_invalid() {
                let _ = DeleteObject(f.into());
            }
        }
        s.font = theme::icon_font(dpi, 16);
        s.small = theme::icon_font(dpi, 10);
        s.font_dpi = dpi;
    }
    let (lefts, widths, _) = geometry(hwnd, s);
    let rect = |i: usize| RECT { left: lefts[i], top: 0, right: lefts[i] + widths[i], bottom: ch };
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
        let members: Vec<RECT> = (0..s.buttons.len()).filter(|&i| s.buttons[i].group == Some(g)).map(rect).collect();
        if let (Some(a), Some(z)) = (members.first(), members.last()) {
            theme::fill_round(dc, &RECT { left: a.left, top: 0, right: z.right, bottom: ch }, t.surface, radius);
        }
    }
    let ui_font = if s.ui_font.is_invalid() { HFONT(GetStockObject(DEFAULT_GUI_FONT).0) } else { s.ui_font };
    for (i, b) in s.buttons.iter().enumerate() {
        let r = rect(i);
        if b.is_spacer() {
            continue;
        }
        if b.is_separator() {
            let (x, h) = ((r.left + r.right) / 2, ch * 6 / 10);
            let line = CreateSolidBrush(COLORREF(t.border));
            FillRect(dc, &RECT { left: x, top: (ch - h) / 2, right: x + scale(1, dpi).max(1), bottom: (ch + h) / 2 }, line);
            let _ = DeleteObject(line.into());
            continue;
        }
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
        let glyph = |c: u32| -> Vec<u16> { char::from_u32(c).map(|c| c.to_string()).unwrap_or_default().encode_utf16().collect() };
        let mut g = glyph(b.glyph);
        let flags = DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX;
        if b.label.is_empty() && !b.dropdown {
            let mut tr = r;
            DrawTextW(dc, &mut g, &mut tr, DT_CENTER | flags);
            continue;
        }
        // Glyph, label, chevron, left to right.
        let mut x = r.left + scale(12, dpi);
        let mut tr = RECT { left: x, right: x + scale(16, dpi), ..r };
        DrawTextW(dc, &mut g, &mut tr, DT_CENTER | flags);
        x = tr.right;
        if !b.label.is_empty() {
            SelectObject(dc, ui_font.into());
            let mut label = b.label.clone();
            let mut tr = RECT { left: x + scale(8, dpi), right: r.right, ..r };
            DrawTextW(dc, &mut label, &mut tr, flags);
            let mut size = windows::Win32::Foundation::SIZE::default();
            let _ = GetTextExtentPoint32W(dc, &b.label, &mut size);
            x = tr.left + size.cx;
        }
        if b.dropdown {
            SelectObject(dc, s.small.into());
            let mut c = glyph(0xE70D);
            let mut tr = RECT { left: x + scale(6, dpi), right: x + scale(16, dpi), ..r };
            DrawTextW(dc, &mut c, &mut tr, DT_CENTER | flags);
        }
        SelectObject(dc, s.font.into());
    }
    SelectObject(dc, old_font);
    let _ = BitBlt(wdc, 0, 0, cw, ch, Some(dc), 0, 0, SRCCOPY);
    SelectObject(dc, old_bmp);
    let _ = DeleteObject(bmp.into());
    let _ = DeleteDC(dc);
    let _ = EndPaint(hwnd, &ps);
}

/// The button (not a separator or the spacer) under the mouse position in `lparam`.
unsafe fn hit_at(hwnd: HWND, s: &State, lparam: LPARAM) -> Option<usize> {
    let (lefts, widths, _) = geometry(hwnd, s);
    hit(lparam.0 as i16 as i32, &lefts, &widths).filter(|&i| s.buttons[i].cmd != 0)
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
            for f in [s.font, s.small] {
                if !f.is_invalid() {
                    let _ = DeleteObject(f.into());
                }
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
        WM_SETFONT => {
            s.ui_font = HFONT(wparam.0 as *mut c_void);
            update_tool_rects(hwnd, s);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        TBM_STATE => {
            let Some(b) = s.buttons.iter().find(|b| b.cmd != 0 && b.cmd as usize == wparam.0) else { return LRESULT(-1) };
            return LRESULT(b.enabled as isize | (b.checked as isize) << 1);
        }
        TBM_CENTER => {
            let Some(i) = s.buttons.iter().position(|b| b.cmd != 0 && b.cmd as usize == wparam.0) else { return LRESULT(-1) };
            let r = rect(hwnd, s, i);
            return LRESULT(((r.left + r.right) / 2) as isize);
        }
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
        let (lefts, w) = edges(&[40; 3], &[None; 3], 8);
        assert_eq!((lefts.as_slice(), w), (&[0, 40, 80][..], 120));
        let widths = [40; 3];
        assert_eq!(hit(0, &lefts, &widths), Some(0));
        assert_eq!(hit(39, &lefts, &widths), Some(0));
        assert_eq!(hit(40, &lefts, &widths), Some(1));
        assert_eq!(hit(119, &lefts, &widths), Some(2));
        assert_eq!(hit(120, &lefts, &widths), None);
        assert_eq!(hit(-1, &lefts, &widths), None);
        // Mixed widths: a labelled button, a separator, a glyph button.
        let (lefts, w) = edges(&[90, 17, 36], &[None; 3], 8);
        assert_eq!((lefts.as_slice(), w), (&[0, 90, 107][..], 143));
        assert_eq!(hit(100, &lefts, &[90, 17, 36]), Some(1));
    }

    #[test]
    fn groups_get_gaps_around_them() {
        // [Back Forward] Up: a gap after the segment only.
        let (lefts, w) = edges(&[36; 3], &[Some(0), Some(0), None], 8);
        assert_eq!((lefts.as_slice(), w), (&[0, 36, 80][..], 116));
        // [List Grid] Terminal Connect More.
        let (lefts, w) = edges(&[36; 5], &[Some(0), Some(0), None, None, None], 8);
        assert_eq!((lefts.as_slice(), w), (&[0, 36, 80, 116, 152][..], 188));
        // Two segments side by side are kept apart.
        assert_eq!(edges(&[36; 2], &[Some(0), Some(1)], 8).0, [0, 44]);
        assert_eq!(hit(40, &[0, 44], &[36, 36]), None, "the gap is no button");
    }

    #[test]
    fn label_and_chevron_widen_a_button() {
        assert_eq!(labelled_width(0, false, 96), 36);
        assert_eq!(labelled_width(30, false, 96), 12 + 16 + 8 + 30 + 12);
        assert_eq!(labelled_width(30, true, 96), 12 + 16 + 8 + 30 + 16 + 12);
        assert_eq!(labelled_width(0, true, 192), 2 * (12 + 16 + 16 + 12));
    }

    #[test]
    fn separators_spacer_labels_and_hooks() {
        unsafe {
            let parent = CreateWindowExW(
                WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW,
                0, 0, 800, 200, None, None, None, None,
            )
            .unwrap();
            let tb = create(parent, 7, &[(0xE710, 1, "New"), SEPARATOR, (0xE8C6, 2, "Cut"), SPACER, (0xE7B3, 3, "Preview")]).unwrap();
            let _ = MoveWindow(tb, 0, 0, 600, 32, false);
            let bw = button_width(GetDpiForWindow(tb));
            let before = width(tb);
            set_label(tb, 1, "New", true);
            assert!(width(tb) > before + bw / 2, "label and chevron add width: {before} -> {}", width(tb));
            // The spacer pushes the last button to the right edge.
            let r = button_rect(tb, 3).unwrap();
            let mut tbr = RECT::default();
            let _ = GetWindowRect(tb, &mut tbr);
            assert_eq!(r.right, tbr.right);
            assert_eq!(SendMessageW(tb, TBM_STATE, Some(WPARAM(2)), None).0, 1);
            set_enabled(tb, 2, false);
            set_checked(tb, 3, true);
            assert_eq!(SendMessageW(tb, TBM_STATE, Some(WPARAM(2)), None).0, 0);
            assert_eq!(SendMessageW(tb, TBM_STATE, Some(WPARAM(3)), None).0, 3);
            assert_eq!(SendMessageW(tb, TBM_STATE, Some(WPARAM(0)), None).0, -1, "a separator is no button");
            let cx = SendMessageW(tb, TBM_CENTER, Some(WPARAM(3)), None).0 as i32;
            assert_eq!(cx, (600 - bw + 600) / 2);
            DestroyWindow(parent).unwrap();
        }
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
