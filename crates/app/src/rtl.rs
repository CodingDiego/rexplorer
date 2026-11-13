//! Right-to-left layout (#39), on only when the language file's `[meta]` says `direction=rtl`: a left-to-right
//! language takes none of these branches. Windows mirrors a WS_EX_LAYOUTRTL window and every child of it (client
//! coordinates, mouse positions, painting run from the right), so the controls keep their left-to-right layout,
//! hit-testing and drag arithmetic unchanged. What the mirroring leaves undone is here.

use rexplorer_core::i18n::is_rtl;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{DRAW_TEXT_FORMAT, DT_RTLREADING, DC_LAYOUT, GetLayout, HDC, SetLayout};
use windows::Win32::UI::Controls::{TASKDIALOG_FLAGS, TDF_RTL_LAYOUT};
use windows::Win32::UI::Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_LEFT, VK_RIGHT};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongW, MB_RIGHT, MB_RTLREADING, MESSAGEBOX_STYLE, TPM_LAYOUTRTL, TRACK_POPUP_MENU_FLAGS, WINDOW_EX_STYLE, WS_EX_LAYOUTRTL,
    WS_EX_RTLREADING,
};

/// Extended style of a top-level window or dialog: mirrored, its children with it.
pub fn layout() -> WINDOW_EX_STYLE {
    if is_rtl() { WS_EX_LAYOUTRTL } else { WINDOW_EX_STYLE(0) }
}

/// Extended style of an Edit holding the user's own words (search, palette); paths stay left to right.
pub fn reading() -> WINDOW_EX_STYLE {
    if is_rtl() { WS_EX_RTLREADING } else { WINDOW_EX_STYLE(0) }
}

/// DrawText flags for words (not glyphs).
pub fn text(f: DRAW_TEXT_FORMAT) -> DRAW_TEXT_FORMAT {
    if is_rtl() { f | DT_RTLREADING } else { f }
}

pub fn message_box() -> MESSAGEBOX_STYLE {
    if is_rtl() { MB_RTLREADING | MB_RIGHT } else { MESSAGEBOX_STYLE(0) }
}

pub fn task_dialog() -> TASKDIALOG_FLAGS {
    if is_rtl() { TDF_RTL_LAYOUT } else { TASKDIALOG_FLAGS(0) }
}

/// A menu opens leftwards from its point: the points are the start (logical left) of what it drops from.
/// TPM_LAYOUTRTL mirrors the alignment flags too: its default left alignment already puts the menu's right edge
/// at the point (TPM_RIGHTALIGN with it opened the menu rightwards, seen live).
pub fn menu() -> TRACK_POPUP_MENU_FLAGS {
    if is_rtl() { TPM_LAYOUTRTL } else { TRACK_POPUP_MENU_FLAGS(0) }
}

/// Where a menu under screen rectangle `r` starts (see `menu`).
pub fn menu_x(r: &RECT) -> i32 {
    start_x(r, is_rtl())
}

/// Gives a memory DC `like`'s layout; call after its bitmap is selected. A double buffer drawn left to right
/// comes back flipped (text mirrored) from the BitBlt to a mirrored window DC, and a picture blitted from a plain
/// DC into a mirrored buffer comes out mirrored: both DCs of a copy take the same layout.
pub unsafe fn match_dc(mem: HDC, like: HDC) {
    if is_rtl() {
        unsafe { SetLayout(mem, DC_LAYOUT(GetLayout(like))) };
    }
}

/// A Segoe Fluent Icons code point that points along the reading direction, turned around.
pub fn glyph(c: char) -> char {
    mirror_glyph(c, is_rtl())
}

/// Left and Right swap meaning in a mirrored strip (tabs, breadcrumb, toolbars, Home): Left moves to the next item.
pub fn key(k: VIRTUAL_KEY) -> VIRTUAL_KEY {
    swap_arrow(k, is_rtl())
}

fn start_x(r: &RECT, rtl: bool) -> i32 {
    if rtl { r.right } else { r.left }
}

