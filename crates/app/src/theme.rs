//! Applies a `rexplorer_core::settings::Theme` to the main window and the common controls.
#![allow(dead_code)]

use rexplorer_core::settings::{Contrast, Theme};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
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
    if t.contrast.is_some() {
        // Plain system frame; also resets what a normal theme set before a live switch.
        dwm_set(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &BOOL::from(false));
        dwm_set(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &DWMSBT_AUTO);
        dwm_set(hwnd, DWMWA_CAPTION_COLOR, &COLORREF(DWMWA_COLOR_DEFAULT));
        dwm_set(hwnd, DWMWA_BORDER_COLOR, &COLORREF(DWMWA_COLOR_DEFAULT));
        let _ = unsafe { SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED) };
        return;
    }
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

/// DrawTextW of `s`, skipped when it is empty: an empty Vec<u16> hands DrawTextW a dangling pointer, which it read
/// and faulted on (the preview pane crash, 2026-11-11). An empty text measures (DT_CALCRECT) zero wide.
pub unsafe fn draw_text(dc: HDC, s: &str, r: &mut RECT, flags: DRAW_TEXT_FORMAT) {
    if s.is_empty() {
        if flags.0 & DT_CALCRECT.0 != 0 {
            r.right = r.left;
        }
        return;
    }
    let mut v: Vec<u16> = s.encode_utf16().collect();
    unsafe { DrawTextW(dc, &mut v, r, crate::rtl::text(flags)) };
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
        let rect = crate::rtl::screen_to_local(wr, o, crate::rtl::mirrored(owner));
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
        // A mirrored window DC's (0, 0) is the window's top-right corner.
        let rtl = crate::rtl::mirrored(hwnd);
        let corner = POINT { x: if rtl { wr.right } else { wr.left }, y: wr.top };
        let at = crate::rtl::screen_to_local(RECT { left: origin.x, top: origin.y, right: origin.x, bottom: origin.y }, corner, rtl);
        let (ox, oy) = (at.left, at.top);
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

/// Windows' Settings > Accessibility > Text size in percent; 0 until first read.
static TEXT_SCALE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The text size (100 to 225 percent), read on first use and again by `refresh_text_scale`.
pub fn text_scale() -> u32 {
    match TEXT_SCALE.load(std::sync::atomic::Ordering::Relaxed) {
        0 => {
            refresh_text_scale();
            TEXT_SCALE.load(std::sync::atomic::Ordering::Relaxed)
        }
        s => s,
    }
}

/// Re-reads the text size (on WM_SETTINGCHANGE); true when it changed.
pub fn refresh_text_scale() -> bool {
    let s = clamp_text_scale(rexplorer_core::import_explorer::dword("Software\\Microsoft\\Accessibility", "TextScaleFactor"));
    TEXT_SCALE.swap(s, std::sync::atomic::Ordering::Relaxed) != s
}

/// The registry value, absent meaning 100 %; out-of-range values are clamped to what Settings offers.
fn clamp_text_scale(raw: Option<u32>) -> u32 {
    raw.unwrap_or(100).clamp(100, 225)
}

/// `dpi` grown by `pct` percent of text size.
fn scaled_dpi(dpi: u32, pct: u32) -> u32 {
    (dpi * pct + 50) / 100
}

/// `dpi` grown by the text size: pass it for fonts and for metrics that hold text (rows, bars, tiles, dialogs),
/// never for glyph or icon sizes.
pub fn text_dpi(dpi: u32) -> u32 {
    scaled_dpi(dpi, text_scale())
}

/// LOGFONT height of a `pt`-point font at `dpi` and `pct` percent text size.
fn font_height(pt: i32, dpi: u32, pct: u32) -> i32 {
    -muldiv(pt, scaled_dpi(dpi, pct) as i32, 72)
}

/// Section headers: the theme's face, semibold.
pub fn header_font(t: &Theme, dpi: u32, pt: i32) -> HFONT {
    let f = HSTRING::from(t.font.as_str());
    unsafe {
        CreateFontW(
            font_height(pt, dpi, text_scale()), 0, 0, 0, FW_SEMIBOLD.0 as i32, 0, 0, 0, DEFAULT_CHARSET,
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
    let h = font_height(t.font_size, dpi, text_scale());
    let f = create_font(h, &t.font);
    if f.is_invalid() { create_font(h, "Segoe UI") } else { f }
}

pub fn icon_font(dpi: u32, px: i32) -> HFONT {
    first_existing(-muldiv(px, dpi as i32, 96), "Segoe Fluent Icons", "Segoe MDL2 Assets")
}

pub fn mono_font(dpi: u32, pt: i32) -> HFONT {
    first_existing(font_height(pt, dpi, text_scale()), "Cascadia Mono", "Consolas")
}

pub fn blend(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    (0..3).fold(0, |acc, i| {
        let (x, y) = (((a >> (8 * i)) & 0xFF) as f32, ((b >> (8 * i)) & 0xFF) as f32);
        acc | (((x + (y - x) * t).round() as u32) << (8 * i))
    })
}

pub fn hover(t: &Theme) -> u32 {
    if t.contrast.is_some() { t.background } else { blend(t.background, t.text, 0.08) }
}

pub fn pressed(t: &Theme) -> u32 {
    if t.contrast.is_some() { t.selection } else { blend(t.background, t.text, 0.14) }
}

/// `base` as the user sees it: while a Windows contrast theme is on, the system colors replace the palette.
/// `sys` is the color lookup (`sys_color`, or a fake in tests); it is never called when `high_contrast` is false.
pub fn effective_theme(base: &Theme, high_contrast: bool, sys: impl Fn(SYS_COLOR_INDEX) -> u32) -> Theme {
    let mut t = base.clone();
    if !high_contrast {
        return t;
    }
    // secondary_text is WINDOWTEXT on purpose: GRAYTEXT is for disabled text only.
    (t.background, t.surface) = (sys(COLOR_WINDOW), sys(COLOR_WINDOW));
    (t.text, t.secondary_text, t.border) = (sys(COLOR_WINDOWTEXT), sys(COLOR_WINDOWTEXT), sys(COLOR_WINDOWTEXT));
    (t.accent, t.selection) = (sys(COLOR_HIGHLIGHT), sys(COLOR_HIGHLIGHT));
    (t.dark, t.mica) = (false, false);
    t.contrast = Some(Contrast {
        highlight_text: sys(COLOR_HIGHLIGHTTEXT),
        hot: sys(COLOR_HOTLIGHT),
        gray: sys(COLOR_GRAYTEXT),
        button_face: sys(COLOR_BTNFACE),
        button_text: sys(COLOR_BTNTEXT),
    });
    t
}

/// True while Windows has a contrast theme on.
pub fn high_contrast_on() -> bool {
    let mut hc = HIGHCONTRASTW { cbSize: size_of::<HIGHCONTRASTW>() as u32, ..Default::default() };
    unsafe { SystemParametersInfoW(SPI_GETHIGHCONTRAST, hc.cbSize, Some(&mut hc as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) }
        .is_ok()
        && hc.dwFlags.0 & HCF_HIGHCONTRASTON.0 != 0
}

/// The real system color lookup.
pub fn sys_color(i: SYS_COLOR_INDEX) -> u32 {
    unsafe { GetSysColor(i) }
}

/// Messages that mean the system colors or the contrast theme may have changed.
pub fn is_color_change(msg: u32, wparam: usize, setting: Option<&str>) -> bool {
    match msg {
        WM_SYSCOLORCHANGE | WM_THEMECHANGED => true,
        WM_SETTINGCHANGE => wparam == SPI_SETHIGHCONTRAST.0 as usize || setting == Some("ImmersiveColorSet"),
        _ => false,
    }
}

/// Text or glyph color on a `t.selection` / `t.accent` fill: HIGHLIGHTTEXT in a contrast theme, else `color`.
pub fn on_selection(t: &Theme, color: u32) -> u32 {
    t.contrast.map_or(color, |c| c.highlight_text)
}

/// Outlines `r` (same right/bottom +1 convention as `fill_round`) with a `width`-px pen drawn inside `r`, corners rounded by `radius`.
pub unsafe fn outline_round(dc: HDC, r: &RECT, color: u32, radius: i32, width: i32) {
    unsafe {
        let pen = CreatePen(PS_INSIDEFRAME, width, COLORREF(color));
        let (ob, op) = (SelectObject(dc, GetStockObject(NULL_BRUSH)), SelectObject(dc, pen.into()));
        let _ = RoundRect(dc, r.left, r.top, r.right + 1, r.bottom + 1, 2 * radius, 2 * radius);
        SelectObject(dc, ob);
        SelectObject(dc, op);
        let _ = DeleteObject(pen.into());
    }
}

/// Hover by shape in a contrast theme: call right after a hover `fill_round`. Nothing in normal themes.
pub unsafe fn hot_outline(dc: HDC, r: &RECT, t: &Theme, radius: i32, dpi: u32) {
    if let Some(c) = t.contrast {
        unsafe { outline_round(dc, r, c.hot, radius, (dpi as i32 / 96).max(1)) };
    }
}

/// A push button in system colors: face (selection when pressed), 1 px outline, focus rectangle. Returns the label
/// color, or None (nothing drawn) outside a contrast theme.
pub unsafe fn contrast_button(dc: HDC, r: &RECT, t: &Theme, pressed: bool, focused: bool, disabled: bool, radius: i32) -> Option<u32> {
    let c = t.contrast?;
    let edge = if disabled { c.gray } else { c.button_text };
    unsafe {
        fill_round(dc, r, if pressed { t.selection } else { c.button_face }, radius);
        outline_round(dc, r, edge, radius, 1);
        if focused {
            let _ = DrawFocusRect(dc, &RECT { left: r.left + 3, top: r.top + 3, right: r.right - 3, bottom: r.bottom - 3 });
        }
    }
    Some(if disabled { c.gray } else if pressed { c.highlight_text } else { c.button_text })
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

    /// 0x0010_0000 + index: every system color distinct and recognizable.
    fn fake(i: SYS_COLOR_INDEX) -> u32 {
        0x0010_0000 + i.0 as u32
    }

    /// A contrast theme from a white-on-black style lookup; BTNFACE, BTNTEXT and GRAYTEXT distinct. Colors are
    /// R == B so the DIB (0x00RRGGBB) and COLORREF (0x00BBGGRR) orders agree.
    pub(crate) fn contrast_theme(base: Theme) -> Theme {
        effective_theme(&base, true, |i| match i {
            COLOR_WINDOW | COLOR_HIGHLIGHTTEXT => 0xFFFFFF,
            COLOR_WINDOWTEXT => 0x000000,
            COLOR_HIGHLIGHT => 0xA000A0,
            COLOR_BTNFACE => 0xC0C0C0,
            COLOR_BTNTEXT => 0x101010,
            COLOR_GRAYTEXT => 0x808080,
            _ => 0x0000FF,
        })
    }

    /// Runs `f` on a 40x40 32-bit DIB filled 0x2060A0, then hands it the pixel reader.
    fn on_dib(f: impl FnOnce(HDC, &dyn Fn(usize, usize) -> u32)) {
        unsafe {
            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: 40, biHeight: -40, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bmp = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
            let old = SelectObject(dc, bmp.into());
            let brush = CreateSolidBrush(COLORREF(0x2060A0));
            FillRect(dc, &RECT { left: 0, top: 0, right: 40, bottom: 40 }, brush);
            let px = std::slice::from_raw_parts(bits as *const u32, 1600);
            f(dc, &|x, y| { let _ = GdiFlush(); px[y * 40 + x] & 0xFF_FFFF });
            SelectObject(dc, old);
            let _ = DeleteObject(brush.into());
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(dc);
        }
    }

    #[test]
    fn empty_text_draws_nothing_and_measures_zero_wide() {
        // The breadcrumb and group headers draw names that can be empty, with DT_END_ELLIPSIS (a Home start crashed).
        unsafe {
            let dc = CreateCompatibleDC(None);
            let mut r = RECT { left: 10, top: 0, right: 200, bottom: 20 };
            draw_text(dc, "", &mut r, DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);
            assert_eq!(r, RECT { left: 10, top: 0, right: 200, bottom: 20 }, "nothing drawn, rect untouched");
            draw_text(dc, "", &mut r, DT_SINGLELINE | DT_CALCRECT);
            assert_eq!(r.right, r.left, "an empty text measures zero wide");
            let mut m = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            draw_text(dc, "abc", &mut m, DT_SINGLELINE | DT_CALCRECT);
            assert!(m.right > 0, "a real text still measures");
            let _ = DeleteDC(dc);
        }
    }

    #[test]
    fn effective_theme_off_returns_base() {
        assert_eq!(effective_theme(&Theme::dark(), false, |_| panic!("read")), Theme::dark());
    }

    #[test]
    fn effective_theme_maps_system_colors() {
        let base = Theme { icon_theme: Some("x".into()), font: "F".into(), font_size: 13, ..Theme::dark() };
        let t = effective_theme(&base, true, fake);
        assert_eq!((t.background, t.surface), (fake(COLOR_WINDOW), fake(COLOR_WINDOW)));
        assert_eq!((t.text, t.secondary_text, t.border), (fake(COLOR_WINDOWTEXT), fake(COLOR_WINDOWTEXT), fake(COLOR_WINDOWTEXT)));
        assert_eq!((t.accent, t.selection), (fake(COLOR_HIGHLIGHT), fake(COLOR_HIGHLIGHT)));
        assert_eq!(
            t.contrast,
            Some(Contrast { highlight_text: fake(COLOR_HIGHLIGHTTEXT), hot: fake(COLOR_HOTLIGHT), gray: fake(COLOR_GRAYTEXT), button_face: fake(COLOR_BTNFACE), button_text: fake(COLOR_BTNTEXT) })
        );
        assert!(!t.dark && !t.mica);
        assert_eq!((&t.name, &t.icon_theme, &t.font, t.font_size), (&base.name, &base.icon_theme, &base.font, base.font_size));
    }

    #[test]
    fn effective_theme_ignores_base_palette() {
        let lookups: [fn(SYS_COLOR_INDEX) -> u32; 2] = [
            |i| if i == COLOR_WINDOW { 0xFFFFFF } else if i == COLOR_WINDOWTEXT { 0 } else { 0x0000FF },
            |i| if i == COLOR_WINDOW { 0 } else if i == COLOR_WINDOWTEXT { 0xFFFFFF } else { 0x0000FF },
        ];
        for sys in lookups {
            let (a, b) = (effective_theme(&Theme::light(), true, sys), effective_theme(&Theme::dark(), true, sys));
            assert_eq!(
                [a.background, a.surface, a.text, a.secondary_text, a.accent, a.selection, a.border],
                [b.background, b.surface, b.text, b.secondary_text, b.accent, b.selection, b.border]
            );
            assert_eq!(a.contrast, b.contrast);
            assert!(!a.dark && !b.dark);
        }
    }

    #[test]
    fn hover_pressed_in_contrast() {
        let c = contrast_theme(Theme::dark());
        assert_eq!((hover(&c), pressed(&c)), (c.background, c.selection));
        let l = Theme::light();
        assert_eq!((hover(&l), pressed(&l)), (blend(l.background, l.text, 0.08), blend(l.background, l.text, 0.14)));
    }

    #[test]
    fn on_selection_only_in_contrast() {
        assert_eq!(on_selection(&Theme::light(), 0x123456), 0x123456);
        let c = contrast_theme(Theme::light());
        assert_eq!(on_selection(&c, 0x123456), c.contrast.unwrap().highlight_text);
    }

    #[test]
    fn color_change_messages() {
        let hc = SPI_SETHIGHCONTRAST.0 as usize;
        assert!(is_color_change(WM_SYSCOLORCHANGE, 0, None));
        assert!(is_color_change(WM_THEMECHANGED, 0, None));
        assert!(is_color_change(WM_SETTINGCHANGE, hc, None));
        assert!(is_color_change(WM_SETTINGCHANGE, 0, Some("ImmersiveColorSet")));
        assert!(!is_color_change(WM_SETTINGCHANGE, 0, Some("intl")));
        assert!(!is_color_change(WM_SETTINGCHANGE, SPI_SETWORKAREA.0 as usize, None));
        assert!(!is_color_change(WM_SIZE, 0, None));
    }

    #[test]
    fn outline_round_draws_the_edge_only() {
        on_dib(|dc, at| unsafe {
            outline_round(dc, &RECT { left: 4, top: 4, right: 36, bottom: 36 }, 0, 6, 1);
            assert_eq!(at(20, 4), 0, "the edge");
            assert_eq!((at(20, 20), at(1, 1)), (0xA06020, 0xA06020), "inside and outside untouched");
            hot_outline(dc, &RECT { left: 8, top: 8, right: 32, bottom: 32 }, &Theme::light(), 4, 96);
            assert_eq!(at(20, 8), 0xA06020, "no hot outline in a normal theme");
        });
    }

    #[test]
    fn contrast_button_draws_system_colors() {
        let t = contrast_theme(Theme::light());
        let c = t.contrast.unwrap();
        let rc = RECT { left: 4, top: 4, right: 36, bottom: 36 };
        on_dib(|dc, at| unsafe {
            assert_eq!(contrast_button(dc, &rc, &t, false, false, false, 4), Some(c.button_text));
            assert_eq!((at(20, 20), at(20, 4)), (c.button_face, c.button_text));
        });
        on_dib(|dc, at| unsafe {
            assert_eq!(contrast_button(dc, &rc, &t, true, false, false, 4), Some(c.highlight_text));
            assert_eq!(at(20, 20), t.selection);
        });
        on_dib(|dc, at| unsafe {
            assert_eq!(contrast_button(dc, &rc, &t, false, false, true, 4), Some(c.gray));
            assert_eq!(at(20, 4), c.gray);
        });
        on_dib(|dc, at| unsafe {
            assert_eq!(contrast_button(dc, &rc, &Theme::light(), false, true, false, 4), None);
            assert_eq!((at(20, 20), at(20, 4)), (0xA06020, 0xA06020));
        });
    }

    #[test]
    fn text_scale_reads_and_clamps() {
        assert_eq!(clamp_text_scale(None), 100, "no value: 100 %");
        assert_eq!([50, 100, 150, 200, 225, 300].map(|v| clamp_text_scale(Some(v))), [100, 100, 150, 200, 225, 225]);
    }

    #[test]
    fn text_scale_grows_fonts_and_metrics() {
        // A 9 pt font at 96 DPI and text size 100 / 150 / 200 / 225 %.
        assert_eq!([100, 150, 200, 225].map(|p| font_height(9, 96, p)), [-12, -18, -24, -27]);
        // A 44 px bar, scaled the way ui::layout does with text_dpi.
        assert_eq!([100, 150, 200, 225].map(|p| 44 * scaled_dpi(96, p) as i32 / 96), [44, 66, 88, 99]);
        // Text size on top of display scaling multiplies.
        assert_eq!(scaled_dpi(144, 150), 216);
        assert_eq!(font_height(9, 240, 225), -68);
    }

    #[test]
    fn display_scaling_100_to_250() {
        let dpis = [96, 120, 144, 168, 192, 240];
        assert_eq!(dpis.map(|d| scaled_dpi(d, 100)), dpis, "100 % text size leaves the DPI alone");
        assert_eq!(dpis.map(|d| font_height(9, d, 100)), [-12, -15, -18, -21, -24, -30]);
        assert_eq!(dpis.map(card_radius), [10, 12, 15, 17, 20, 25]);
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
            for t in [contrast_theme(Theme::light()), Theme::light(), Theme::dark()] {
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
