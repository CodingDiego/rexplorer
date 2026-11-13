//! Windows word-delete keys for native single-line Edits: Ctrl+Backspace (arrives as WM_CHAR 0x7F) and Ctrl+Delete.

use super::*;

const SUBCLASS_ID: usize = 0xED17;

/// Adds the word-delete keys to `edit`. Own subclass id, so it chains with the caller's own subclass.
pub(crate) unsafe fn attach(edit: HWND) {
    let _ = SetWindowSubclass(edit, Some(subclass), SUBCLASS_ID, 0);
}

/// Explorer's address bar suggestions: paths and typed URLs as you type; forced on/off so the per-user AutoSuggest/Append keys do not decide.
pub(crate) unsafe fn autocomplete(edit: HWND) -> Result<()> {
    use windows::Win32::UI::Shell::{SHACF_AUTOAPPEND_FORCE_OFF, SHACF_AUTOSUGGEST_FORCE_ON, SHACF_FILESYSTEM, SHACF_URLALL, SHAutoComplete};
    SHAutoComplete(edit, SHACF_FILESYSTEM | SHACF_URLALL | SHACF_AUTOSUGGEST_FORCE_ON | SHACF_AUTOAPPEND_FORCE_OFF)
}

/// Whether the address bar's suggestion list shows: SHAutoComplete's popup, a top-level window of this thread.
pub(crate) unsafe fn suggestions_open() -> bool {
    unsafe extern "system" fn find(h: HWND, found: LPARAM) -> windows::core::BOOL {
        let mut class = [0u16; 32];
        let n = unsafe { GetClassNameW(h, &mut class) }.max(0) as usize;
        if String::from_utf16_lossy(&class[..n]) == "Auto-Suggest Dropdown" && unsafe { IsWindowVisible(h) }.as_bool() {
            unsafe { *(found.0 as *mut bool) = true };
            return windows::core::BOOL(0);
        }
        windows::core::BOOL(1)
    }
    let mut found = false;
    let _ = EnumThreadWindows(windows::Win32::System::Threading::GetCurrentThreadId(), Some(find), LPARAM(&mut found as *mut bool as isize));
    found
}

/// 0 blank, 1 word character, 2 punctuation.
fn class(c: u16) -> u8 {
    match char::from_u32(c as u32) {
        Some(c) if c.is_whitespace() => 0,
        Some(c) if c.is_alphanumeric() || c == '_' => 1,
        _ => 2,
    }
}

/// Start of the word before `pos`: skips blanks, then one run of a kind.
fn prev_boundary(t: &[u16], pos: usize) -> usize {
    let mut i = pos.min(t.len());
    while i > 0 && class(t[i - 1]) == 0 {
        i -= 1;
    }
    if i > 0 {
        let k = class(t[i - 1]);
        while i > 0 && class(t[i - 1]) == k {
            i -= 1;
        }
    }
    i
}

/// Start of the word after `pos`: skips one run of a kind, then blanks.
fn next_boundary(t: &[u16], pos: usize) -> usize {
    let mut i = pos.min(t.len());
    if i < t.len() && class(t[i]) != 0 {
        let k = class(t[i]);
        while i < t.len() && class(t[i]) == k {
            i += 1;
        }
    }
    while i < t.len() && class(t[i]) == 0 {
        i += 1;
    }
    i
}

/// Deletes the selection, or the span from the caret to the boundary `find` picks.
unsafe fn delete_word(edit: HWND, find: fn(&[u16], usize) -> usize) {
    let (mut a, mut b) = (0u32, 0u32);
    SendMessageW(edit, EM_GETSEL, Some(WPARAM(&mut a as *mut u32 as usize)), Some(LPARAM(&mut b as *mut u32 as isize)));
    if a == b {
        let n = GetWindowTextLengthW(edit).max(0) as usize;
        let mut buf = vec![0u16; n + 1];
        let got = GetWindowTextW(edit, &mut buf).max(0) as usize;
        let to = find(&buf[..got], a as usize) as u32;
        (a, b) = (a.min(to), a.max(to));
        SendMessageW(edit, EM_SETSEL, Some(WPARAM(a as usize)), Some(LPARAM(b as isize)));
    }
    // EM_REPLACESEL with wparam 1 keeps the deletion undoable.
    SendMessageW(edit, EM_REPLACESEL, Some(WPARAM(1)), Some(LPARAM(w!("").as_ptr() as isize)));
}

