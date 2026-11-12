//! "Connect to server": an FTP URL builder + modal dialog, and discovery of cloud-sync roots.
#![allow(dead_code)] // wired into main.rs by a later integration step

use std::ffi::c_void;
use std::path::Path;
use std::sync::Once;

use rexplorer_core::i18n::tr;
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

const ID_HOST: i32 = 101; // edits are ID_HOST.. in label order
const LABELS: [&str; 5] = ["conn.host", "conn.port", "conn.user", "conn.password", "conn.path"];
const DM_GETDEFID: u32 = 0x0400; // WM_USER
const DC_HASDEFID: isize = 0x534B;

/// A small themed form: a title, an optional line of text above the fields, labeled one-line fields (`masked`
/// shows dots) and OK / Cancel. The Connect dialog and the archive password prompt.
pub struct Form<'a> {
    pub title: String,
    pub note: Option<String>,
    /// i18n keys.
    pub labels: &'a [&'static str],
    pub masked: Option<usize>,
    pub ok: String,
}

struct State {
    theme: Theme,
    font: HFONT,
    /// The dialog's own font (the same as `font`), deleted when it closes or a DPI change replaces it.
    own: HFONT,
    /// The dialog is one surface card; fields are rounded wells in the background color.
    bg: HBRUSH,
    field: HBRUSH,
    note: HWND,
    labels: Vec<HWND>,
    edits: Vec<HWND>,
    ok: HWND,
    cancel: HWND,
    ok_text: String,
    masked: Option<usize>,
    /// Each field's text as typed, on OK.
    result: Option<Vec<Vec<u16>>>,
    done: bool,
}

type Rc4 = (i32, i32, i32, i32); // x, y, w, h

#[derive(Debug)]
struct Layout {
    note: Rc4,
    labels: Vec<Rc4>,
    edits: Vec<Rc4>,
    connect: Rc4,
    cancel: Rc4,
}

/// Client size, in 96-DPI pixels, for `rows` fields (a note takes a row): 440 x 280 for Connect's five.
fn client(rows: i32) -> (i32, i32) {
    (440, 80 + 40 * rows)
}

/// The DPI the dialog is laid out at: the window's, grown by the Windows text size (every control holds text).
fn dpi_of(h: HWND) -> i32 {
    theme::text_dpi(unsafe { GetDpiForWindow(h) }) as i32
}

/// Edit rects are the rounded fields; the Edit itself sits inset in its field. The note, if any, is the first row.
fn layout(cw: i32, ch: i32, dpi: i32, fields: usize, note: bool) -> Layout {
    let s = |v: i32| v * dpi / 96;
    let (pad, label_w, row_h, gap, btn_w, btn_h) = (s(16), s(112), s(32), s(8), s(96), s(32));
    let first = i32::from(note);
    let mut l = Layout { note: (pad, pad, cw - 2 * pad, row_h), labels: Vec::new(), edits: Vec::new(), connect: (0, 0, 0, 0), cancel: (0, 0, 0, 0) };
    for i in 0..fields as i32 {
        let y = pad + (first + i) * (row_h + gap);
        l.labels.push((pad, y, label_w, row_h));
        l.edits.push((pad + label_w, y, cw - 2 * pad - label_w, row_h));
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
            hIcon: LoadIconW(Some(hinst), PCWSTR(std::ptr::without_provenance(1))).unwrap_or_default(),
            lpfnWndProc: Some(wndproc),
            lpszClassName: w!("RexConnect"),
            ..Default::default()
        };
        RegisterClassW(&wc); // ERROR_CLASS_ALREADY_EXISTS is harmless
    });
}

/// The Edit's text without its terminator.
unsafe fn wide_text(h: HWND) -> Vec<u16> {
    let mut buf = vec![0u16; GetWindowTextLengthW(h) as usize + 1];
    let n = GetWindowTextW(h, &mut buf) as usize;
    buf.truncate(n);
    buf
}

/// Connect's fields as its URL; None for a host that cannot go into one.
fn collect(t: &[Vec<u16>]) -> Option<String> {
    let t: Vec<String> = t.iter().map(|w| String::from_utf16_lossy(w)).collect();
    let host = t[0].trim();
    // An unparsable port is treated as absent.
    valid_host(host).then(|| ftp_url(host, t[1].trim().parse().ok(), t[2].trim(), &t[3], t[4].trim()))
}

