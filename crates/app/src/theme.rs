//! Applies a `rexplorer_core::settings::Theme` to the main window and the common controls.
#![allow(dead_code)]

use rexplorer_core::settings::Theme;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, HSTRING, PCWSTR, w};

pub struct Brushes {
    pub background: HBRUSH,
    pub surface: HBRUSH,
    pub selection: HBRUSH,
    pub accent: HBRUSH,
}

impl Brushes {
    pub fn new(t: &Theme) -> Brushes {
        unsafe {
            Brushes {
                background: CreateSolidBrush(COLORREF(t.background)),
                surface: CreateSolidBrush(COLORREF(t.surface)),
                selection: CreateSolidBrush(COLORREF(t.selection)),
                accent: CreateSolidBrush(COLORREF(t.accent)),
            }
        }
    }
}

impl Drop for Brushes {
    fn drop(&mut self) {
        unsafe {
            for b in [self.background, self.surface, self.selection, self.accent] {
                let _ = DeleteObject(b.into());
            }
        }
    }
}

fn dwm_set<T>(hwnd: HWND, attr: DWMWINDOWATTRIBUTE, v: &T) {
    // Older builds reject some attributes; ignored on purpose.
    let _ = unsafe { DwmSetWindowAttribute(hwnd, attr, v as *const T as *const _, size_of::<T>() as u32) };
}

pub fn apply_window(hwnd: HWND, t: &Theme) {
    dwm_set(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &BOOL::from(t.dark));
    let backdrop = if t.mica { DWMSBT_MAINWINDOW } else { DWMSBT_AUTO };
    dwm_set(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop);
    // The system title bar and frame take the header's color, so they read as one bar.
    dwm_set(hwnd, DWMWA_CAPTION_COLOR, &COLORREF(t.background));
    dwm_set(hwnd, DWMWA_BORDER_COLOR, &COLORREF(t.background));
    let _ = unsafe {
        SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED)
    };
}

pub fn apply_control(hwnd: HWND, t: &Theme) {
    let name = if t.dark { w!("DarkMode_Explorer") } else { w!("Explorer") };
    let _ = unsafe { SetWindowTheme(hwnd, name, None) };
}

pub fn apply_list(list: HWND, t: &Theme) {
    apply_control(list, t);
    unsafe {
        // No background of its own: the list's custom draw paints the card, and icons then draw transparently
        // (with a color, the icon views paint it behind each icon, over the selection pill).
        for (msg, c) in [(LVM_SETBKCOLOR, CLR_NONE as u32), (LVM_SETTEXTBKCOLOR, CLR_NONE as u32), (LVM_SETTEXTCOLOR, t.text)] {
            SendMessageW(list, msg, Some(WPARAM(0)), Some(LPARAM(c as isize)));
        }
        let header = HWND(SendMessageW(list, LVM_GETHEADER, None, None).0 as *mut _);
        if !header.is_invalid() {
            let name = if t.dark { w!("DarkMode_ItemsView") } else { w!("ItemsView") };
            let _ = SetWindowTheme(header, name, None);
        }
        let _ = InvalidateRect(Some(list), None, true);
    }
}

/// The main window's Edits (address, search) sit on surface-colored fields.
pub fn ctl_color(hdc: HDC, t: &Theme, b: &Brushes) -> HBRUSH {
    unsafe {
        SetTextColor(hdc, COLORREF(t.text));
        SetBkColor(hdc, COLORREF(t.surface));
    }
    b.surface
}

/// Fills `r` with `color`, corners rounded by `radius` pixels (no outline).
pub unsafe fn fill_round(dc: HDC, r: &RECT, color: u32, radius: i32) {
    unsafe {
        let brush = CreateSolidBrush(COLORREF(color));
        let (ob, op) = (SelectObject(dc, brush.into()), SelectObject(dc, GetStockObject(NULL_PEN)));
        // With a null pen RoundRect leaves the right and bottom edge out.
        let _ = RoundRect(dc, r.left, r.top, r.right + 1, r.bottom + 1, 2 * radius, 2 * radius);
        SelectObject(dc, ob);
        SelectObject(dc, op);
        let _ = DeleteObject(brush.into());
    }
}

/// Clips a child window to a rounded rectangle of its current size (a "card"); call again after each resize.
pub fn round_corners(hwnd: HWND, radius: i32) {
    unsafe {
        let mut r = RECT::default();
        if GetWindowRect(hwnd, &mut r).is_ok() {
            // The region belongs to the window from here on.
            let rgn = CreateRoundRectRgn(0, 0, r.right - r.left + 1, r.bottom - r.top + 1, 2 * radius, 2 * radius);
            SetWindowRgn(hwnd, Some(rgn), true);
        }
    }
}

/// Section headers: the theme's face, semibold.
pub fn header_font(t: &Theme, dpi: u32, pt: i32) -> HFONT {
    let f = HSTRING::from(t.font.as_str());
    unsafe {
        CreateFontW(
            -muldiv(pt, dpi as i32, 72), 0, 0, 0, FW_SEMIBOLD.0 as i32, 0, 0, 0, DEFAULT_CHARSET,
            FONT_OUTPUT_PRECISION(0), FONT_CLIP_PRECISION(0), CLEARTYPE_QUALITY, 0, PCWSTR(f.as_ptr()),
        )
    }
}

