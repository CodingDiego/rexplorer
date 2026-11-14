//! Keeps a dialog inside its monitor's work area, and scrolls its content where the work area is smaller than it
//! (a large Windows text size or display scale on a small screen).

use std::mem::size_of;

use windows::Win32::Foundation::{HWND, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MapWindowPoints, MonitorFromRect};
use windows::Win32::UI::Controls::SetScrollInfo;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetSystemMetricsForDpi};
use windows::Win32::UI::WindowsAndMessaging::*;

/// `want` moved, and shrunk where it must be, to lie inside `work`.
pub fn inside(want: RECT, work: RECT) -> RECT {
    let w = (want.right - want.left).min(work.right - work.left);
    let h = (want.bottom - want.top).min(work.bottom - work.top);
    let left = want.left.clamp(work.left, work.right - w);
    let top = want.top.clamp(work.top, work.bottom - h);
    RECT { left, top, right: left + w, bottom: top + h }
}

/// The window size for a `client` area that may not fit in `room`: where one axis does not fit, the other gets the
/// room its scroll bar takes (`bar`: a vertical bar's width, a horizontal bar's height), so one bar does not call
/// for the other.
fn with_bars(window: (i32, i32), room: (i32, i32), bar: (i32, i32)) -> (i32, i32) {
    let (mut w, mut h) = window;
    if h > room.1 {
        w += bar.0;
    }
    if w > room.0 {
        h += bar.1;
    }
    (w, h)
}

/// The work area (the screen without the taskbar) of the monitor nearest `r`; `r` itself if the system will not say.
pub fn work_area(r: &RECT) -> RECT {
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    if unsafe { GetMonitorInfoW(MonitorFromRect(r, MONITOR_DEFAULTTONEAREST), &mut mi) }.as_bool() { mi.rcWork } else { *r }
}

/// The window rectangle that gives a `content`-sized client area, centered on `over` and kept inside the work area
/// of `over`'s monitor. A window smaller than its content scrolls (`Scroll`).
pub fn window_rect(content: (i32, i32), style: WINDOW_STYLE, frame_dpi: u32, over: &RECT) -> RECT {
    let mut wr = RECT { left: 0, top: 0, right: content.0, bottom: content.1 };
    let work = work_area(over);
    let bar = unsafe {
        let _ = AdjustWindowRectExForDpi(&mut wr, style, false, WINDOW_EX_STYLE(0), frame_dpi);
        (GetSystemMetricsForDpi(SM_CXVSCROLL, frame_dpi), GetSystemMetricsForDpi(SM_CYHSCROLL, frame_dpi))
    };
    let (w, h) = with_bars((wr.right - wr.left, wr.bottom - wr.top), (work.right - work.left, work.bottom - work.top), bar);
    let left = over.left + (over.right - over.left - w) / 2;
    let top = over.top + (over.bottom - over.top - h) / 2;
    inside(RECT { left, top, right: left + w, bottom: top + h }, work)
}

/// How far a window's content is scrolled, in pixels. The window places its children at their content position
/// minus this, and paints its content shifted by it.
#[derive(Default, Clone, Copy)]
pub struct Scroll {
    pub x: i32,
    pub y: i32,
    /// What a wheel message left over of a pixel: fine wheels and touchpads send parts of a notch.
    wheel: i32,
}

impl Scroll {
    /// After the window's size or its content's changed: each bar's range and page (a bar shows only while its axis
    /// does not fit) and the position kept in range. Showing a bar shrinks the client area and sends WM_SIZE again;
    /// that second pass changes nothing.
    pub unsafe fn fit(&mut self, hwnd: HWND, content: (i32, i32)) {
        for (bar, len) in [(SB_VERT, content.1), (SB_HORZ, content.0)] {
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let (page, pos) = if bar == SB_VERT { (rc.bottom, self.y) } else { (rc.right, self.x) };
            let si = SCROLLINFO { cbSize: size_of::<SCROLLINFO>() as u32, fMask: SIF_RANGE | SIF_PAGE | SIF_POS, nMin: 0, nMax: len - 1, nPage: page.max(0) as u32, nPos: pos, ..Default::default() };
            let now = SetScrollInfo(hwnd, bar, &si, true);
            if bar == SB_VERT {
                self.y = now;
            } else {
                self.x = now;
            }
        }
    }

    /// Moves `bar` to `to` (kept in range). True when the position changed.
    unsafe fn set(&mut self, hwnd: HWND, bar: SCROLLBAR_CONSTANTS, to: i32) -> bool {
        let si = SCROLLINFO { cbSize: size_of::<SCROLLINFO>() as u32, fMask: SIF_POS, nPos: to, ..Default::default() };
        let now = SetScrollInfo(hwnd, bar, &si, true);
        let pos = if bar == SB_VERT { &mut self.y } else { &mut self.x };
        std::mem::replace(pos, now) != now
    }

