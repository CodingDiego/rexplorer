//! `rexplorer --desktop`: rexplorer's context menu on the desktop. Explorer keeps drawing the desktop (it is the
//! shell); a low-level mouse hook takes right-clicks that land on its icon list and shows our menu instead, built
//! like the file list's: the shell's verbs for the selected icons, or View, Sort by, Refresh and Paste plus the
//! shell's background verbs. A low-level keyboard hook does the same for Shift+F10 and the Menu key on the focused
//! list, at the first selected icon. One helper runs per session; install starts it at sign-in.
//!
//! The press is taken so a plain click can show our menu, which would keep Explorer from starting a right-drag. So
//! once the pointer leaves the drag box the press is replayed to Explorer (tagged, so the hook lets it through), and
//! Explorer runs its own drag and drop menu.
//!
//! The hooks run on their own thread and only classify and post, so they never stall the system's input (Windows
//! drops a hook that is slow to answer); that thread also sends the replay, never the hook callback. Everything
//! else runs on the main thread.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU32, Ordering};

use rexplorer_core::com::ComInit;
use rexplorer_core::desktop::{self, ARRANGE, DesktopView, ICON_SIZES, SORT_KEYS};
use rexplorer_core::folder::ShellFolder;
use rexplorer_core::item::ShellItem;
use rexplorer_core::menu::{ContextMenu, Picked};
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::DeleteObject;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{AttachThreadInput, CreateMutexW, GetCurrentThreadId};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_MOUSE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput, VIRTUAL_KEY, VK_APPS, VK_CONTROL, VK_F10, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, Result, w};

use crate::commands;
use crate::ui::{self, Entry};

/// The helper's hidden window; uninstall closes it by this class.
pub(crate) const CLASS: PCWSTR = w!("RexDesktop");
const WM_DESKTOP_MENU: u32 = WM_APP + 0x60;
/// Posted to the hook thread: replay the right press (LPARAM = its packed point).
const WM_REPLAY: u32 = WM_APP + 0x61;
/// Posted to the window by the keyboard hook: WPARAM = the focused desktop list.
const WM_DESKTOP_KEYMENU: u32 = WM_APP + 0x62;
/// dwExtraInfo of the input we inject: the hooks let it through untouched.
const REPLAY_TAG: usize = 0x5245_5850;

/// The window the hook posts to, and the desktop list a right press started on.
static TARGET: AtomicIsize = AtomicIsize::new(0);
static LIST: AtomicIsize = AtomicIsize::new(0);
static PRESSED: AtomicBool = AtomicBool::new(false);
/// Where the taken press landed (packed), the drag box around it, and the hook thread that replays it.
static PRESS: AtomicIsize = AtomicIsize::new(0);
static DRAG_X: AtomicI32 = AtomicI32::new(0);
static DRAG_Y: AtomicI32 = AtomicI32::new(0);
static HOOK_THREAD: AtomicU32 = AtomicU32::new(0);
/// The menu key whose key-down the keyboard hook took, so its repeats and key-up are taken too.
static SWALLOWED_KEY: AtomicU32 = AtomicU32::new(0);

/// Where a menu opens: at a click, or by the keyboard at the selected icon.
#[derive(Clone, Copy)]
enum At {
    Point(POINT),
    Keyboard,
}

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
        HOOK_THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
        DRAG_X.store(GetSystemMetrics(SM_CXDRAG), Ordering::SeqCst);
        DRAG_Y.store(GetSystemMetrics(SM_CYDRAG), Ordering::SeqCst);
        let Ok(mouse) = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), hinst, 0) else { return };
        // Without the keyboard hook the mouse menu still works.
        let keys = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), hinst, 0).ok();
        let switch = SetWinEventHook(EVENT_SYSTEM_DESKTOPSWITCH, EVENT_SYSTEM_DESKTOPSWITCH, None, Some(desktop_switched), 0, 0, WINEVENT_OUTOFCONTEXT);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_REPLAY {
                replay_right_press(unpack(msg.lParam.0));
            }
        }
        let _ = UnhookWinEvent(switch);
        let _ = UnhookWindowsHookEx(mouse);
        if let Some(keys) = keys {
            let _ = UnhookWindowsHookEx(keys);
        }
    }
}

