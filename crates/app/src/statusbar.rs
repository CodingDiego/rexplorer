//! Themed status bar: centered text (item counts, Finder style), right text (view/profile hints), no border.
#![allow(dead_code)]

use std::ffi::c_void;
use std::sync::Once;

use rexplorer_core::access;
use rexplorer_core::i18n::{tr, trn};
use rexplorer_core::settings::Theme;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{ROLE_SYSTEM_STATICTEXT, ROLE_SYSTEM_STATUSBAR};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Result, w};

const BASE_HEIGHT: i32 = 28;
const PAD: i32 = 8;

struct State {
    theme: Theme,
    font: HFONT,
    left: String,
    right: String,
    acc: Option<access::Server>,
}

fn scale(v: i32, dpi: u32) -> i32 {
    (v * dpi as i32 + 48) / 96
}

pub fn height(dpi: u32) -> i32 {
    scale(BASE_HEIGHT, dpi)
}

pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64 / 1024.0;
    let mut unit = 0;
    while v >= 1024.0 && unit < 3 {
        v /= 1024.0;
        unit += 1;
    }
    format!("{} {}", format!("{v:.1}").replace('.', tr("num.decimal")), ["KB", "MB", "GB", "TB"][unit])
}

pub fn summary(count: usize, selected: usize, selected_bytes: Option<u64>) -> String {
    let mut s = trn("status.items", count as u64);
    if selected > 0 {
        s += &format!(", {}", trn("status.selected", selected as u64));
        if let Some(b) = selected_bytes {
            s += &format!(", {}", format_size(b));
        }
    }
    s
}

pub fn create(parent: HWND, id: usize) -> Result<HWND> {
    static REGISTER: Once = Once::new();
    unsafe {
        let hinst = GetModuleHandleW(None)?.into();
        REGISTER.call_once(|| {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinst,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: HBRUSH((5 + 1) as usize as *mut c_void),
                lpszClassName: w!("RexStatus"),
                ..Default::default()
            };
            RegisterClassW(&wc);
        });
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("RexStatus"),
            None,
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
            0,
            0,
            0,
            0,
            Some(parent),
            Some(HMENU(id as *mut c_void)),
            Some(hinst),
            None,
        )
    }
}

unsafe fn state(hwnd: HWND) -> *mut State {
    GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State
}

