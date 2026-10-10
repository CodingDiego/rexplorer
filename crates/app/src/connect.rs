//! "Connect to server": an FTP URL builder + modal dialog, and discovery of cloud-sync roots.
#![allow(dead_code)] // wired into main.rs by a later integration step

use std::ffi::c_void;
use std::path::Path;
use std::sync::Once;

use rexplorer_core::settings::Theme;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::{GetLogicalDrives, GetVolumeInformationW};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_FOCUS, ODS_SELECTED};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

use crate::theme;

/// Everything outside [A-Za-z0-9-._~] becomes %XX (UTF-8 bytes).
fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn ftp_url(host: &str, port: Option<u16>, user: &str, password: &str, path: &str) -> String {
    let mut url = String::from("ftp://");
    if !user.is_empty() {
        url.push_str(&percent_encode(user));
        if !password.is_empty() {
            url.push(':');
            url.push_str(&percent_encode(password));
        }
        url.push('@');
    }
    url.push_str(host);
    if let Some(p) = port {
        url.push_str(&format!(":{p}"));
    }
    url.push('/');
    let path = path.replace('\\', "/");
    url.push_str(&path.trim_start_matches('/').split('/').map(percent_encode).collect::<Vec<_>>().join("/"));
    url
}

/// The host goes into the URL raw, so it must not be able to end the authority or smuggle in a userinfo/port.
fn valid_host(h: &str) -> bool {
    !h.is_empty() && !h.chars().any(|c| matches!(c, '@' | '/' | '\\' | '?' | '#' | ':') || c.is_whitespace() || c.is_control())
}

/// `ftp://user:pw@host/..` without the password, for titles, messages and the session file; other names unchanged.
pub fn redact(name: &str) -> String {
    if !name.get(..6).is_some_and(|p| p.eq_ignore_ascii_case("ftp://")) {
        return name.to_string();
    }
    let rest = &name[6..];
    let auth = &rest[..rest.find('/').unwrap_or(rest.len())];
    match auth.rfind('@').and_then(|at| auth[..at].find(':').map(|c| (c, at))) {
        Some((colon, at)) => format!("{}{}", &name[..6 + colon], &rest[at..]),
        None => name.to_string(),
    }
}

/// The JSON string after the first `"path":`; shortcut: any `\x` escape yields `x` (no \uXXXX), enough for paths.
fn dropbox_path(json: &str) -> Option<String> {
    let rest = json[json.find("\"path\":")? + 7..].trim_start().strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    loop {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => out.push(chars.next()?),
            c => out.push(c),
        }
    }
}

pub fn cloud_roots() -> Vec<(String, String)> {
    let mut roots = Vec::new();
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    if let Some(p) = env("OneDrive").filter(|p| Path::new(p).is_dir()) {
        roots.push(("OneDrive".to_string(), p));
    }
    unsafe {
        let mask = GetLogicalDrives();
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let root = format!("{}:\\", (b'A' + i as u8) as char);
            let mut label = [0u16; 261];
            let ok = GetVolumeInformationW(&HSTRING::from(root.as_str()), Some(&mut label), None, None, None, None);
            let len = label.iter().position(|&c| c == 0).unwrap_or(label.len());
            if ok.is_ok() && String::from_utf16_lossy(&label[..len]).contains("Google Drive") {
                roots.push(("Google Drive".to_string(), root));
            }
        }
    }
    let dropbox = ["APPDATA", "LOCALAPPDATA"]
        .iter()
        .filter_map(|k| env(k))
        .filter_map(|d| std::fs::read_to_string(Path::new(&d).join("Dropbox").join("info.json")).ok())
        .find_map(|j| dropbox_path(&j));
    if let Some(p) = dropbox {
        roots.push(("Dropbox".to_string(), p));
    }
    if let Some(p) = env("USERPROFILE").map(|u| Path::new(&u).join("iCloudDrive")).filter(|p| p.is_dir()) {
        roots.push(("iCloud Drive".to_string(), p.to_string_lossy().into_owned()));
    }
    roots
}