unsafe fn create_children(hwnd: HWND, st: &mut State, form: &Form) {
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
    if let Some(note) = &form.note {
        let text = HSTRING::from(note.as_str());
        // SS_CENTERIMAGE | SS_ENDELLIPSIS: one line, cut with "..." when the archive name is long.
        st.note = mk(w!("STATIC"), PCWSTR(text.as_ptr()), 0x200 | 0x4000, WINDOW_EX_STYLE(0), -1);
    }
    for (i, key) in form.labels.iter().enumerate() {
        let label = HSTRING::from(tr(key));
        st.labels.push(mk(w!("STATIC"), PCWSTR(label.as_ptr()), 0x200 /* SS_CENTERIMAGE */, WINDOW_EX_STYLE(0), -1));
        let pw = if form.masked == Some(i) { ES_PASSWORD as u32 } else { 0 };
        let style = WS_TABSTOP.0 | ES_AUTOHSCROLL as u32 | pw;
        let edit = mk(w!("EDIT"), PCWSTR::null(), style, WINDOW_EX_STYLE(0), ID_HOST + i as i32);
        crate::edit::attach(edit);
        st.edits.push(edit);
    }
    // Pills drawn on WM_DRAWITEM; Enter still means OK through DM_GETDEFID.
    let (connect, cancel) = (HSTRING::from(st.ok_text.as_str()), HSTRING::from(tr("conn.cancel")));
    st.ok = mk(w!("BUTTON"), PCWSTR(connect.as_ptr()), WS_TABSTOP.0 | BS_OWNERDRAW as u32, WINDOW_EX_STYLE(0), IDOK.0);
    st.cancel = mk(w!("BUTTON"), PCWSTR(cancel.as_ptr()), WS_TABSTOP.0 | BS_OWNERDRAW as u32, WINDOW_EX_STYLE(0), IDCANCEL.0);
}

unsafe fn layout_of(hwnd: HWND, st: &State) -> Layout {
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    layout(rc.right, rc.bottom, dpi_of(hwnd), st.edits.len(), !st.note.0.is_null())
}

