//! Applies a `rexplorer_core::settings::Theme` to the main window and the common controls.
#![allow(dead_code)]

use rexplorer_core::settings::Theme;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, HSTRING, PCSTR, PCWSTR, w};

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

static MENUS_DARK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Dark or light popup menus for the whole process, through uxtheme's unnamed SetPreferredAppMode (ordinal 135;
/// 2 forces dark, 3 forces light) and FlushMenuThemes (ordinal 136), as Explorer and Notepad do. Windows 11 also
/// rounds their corners. Builds without the ordinals keep the classic menus.
pub fn set_menu_theme(dark: bool) {
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    MENUS_DARK.store(dark, std::sync::atomic::Ordering::Relaxed);
    unsafe {
        let Ok(ux) = LoadLibraryW(w!("uxtheme.dll")) else { return };
        let (Some(set), Some(flush)) = (GetProcAddress(ux, PCSTR(135 as *const u8)), GetProcAddress(ux, PCSTR(136 as *const u8))) else { return };
        let set: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(set);
        let flush: unsafe extern "system" fn() = std::mem::transmute(flush);
        set(if dark { 2 } else { 3 });
        flush();
    }
}

/// What `set_menu_theme` last chose: menu glyphs are drawn in the matching text color.
pub fn menus_dark() -> bool {
    MENUS_DARK.load(std::sync::atomic::Ordering::Relaxed)
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

/// Share of pixel (x, y) of a corner square `r` px wide that lies inside the quarter circle of radius `r` centered
/// at (r, r), the square's inner corner: 1 inside the card, 0 in the frame. 4x4 samples per pixel.
pub fn coverage(x: i32, y: i32, r: i32) -> f32 {
    let r = r as f32;
    let mut inside = 0;
    for i in 0..4 {
        for j in 0..4 {
            let (sx, sy) = (x as f32 + (i as f32 + 0.5) / 4.0, y as f32 + (j as f32 + 0.5) / 4.0);
            if (r - sx).powi(2) + (r - sy).powi(2) <= r * r {
                inside += 1;
            }
        }
    }
    inside as f32 / 16.0
}

/// Rounds the corners of the card `rect` (in `dc`'s coordinates) by painting what lies outside a `radius` arc in
/// `outer`, the frame color, antialiased: an arc pixel blends `outer` with the color at its corner's innermost pixel
/// (r - 1, r - 1), which this never writes, so painting again gives the same result. A corner whose innermost pixel
/// is clipped out of `dc` (another window or DC owns that corner) is left alone.
pub unsafe fn paint_corners(dc: HDC, rect: &RECT, radius: i32, outer: u32) {
    let r = radius.min((rect.right - rect.left) / 2).min((rect.bottom - rect.top) / 2);
    if r <= 0 {
        return;
    }
    // Each corner square goes through a 32-bit DIB (BitBlt out, blend, BitBlt back): GetPixel on a memory DC
    // returned other pixels' colors on the development machine.
    unsafe {
        let mem = CreateCompatibleDC(Some(dc));
        let header = BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: r, biHeight: -r, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() };
        let mut bits = std::ptr::null_mut();
        let Ok(bmp) = CreateDIBSection(Some(mem), &BITMAPINFO { bmiHeader: header, ..Default::default() }, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(mem);
            return;
        };
        let old = SelectObject(mem, bmp.into());
        let px = std::slice::from_raw_parts_mut(bits as *mut u32, (r * r) as usize);
        // DIB pixels are 0x00RRGGBB, a COLORREF 0x00BBGGRR; `blend` works on either as long as both match.
        let outer = (outer & 0x00FF00) | (outer & 0xFF) << 16 | (outer >> 16) & 0xFF;
        for (right, bottom) in [(false, false), (true, false), (false, true), (true, true)] {
            let (left, top) = (if right { rect.right - r } else { rect.left }, if bottom { rect.bottom - r } else { rect.top });
            // Corner-local (x, y), (0, 0) the outermost pixel, to an index into the square.
            let at = |x: i32, y: i32| ((if bottom { r - 1 - y } else { y }) * r + if right { r - 1 - x } else { x }) as usize;
            let inner_at = (left + if right { 0 } else { r - 1 }, top + if bottom { 0 } else { r - 1 });
            if !PtVisible(dc, inner_at.0, inner_at.1).as_bool() {
                continue;
            }
            let _ = BitBlt(mem, 0, 0, r, r, Some(dc), left, top, SRCCOPY);
            let _ = GdiFlush();
            let inner = px[at(r - 1, r - 1)] & 0xFF_FFFF;
            for y in 0..r {
                for x in 0..r {
                    let c = coverage(x, y, r);
                    if c < 1.0 {
                        px[at(x, y)] = blend(outer, inner, c);
                    }
                }
            }
            let _ = BitBlt(dc, left, top, r, r, Some(mem), 0, 0, SRCCOPY);
        }
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
    }
}

/// Corner radius of the content cards (the list, the Home page, the preview) at `dpi`.
pub fn card_radius(dpi: u32) -> i32 {
    10 * dpi as i32 / 96
}