// ---- dialog ----

const ID_HOST: i32 = 101; // edits are ID_HOST..ID_HOST+5 in label order
const LABELS: [PCWSTR; 5] = [w!("Host"), w!("Port (optional)"), w!("User"), w!("Password"), w!("Path")];
const DM_GETDEFID: u32 = 0x0400; // WM_USER
const DC_HASDEFID: isize = 0x534B;

struct State {
    theme: Theme,
    font: HFONT,
    /// The dialog is one surface card; fields are rounded wells in the background color.
    bg: HBRUSH,
    field: HBRUSH,
    labels: [HWND; 5],
    edits: [HWND; 5],
    ok: HWND,
    cancel: HWND,
    result: Option<String>,
    done: bool,
}

type Rc4 = (i32, i32, i32, i32); // x, y, w, h

#[derive(Debug)]
struct Layout {
    labels: [Rc4; 5],
    edits: [Rc4; 5],
    connect: Rc4,
    cancel: Rc4,
}

/// Client size the dialog is laid out for, in 96-DPI pixels.
const CLIENT: (i32, i32) = (440, 280);

/// Edit rects are the rounded fields; the Edit itself sits inset in its field.
fn layout(cw: i32, ch: i32, dpi: i32) -> Layout {
    let s = |v: i32| v * dpi / 96;
    let (pad, label_w, row_h, gap, btn_w, btn_h) = (s(16), s(112), s(32), s(8), s(96), s(32));
    let mut l = Layout { labels: Default::default(), edits: Default::default(), connect: (0, 0, 0, 0), cancel: (0, 0, 0, 0) };
    for i in 0..5 {
        let y = pad + i as i32 * (row_h + gap);
        l.labels[i] = (pad, y, label_w, row_h);
        l.edits[i] = (pad + label_w, y, cw - 2 * pad - label_w, row_h);
    }
    let by = ch - pad - btn_h;
    l.cancel = (cw - pad - btn_w, by, btn_w, btn_h);
    l.connect = (cw - pad - 2 * btn_w - gap, by, btn_w, btn_h);
    l
}

fn register() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let hinst = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
        let wc = WNDCLASSW {
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: LoadIconW(Some(hinst), PCWSTR(1 as *const u16)).unwrap_or_default(),
            lpfnWndProc: Some(wndproc),
            lpszClassName: w!("RexConnect"),
            ..Default::default()
        };
        RegisterClassW(&wc); // ERROR_CLASS_ALREADY_EXISTS is harmless
    });
}

unsafe fn text_of(h: HWND) -> String {
    let mut buf = vec![0u16; GetWindowTextLengthW(h) as usize + 1];
    let n = GetWindowTextW(h, &mut buf) as usize;
    String::from_utf16_lossy(&buf[..n])
}

unsafe fn collect(st: &State) -> Option<String> {
    let t: Vec<String> = st.edits.iter().map(|&e| text_of(e)).collect();
    let host = t[0].trim();
    // An unparsable port is treated as absent.
    valid_host(host).then(|| ftp_url(host, t[1].trim().parse().ok(), t[2].trim(), &t[3], t[4].trim()))
}