/// A desktop switch (Win+L, Ctrl+Alt+Del, a UAC prompt) hides from the hooks the release of a press or key they took:
/// forget both, so the next move does not replay a press nobody holds and the next such key is not taken.
unsafe extern "system" fn desktop_switched(_: HWINEVENTHOOK, _: u32, _: HWND, _: i32, _: i32, _: u32, _: u32) {
    PRESSED.store(false, Ordering::SeqCst);
    SWALLOWED_KEY.store(0, Ordering::SeqCst);
}

/// Takes a right press and release on the desktop's icon list; a press that turns into a drag is replayed to Explorer
/// from the hook thread. Any other input passes on untouched.
unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let m = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        if m.dwExtraInfo == REPLAY_TAG {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        match wparam.0 as u32 {
            WM_RBUTTONDOWN => {
                let h = WindowFromPoint(m.pt);
                if desktop::is_desktop_list(h) {
                    LIST.store(h.0 as isize, Ordering::SeqCst);
                    PRESS.store(pack(m.pt), Ordering::SeqCst);
                    PRESSED.store(true, Ordering::SeqCst);
                    return LRESULT(1);
                }
                // A press elsewhere ends one whose release never reached the hook.
                PRESSED.store(false, Ordering::SeqCst);
            }
            // Moves are never taken. The replay, not the move, clears PRESSED: a release the hook sees before the
            // replay runs then gets our menu instead of leaving a replayed press down. Extra posts do nothing.
            WM_MOUSEMOVE
                if PRESSED.load(Ordering::SeqCst)
                    && beyond_drag(m.pt, unpack(PRESS.load(Ordering::SeqCst)), (DRAG_X.load(Ordering::SeqCst), DRAG_Y.load(Ordering::SeqCst))) =>
            {
                // SendInput inside the callback would re-enter the hook: the hook thread sends it.
                let _ = PostThreadMessageW(HOOK_THREAD.load(Ordering::SeqCst), WM_REPLAY, WPARAM(0), LPARAM(PRESS.load(Ordering::SeqCst)));
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

/// Whether `pt` left the drag box (`drag` = half-extents) around the press point.
fn beyond_drag(pt: POINT, press: POINT, drag: (i32, i32)) -> bool {
    (pt.x - press.x).abs() > drag.0 || (pt.y - press.y).abs() > drag.1
}

/// Gives Explorer the press we took, so it runs its own right-drag: press where it was, then back to the pointer.
/// One call, so no other input interleaves.
unsafe fn replay_right_press(press: POINT) {
    // Released (or forgotten) meanwhile: its release got our menu, and a press now would stay down. GetAsyncKeyState
    // cannot tell: a press the hook took never reaches the async key state. Hook callbacks run on this thread only
    // between messages, so no release slips in between this check and SendInput.
    if !PRESSED.swap(false, Ordering::SeqCst) {
        return;
    }
    let Some(now) = cursor_pos() else { return };
    // SendInput presses physical buttons; with the buttons swapped the logical right one is the physical left.
    let down = if GetSystemMetrics(SM_SWAPBUTTON) != 0 { MOUSEEVENTF_LEFTDOWN } else { MOUSEEVENTF_RIGHTDOWN };
    let origin = POINT { x: GetSystemMetrics(SM_XVIRTUALSCREEN), y: GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let size = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
    let mouse = |dx: i32, dy: i32, flags: MOUSE_EVENT_FLAGS| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 0, dwFlags: flags, time: 0, dwExtraInfo: REPLAY_TAG } },
    };
    let goto = |pt| {
        let (x, y) = to_absolute(pt, origin, size);
        mouse(x, y, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK)
    };
    SendInput(&[goto(press), mouse(0, 0, down), goto(now)], size_of::<INPUT>() as i32);
}

/// A virtual-screen pixel as SendInput's absolute coordinate (0..=65535 across the virtual desktop). Windows maps it
/// back as `n * size / 65536` rounded down, so round up here, or the press can land a pixel short.
fn to_absolute(pt: POINT, origin: POINT, size: (i32, i32)) -> (i32, i32) {
    let axis = |v: i32, o: i32, s: i32| {
        let s = s.max(1) as i64;
        (((v - o) as i64 * 65536 + s - 1) / s) as i32
    };
    (axis(pt.x, origin.x, size.0), axis(pt.y, origin.y, size.1))
}

unsafe fn cursor_pos() -> Option<POINT> {
    let mut pt = POINT::default();
    GetCursorPos(&mut pt).ok().map(|_| pt)
}

/// Takes Shift+F10 and the Menu key while the desktop list has the focus (and their repeats and releases), and has
/// our window answer them; any other key passes on untouched.
unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let vk = (*(lparam.0 as *const KBDLLHOOKSTRUCT)).vkCode;
        let held = |k: VIRTUAL_KEY| GetAsyncKeyState(k.0 as i32) < 0;
        // 0 means none taken; some injected input carries vkCode 0 and must not match it.
        let taken = vk != 0 && vk == SWALLOWED_KEY.load(Ordering::SeqCst);
        match wparam.0 as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN if taken => return LRESULT(1),
            WM_KEYDOWN | WM_SYSKEYDOWN if is_menu_key(vk as u16, held(VK_SHIFT), held(VK_CONTROL), held(VK_MENU)) => {
                if let Some(list) = desktop_focus() {
                    SWALLOWED_KEY.store(vk, Ordering::SeqCst);
                    let target = HWND(TARGET.load(Ordering::SeqCst) as *mut _);
                    let _ = PostMessageW(Some(target), WM_DESKTOP_KEYMENU, WPARAM(list.0 as usize), LPARAM(0));
                    return LRESULT(1);
                }
            }
            WM_KEYUP | WM_SYSKEYUP if taken => {
                SWALLOWED_KEY.store(0, Ordering::SeqCst);
                return LRESULT(1);
            }
            _ => {}
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

/// The keys that open a context menu: Menu, or Shift+F10 (no Ctrl or Alt).
fn is_menu_key(vk: u16, shift: bool, ctrl: bool, alt: bool) -> bool {
    !ctrl && !alt && (vk == VK_APPS.0 || (vk == VK_F10.0 && shift))
}

/// The desktop list if it has the keyboard focus; while an icon is renamed the focus is the edit control, so None.
unsafe fn desktop_focus() -> Option<HWND> {
    let tid = GetWindowThreadProcessId(GetForegroundWindow(), None);
    let mut info = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
    GetGUIThreadInfo(tid, &mut info).ok()?;
    desktop::is_desktop_list(info.hwndFocus).then_some(info.hwndFocus)
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
            show(hwnd, HWND(wparam.0 as *mut _), At::Point(unpack(lparam.0)));
            LRESULT(0)
        }
        WM_DESKTOP_KEYMENU => {
            show(hwnd, HWND(wparam.0 as *mut _), At::Keyboard);
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

unsafe fn show(hwnd: HWND, list: HWND, at: At) {
    // A right-click while a menu is up: close it and answer this click once it is gone, as Explorer does (nesting
    // the second menu inside the first would re-enter ACTIVE). A click while the shell is still building one is
    // dropped.
    if BUSY.with(|b| b.replace(true)) {
        if ACTIVE.with(|a| a.try_borrow().map_or(true, |m| m.is_some())) {
            let _ = EndMenu();
            let (msg, lp) = match at {
                At::Point(pt) => (WM_DESKTOP_MENU, pack(pt)),
                At::Keyboard => (WM_DESKTOP_KEYMENU, 0),
            };
            let _ = PostMessageW(Some(hwnd), msg, WPARAM(list.0 as usize), LPARAM(lp));
        }
        return;
    }
    menu_for(hwnd, list, at);
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

unsafe fn menu_for(hwnd: HWND, list: HWND, at: At) {
    // Explorer restarts replace the view: look it up for every menu.
    let Ok(view) = DesktopView::find() else { return };
    let (index, pt) = match at {
        At::Point(pt) => {
            let index = DesktopView::index_at(list, pt);
            let _ = view.select_for_menu(list, index);
            (index, pt)
        }
        // A key acts on the selection as it is, with nothing under a pointer to select; the menu opens at the focused
        // icon, as in Explorer (the first selected one when the focus is elsewhere).
        At::Keyboard => {
            let sel = DesktopView::selection(list);
            let index = DesktopView::focused(list).filter(|f| sel.contains(f)).or(sel.first().copied());
            let at_icon = index.and_then(|i| view.item_point(list, i).ok()).map(|p| POINT { x: p.x + 8, y: p.y + 8 });
            (index, at_icon.or_else(|| cursor_pos()).unwrap_or_default())
        }
    };
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
    // The desktop gets the keyboard, as after Explorer's own menu (the swallowed click never activated it): after a
    // cancel, or a command the view carries out (Rename's edit box needs the keys), or a shell verb that only touches
    // the clipboard or creates a file in place (Copy, Cut, Create shortcut: Ctrl+V and the arrows must work next). Not
    // after Paste or any other shell verb, which may open a window of their own. Only while the helper still has the
    // foreground: after Alt+Tab or a click on another window the user chose where to be.
    let to_desktop = matches!(picked, None | Some(Err(_)) | Some(Ok(Picked::Cancelled | Picked::Rename)))
        || matches!(picked, Some(Ok(Picked::Own(id))) if id != PASTE)
        || matches!(&picked, Some(Ok(Picked::Invoked(v))) if keeps_desktop_keyboard(v));
    if to_desktop && GetForegroundWindow() == hwnd {
        let _ = SetForegroundWindow(GetAncestor(list, GA_ROOT));
    }
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

/// The shell verbs after which the desktop gets the keyboard back: they only touch the clipboard or create a file in
/// place and open no window ("link" is Create shortcut). Delete shows its own confirmation and progress, so it stays out.
fn keeps_desktop_keyboard(verb: &str) -> bool {
    ["copy", "cut", "link"].iter().any(|v| verb.eq_ignore_ascii_case(v))
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

    #[test]
    fn menu_keys() {
        assert!(is_menu_key(VK_APPS.0, false, false, false));
        assert!(is_menu_key(VK_F10.0, true, false, false));
        assert!(!is_menu_key(VK_F10.0, false, false, false));
        assert!(!is_menu_key(VK_F10.0, true, true, false));
        assert!(!is_menu_key(VK_APPS.0, false, false, true));
    }

    #[test]
    fn virtual_screen_to_absolute() {
        // Every pixel comes back exact through Windows' mapping (n * size / 65536, rounded down) and stays in range.
        for (origin, size) in [(POINT { x: -1920, y: -400 }, (4480, 1440)), (POINT { x: 0, y: 0 }, (1920, 1080)), (POINT { x: 0, y: 0 }, (3, 3))] {
            for i in 0..size.0.max(size.1) {
                let pt = POINT { x: origin.x + i.min(size.0 - 1), y: origin.y + i.min(size.1 - 1) };
                let (x, y) = to_absolute(pt, origin, size);
                assert!((0..=65535).contains(&x) && (0..=65535).contains(&y));
                let back = |n: i32, o: i32, s: i32| (n as i64 * s as i64 / 65536) as i32 + o;
                assert_eq!((back(x, origin.x, size.0), back(y, origin.y, size.1)), (pt.x, pt.y));
            }
        }
    }

    #[test]
    fn verbs_that_return_the_keyboard() {
        for v in ["copy", "COPY", "cut", "Cut", "link"] {
            assert!(keeps_desktop_keyboard(v), "{v}");
        }
        for v in ["delete", "open", "properties", "paste", ""] {
            assert!(!keeps_desktop_keyboard(v), "{v}");
        }
    }

    #[test]
    fn drag_box() {
        let (press, drag) = (POINT { x: 100, y: 100 }, (4, 4));
        assert!(!beyond_drag(POINT { x: 104, y: 96 }, press, drag));
        assert!(beyond_drag(POINT { x: 105, y: 100 }, press, drag));
        assert!(beyond_drag(POINT { x: 100, y: 95 }, press, drag));
    }
}
