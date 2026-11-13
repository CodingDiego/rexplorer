//! Row of glyph buttons (Segoe Fluent Icons), window class "RexToolbar".
//! A click sends WM_COMMAND(cmd) to the parent; each button has a tooltip. Buttons put in a group with
//! `set_group` share one pill-shaped segment; a gap separates a group from its neighbours.
//! `set_label` gives a button a text label right of its glyph (in the font set with WM_SETFONT) and optionally a
//! dropdown chevron. The entries `SEPARATOR` (a thin vertical line) and `SPACER` (takes the width left over, so
//! the buttons after it sit at the right edge) are not buttons.

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::Once;

use rexplorer_core::access;
use rexplorer_core::settings::Theme;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{ROLE_SYSTEM_BUTTONMENU, ROLE_SYSTEM_PUSHBUTTON, ROLE_SYSTEM_TOOLBAR, STATE_SYSTEM_HASPOPUP};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
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
    /// Opens a menu without a chevron (More): Down opens it, screen readers hear a menu button.
    menu: bool,
    /// Press-and-hold or Down asks the parent for this button's menu (WM_CONTEXTMENU): Back and Forward's history.
    hold: bool,
}

/// Timer of a press that may become a hold.
const HOLD_TIMER: usize = 1;
/// How long a press must last to open the menu, as Explorer's Back button.
const HOLD_MS: u32 = 500;

