//! Windows word-delete keys for native single-line Edits: Ctrl+Backspace (arrives as WM_CHAR 0x7F) and Ctrl+Delete.

use super::*;

const SUBCLASS_ID: usize = 0xED17;

/// Adds the word-delete keys to `edit`. Own subclass id, so it chains with the caller's own subclass.
pub(crate) unsafe fn attach(edit: HWND) {
    let _ = SetWindowSubclass(edit, Some(subclass), SUBCLASS_ID, 0);
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
