//! `rexplorer --desktop`: rexplorer's context menu on the desktop. Explorer keeps drawing the desktop (it is the
//! shell); a low-level mouse hook takes right-clicks that land on its icon list and shows our menu instead, built
//! like the file list's: the shell's verbs for the selected icons, or View, Sort by, Refresh and Paste plus the
//! shell's background verbs. One helper runs per session; install starts it at sign-in.
//!
//! The hook runs on its own thread and only classifies and posts, so it never stalls the system's mouse input
//! (Windows drops a hook that is slow to answer). Everything else runs on the main thread.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use rexplorer_core::com::ComInit;
use rexplorer_core::desktop::{self, ARRANGE, DesktopView, ICON_SIZES, SORT_KEYS};
use rexplorer_core::folder::ShellFolder;
use rexplorer_core::item::ShellItem;
use rexplorer_core::menu::{ContextMenu, Picked};
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::DeleteObject;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{AttachThreadInput, CreateMutexW, GetCurrentThreadId};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, Result, w};

use crate::commands;
use crate::ui::{self, Entry};

/// The helper's hidden window; uninstall closes it by this class.
pub(crate) const CLASS: PCWSTR = w!("RexDesktop");
const WM_DESKTOP_MENU: u32 = WM_APP + 0x60;

/// The window the hook posts to, and the desktop list a right press started on.
static TARGET: AtomicIsize = AtomicIsize::new(0);
static LIST: AtomicIsize = AtomicIsize::new(0);
static PRESSED: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// The shell menu being shown: its owner-drawn and submenu messages are forwarded to it.
    static ACTIVE: RefCell<Option<ContextMenu>> = const { RefCell::new(None) };
    /// A menu is being built or shown.
    static BUSY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

// Own entry ids (`ui::build_menu` offsets them past the shell's).
const REFRESH: usize = 1;
const PASTE: usize = 2;
const ICON: usize = 0x100;
const ARRANGE_ID: usize = 0x110;
const SORT: usize = 0x120;

pub(crate) fn run() -> Result<()> {
    unsafe {
        let _once = CreateMutexW(None, true, w!("Local\\rexplorer-desktop"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _com = ComInit::ole();
        let hinst = GetModuleHandleW(None)?.into();
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst, lpszClassName: CLASS, ..Default::default() });
        let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, CLASS, w!("rexplorer desktop menu"), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst), None)?;
        TARGET.store(hwnd.0 as isize, Ordering::SeqCst);
        std::thread::spawn(hook_thread);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

fn hook_thread() {
    unsafe {
        let hinst = GetModuleHandleW(None).ok().map(Into::into);
        let Ok(hook) = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), hinst, 0) else { return };
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
        let _ = UnhookWindowsHookEx(hook);
    }
}