pub fn set_theme(hwnd: HWND, t: &Theme) {
    unsafe {
        let st = state(hwnd);
        if !st.is_null() {
            (*st).theme = t.clone();
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

pub fn set_text(hwnd: HWND, left: &str, right: &str) {
    unsafe {
        let st = state(hwnd);
        if !st.is_null() && ((*st).left != left || (*st).right != right) {
            let (new_left, new_right) = ((*st).left != left, (*st).right != right);
            (*st).left = left.to_string();
            (*st).right = right.to_string();
            // The window text carries the left text too: screen readers and UI Automation read it.
            let t = windows::core::HSTRING::from(left);
            let _ = SetWindowTextW(hwnd, windows::core::PCWSTR(t.as_ptr()));
            let _ = InvalidateRect(Some(hwnd), None, false);
            access::event(EVENT_OBJECT_NAMECHANGE, hwnd, None);
            if new_left {
                access::event(EVENT_OBJECT_NAMECHANGE, hwnd, Some(0));
            }
            if new_right {
                access::event(EVENT_OBJECT_NAMECHANGE, hwnd, Some(1));
            }
        }
    }
}

/// The left text's area and the right text's rect, as painted.
unsafe fn text_rects(dc: HDC, st: &State, w: i32, h: i32, dpi: u32) -> (RECT, RECT) {
    let pad = scale(PAD, dpi);
    let flags = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS;
    let mut right: Vec<u16> = st.right.encode_utf16().collect();
    let mut rr = RECT { left: pad, top: 0, right: w - pad, bottom: h };
    DrawTextW(dc, &mut right, &mut rr, flags | DT_RIGHT | DT_CALCRECT);
    let rw = rr.right - rr.left;
    let rr = RECT { left: (w - pad - rw).max(pad), top: 0, right: w - pad, bottom: h };
    // Centered in the width the right text leaves free on both sides.
    let side = pad + rw + pad;
    let lr = RECT { left: side.min(w / 2), top: 0, right: (w - side).max(w / 2), bottom: h };
    (lr, rr)
}

unsafe fn acc_tree(hwnd: HWND, st: &State, t: &mut access::Tree) {
    use windows::Win32::UI::Controls::STATE_SYSTEM_INVISIBLE;
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let dpi = GetDpiForWindow(hwnd).max(96);
    let dc = GetDC(Some(hwnd));
    let font = if st.font.is_invalid() { GetStockObject(DEFAULT_GUI_FONT) } else { st.font.into() };
    let old = SelectObject(dc, font);
    let (l, r) = text_rects(dc, st, rc.right, rc.bottom, dpi);
    SelectObject(dc, old);
    ReleaseDC(Some(hwnd), dc);
    t.name = if st.left.is_empty() { tr("status.window_name").into() } else { st.left.clone() };
    t.role = ROLE_SYSTEM_STATUSBAR;
    for (text, rect) in [(&st.left, l), (&st.right, r)] {
        t.items.push(access::Item {
            name: text.clone(),
            role: ROLE_SYSTEM_STATICTEXT,
            state: STATE_SYSTEM_READONLY | if text.is_empty() { STATE_SYSTEM_INVISIBLE.0 } else { 0 },
            rect,
            ..Default::default()
        });
    }
}

unsafe fn paint(hwnd: HWND, st: &State) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let (w, h) = (rc.right, rc.bottom);
    let dpi = GetDpiForWindow(hwnd).max(96);

    let mem = CreateCompatibleDC(Some(hdc));
    let bmp = CreateCompatibleBitmap(hdc, w.max(1), h.max(1));
    let old_bmp = SelectObject(mem, bmp.into());
    let font = if st.font.is_invalid() { GetStockObject(DEFAULT_GUI_FONT) } else { st.font.into() };
    let old_font = SelectObject(mem, font);

    let bg = CreateSolidBrush(COLORREF(st.theme.background));
    FillRect(mem, &rc, bg);
    let _ = DeleteObject(bg.into());

    SetBkMode(mem, TRANSPARENT);
    SetTextColor(mem, COLORREF(st.theme.secondary_text));
    let flags = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS;
    let (mut lr, mut rr) = text_rects(mem, st, w, h, dpi);
    let mut right: Vec<u16> = st.right.encode_utf16().collect();
    DrawTextW(mem, &mut right, &mut rr, flags | DT_RIGHT);
    let mut left: Vec<u16> = st.left.encode_utf16().collect();
    DrawTextW(mem, &mut left, &mut lr, flags | DT_CENTER);

    let _ = BitBlt(hdc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
    SelectObject(mem, old_font);
    SelectObject(mem, old_bmp);
    let _ = DeleteObject(bmp.into());
    let _ = DeleteDC(mem);
    let _ = EndPaint(hwnd, &ps);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let st = Box::new(State {
                theme: Theme::light(),
                font: HFONT::default(),
                left: String::new(),
                right: String::new(),
                acc: None,
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(st) as isize);
        }
        WM_NCDESTROY => {
            let st = state(hwnd);
            if !st.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(st)); // the font belongs to the parent
            }
        }
        WM_SETFONT => {
            let st = state(hwnd);
            if !st.is_null() {
                (*st).font = HFONT(wparam.0 as *mut c_void);
                if lparam.0 != 0 {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_GETOBJECT if access::is_client(lparam) => {
            let st = state(hwnd);
            if !st.is_null() {
                return (*st).acc.get_or_insert_with(|| access::Server::new(hwnd)).answer(wparam);
            }
        }
        access::WM_ACC_ITEMS => {
            let st = state(hwnd);
            if !st.is_null() {
                acc_tree(hwnd, &*st, &mut *(lparam.0 as *mut access::Tree));
                return LRESULT(1);
            }
        }
        WM_PAINT => {
            let st = state(hwnd);
            if !st.is_null() {
                paint(hwnd, &*st);
                return LRESULT(0);
            }
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::PCWSTR;

    fn pump() {
        unsafe {
            let mut m = MSG::default();
            while PeekMessageW(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&m);
                DispatchMessageW(&m);
            }
        }
    }

    #[test]
    fn sizes() {
        let t = [
            (0, "0 B"),
            (999, "999 B"),
            (1024, "1.0 KB"),
            (1536 * 1024, "1.5 MB"),
            ((2.3 * 1024.0 * 1024.0 * 1024.0) as u64, "2.3 GB"),
            ((1.1 * 1024f64.powi(4)) as u64, "1.1 TB"),
        ];
        for (b, s) in t {
            assert_eq!(format_size(b), s);
        }
    }

    #[test]
    fn summaries() {
        assert_eq!(summary(0, 0, None), "0 items");
        assert_eq!(summary(1, 0, None), "1 item");
        assert_eq!(summary(12, 0, None), "12 items");
        assert_eq!(summary(12, 3, None), "12 items, 3 selected");
        assert_eq!(summary(12, 3, Some(4_718_592)), "12 items, 3 selected, 4.5 MB");
    }

    #[test]
    fn window_roundtrip() {
        unsafe {
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 800, 600, None, None, None, None).unwrap();
            let sb = create(parent, 7).unwrap();
            assert_eq!(height(96), 28);
            assert_eq!(height(192), 56);
            set_theme(sb, &Theme::dark());
            set_text(sb, "3 items", "Details");
            set_text(sb, "3 items", "Details");
            let _ = MoveWindow(sb, 0, 0, 400, height(96), true);
            let _ = UpdateWindow(sb);
            pump();
            assert_eq!((*state(sb)).left, "3 items");
            let mut buf = [0u16; 16];
            let n = GetWindowTextW(sb, &mut buf) as usize;
            assert_eq!(String::from_utf16_lossy(&buf[..n]), "3 items", "the window text carries the left text");
            let _ = DestroyWindow(parent);
            pump();
        }
    }

    #[test]
    fn accessible_status_text() {
        use crate::acc_test::*;
        use rexplorer_core::com::ComInit;
        use windows::Win32::UI::Accessibility::*;
        use windows::Win32::UI::WindowsAndMessaging::{EVENT_OBJECT_NAMECHANGE, STATE_SYSTEM_READONLY};
        use windows::Win32::UI::Controls::STATE_SYSTEM_INVISIBLE;
        let _com = ComInit::sta();
        unsafe {
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_POPUP, 0, 0, 800, 600, None, None, None, None).unwrap();
            let sb = create(parent, 8).unwrap();
            let _ = MoveWindow(sb, 0, 0, 400, height(96), true);
            set_text(sb, "3 items", "Details");
            let acc = object(sb);
            let r = root(&acc);
            assert_eq!(r.name, "3 items");
            assert_eq!(r.role, ROLE_SYSTEM_STATUSBAR);
            let k = kids(&acc);
            assert_eq!([k[0].name.as_str(), k[1].name.as_str()], ["3 items", "Details"]);
            assert!(k.iter().all(|k| k.role == ROLE_SYSTEM_STATICTEXT && k.state & STATE_SYSTEM_READONLY != 0));
            assert!(acc.get_accDefaultAction(&windows::Win32::System::Variant::VARIANT::from(1)).is_err());
            let dpi = GetDpiForWindow(sb).max(96);
            let want = screen(sb, RECT { left: 0, top: 0, right: 400 - scale(PAD, dpi), bottom: 0 }).right;
            assert_eq!(k[1].rect.right, want);
            let hk = hook();
            set_text(sb, "4 items", "Details");
            let ev = hk.take();
            assert!(ev.contains(&(EVENT_OBJECT_NAMECHANGE, sb, 1)), "{ev:?}");
            assert!(ev.contains(&(EVENT_OBJECT_NAMECHANGE, sb, 0)), "{ev:?}");
            assert!(!ev.contains(&(EVENT_OBJECT_NAMECHANGE, sb, 2)), "right text did not change: {ev:?}");
            set_text(sb, "4 items", "Other");
            assert!(hk.take().contains(&(EVENT_OBJECT_NAMECHANGE, sb, 2)));
            set_text(sb, "", "Details");
            assert_ne!(kid(&acc, 1).state & STATE_SYSTEM_INVISIBLE.0, 0);
            assert_eq!(root(&acc).name, "Status bar", "never an empty name");
            drop(hk);
            let _ = DestroyWindow(parent);
            pump();
            assert_eq!(acc.accChildCount().unwrap_err().code(), windows::Win32::Foundation::CO_E_OBJNOTCONNECTED);
        }
    }
}
