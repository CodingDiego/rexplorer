//! The copy engine's conflict dialog, answered from a test: found by its window class in this process and pressed
//! through its accessibility tree, language-neutral (by structure and position). Shared by tests/data_ops.rs and the
//! undo unit tests (`#[path]`), so it names no item of the crate.
#![allow(dead_code)]

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize, IDispatch};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{AccessibleChildren, AccessibleObjectFromWindow, IAccessible, ROLE_SYSTEM_PUSHBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible, OBJID_CLIENT, PostMessageW, WM_CLOSE,
};
use windows::core::{BOOL, Interface};

/// The answers of the shell's own "Replace or Skip Files" dialog, in its order.
pub const REPLACE: usize = 0;
pub const SKIP: usize = 1;

/// The push buttons with an action in the MSAA tree under `acc`, grouped by parent, in order (the title bar's
/// buttons are simple children without an object of their own).
fn action_buttons(acc: &IAccessible, depth: usize, out: &mut Vec<Vec<IAccessible>>) {
    let n = unsafe { acc.accChildCount() }.unwrap_or(0).clamp(0, 256) as usize;
    let mut kids = vec![VARIANT::default(); n];
    let mut got = 0;
    if depth > 12 || unsafe { AccessibleChildren(acc, 0, &mut kids, &mut got) }.is_err() {
        return;
    }
    let me = VARIANT::from(0i32);
    let mut mine = Vec::new();
    for k in kids.iter().take(got.max(0) as usize) {
        let Ok(sub) = IDispatch::try_from(k).and_then(|d| d.cast::<IAccessible>()) else { continue };
        let role = unsafe { sub.get_accRole(&me) }.ok().and_then(|v| i32::try_from(&v).ok());
        let action = unsafe { sub.get_accDefaultAction(&me) }.map(|b| !b.is_empty()).unwrap_or(false);
        if role == Some(ROLE_SYSTEM_PUSHBUTTON as i32) && action {
            mine.push(sub.clone());
        }
        action_buttons(&sub, depth + 1, out);
    }
    if !mine.is_empty() {
        out.push(mine);
    }
}

/// The answers of the copy engine's conflict panel, Replace, Skip and Compare: three action buttons under one
/// parent and none elsewhere. The progress panel has three too, but apart (the details toggle under the dialog,
/// Pause and Cancel under the panel), so the structure tells them apart in any UI language.
fn conflict_answers(groups: Vec<Vec<IAccessible>>) -> Option<Vec<IAccessible>> {
    match <[_; 1]>::try_from(groups) {
        Ok([one]) if one.len() == 3 => Some(one),
        _ => None,
    }
}

/// The OperationStatusWindow windows of this process, with whether each is visible.
pub fn status_windows() -> Vec<(isize, bool)> {
    unsafe extern "system" fn status(h: HWND, found: LPARAM) -> BOOL {
        let mut pid = 0;
        let mut class = [0u16; 32];
        let n = unsafe {
            GetWindowThreadProcessId(h, Some(&mut pid));
            GetClassNameW(h, &mut class)
        } as usize;
        if pid == std::process::id() && String::from_utf16_lossy(&class[..n]) == "OperationStatusWindow" {
            unsafe { (*(found.0 as *mut Vec<(isize, bool)>)).push((h.0 as isize, IsWindowVisible(h).as_bool())) };
        }
        BOOL(1)
    }
    let mut found: Vec<(isize, bool)> = Vec::new();
    let _ = unsafe { EnumWindows(Some(status), LPARAM(&mut found as *mut _ as isize)) };
    found
}

/// Presses answer `pick` (REPLACE, SKIP) in the first conflict panel of the copy engine this process shows within
/// 15 s. Err names the buttons last seen when it pressed none.
/// Join it before any assert or unwrap: a thread left running would press a button in the next test's dialog.
pub fn answer_conflict(pick: usize) -> std::thread::JoinHandle<std::result::Result<(), String>> {
    answer_conflict_after(pick, || {})
}

/// `answer_conflict`, running `before` once the conflict panel is on screen and before the answer is pressed.
pub fn answer_conflict_after(pick: usize, before: impl FnOnce() + Send + 'static) -> std::thread::JoinHandle<std::result::Result<(), String>> {
    answer_conflict_within(150, pick, before)
}

/// `answer_conflict_after`, giving up (and cancelling such a window) after `tries` tenths of a second. Call it before
/// the test starts its operation: it acts only on windows that appear after the call, never on another test's.
pub fn answer_conflict_within(tries: usize, pick: usize, before: impl FnOnce() + Send + 'static) -> std::thread::JoinHandle<std::result::Result<(), String>> {
    let older: Vec<isize> = status_windows().into_iter().map(|w| w.0).collect();
    let new_windows = move || status_windows().into_iter().filter(|w| !older.contains(&w.0)).collect::<Vec<_>>();
    std::thread::spawn(move || {
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let r = (|| {
            let me = VARIANT::from(0i32);
            let mut last = String::from("no visible OperationStatusWindow");
            for _ in 0..tries {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let Some(&(found, _)) = new_windows().iter().find(|w| w.1) else { continue };
                let mut p = std::ptr::null_mut();
                if unsafe { AccessibleObjectFromWindow(HWND(found as *mut _), OBJID_CLIENT.0 as u32, &IAccessible::IID, &mut p) }.is_err() || p.is_null() {
                    continue;
                }
                let acc = unsafe { IAccessible::from_raw(p) };
                let mut groups = Vec::new();
                action_buttons(&acc, 0, &mut groups);
                let names: Vec<Vec<String>> = groups.iter().map(|g| g.iter().map(|b| unsafe { b.get_accName(&me) }.map(|n| n.to_string()).unwrap_or_default()).collect()).collect();
                last = format!("action buttons by parent: {names:?}");
                // The progress panel may show first; the answers once the conflict is asked.
                if let Some(answers) = conflict_answers(groups) {
                    before();
                    return unsafe { answers[pick].accDoDefaultAction(&me) }.map_err(|e| format!("{e} pressing in {last}"));
                }
            }
            // Never answered: closing every such window this operation opened, shown or not, cancels it, so the test
            // fails instead of waiting for a person.
            for (h, _) in new_windows() {
                let _ = unsafe { PostMessageW(Some(HWND(h as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0)) };
            }
            Err(format!("no conflict panel within {} s; {last}", tries / 10))
        })();
        unsafe { CoUninitialize() };
        r
    })
}