unsafe fn create_children(hwnd: HWND, st: &mut State) {
    let hinst = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
    let mk = |class: PCWSTR, text: PCWSTR, style: u32, ex: WINDOW_EX_STYLE, id: i32| {
        let h = CreateWindowExW(
            ex,
            class,
            text,
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(style),
            0,
            0,
            0,
            0,
            Some(hwnd),
            Some(HMENU(id as usize as *mut c_void)),
            Some(hinst),
            None,
        )
        .unwrap_or_default();
        SendMessageW(h, WM_SETFONT, Some(WPARAM(st.font.0 as usize)), Some(LPARAM(1)));
        h
    };
    for i in 0..5 {
        st.labels[i] = mk(w!("STATIC"), LABELS[i], 0x200 /* SS_CENTERIMAGE */, WINDOW_EX_STYLE(0), -1);
        let pw = if i == 3 { ES_PASSWORD as u32 } else { 0 };
        let style = WS_TABSTOP.0 | ES_AUTOHSCROLL as u32 | pw;
        st.edits[i] = mk(w!("EDIT"), PCWSTR::null(), style, WINDOW_EX_STYLE(0), ID_HOST + i as i32);
        crate::edit::attach(st.edits[i]);
    }
    // Pills drawn on WM_DRAWITEM; Enter still means Connect through DM_GETDEFID.
    st.ok = mk(w!("BUTTON"), w!("Connect"), WS_TABSTOP.0 | BS_OWNERDRAW as u32, WINDOW_EX_STYLE(0), IDOK.0);
    st.cancel = mk(w!("BUTTON"), w!("Cancel"), WS_TABSTOP.0 | BS_OWNERDRAW as u32, WINDOW_EX_STYLE(0), IDCANCEL.0);
}

unsafe fn layout_of(hwnd: HWND) -> Layout {
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    layout(rc.right, rc.bottom, GetDpiForWindow(hwnd) as i32)
}

unsafe fn place(hwnd: HWND, st: &State) {
    let l = layout_of(hwnd);
    let dpi = GetDpiForWindow(hwnd) as i32;
    // A single-line Edit draws its text at the top: one line high, centered in its field.
    let eh = st.theme.font_size * dpi / 72 * 4 / 3 + 4 * dpi / 96;
    let inset = 8 * dpi / 96;
    let mv = |h: HWND, r: Rc4| {
        let _ = MoveWindow(h, r.0, r.1, r.2, r.3, true);
    };
    for i in 0..5 {
        mv(st.labels[i], l.labels[i]);
        let (x, y, w, h) = l.edits[i];
        mv(st.edits[i], (x + inset, y + (h - eh) / 2, w - 2 * inset, eh));
    }
    mv(st.ok, l.connect);
    mv(st.cancel, l.cancel);
}