/// Paints the corners of the card window `card` on `dc`, a DC of `owner` (the card itself, or a child of it such as
/// the list's header) in `owner`'s client coordinates. Call at the end of the owner's painting.
pub unsafe fn paint_card_corners(card: HWND, owner: HWND, dc: HDC, outer: u32) {
    unsafe {
        let mut wr = RECT::default();
        if GetWindowRect(card, &mut wr).is_err() {
            return;
        }
        let mut o = POINT::default();
        let _ = ClientToScreen(owner, &mut o);
        let rect = RECT { left: wr.left - o.x, top: wr.top - o.y, right: wr.right - o.x, bottom: wr.bottom - o.y };
        paint_corners(dc, &rect, card_radius(GetDpiForWindow(card)), outer);
    }
}

/// After a card's own handling of `msg`: its scroll bars (non-client) may have been redrawn, square, so their
/// corners are painted again. A resize repaints the whole card, frame included, as the corners moved.
pub unsafe fn card_message(card: HWND, msg: u32, outer: u32) {
    unsafe {
        match msg {
            WM_SIZE => {
                let _ = RedrawWindow(Some(card), None, None, RDW_INVALIDATE | RDW_FRAME);
            }
            WM_NCPAINT | WM_VSCROLL | WM_HSCROLL | WM_MOUSEWHEEL | WM_MOUSEHWHEEL | WM_NCMOUSEMOVE | WM_NCMOUSELEAVE
            | WM_NCLBUTTONDOWN | WM_NCLBUTTONUP | WM_KEYDOWN => paint_frame_corners(card, card_radius(GetDpiForWindow(card)), outer),
            _ => {}
        }
    }
}

/// The corners of a card that fall in its non-client area (its scroll bars): call after the window's own
/// WM_NCPAINT, and after anything that redraws a scroll bar.
pub unsafe fn paint_frame_corners(hwnd: HWND, radius: i32, outer: u32) {
    unsafe {
        let (mut wr, mut cr) = (RECT::default(), RECT::default());
        if GetWindowRect(hwnd, &mut wr).is_err() || GetClientRect(hwnd, &mut cr).is_err() {
            return;
        }
        // The client area, in window coordinates, paints its own corners.
        let mut origin = POINT::default();
        let _ = ClientToScreen(hwnd, &mut origin);
        let dc = GetWindowDC(Some(hwnd));
        let (ox, oy) = (origin.x - wr.left, origin.y - wr.top);
        ExcludeClipRect(dc, ox, oy, ox + cr.right, oy + cr.bottom);
        paint_corners(dc, &RECT { left: 0, top: 0, right: wr.right - wr.left, bottom: wr.bottom - wr.top }, radius, outer);
        ReleaseDC(Some(hwnd), dc);
    }
}

/// Clips a child window to a rounded rectangle of its current size (a "card"); call again after each resize.
/// Jagged (a region has no partial pixels): only for windows that cannot paint their own corners (the embedded
/// Explorer view, the palette popup).
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
    fn corner_coverage() {
        let r = 10;
        assert_eq!(coverage(r - 1, r - 1, r), 1.0, "next to the center: all card");
        assert_eq!(coverage(0, 0, r), 0.0, "the far corner: all frame");
        let arc = coverage(3, 2, r);
        assert!(arc > 0.0 && arc < 1.0, "the arc crosses pixel (3, 2): {arc}");
        assert_eq!(coverage(0, r - 1, r), coverage(r - 1, 0, r), "symmetric");
    }

    #[test]
    fn painted_corners_blend_and_repaint_the_same() {
        unsafe {
            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: 40, biHeight: -40, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bmp = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
            let old = SelectObject(dc, bmp.into());
            let rc = RECT { left: 0, top: 0, right: 40, bottom: 40 };
            let surface = CreateSolidBrush(COLORREF(0x2060A0));
            FillRect(dc, &rc, surface);
            // Read the bitmap's memory (0x00RRGGBB), not GetPixel.
            let px = std::slice::from_raw_parts(bits as *const u32, 1600);
            let at = |x: usize, y: usize| { let _ = GdiFlush(); px[y * 40 + x] & 0xFF_FFFF };
            paint_corners(dc, &rc, 10, 0x000000);
            let first: Vec<u32> = (0..40).flat_map(|y| (0..40).map(move |x| (x, y))).map(|(x, y)| at(x, y)).collect();
            assert_eq!((at(0, 0), at(39, 0), at(0, 39), at(39, 39)), (0, 0, 0, 0), "every corner is the frame");
            assert_eq!((at(20, 20), at(9, 9), at(30, 30)), (0xA06020, 0xA06020, 0xA06020), "the card untouched");
            let mid = at(3, 2);
            assert!(mid & 0xFF > 0 && mid & 0xFF < 0x20, "the arc pixel is a blend: {mid:#08x}");
            assert_eq!(at(36, 2), mid, "mirrored");
            paint_corners(dc, &rc, 10, 0x000000);
            let second: Vec<u32> = (0..40).flat_map(|y| (0..40).map(move |x| (x, y))).map(|(x, y)| at(x, y)).collect();
            assert_eq!(first, second, "idempotent");
            SelectObject(dc, old);
            let _ = DeleteObject(surface.into());
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(dc);
        }
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