/// Takes a right press and release on the desktop's icon list; any other input passes on untouched.
unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let m = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        match wparam.0 as u32 {
            WM_RBUTTONDOWN => {
                let h = WindowFromPoint(m.pt);
                if desktop::is_desktop_list(h) {
                    LIST.store(h.0 as isize, Ordering::SeqCst);
                    PRESSED.store(true, Ordering::SeqCst);
                    return LRESULT(1);
                }
            }
            WM_RBUTTONUP if PRESSED.swap(false, Ordering::SeqCst) => {
                let target = HWND(TARGET.load(Ordering::SeqCst) as *mut _);
                let _ = PostMessageW(Some(target), WM_DESKTOP_MENU, WPARAM(LIST.load(Ordering::SeqCst) as usize), LPARAM(pack(m.pt)));
                return LRESULT(1);
            }
            _ => {}
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

/// A screen point in one LPARAM (64-bit), negative coordinates of left or upper monitors included.
fn pack(pt: POINT) -> isize {
    (((pt.y as u32 as u64) << 32) | pt.x as u32 as u64) as isize
}

fn unpack(v: isize) -> POINT {
    POINT { x: v as u64 as u32 as i32, y: (v as u64 >> 32) as u32 as i32 }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_DESKTOP_MENU => {
            show(hwnd, HWND(wparam.0 as *mut _), unpack(lparam.0));
            LRESULT(0)
        }
        WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR => ACTIVE
            .with(|a| a.borrow().as_ref().and_then(|m| m.handle_message(msg, wparam, lparam)))
            .unwrap_or_else(|| DefWindowProcW(hwnd, msg, wparam, lparam)),
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// The background menu's own part, ahead of the shell's: View, Sort by | Refresh, Paste.
unsafe fn background_entries(view: &DesktopView) -> Vec<Entry> {
    let size = view.icon_size().unwrap_or(0);
    let mut look: Vec<Entry> = ICON_SIZES.iter().enumerate().map(|(i, (t, s))| Entry { checked: *s == size, ..Entry::new(ICON + i, t, 0) }).collect();
    look.push(Entry::sep());
    look.extend(ARRANGE.iter().enumerate().map(|(i, (t, f))| Entry { checked: view.has_flag(*f).unwrap_or(false), ..Entry::new(ARRANGE_ID + i, t, 0) }));
    let sort = SORT_KEYS.iter().enumerate().map(|(i, (t, _))| Entry::new(SORT + i, t, 0)).collect();
    vec![
        Entry { sub: look, ..Entry::new(0, "View", 0xE8A9) },
        Entry { sub: sort, ..Entry::new(0, "Sort by", 0xE8CB) },
        Entry::sep(),
        Entry::new(REFRESH, "Refresh", 0xE72C),
        Entry::new(PASTE, "Paste", 0xE77F).enabled(commands::clipboard_has_files()),
        Entry::sep(),
    ]
}

unsafe fn show(hwnd: HWND, list: HWND, pt: POINT) {
    // A right-click while a menu is up: close it and answer this click once it is gone, as Explorer does (nesting
    // the second menu inside the first would re-enter ACTIVE). A click while the shell is still building one is
    // dropped.
    if BUSY.with(|b| b.replace(true)) {
        if ACTIVE.with(|a| a.try_borrow().map_or(true, |m| m.is_some())) {
            let _ = EndMenu();
            let _ = PostMessageW(Some(hwnd), WM_DESKTOP_MENU, WPARAM(list.0 as usize), LPARAM(pack(pt)));
        }
        return;
    }
    menu_for(hwnd, list, pt);
    BUSY.with(|b| b.set(false));
}

/// Gives the helper the foreground: a menu whose owner is not in front ignores clicks outside it, so it would never
/// close. The click that opened it was taken from Explorer, which leaves the helper without the right to take the
/// foreground; sharing the foreground thread's input state for the call grants it.
unsafe fn take_foreground(hwnd: HWND) {
    let front = GetWindowThreadProcessId(GetForegroundWindow(), None);
    let me = GetCurrentThreadId();
    let attached = front != 0 && front != me && AttachThreadInput(front, me, true).as_bool();
    let _ = SetForegroundWindow(hwnd);
    if attached {
        let _ = AttachThreadInput(front, me, false);
    }
}

unsafe fn menu_for(hwnd: HWND, list: HWND, pt: POINT) {
    // Explorer restarts replace the view: look it up for every menu.
    let Ok(view) = DesktopView::find() else { return };
    let index = DesktopView::index_at(list, pt);
    let _ = view.select_for_menu(list, index);
    let Ok(root) = ShellFolder::desktop() else { return };
    let items: Vec<ShellItem> = match index {
        Some(_) => DesktopView::selection(list).into_iter().filter_map(|i| view.item(i).ok()).map(|pidl| ShellItem { parent: root.folder.clone(), pidl }).collect(),
        None => Vec::new(),
    };
    let refs: Vec<&ShellItem> = items.iter().collect();
    let menu = if refs.is_empty() { ContextMenu::for_background(&root, hwnd) } else { ContextMenu::for_items(&root, &refs, hwnd) };
    let Ok(menu) = menu else { return };
    let own = if refs.is_empty() { background_entries(&view) } else { Vec::new() };
    let mut bitmaps = Vec::new();
    let hmenu = ui::build_menu(hwnd, &own, &mut bitmaps);
    let at = GetMenuItemCount(Some(hmenu)).max(0) as u32;
    let extended = GetAsyncKeyState(VK_SHIFT.0 as i32) < 0;
    // Explorer's menus follow Windows' app mode; read it for every menu, so a switch applies at once.
    crate::theme::set_menu_theme(rexplorer_core::import_explorer::apps_use_dark());
    take_foreground(hwnd);
    ACTIVE.with(|a| *a.borrow_mut() = Some(menu));
    let picked = ACTIVE.with(|a| a.borrow().as_ref().map(|m| m.show_in(hwnd, hmenu, at, pt.x, pt.y, extended)));
    let menu = ACTIVE.with(|a| a.borrow_mut().take());
    let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(hmenu);
    for b in bitmaps {
        let _ = DeleteObject(b.into());
    }
    let _ = match picked {
        Some(Ok(Picked::Rename)) => index.map_or(Ok(()), |i| view.rename(i)),
        Some(Ok(Picked::Own(REFRESH))) => view.refresh(),
        Some(Ok(Picked::Own(PASTE))) => menu.map_or(Ok(()), |m| m.invoke_verb(hwnd, "paste", false)),
        Some(Ok(Picked::Own(id))) if id >= SORT => SORT_KEYS.get(id - SORT).map_or(Ok(()), |(_, k)| view.sort_by(*k)),
        Some(Ok(Picked::Own(id))) if id >= ARRANGE_ID => ARRANGE.get(id - ARRANGE_ID).map_or(Ok(()), |(_, f)| view.toggle_flag(*f)),
        Some(Ok(Picked::Own(id))) if id >= ICON => ICON_SIZES.get(id - ICON).map_or(Ok(()), |(_, s)| view.set_icon_size(*s)),
        _ => Ok(()),
    };
}

/// Closes a running helper (before install or uninstall replaces or removes the exe). True if one was running.
pub(crate) fn stop() -> bool {
    unsafe {
        let Ok(h) = FindWindowW(CLASS, None) else { return false };
        let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_survive_the_lparam() {
        for pt in [POINT { x: 0, y: 0 }, POINT { x: -1920, y: 1080 }, POINT { x: 2559, y: -400 }] {
            let back = unpack(pack(pt));
            assert_eq!((back.x, back.y), (pt.x, pt.y));
        }
    }
}