    /// WM_VSCROLL, WM_HSCROLL or WM_MOUSEWHEEL (three `line`s a notch). True when the content moved: the caller
    /// places its children again and repaints.
    pub unsafe fn on_message(&mut self, hwnd: HWND, msg: u32, wparam: WPARAM, line: i32) -> bool {
        let bar = if msg == WM_HSCROLL { SB_HORZ } else { SB_VERT };
        let mut si = SCROLLINFO { cbSize: size_of::<SCROLLINFO>() as u32, fMask: SIF_ALL, ..Default::default() };
        if GetScrollInfo(hwnd, bar, &mut si).is_err() {
            return false;
        }
        let page = si.nPage as i32;
        let to = if msg == WM_MOUSEWHEEL {
            let turned = self.wheel + ((wparam.0 >> 16) as i16 as i32) * 3 * line;
            self.wheel = turned % WHEEL_DELTA as i32;
            si.nPos - turned / WHEEL_DELTA as i32
        } else {
            match SCROLLBAR_COMMAND((wparam.0 & 0xFFFF) as i32) {
                SB_LINEUP => si.nPos - line,
                SB_LINEDOWN => si.nPos + line,
                SB_PAGEUP => si.nPos - page,
                SB_PAGEDOWN => si.nPos + page,
                SB_THUMBTRACK | SB_THUMBPOSITION => si.nTrackPos,
                SB_TOP => si.nMin,
                SB_BOTTOM => si.nMax,
                _ => si.nPos,
            }
        };
        self.set(hwnd, bar, to)
    }

    /// Scrolls just far enough to show `child` (the control that took the keyboard focus). True when the content
    /// moved.
    pub unsafe fn reveal(&mut self, hwnd: HWND, child: HWND) -> bool {
        if child.0.is_null() || !IsChild(hwnd, child).as_bool() {
            return false;
        }
        let (mut r, mut rc) = (RECT::default(), RECT::default());
        let _ = GetWindowRect(child, &mut r);
        let _ = GetClientRect(hwnd, &mut rc);
        // As a rectangle (two points), so a right-to-left window's mirrored client area maps its edges the right way.
        let mut p = [POINT { x: r.left, y: r.top }, POINT { x: r.right, y: r.bottom }];
        MapWindowPoints(None, Some(hwnd), &mut p);
        let (left, right) = (p[0].x.min(p[1].x), p[0].x.max(p[1].x));
        let (dx, dy) = (shift(left, right, rc.right), shift(p[0].y, p[1].y, rc.bottom));
        let moved_x = dx != 0 && self.set(hwnd, SB_HORZ, self.x + dx);
        let moved_y = dy != 0 && self.set(hwnd, SB_VERT, self.y + dy);
        moved_x || moved_y
    }
}

/// How far to scroll so the span `from..to` shows in `0..page`: its start wins when it is longer than the page, and
/// one that already fills the page stays where the user scrolled it.
fn shift(from: i32, to: i32, page: i32) -> i32 {
    if from < 0 && to >= page {
        0
    } else if from < 0 {
        from
    } else if to > page {
        (to - page).min(from)
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT { left, top, right, bottom }
    }

    #[test]
    fn a_window_is_moved_and_shrunk_into_the_work_area() {
        let work = r(0, 0, 1920, 1040);
        assert_eq!(inside(r(100, 100, 600, 500), work), r(100, 100, 600, 500), "one that fits stays");
        // Folder options at Text size 225% on 1920x1080 (batch 3A, bug 1): 1368 x 1416 at (106, -205).
        assert_eq!(inside(r(106, -205, 1474, 1211), work), r(106, 0, 1474, 1040));
        assert_eq!(inside(r(1800, 900, 2300, 1300), work), r(1420, 640, 1920, 1040), "moved back in, size kept");
        assert_eq!(inside(r(-50, -50, 3000, 3000), work), work, "larger both ways: the whole work area");
        // A monitor left of the primary one has negative coordinates.
        assert_eq!(inside(r(-2000, 50, -1500, 450), r(-1920, 0, 0, 1040)), r(-1920, 50, -1420, 450));
    }

    #[test]
    fn a_window_too_tall_gets_the_width_of_its_scroll_bar() {
        let room = (1920, 1040);
        assert_eq!(with_bars((1368, 1416), room, (17, 17)), (1385, 1416), "too tall: wider by the vertical bar");
        assert_eq!(with_bars((1368, 900), room, (17, 17)), (1368, 900), "fits: unchanged");
        assert_eq!(with_bars((2000, 900), room, (17, 17)), (2000, 917), "too wide: taller by the horizontal bar");
        assert_eq!(with_bars((1910, 1416), room, (17, 17)), (1927, 1433), "the vertical bar makes it too wide: both");
    }

    #[test]
    fn revealing_scrolls_no_further_than_needed() {
        assert_eq!(shift(10, 40, 100), 0, "in view");
        assert_eq!(shift(-30, 0, 100), -30, "above: back to its start");
        assert_eq!(shift(90, 130, 100), 30, "below: until its end shows");
        assert_eq!(shift(50, 250, 100), 50, "longer than the page: its start at the top");
        assert_eq!(shift(-50, 150, 100), 0, "fills the page: stays");
        assert_eq!(shift(-150, 100, 100), 0, "its end at the page's end: stays");
    }
}