impl Button {
    fn opens_menu(&self) -> bool {
        self.dropdown || self.menu
    }

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
    acc: Option<access::Server>,
    /// Keyboard focus: the button (index into `buttons`) the arrow keys moved to, drawn while `focused`.
    focus: usize,
    focused: bool,
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
                    menu: false,
                    hold: false,
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
            acc: None,
            focus: 0,
            focused: false,
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

/// Marks the button that sends `cmd` as one that opens a menu, without a chevron.
pub fn set_menu(hwnd: HWND, cmd: u16) {
    unsafe {
        if let Some(s) = state(hwnd) {
            s.buttons.iter_mut().filter(|b| b.cmd == cmd).for_each(|b| b.menu = true);
        }
    }
}

/// Gives the buttons that send `cmds` a menu on press-and-hold and on Down, which the parent shows on WM_CONTEXTMENU.
pub fn set_hold(hwnd: HWND, cmds: &[u16]) {
    unsafe {
        if let Some(s) = state(hwnd) {
            s.buttons.iter_mut().filter(|b| cmds.contains(&b.cmd)).for_each(|b| b.hold = true);
        }
    }
}

/// The parent's WM_CONTEXTMENU at the center of button `i`, as a right-click there sends; no state reference may be
/// used after it.
unsafe fn hold_menu(hwnd: HWND, s: &State, i: usize) {
    let r = rect(hwnd, s, i);
    let mut p = POINT { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 };
    let _ = ClientToScreen(hwnd, &mut p);
    if let Ok(parent) = GetParent(hwnd) {
        let at = ((p.y as i16 as u16 as isize) << 16) | p.x as i16 as u16 as isize;
        SendMessageW(parent, WM_CONTEXTMENU, Some(WPARAM(hwnd.0 as usize)), Some(LPARAM(at)));
    }
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
            let mut changed = Vec::new();
            for (i, b) in s.buttons.iter_mut().enumerate().filter(|(_, b)| b.cmd == cmd && b.checked != checked) {
                b.checked = checked;
                changed.push(i);
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            state_changed(hwnd, &changed);
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
            let mut changed = Vec::new();
            for (i, b) in s.buttons.iter_mut().enumerate().filter(|(_, b)| b.cmd == cmd && b.enabled != enabled) {
                b.enabled = enabled;
                changed.push(i);
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            state_changed(hwnd, &changed);
        }
    }
}

/// Index of the k-th button that sends a command (separators and the spacer are no accessible items).
fn nth_button(buttons: &[Button], k: usize) -> Option<usize> {
    buttons.iter().enumerate().filter(|(_, b)| b.cmd != 0).nth(k).map(|(i, _)| i)
}

/// Accessible item index of the button at `i`.
fn item_of(buttons: &[Button], i: usize) -> usize {
    buttons[..i].iter().filter(|b| b.cmd != 0).count()
}

/// Raises STATECHANGE for the buttons at `indexes` (button indexes, mapped to item indexes), after the change is done.
fn state_changed(hwnd: HWND, indexes: &[usize]) {
    let Some(s) = (unsafe { state(hwnd) }) else { return };
    for &i in indexes {
        if s.buttons[i].cmd != 0 {
            access::event(EVENT_OBJECT_STATECHANGE, hwnd, Some(item_of(&s.buttons, i)));
        }
    }
}

/// Where the keyboard focus goes from button `cur` on Left/Right/Home/End, among the buttons `ok` marks focusable
/// (a command and enabled); nothing wraps.
fn step_focus(ok: &[bool], cur: usize, key: VIRTUAL_KEY) -> usize {
    let (Some(first), Some(last)) = (ok.iter().position(|&b| b), ok.iter().rposition(|&b| b)) else { return cur };
    let cur = cur.min(ok.len());
    match key {
        VK_LEFT => ok[..cur].iter().rposition(|&b| b).unwrap_or(cur),
        VK_RIGHT => ok.iter().skip(cur + 1).position(|&b| b).map_or(cur, |k| cur + 1 + k),
        VK_HOME => first,
        VK_END => last,
        _ => cur,
    }
}

fn focusable(buttons: &[Button]) -> Vec<bool> {
    buttons.iter().map(|b| b.cmd != 0 && b.enabled).collect()
}

unsafe fn acc_tree(hwnd: HWND, s: &State, t: &mut access::Tree) {
    let mut buf = [0u16; 64];
    let n = GetWindowTextW(hwnd, &mut buf) as usize;
    t.name = String::from_utf16_lossy(&buf[..n]);
    t.role = ROLE_SYSTEM_TOOLBAR;
    t.focusable = true;
    t.focus = (s.focused && s.focus < s.buttons.len() && s.buttons[s.focus].cmd != 0).then(|| item_of(&s.buttons, s.focus));
    for (i, b) in s.buttons.iter().enumerate().filter(|(_, b)| b.cmd != 0) {
        let tip = String::from_utf16_lossy(b.tip.split(|&c| c == 0).next().unwrap_or_default());
        // "Back (Alt+Left)": a reader says the name, and the key hint only as the shortcut.
        let (name, shortcut) = match tip.strip_suffix(')').and_then(|t| t.rsplit_once(" (")) {
            Some((n, k)) => (n.to_string(), k.to_string()),
            None => (tip, String::new()),
        };
        let mut state = 0;
        if !b.enabled {
            state |= STATE_SYSTEM_UNAVAILABLE.0;
        }
        if b.checked {
            state |= STATE_SYSTEM_CHECKED;
        }
        if b.opens_menu() {
            state |= STATE_SYSTEM_HASPOPUP;
        }
        t.items.push(access::Item {
            name,
            role: if b.opens_menu() { ROLE_SYSTEM_BUTTONMENU } else { ROLE_SYSTEM_PUSHBUTTON },
            state,
            rect: rect(hwnd, s, i),
            action: rexplorer_core::i18n::tr("tb.action_press"),
            shortcut,
            ..Default::default()
        });
    }
}

/// What a click on a button does; no state reference may be used after it (the parent may destroy us).
unsafe fn command(hwnd: HWND, cmd: u16) {
    if let Ok(parent) = GetParent(hwnd) {
        SendMessageW(parent, WM_COMMAND, Some(WPARAM(cmd as usize)), Some(LPARAM(hwnd.0 as isize)));
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
    let Ok(tip) = CreateWindowExW(WS_EX_TOPMOST | crate::rtl::layout(), TOOLTIPS_CLASSW, PCWSTR::null(), style, 0, 0, 0, 0, Some(hwnd), None, None, None) else {
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
    crate::rtl::match_dc(dc, wdc);
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
            let fr = RECT { left: r.left + m, top: r.top + my, right: r.right - m, bottom: r.bottom - my };
            theme::fill_round(dc, &fr, c, rad);
            if b.enabled && s.hover == Some(i) && s.pressed != Some(i) {
                theme::hot_outline(dc, &fr, t, rad, dpi);
            }
        }
        let lit = fill == Some(t.selection); // glyph on a selection fill (pressed is selection too in contrast)
        let text = match t.contrast {
            Some(c) if !b.enabled => c.gray,
            _ if !b.enabled => theme::blend(t.background, t.secondary_text, 0.5),
            _ if lit => theme::on_selection(t, t.text),
            _ => t.text,
        };
        SetTextColor(dc, COLORREF(text));
        let glyph = |c: u32| -> Vec<u16> { char::from_u32(c).map(|c| crate::rtl::glyph(c).to_string()).unwrap_or_default().encode_utf16().collect() };
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
            DrawTextW(dc, &mut label, &mut tr, crate::rtl::text(flags));
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
    if s.focused && s.focus < s.buttons.len() && s.buttons[s.focus].cmd != 0 {
        // DrawFocusRect inverts what is under it, so it shows on every fill and in the contrast themes.
        let r = rect(s.focus);
        let i = scale(4, dpi);
        SetTextColor(dc, COLORREF(t.text));
        SetBkColor(dc, COLORREF(t.background));
        let _ = DrawFocusRect(dc, &RECT { left: r.left + i, top: r.top + i, right: r.right - i, bottom: r.bottom - i });
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
        WM_GETOBJECT if access::is_client(lparam) => return s.acc.get_or_insert_with(|| access::Server::new(hwnd)).answer(wparam),
        access::WM_ACC_ITEMS => {
            acc_tree(hwnd, s, &mut *(lparam.0 as *mut access::Tree));
            return LRESULT(1);
        }
        access::WM_ACC_INVOKE => {
            let cmd = nth_button(&s.buttons, wparam.0).map(|i| &s.buttons[i]).filter(|b| b.enabled).map(|b| b.cmd);
            if let Some(cmd) = cmd {
                command(hwnd, cmd);
            }
        }
        WM_PAINT => paint(hwnd, s),
        WM_SETFOCUS | WM_KILLFOCUS => {
            s.focused = msg == WM_SETFOCUS;
            if s.focused
                && let Some(i) = focusable(&s.buttons).iter().position(|&b| b) {
                    s.focus = i;
                    access::event(EVENT_OBJECT_FOCUS, hwnd, Some(item_of(&s.buttons, i)));
                }
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let key = crate::rtl::key(VIRTUAL_KEY(wparam.0 as u16));
            let cur = s.buttons.get(s.focus).filter(|b| b.cmd != 0 && b.enabled);
            match key {
                VK_LEFT | VK_RIGHT | VK_HOME | VK_END if msg == WM_KEYDOWN => {
                    let to = step_focus(&focusable(&s.buttons), s.focus, key);
                    if to != s.focus {
                        s.focus = to;
                        let _ = InvalidateRect(Some(hwnd), None, false);
                        access::event(EVENT_OBJECT_FOCUS, hwnd, Some(item_of(&s.buttons, to)));
                    }
                }
                // What a click sends; Down and Alt+Down (a system key) open a dropdown button's menu only.
                VK_RETURN | VK_SPACE if msg == WM_KEYDOWN => {
                    if let Some(cmd) = cur.map(|b| b.cmd) {
                        command(hwnd, cmd);
                    }
                }
                VK_DOWN => {
                    if let Some(cmd) = cur.filter(|b| b.opens_menu()).map(|b| b.cmd) {
                        command(hwnd, cmd);
                    } else if cur.is_some_and(|b| b.hold) {
                        hold_menu(hwnd, s, s.focus);
                    }
                }
                // The main window hands the focus on to the view.
                VK_ESCAPE if msg == WM_KEYDOWN => {
                    if let Ok(parent) = GetParent(hwnd) {
                        let _ = SetFocus(Some(parent));
                    }
                }
                _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
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
            if let Some(i) = s.pressed {
                SetCapture(hwnd);
                if s.buttons[i].hold {
                    SetTimer(Some(hwnd), HOLD_TIMER, HOLD_MS, None);
                }
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        // Held long enough: the menu instead of the click.
        WM_TIMER if wparam.0 == HOLD_TIMER => {
            let _ = KillTimer(Some(hwnd), HOLD_TIMER);
            if let Some(i) = s.pressed.filter(|&i| s.buttons[i].hold) {
                s.pressed = None;
                let _ = ReleaseCapture();
                let _ = InvalidateRect(Some(hwnd), None, false);
                hold_menu(hwnd, s, i);
            }
        }
        WM_LBUTTONUP => {
            let _ = KillTimer(Some(hwnd), HOLD_TIMER);
            let pressed = s.pressed.take();
            let over = hit_at(hwnd, s, lparam);
            let cmd = pressed.filter(|&i| over == Some(i)).map(|i| s.buttons[i].cmd);
            let _ = ReleaseCapture();
            let _ = InvalidateRect(Some(hwnd), None, false);
            // No state reference is used after this: the parent may do anything, including destroying us.
            if let Some(cmd) = cmd {
                command(hwnd, cmd);
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

    static LAST_CONTEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    unsafe extern "system" fn parent_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_CONTEXTMENU {
            LAST_CONTEXT.store(wparam.0, std::sync::atomic::Ordering::SeqCst);
            return LRESULT(0);
        }
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
    fn right_click_reaches_the_parent_as_a_context_menu() {
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap().into();
            let wc = WNDCLASSW { lpfnWndProc: Some(parent_proc), hInstance: hinst, lpszClassName: w!("RexToolbarTestParent"), ..Default::default() };
            RegisterClassW(&wc);
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("RexToolbarTestParent"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 400, 200, None, None, Some(hinst), None).unwrap();
            let tb = create(parent, 7, &[(0xE72B, 11, "Back"), (0xE72A, 12, "Forward")]).unwrap();
            let _ = MoveWindow(tb, 0, 0, 200, 32, false);
            let at = LPARAM(10 + (10 << 16));
            SendMessageW(tb, WM_RBUTTONDOWN, Some(WPARAM(2)), Some(at));
            SendMessageW(tb, WM_RBUTTONUP, Some(WPARAM(0)), Some(at));
            assert_eq!(LAST_CONTEXT.load(std::sync::atomic::Ordering::SeqCst), tb.0 as usize);
            DestroyWindow(parent).unwrap();
        }
    }

    thread_local! {
        /// (message, wparam) of the WM_COMMAND and WM_CONTEXTMENU the hold test's parent got.
        static HOLD_NOTES: std::cell::RefCell<Vec<(u32, usize)>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    unsafe extern "system" fn hold_parent(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_CONTEXTMENU || msg == WM_COMMAND {
            HOLD_NOTES.with(|n| n.borrow_mut().push((msg, wparam.0)));
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    #[test]
    fn press_and_hold_and_down_open_the_menu_instead_of_clicking() {
        let take = || HOLD_NOTES.with(|n| std::mem::take(&mut *n.borrow_mut()));
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap().into();
            let wc = WNDCLASSW { lpfnWndProc: Some(hold_parent), hInstance: hinst, lpszClassName: w!("RexToolbarHoldParent"), ..Default::default() };
            RegisterClassW(&wc);
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("RexToolbarHoldParent"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 400, 200, None, None, Some(hinst), None).unwrap();
            let tb = create(parent, 7, &[(0xE72B, 21, "Back"), (0xE72A, 22, "Up")]).unwrap();
            let _ = MoveWindow(tb, 0, 0, 200, 32, false);
            set_hold(tb, &[21]);
            let bw = button_width(GetDpiForWindow(tb)) as isize;
            let (first, second) = (LPARAM(bw / 2 + (10 << 16)), LPARAM(bw + bw / 2 + (10 << 16)));
            let menu = (WM_CONTEXTMENU, tb.0 as usize);
            take();
            SendMessageW(tb, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(first));
            SendMessageW(tb, WM_TIMER, Some(WPARAM(HOLD_TIMER)), None);
            SendMessageW(tb, WM_LBUTTONUP, Some(WPARAM(0)), Some(first));
            assert_eq!(take(), [menu], "held: the menu, no click");
            // A button without a hold menu ignores the timer and clicks.
            SendMessageW(tb, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(second));
            SendMessageW(tb, WM_TIMER, Some(WPARAM(HOLD_TIMER)), None);
            SendMessageW(tb, WM_LBUTTONUP, Some(WPARAM(0)), Some(second));
            assert_eq!(take(), [(WM_COMMAND, 22)]);
            // Down on the focused hold button opens the same menu.
            SendMessageW(tb, WM_SETFOCUS, None, None);
            SendMessageW(tb, WM_KEYDOWN, Some(WPARAM(VK_DOWN.0 as usize)), None);
            assert_eq!(take(), [menu]);
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

    static ACC_CMD: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    unsafe extern "system" fn acc_parent_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_COMMAND {
            ACC_CMD.store(wparam.0, std::sync::atomic::Ordering::SeqCst);
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    #[test]
    fn accessible_buttons() {
        use crate::acc_test::*;
        use rexplorer_core::com::ComInit;
        use std::sync::atomic::Ordering::SeqCst;
        use windows::Win32::Foundation::{CO_E_OBJNOTCONNECTED, DISP_E_MEMBERNOTFOUND};
        let _com = ComInit::sta();
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap().into();
            let wc = WNDCLASSW { lpfnWndProc: Some(acc_parent_proc), hInstance: hinst, lpszClassName: w!("RexToolbarAccParent"), ..Default::default() };
            RegisterClassW(&wc);
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("RexToolbarAccParent"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 800, 200, None, None, Some(hinst), None).unwrap();
            let tb = create(parent, 7, &[(0xE72B, 11, "Back (Alt+Left)"), SEPARATOR, (0xE72A, 12, "Forward"), SPACER, (0xE7B3, 13, "Preview")]).unwrap();
            let _ = MoveWindow(tb, 0, 0, 600, 32, false);
            let _ = SetWindowTextW(tb, w!("Navigation"));
            set_label(tb, 13, "Preview", true);
            let acc = object(tb);
            let k = kids(&acc);
            // The tooltip's key hint is the item's shortcut, not part of its name.
            assert_eq!(k.iter().map(|k| k.name.as_str()).collect::<Vec<_>>(), ["Back", "Forward", "Preview"]);
            let v = |n: i32| windows::Win32::System::Variant::VARIANT::from(n);
            assert_eq!(acc.get_accKeyboardShortcut(&v(1)).unwrap().to_string(), "Alt+Left");
            assert_eq!(acc.get_accKeyboardShortcut(&v(2)).unwrap_err().code(), DISP_E_MEMBERNOTFOUND);
            assert_eq!(k.iter().map(|k| k.role).collect::<Vec<_>>(), [ROLE_SYSTEM_PUSHBUTTON, ROLE_SYSTEM_PUSHBUTTON, ROLE_SYSTEM_BUTTONMENU]);
            assert_ne!(k[2].state & STATE_SYSTEM_HASPOPUP, 0);
            let r = button_rect(tb, 13).unwrap();
            assert_eq!((k[2].rect.left, k[2].rect.top, k[2].rect.right, k[2].rect.bottom), (r.left, r.top, r.right, r.bottom));
            set_checked(tb, 11, true);
            assert_ne!(kid(&acc, 1).state & STATE_SYSTEM_CHECKED, 0);
            set_enabled(tb, 12, false);
            assert_ne!(kid(&acc, 2).state & STATE_SYSTEM_UNAVAILABLE.0, 0);
            let r0 = root(&acc);
            assert_eq!(r0.name, "Navigation");
            assert_eq!(r0.role, ROLE_SYSTEM_TOOLBAR);
            ACC_CMD.store(0, SeqCst);
            invoke(&acc, 1).unwrap();
            assert_eq!(ACC_CMD.load(SeqCst), 11, "the same command a click sends");
            ACC_CMD.store(0, SeqCst);
            assert_eq!(invoke(&acc, 2).unwrap_err().code(), DISP_E_MEMBERNOTFOUND);
            assert_eq!(ACC_CMD.load(SeqCst), 0);
            let hk = hook();
            set_enabled(tb, 12, true);
            assert!(hk.take().contains(&(EVENT_OBJECT_STATECHANGE, tb, 2)));
            drop(hk);
            DestroyWindow(parent).unwrap();
            assert_eq!(acc.accChildCount().unwrap_err().code(), CO_E_OBJNOTCONNECTED);
        }
    }

    #[test]
    fn step_focus_skips_unfocusable_and_never_wraps() {
        // [Back off] [sep] [Fwd] [Up] [spacer] [Menu]
        let ok = [false, false, true, true, false, true];
        assert_eq!(step_focus(&ok, 2, VK_LEFT), 2, "nothing before the first");
        assert_eq!(step_focus(&ok, 2, VK_RIGHT), 3);
        assert_eq!(step_focus(&ok, 3, VK_RIGHT), 5, "the spacer is skipped");
        assert_eq!(step_focus(&ok, 5, VK_RIGHT), 5, "no wrap");
        assert_eq!(step_focus(&ok, 5, VK_LEFT), 3);
        assert_eq!((step_focus(&ok, 3, VK_HOME), step_focus(&ok, 3, VK_END)), (2, 5));
        assert_eq!(step_focus(&[false; 3], 1, VK_END), 1, "nothing focusable");
        assert_eq!(step_focus(&ok, 3, VK_UP), 3);
    }

    static KEY_CMD: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    unsafe extern "system" fn key_parent_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_COMMAND {
            KEY_CMD.store(wparam.0, std::sync::atomic::Ordering::SeqCst);
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    #[test]
    fn keyboard_moves_presses_and_announces_focus() {
        use crate::acc_test::*;
        use rexplorer_core::com::ComInit;
        use std::sync::atomic::Ordering::SeqCst;
        let _com = ComInit::sta();
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap().into();
            let wc = WNDCLASSW { lpfnWndProc: Some(key_parent_proc), hInstance: hinst, lpszClassName: w!("RexToolbarKeyParent"), ..Default::default() };
            RegisterClassW(&wc);
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("RexToolbarKeyParent"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 800, 200, None, None, Some(hinst), None).unwrap();
            let tb = create(parent, 7, &[(1, 11, "Back"), SEPARATOR, (2, 12, "Forward"), (3, 13, "Sort")]).unwrap();
            let _ = MoveWindow(tb, 0, 0, 600, 32, false);
            set_label(tb, 13, "Sort", true);
            set_enabled(tb, 11, false);
            let key = |k: VIRTUAL_KEY, msg: u32| SendMessageW(tb, msg, Some(WPARAM(k.0 as usize)), None);
            let at = || state(tb).unwrap().focus;
            let hk = hook();
            SendMessageW(tb, WM_SETFOCUS, None, None);
            assert_eq!(at(), 2, "the first enabled button, not the disabled Back");
            assert!(hk.take().contains(&(EVENT_OBJECT_FOCUS, tb, 2)));
            assert_eq!(object(tb).accChildCount().unwrap(), 3);
            key(VK_RIGHT, WM_KEYDOWN);
            assert_eq!(at(), 3);
            assert!(hk.take().contains(&(EVENT_OBJECT_FOCUS, tb, 3)));
            key(VK_LEFT, WM_KEYDOWN);
            assert_eq!(at(), 2);
            KEY_CMD.store(0, SeqCst);
            key(VK_DOWN, WM_KEYDOWN);
            key(VK_DOWN, WM_SYSKEYDOWN);
            assert_eq!(KEY_CMD.load(SeqCst), 0, "Down opens dropdown buttons only");
            key(VK_RETURN, WM_KEYDOWN);
            assert_eq!(KEY_CMD.load(SeqCst), 12);
            KEY_CMD.store(0, SeqCst);
            set_menu(tb, 12);
            key(VK_DOWN, WM_KEYDOWN);
            assert_eq!(KEY_CMD.load(SeqCst), 12, "Down opens a menu button without a chevron (More)");
            key(VK_END, WM_KEYDOWN);
            KEY_CMD.store(0, SeqCst);
            key(VK_SPACE, WM_KEYDOWN);
            assert_eq!(KEY_CMD.load(SeqCst), 13);
            KEY_CMD.store(0, SeqCst);
            key(VK_DOWN, WM_SYSKEYDOWN);
            assert_eq!(KEY_CMD.load(SeqCst), 13, "Alt+Down opens the dropdown");
            KEY_CMD.store(0, SeqCst);
            key(VK_ESCAPE, WM_KEYDOWN);
            assert_eq!(KEY_CMD.load(SeqCst), 0, "Esc presses nothing");
            SendMessageW(tb, WM_KILLFOCUS, None, None);
            assert!(!state(tb).unwrap().focused);
            drop(hk);
            DestroyWindow(parent).unwrap();
        }
    }

    #[test]
    fn nth_button_skips_separators_and_spacer() {
        let b = |glyph, cmd| Button { glyph, cmd, tip: vec![0], enabled: true, checked: false, group: None, label: vec![], dropdown: false, menu: false, hold: false };
        let v = [b(1, 11), b(0, 0), b(2, 12), b(1, 0), b(3, 13)];
        assert_eq!([0, 1, 2, 3].map(|k| nth_button(&v, k)), [Some(0), Some(2), Some(4), None]);
    }
}