/// Rounded `a * b / c` (what MulDiv does; its feature is not enabled here).
fn muldiv(a: i32, b: i32, c: i32) -> i32 {
    ((a as i64 * b as i64 + c as i64 / 2) / c as i64) as i32
}

fn create_font(height: i32, face: &str) -> HFONT {
    let f = HSTRING::from(face);
    unsafe {
        CreateFontW(
            height, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0, DEFAULT_CHARSET,
            FONT_OUTPUT_PRECISION(0), FONT_CLIP_PRECISION(0), CLEARTYPE_QUALITY, 0,
            PCWSTR(f.as_ptr()),
        )
    }
}

/// True when GDI realizes `face` itself rather than substituting another face.
fn face_exists(face: &str) -> bool {
    unsafe {
        let font = create_font(-12, face);
        let dc = CreateCompatibleDC(None);
        let old = SelectObject(dc, font.into());
        let mut buf = [0u16; 64];
        let n = (GetTextFaceW(dc, Some(&mut buf)).max(1) - 1) as usize;
        SelectObject(dc, old);
        let _ = DeleteDC(dc);
        let _ = DeleteObject(font.into());
        String::from_utf16_lossy(&buf[..n]).eq_ignore_ascii_case(face)
    }
}

fn first_existing(height: i32, primary: &str, fallback: &str) -> HFONT {
    create_font(height, if face_exists(primary) { primary } else { fallback })
}

pub fn ui_font(t: &Theme, dpi: u32) -> HFONT {
    let h = -muldiv(t.font_size, dpi as i32, 72);
    let f = create_font(h, &t.font);
    if f.is_invalid() { create_font(h, "Segoe UI") } else { f }
}

pub fn icon_font(dpi: u32, px: i32) -> HFONT {
    first_existing(-muldiv(px, dpi as i32, 96), "Segoe Fluent Icons", "Segoe MDL2 Assets")
}

pub fn mono_font(dpi: u32, pt: i32) -> HFONT {
    first_existing(-muldiv(pt, dpi as i32, 72), "Cascadia Mono", "Consolas")
}

pub fn blend(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    (0..3).fold(0, |acc, i| {
        let (x, y) = (((a >> (8 * i)) & 0xFF) as f32, ((b >> (8 * i)) & 0xFF) as f32);
        acc | (((x + (y - x) * t).round() as u32) << (8 * i))
    })
}

pub fn hover(t: &Theme) -> u32 {
    blend(t.background, t.text, 0.08)
}

pub fn pressed(t: &Theme) -> u32 {
    blend(t.background, t.text, 0.14)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pump() {
        unsafe {
            let mut m = MSG::default();
            while PeekMessageW(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&m);
                DispatchMessageW(&m);
            }
        }
    }

    fn face_of(f: HFONT) -> String {
        unsafe {
            let mut lf = LOGFONTW::default();
            let n = GetObjectW(f.into(), size_of::<LOGFONTW>() as i32, Some(&mut lf as *mut _ as *mut _));
            assert!(n > 0, "GetObjectW failed");
            let len = lf.lfFaceName.iter().position(|&c| c == 0).unwrap_or(32);
            String::from_utf16_lossy(&lf.lfFaceName[..len])
        }
    }

    #[test]
    fn blend_endpoints_and_mid() {
        assert_eq!(blend(0x102030, 0xF0E0D0, 0.0), 0x102030);
        assert_eq!(blend(0x102030, 0xF0E0D0, 1.0), 0xF0E0D0);
        assert_eq!(blend(0x000000, 0xFEFEFE, 0.5), 0x7F7F7F);
        assert_eq!(blend(0x000000, 0xFFFFFF, 2.0), 0xFFFFFF);
    }

    #[test]
    fn fonts_valid() {
        let t = Theme::light();
        let ui = ui_font(&t, 96);
        assert!(!ui.is_invalid());
        println!("ui face: {} (requested {})", face_of(ui), t.font);
        let ic = icon_font(96, 16);
        assert!(!ic.is_invalid());
        let icf = face_of(ic);
        println!("icon face: {icf}");
        assert!(icf == "Segoe Fluent Icons" || icf == "Segoe MDL2 Assets");
        let m = mono_font(96, 10);
        assert!(!m.is_invalid());
        let mf = face_of(m);
        println!("mono face: {mf}");
        assert!(mf == "Cascadia Mono" || mf == "Consolas");
    }

    #[test]
    fn apply_does_not_panic() {
        unsafe {
            let icc = INITCOMMONCONTROLSEX {
                dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_LISTVIEW_CLASSES,
            };
            let _ = InitCommonControlsEx(&icc);
            let parent = CreateWindowExW(
                WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW,
                0, 0, 800, 600, None, None, None, None,
            )
            .unwrap();
            let list = CreateWindowExW(
                WINDOW_EX_STYLE(0), w!("SysListView32"), PCWSTR::null(), WS_CHILD,
                0, 0, 300, 200, Some(parent), None, None, None,
            )
            .unwrap();
            for t in [Theme::light(), Theme::dark()] {
                apply_window(parent, &t);
                apply_list(list, &t);
                apply_control(list, &t);
                let b = Brushes::new(&t);
                let dc = GetDC(Some(parent));
                assert_eq!(ctl_color(dc, &t, &b), b.surface);
                ReleaseDC(Some(parent), dc);
                pump();
            }
            assert_eq!(SendMessageW(list, LVM_GETTEXTCOLOR, None, None).0 as u32, Theme::dark().text);
            DestroyWindow(parent).unwrap();
        }
    }
}