const GLYPH_PAIRS: [(char, char); 2] = [('\u{E72B}', '\u{E72A}'), ('\u{E76B}', '\u{E76C}')]; // Back/Forward, ChevronLeft/Right

fn mirror_glyph(c: char, rtl: bool) -> char {
    if !rtl {
        return c;
    }
    GLYPH_PAIRS.iter().find_map(|&(a, b)| if c == a { Some(b) } else if c == b { Some(a) } else { None }).unwrap_or(c)
}

fn swap_arrow(k: VIRTUAL_KEY, rtl: bool) -> VIRTUAL_KEY {
    match k {
        VK_LEFT if rtl => VK_RIGHT,
        VK_RIGHT if rtl => VK_LEFT,
        _ => k,
    }
}

/// Screen rectangle `r` in the coordinates of a window (or DC) whose (0, 0) is at screen point `origin`: a mirrored
/// one counts x leftwards from there, as ClientToScreen of its (0, 0) is its right edge.
pub fn screen_to_local(r: RECT, origin: POINT, mirrored: bool) -> RECT {
    let (top, bottom) = (r.top - origin.y, r.bottom - origin.y);
    if mirrored {
        RECT { left: origin.x - r.right, top, right: origin.x - r.left, bottom }
    } else {
        RECT { left: r.left - origin.x, top, right: r.right - origin.x, bottom }
    }
}

/// The window lays out right to left (WS_EX_LAYOUTRTL, set or inherited).
pub fn mirrored(hwnd: HWND) -> bool {
    unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_LAYOUTRTL.0 != 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rexplorer_core::i18n::Table;
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_DOWN, VK_HOME};

    /// The hidden right-to-left test language (crates/core/tests/lang/xr.ini), never shipped.
    fn test_language_rtl() -> bool {
        Table::load(&std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../core/tests"), "xr").rtl()
    }

    #[test]
    fn rtl_language_swaps_left_and_right() {
        let rtl = test_language_rtl();
        assert!(rtl);
        assert_eq!((swap_arrow(VK_LEFT, rtl), swap_arrow(VK_RIGHT, rtl)), (VK_RIGHT, VK_LEFT));
        assert_eq!((swap_arrow(VK_HOME, rtl), swap_arrow(VK_DOWN, rtl)), (VK_HOME, VK_DOWN));
        assert_eq!((swap_arrow(VK_LEFT, false), swap_arrow(VK_RIGHT, false)), (VK_LEFT, VK_RIGHT), "left to right: untouched");
    }

    #[test]
    fn rtl_language_turns_direction_glyphs() {
        let rtl = test_language_rtl();
        assert_eq!((mirror_glyph('\u{E72B}', rtl), mirror_glyph('\u{E72A}', rtl)), ('\u{E72A}', '\u{E72B}'), "Back and Forward");
        assert_eq!((mirror_glyph('\u{E76C}', rtl), mirror_glyph('\u{E76B}', rtl)), ('\u{E76B}', '\u{E76C}'), "chevrons");
        assert_eq!(mirror_glyph('\u{E70D}', rtl), '\u{E70D}', "a down chevron has no direction");
        assert_eq!(mirror_glyph('\u{E72B}', false), '\u{E72B}');
    }

    #[test]
    fn mirrored_rects_count_from_the_right() {
        let rtl = test_language_rtl();
        // A 100 px card at screen x 210..310 in a window whose client spans screen x 200..600.
        let card = RECT { left: 210, top: 50, right: 310, bottom: 90 };
        assert_eq!(screen_to_local(card, POINT { x: 200, y: 40 }, false), RECT { left: 10, top: 10, right: 110, bottom: 50 });
        // Mirrored, the client's (0, 0) is its right edge, screen x 600: the card is 290..390 from there.
        assert_eq!(screen_to_local(card, POINT { x: 600, y: 40 }, rtl), RECT { left: 290, top: 10, right: 390, bottom: 50 });
        assert_eq!((start_x(&card, false), start_x(&card, rtl)), (210, 310), "a menu under it starts at its reading start");
    }
}