/// Connect is an accent pill with white text, Cancel a background-colored one; pressed or focused buttons darken.
unsafe fn draw_button(st: &State, di: &DRAWITEMSTRUCT) {
    let t = &st.theme;
    let ok = di.CtlID as i32 == IDOK.0;
    let base = if ok { t.accent } else { t.background };
    let fill = if di.itemState.0 & ODS_SELECTED.0 != 0 {
        theme::blend(base, 0, 0.2)
    } else if di.itemState.0 & ODS_FOCUS.0 != 0 {
        theme::blend(base, t.text, 0.1)
    } else {
        base
    };
    let dc = di.hDC;
    let surface = CreateSolidBrush(COLORREF(t.surface));
    FillRect(dc, &di.rcItem, surface);
    let _ = DeleteObject(surface.into());
    theme::fill_round(dc, &di.rcItem, fill, 8 * GetDpiForWindow(di.hwndItem) as i32 / 96);
    SetBkMode(dc, TRANSPARENT);
    SetTextColor(dc, COLORREF(if ok { 0x00FF_FFFF } else { t.text }));
    let old = SelectObject(dc, st.font.into());
    let mut r = di.rcItem;
    let mut text: Vec<u16> = if ok { "Connect" } else { "Cancel" }.encode_utf16().collect();
    DrawTextW(dc, &mut text, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    SelectObject(dc, old);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lparam.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let st = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if st.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let st = &mut *st;
    match msg {
        WM_SIZE => {
            if !st.ok.0.is_null() {
                place(hwnd, st);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => {
            let dc = HDC(wparam.0 as *mut c_void);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            FillRect(dc, &rc, st.bg);
            let radius = 8 * GetDpiForWindow(hwnd) as i32 / 96;
            for (x, y, w, h) in layout_of(hwnd).edits {
                theme::fill_round(dc, &RECT { left: x, top: y, right: x + w, bottom: y + h }, st.theme.background, radius);
            }
            LRESULT(1)
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => {
            let hdc = HDC(wparam.0 as *mut c_void);
            let in_field = st.edits.contains(&HWND(lparam.0 as *mut c_void));
            SetTextColor(hdc, COLORREF(if in_field { st.theme.text } else { st.theme.secondary_text }));
            SetBkColor(hdc, COLORREF(if in_field { st.theme.background } else { st.theme.surface }));
            LRESULT(if in_field { st.field.0 } else { st.bg.0 } as isize)
        }
        WM_DRAWITEM => {
            draw_button(st, &*(lparam.0 as *const DRAWITEMSTRUCT));
            LRESULT(1)
        }
        // IsDialogMessageW asks this to find the Enter button; without it Enter does nothing in a plain window.
        DM_GETDEFID => LRESULT((DC_HASDEFID << 16) | IDOK.0 as isize),
        WM_COMMAND => {
            match (wparam.0 & 0xFFFF) as i32 {
                x if x == IDOK.0 => {
                    st.result = collect(st);
                    st.done = true;
                }
                x if x == IDCANCEL.0 => st.done = true,
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            st.done = true;
            LRESULT(0)
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0); // dialog() owns and drops the State
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub fn dialog(parent: HWND, t: &Theme, font: HFONT) -> Option<String> {
    register();
    unsafe {
        let font = if font.is_invalid() { HFONT(GetStockObject(DEFAULT_GUI_FONT).0) } else { font };
        let mut st = Box::new(State {
            theme: t.clone(),
            font,
            bg: CreateSolidBrush(COLORREF(t.surface)),
            field: CreateSolidBrush(COLORREF(t.background)),
            labels: [HWND::default(); 5],
            edits: [HWND::default(); 5],
            ok: HWND::default(),
            cancel: HWND::default(),
            result: None,
            done: false,
        });
        let has_parent = !parent.0.is_null();
        let dpi = if has_parent { GetDpiForWindow(parent) as i32 } else { 96 };
        // The window size that gives a CLIENT-sized client area under a caption.
        let mut wr = RECT { left: 0, top: 0, right: CLIENT.0 * dpi / 96, bottom: CLIENT.1 * dpi / 96 };
        let _ = AdjustWindowRectExForDpi(&mut wr, WS_POPUP | WS_CAPTION | WS_SYSMENU, false, WINDOW_EX_STYLE(0), dpi as u32);
        let (w, h) = (wr.right - wr.left, wr.bottom - wr.top);
        let mut pr = RECT::default();
        if !has_parent || GetWindowRect(parent, &mut pr).is_err() {
            pr = RECT { left: 0, top: 0, right: GetSystemMetrics(SM_CXSCREEN), bottom: GetSystemMetrics(SM_CYSCREEN) };
        }
        let x = pr.left + (pr.right - pr.left - w) / 2;
        let y = pr.top + (pr.bottom - pr.top - h) / 2;
        let hinst = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
        let created = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("RexConnect"),
            w!("Connect to FTP"),
            WS_POPUP | WS_CAPTION | WS_SYSMENU,
            x,
            y,
            w,
            h,
            has_parent.then_some(parent),
            None,
            Some(hinst),
            Some(&mut *st as *mut State as *const c_void),
        );
        let result = match created {
            Ok(hwnd) => {
                let dark = t.dark as i32;
                let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const i32 as *const c_void, 4);
                // The title bar and frame merge into the card.
                for attr in [DWMWA_CAPTION_COLOR, DWMWA_BORDER_COLOR] {
                    let _ = DwmSetWindowAttribute(hwnd, attr, &t.surface as *const u32 as *const c_void, 4);
                }
                create_children(hwnd, &mut st);
                place(hwnd, &st);
                if has_parent {
                    let _ = EnableWindow(parent, false);
                }
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetFocus(Some(st.edits[0]));
                let mut msg = MSG::default();
                while !st.done {
                    if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        PostQuitMessage(msg.wParam.0 as i32); // re-post WM_QUIT for the outer loop
                        break;
                    }
                    if !IsDialogMessageW(hwnd, &msg).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                if has_parent {
                    let _ = EnableWindow(parent, true); // before destroy, so activation returns to the parent
                }
                let _ = DestroyWindow(hwnd);
                st.result.take()
            }
            Err(_) => None,
        };
        let _ = DeleteObject(st.bg.into());
        let _ = DeleteObject(st.field.into());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ftp_url_table() {
        assert_eq!(ftp_url("h.example", None, "", "", ""), "ftp://h.example/");
        assert_eq!(ftp_url("h", None, "", "ignored", "/a/b"), "ftp://h/a/b");
        assert_eq!(ftp_url("h", None, "bob", "", "x"), "ftp://bob@h/x");
        assert_eq!(ftp_url("h", None, "me@x.org", "p:w@d/1", ""), "ftp://me%40x.org:p%3Aw%40d%2F1@h/");
        assert_eq!(ftp_url("h", Some(2121), "u", "p", "\\\\dir\\sub"), "ftp://u:p@h:2121/dir/sub");
        assert_eq!(ftp_url("h", None, "\u{f1}", "", ""), "ftp://%C3%B1@h/");
    }

    #[test]
    fn ftp_url_encodes_path_segments() {
        assert_eq!(ftp_url("h", None, "", "", "my dir/a#b/c?d"), "ftp://h/my%20dir/a%23b/c%3Fd");
    }

    #[test]
    fn host_validation() {
        assert!(valid_host("ftp.example.com") && valid_host("10.0.0.1"));
        for bad in ["", "a@b", "a/b", "a\\b", "a?b", "a#b", "a:21", "a b", "a\tb"] {
            assert!(!valid_host(bad), "{bad}");
        }
    }

    #[test]
    fn redact_drops_the_password_only() {
        assert_eq!(redact("ftp://bob:s3cret@h/x"), "ftp://bob@h/x");
        assert_eq!(redact("ftp://bob:s3cret@h:2121/x"), "ftp://bob@h:2121/x");
        assert_eq!(redact("ftp://me%40x.org:p%40ss@h/"), "ftp://me%40x.org@h/");
        assert_eq!(redact("ftp://bob@h/x"), "ftp://bob@h/x");
        assert_eq!(redact("ftp://h/a:b@c"), "ftp://h/a:b@c"); // the colon and @ are in the path
        assert_eq!(redact("C:\\a:b@c"), "C:\\a:b@c");
        assert_eq!(redact(&ftp_url("h", None, "u", "p@:/", "")), "ftp://u@h/");
    }

    #[test]
    fn dropbox_json() {
        let j = r#"{"personal": {"path": "C:\\Users\\a b\\Dropbox", "host": 1}}"#;
        assert_eq!(dropbox_path(j).as_deref(), Some("C:\\Users\\a b\\Dropbox"));
        assert_eq!(dropbox_path("{}"), None);
    }

    #[test]
    fn cloud_roots_are_real() {
        for (label, target) in cloud_roots() {
            assert!(!label.is_empty());
            let drive_root = target.len() == 3 && target.ends_with(":\\");
            assert!(Path::new(&target).exists() || drive_root, "{target}");
        }
    }

    #[test]
    fn layout_rows_do_not_overlap() {
        for dpi in [96, 144, 192] {
            let l = layout(CLIENT.0 * dpi / 96, CLIENT.1 * dpi / 96, dpi);
            for i in 1..5 {
                assert!(l.edits[i].1 >= l.edits[i - 1].1 + l.edits[i - 1].3);
            }
            assert!(l.connect.1 >= l.edits[4].1 + l.edits[4].3);
            assert!(l.connect.0 + l.connect.2 <= l.cancel.0);
        }
    }
}