unsafe extern "system" fn subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    match msg {
        WM_CHAR if wparam.0 == 0x7F => {
            delete_word(hwnd, prev_boundary);
            LRESULT(0)
        }
        WM_KEYDOWN if wparam.0 == VK_DELETE.0 as usize && GetKeyState(VK_CONTROL.0 as i32) < 0 => {
            delete_word(hwnd, next_boundary);
            LRESULT(0)
        }
        _ => DefSubclassProc(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn address_autocomplete_attaches_and_keeps_typing() {
        unsafe {
            let _com = rexplorer_core::com::ComInit::sta();
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 200, 100, None, None, None, None).unwrap();
            let edit = CreateWindowExW(WINDOW_EX_STYLE(0), WC_EDITW, PCWSTR::null(), WS_CHILD, 0, 0, 100, 20, Some(parent), None, None, None).unwrap();
            assert!(autocomplete(edit).is_ok());
            attach(edit); // after the autocomplete's own subclass, so it sees the key first
            let text = HSTRING::from(r"C:\x y");
            SendMessageW(edit, WM_SETTEXT, None, Some(LPARAM(text.as_ptr() as isize)));
            SendMessageW(edit, EM_SETSEL, Some(WPARAM(6)), Some(LPARAM(6)));
            SendMessageW(edit, WM_CHAR, Some(WPARAM(0x7F)), None);
            let mut buf = [0u16; 32];
            let n = SendMessageW(edit, WM_GETTEXT, Some(WPARAM(buf.len())), Some(LPARAM(buf.as_mut_ptr() as isize))).0 as usize;
            assert_eq!(String::from_utf16_lossy(&buf[..n]), r"C:\x ");
            let _ = DestroyWindow(parent);
        }
    }

    #[test]
    fn suggestions_open_sees_only_a_shown_dropdown() {
        unsafe {
            assert!(!suggestions_open());
            let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap().into();
            unsafe extern "system" fn plain(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
                unsafe { DefWindowProcW(h, m, w, l) }
            }
            let wc = WNDCLASSW { lpfnWndProc: Some(plain), hInstance: hinst, lpszClassName: w!("Auto-Suggest Dropdown"), ..Default::default() };
            RegisterClassW(&wc);
            let popup = CreateWindowExW(WINDOW_EX_STYLE(0), w!("Auto-Suggest Dropdown"), PCWSTR::null(), WS_POPUP, 0, 0, 10, 10, None, None, Some(hinst), None).unwrap();
            assert!(!suggestions_open(), "hidden");
            let _ = ShowWindow(popup, SW_SHOWNOACTIVATE);
            assert!(suggestions_open());
            let _ = DestroyWindow(popup);
        }
    }

    #[test]
    fn back_by_word() {
        let t = u("foo bar baz");
        assert_eq!(prev_boundary(&t, 11), 8);
        assert_eq!(prev_boundary(&t, 8), 4);
        assert_eq!(prev_boundary(&t, 4), 0);
        assert_eq!(prev_boundary(&t, 0), 0);
        assert_eq!(prev_boundary(&u("a.b"), 3), 2);
        assert_eq!(prev_boundary(&u("a..."), 4), 1);
        assert_eq!(prev_boundary(&u("  "), 2), 0);
    }

    #[test]
    fn forward_by_word() {
        let t = u("foo bar baz");
        assert_eq!(next_boundary(&t, 0), 4);
        assert_eq!(next_boundary(&t, 5), 8);
        assert_eq!(next_boundary(&t, 8), 11);
        assert_eq!(next_boundary(&t, 11), 11);
        assert_eq!(next_boundary(&u("a.b"), 0), 1);
        assert_eq!(next_boundary(&u("a  "), 1), 3);
    }
}