unsafe fn place(hwnd: HWND, st: &State) {
    let l = layout_of(hwnd, st);
    let dpi = dpi_of(hwnd);
    // A single-line Edit draws its text at the top: one line high, centered in its field.
    let eh = st.theme.font_size * dpi / 72 * 4 / 3 + 4 * dpi / 96;
    let inset = 8 * dpi / 96;
    let mv = |h: HWND, r: Rc4| {
        let _ = MoveWindow(h, r.0, r.1, r.2, r.3, true);
    };
    if !st.note.0.is_null() {
        mv(st.note, l.note);
    }
    for i in 0..st.edits.len() {
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
    let dpi = dpi_of(di.hwndItem);
    let focus = di.itemState.0 & ODS_FOCUS.0 != 0;
    // One pixel in: the outline includes its right and bottom edge.
    let inner = RECT { right: di.rcItem.right - 1, bottom: di.rcItem.bottom - 1, ..di.rcItem };
    let contrast = theme::contrast_button(dc, &inner, t, di.itemState.0 & ODS_SELECTED.0 != 0, focus, false, 8 * dpi / 96);
    if contrast.is_none() {
        theme::fill_round(dc, &di.rcItem, fill, 8 * dpi / 96);
    }
    SetBkMode(dc, TRANSPARENT);
    SetTextColor(dc, COLORREF(contrast.unwrap_or(if ok { 0x00FF_FFFF } else { t.text })));
    let old = SelectObject(dc, st.font.into());
    let mut r = di.rcItem;
    let mut text: Vec<u16> = if ok { st.ok_text.as_str() } else { tr("conn.cancel") }.encode_utf16().collect();
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
        // Moved to a monitor of another scale: take the suggested rect (WM_SIZE lays the controls out again).
        WM_DPICHANGED => {
            // The font is sized for the previous monitor: make one for this one.
            let f = theme::ui_font(&st.theme, (wparam.0 & 0xFFFF) as u32);
            for h in st.labels.iter().chain(&st.edits).copied().chain([st.note, st.ok, st.cancel]).filter(|h| !h.0.is_null()) {
                SendMessageW(h, WM_SETFONT, Some(WPARAM(f.0 as usize)), Some(LPARAM(1)));
            }
            if !st.own.is_invalid() {
                let _ = DeleteObject(st.own.into());
            }
            (st.own, st.font) = (f, f);
            let r = &*(lparam.0 as *const RECT);
            let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            let _ = InvalidateRect(Some(hwnd), None, true);
            LRESULT(0)
        }
        WM_ERASEBKGND => {
            let dc = HDC(wparam.0 as *mut c_void);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            FillRect(dc, &rc, st.bg);
            let radius = 8 * dpi_of(hwnd) / 96;
            for (x, y, w, h) in layout_of(hwnd, st).edits {
                let field = RECT { left: x, top: y, right: x + w, bottom: y + h };
                theme::fill_round(dc, &field, st.theme.background, radius);
                if st.theme.contrast.is_some() {
                    theme::outline_round(dc, &field, st.theme.text, radius, 1);
                }
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
                    st.result = Some(st.edits.iter().map(|&e| wide_text(e)).collect());
                    // The masked field's text is overwritten in the Edit's own buffer before it goes.
                    if let Some(&e) = st.masked.and_then(|i| st.edits.get(i)) {
                        let blank: Vec<u16> = std::iter::repeat_n(u16::from(b' '), GetWindowTextLengthW(e) as usize).chain([0]).collect();
                        let _ = SetWindowTextW(e, PCWSTR(blank.as_ptr()));
                        let _ = SetWindowTextW(e, w!(""));
                    }
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

/// The Connect dialog: the FTP URL its fields make, None when cancelled or the host cannot be used.
pub fn dialog(parent: HWND, t: &Theme) -> Option<String> {
    let f = Form { title: tr("conn.title").into(), note: None, labels: &LABELS, masked: Some(3), ok: tr("conn.connect").into() };
    collect(&form(parent, t, &f)?)
}

/// Shows `f` modally over `parent`: each field's text on OK, None on Cancel or close.
pub fn form(parent: HWND, t: &Theme, f: &Form) -> Option<Vec<Vec<u16>>> {
    register();
    unsafe {
        let has_parent = !parent.0.is_null();
        let frame_dpi = if has_parent { GetDpiForWindow(parent) } else { 96 };
        // Its own font: the main window replaces (and deletes) its font on a text size change while this is open.
        let font = theme::ui_font(t, frame_dpi);
        let font = if font.is_invalid() { HFONT(GetStockObject(DEFAULT_GUI_FONT).0) } else { font };
        let mut st = Box::new(State {
            theme: t.clone(),
            font,
            own: font,
            bg: CreateSolidBrush(COLORREF(t.surface)),
            field: CreateSolidBrush(COLORREF(t.background)),
            note: HWND::default(),
            labels: Vec::new(),
            edits: Vec::new(),
            ok: HWND::default(),
            cancel: HWND::default(),
            ok_text: f.ok.clone(),
            masked: f.masked,
            result: None,
            done: false,
        });
        let dpi = theme::text_dpi(frame_dpi) as i32;
        let size = client(f.labels.len() as i32 + i32::from(f.note.is_some()));
        // The window size that gives that client area under a caption (the frame follows the display scale only).
        let mut wr = RECT { left: 0, top: 0, right: size.0 * dpi / 96, bottom: size.1 * dpi / 96 };
        let _ = AdjustWindowRectExForDpi(&mut wr, WS_POPUP | WS_CAPTION | WS_SYSMENU, false, WINDOW_EX_STYLE(0), frame_dpi);
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
            &HSTRING::from(f.title.as_str()),
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
                // A contrast theme keeps the system's caption and frame.
                if t.contrast.is_none() {
                    let dark = t.dark as i32;
                    let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const i32 as *const c_void, 4);
                    // The title bar and frame merge into the card.
                    for attr in [DWMWA_CAPTION_COLOR, DWMWA_BORDER_COLOR] {
                        let _ = DwmSetWindowAttribute(hwnd, attr, &t.surface as *const u32 as *const c_void, 4);
                    }
                }
                create_children(hwnd, &mut st, f);
                place(hwnd, &st);
                if has_parent {
                    let _ = EnableWindow(parent, false);
                }
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetFocus(Some(st.edits[0]));
                let mut msg = MSG::default();
                #[allow(clippy::while_immutable_condition)] // st.done is set by the window procedure during GetMessageW
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
        if !st.own.is_invalid() {
            let _ = DeleteObject(st.own.into());
        }
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
        assert_eq!(client(5), (440, 280), "Connect keeps its size");
        for dpi in [96, 144, 192] {
            for (fields, note) in [(5, false), (1, true)] {
                let size = client(fields as i32 + i32::from(note));
                let l = layout(size.0 * dpi / 96, size.1 * dpi / 96, dpi, fields, note);
                for i in 1..fields {
                    assert!(l.edits[i].1 >= l.edits[i - 1].1 + l.edits[i - 1].3);
                }
                if note {
                    assert!(l.edits[0].1 >= l.note.1 + l.note.3, "the field is below the note");
                }
                assert!(l.connect.1 >= l.edits[fields - 1].1 + l.edits[fields - 1].3);
                assert!(l.connect.0 + l.connect.2 <= l.cancel.0);
            }
        }
    }
}
