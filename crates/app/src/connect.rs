//! "Connect to server": an FTP URL builder + modal dialog, and discovery of cloud-sync roots.
#![allow(dead_code)] // wired into main.rs by a later integration step

use std::ffi::c_void;
use std::path::Path;
use std::sync::Once;

use rexplorer_core::settings::Theme;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::{GetLogicalDrives, GetVolumeInformationW};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::SetWindowTheme;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

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
    url.push_str(path.replace('\\', "/").trim_start_matches('/'));
    url
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
    bg: HBRUSH,
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

fn layout(cw: i32, ch: i32, dpi: i32) -> Layout {
    let s = |v: i32| v * dpi / 96;
    let (pad, label_w, row_h, gap, btn_w, btn_h) = (s(12), s(100), s(24), s(8), s(88), s(26));
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
    (!host.is_empty()).then(|| ftp_url(host, t[1].trim().parse().ok(), t[2].trim(), &t[3], t[4].trim()))
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
        st.edits[i] = mk(w!("EDIT"), PCWSTR::null(), style, WS_EX_CLIENTEDGE, ID_HOST + i as i32);
    }
    st.ok = mk(w!("BUTTON"), w!("Connect"), WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32, WINDOW_EX_STYLE(0), IDOK.0);
    st.cancel = mk(w!("BUTTON"), w!("Cancel"), WS_TABSTOP.0 | BS_PUSHBUTTON as u32, WINDOW_EX_STYLE(0), IDCANCEL.0);
    if st.theme.dark {
        for h in [st.ok, st.cancel] {
            let _ = SetWindowTheme(h, w!("DarkMode_Explorer"), PCWSTR::null());
        }
        for h in st.edits {
            let _ = SetWindowTheme(h, w!("DarkMode_CFD"), PCWSTR::null());
        }
    }
}

unsafe fn place(hwnd: HWND, st: &State) {
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let l = layout(rc.right, rc.bottom, GetDpiForWindow(hwnd) as i32);
    let mv = |h: HWND, r: Rc4| {
        let _ = MoveWindow(h, r.0, r.1, r.2, r.3, true);
    };
    for i in 0..5 {
        mv(st.labels[i], l.labels[i]);
        mv(st.edits[i], l.edits[i]);
    }
    mv(st.ok, l.connect);
    mv(st.cancel, l.cancel);
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
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            FillRect(HDC(wparam.0 as *mut c_void), &rc, st.bg);
            LRESULT(1)
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => {
            let hdc = HDC(wparam.0 as *mut c_void);
            SetTextColor(hdc, COLORREF(st.theme.text));
            SetBkColor(hdc, COLORREF(st.theme.background));
            LRESULT(st.bg.0 as isize)
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
            bg: CreateSolidBrush(COLORREF(t.background)),
            labels: [HWND::default(); 5],
            edits: [HWND::default(); 5],
            ok: HWND::default(),
            cancel: HWND::default(),
            result: None,
            done: false,
        });
        let has_parent = !parent.0.is_null();
        let dpi = if has_parent { GetDpiForWindow(parent) as i32 } else { 96 };
        let (w, h) = (420 * dpi / 96, 260 * dpi / 96);
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
                create_children(hwnd, &mut st);
                place(hwnd, &st);
                if has_parent {
                    let _ = EnableWindow(parent, false);
                }
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetFocus(Some(st.edits[0]));
                let mut msg = MSG::default();
                while !st.done && GetMessageW(&mut msg, None, 0, 0).as_bool() {
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
            let l = layout(400 * dpi / 96, 220 * dpi / 96, dpi);
            for i in 1..5 {
                assert!(l.edits[i].1 >= l.edits[i - 1].1 + l.edits[i - 1].3);
            }
            assert!(l.connect.1 >= l.edits[4].1 + l.edits[4].3);
            assert!(l.connect.0 + l.connect.2 <= l.cancel.0);
        }
    }
}
