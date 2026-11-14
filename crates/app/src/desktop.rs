//! `rexplorer --desktop`: rexplorer's context menu on the desktop. Explorer keeps drawing the desktop (it is the
//! shell); a low-level mouse hook takes right-clicks that land on its icon list and shows our menu instead, built
//! like the file list's: the shell's verbs for the selected icons, or View, Sort by, Refresh and Paste plus the
//! shell's background verbs. A low-level keyboard hook does the same for Shift+F10 and the Menu key on the focused
//! list, at the first selected icon. One helper runs per session; `--register` starts it at sign-in.
//!
//! The press is taken so a plain click can show our menu, which would keep Explorer from starting a right-drag. So
//! once the pointer leaves the drag box the press is replayed to Explorer (tagged, so the hook lets it through), and
//! Explorer runs its own drag and drop menu.
//!
//! The hooks run on their own thread and only classify and post, so they never stall the system's input (Windows
//! drops a hook that is slow to answer); that thread also sends the replay, never the hook callback. Everything
//! else runs on the main thread.

use std::cell::RefCell;
use std::sync::atomic::{AtomicI32, AtomicIsize, AtomicU32, Ordering};

use rexplorer_core::com::ComInit;
use rexplorer_core::desktop::{self, ARRANGE, DesktopView, ICON_SIZES, SORT_KEYS};
use rexplorer_core::folder::ShellFolder;
use rexplorer_core::i18n::tr;
use rexplorer_core::item::ShellItem;
use rexplorer_core::menu::{ContextMenu, Picked};
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::DeleteObject;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{AttachThreadInput, CreateMutexW, GetCurrentThreadId};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput, VIRTUAL_KEY, VK_APPS, VK_CONTROL, VK_E,
    VK_F10, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
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
/// Posted to the window by the keyboard hook: Win+E was pressed.
const WM_DESKTOP_WIN_E: u32 = WM_APP + 0x63;
/// dwExtraInfo of the input we inject: the hooks let it through untouched.
const REPLAY_TAG: usize = 0x5245_5850;

/// The window the hook posts to, and the desktop list a right press started on.
static TARGET: AtomicIsize = AtomicIsize::new(0);
static LIST: AtomicIsize = AtomicIsize::new(0);
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

/// Where a right press the mouse hook took stands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Drag {
    Idle,
    /// A press on the desktop list was taken: its release gets our menu, a drag replays it.
    Pressed,
    /// The replay is sent and its tagged press has not reached the hook yet.
    Replaying,
    /// The user's release reached the hook before the replay's tagged press (it was already queued when the replay
    /// was sent): the replay's input is taken up to its last move, so no press stays down. `pressed`: its press
    /// has gone by.
    Overtaken { pressed: bool },
}

/// What the mouse hook sees, as far as a taken press is concerned; `Replay` is the hook thread about to send it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Event {
    PressOnList,
    PressElsewhere,
    MoveBeyond,
    Release,
    Replay,
    TaggedPress,
    TaggedMove,
    Forget,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Act {
    /// Let the input through (for `Replay`: send nothing).
    Pass,
    Swallow,
    PostReplay,
    /// Swallow the release and show our menu.
    Menu,
    SendReplay,
}

/// The press state machine. A real press can only follow a release, and SendInput queues the replay's three events
/// together, behind any real input already queued; so a release seen while the replay is in flight came first.
fn step(s: Drag, e: Event) -> (Drag, Act) {
    use {Act::*, Drag::*, Event::*};
    match (s, e) {
        (_, Forget) => (Idle, Pass),
        (_, PressOnList) => (Pressed, Swallow),
        (Pressed, PressElsewhere) => (Idle, Pass),
        (Pressed, MoveBeyond) => (Pressed, PostReplay),
        (Pressed, Release) => (Idle, Menu),
        (Pressed, Replay) => (Replaying, SendReplay),
        (Replaying, TaggedPress) => (Idle, Pass),
        (Replaying, Release) => (Overtaken { pressed: false }, Pass),
        (Overtaken { pressed: false }, TaggedMove) => (s, Swallow),
        (Overtaken { pressed: false }, TaggedPress) => (Overtaken { pressed: true }, Swallow),
        (Overtaken { pressed: true }, TaggedMove) => (Idle, Swallow),
        _ => (s, Pass),
    }
}

thread_local! {
    /// The hook thread's press state: the hooks, the desktop-switch event and the replay all run on that thread.
    static DRAG: std::cell::Cell<Drag> = const { std::cell::Cell::new(Drag::Idle) };
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
        rexplorer_core::packaged::warm();
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
    advance(Event::Forget);
    SWALLOWED_KEY.store(0, Ordering::SeqCst);
}

fn advance(e: Event) -> Act {
    DRAG.with(|d| {
        let (s, act) = step(d.get(), e);
        d.set(s);
        act
    })
}

/// Takes a right press and release on the desktop's icon list; a press that turns into a drag is replayed to Explorer
/// from the hook thread. Any other input passes on untouched.
unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let m = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        let tagged = m.dwExtraInfo == REPLAY_TAG;
        let event = match wparam.0 as u32 {
            // The replay presses the physical button, which is the left one with the buttons swapped.
            WM_RBUTTONDOWN | WM_LBUTTONDOWN if tagged => Some(Event::TaggedPress),
            WM_MOUSEMOVE if tagged => Some(Event::TaggedMove),
            _ if tagged => None,
            WM_RBUTTONDOWN => {
                let h = WindowFromPoint(m.pt);
                if desktop::is_desktop_list(h) {
                    LIST.store(h.0 as isize, Ordering::SeqCst);
                    PRESS.store(pack(m.pt), Ordering::SeqCst);
                    Some(Event::PressOnList)
                } else {
                    // A press elsewhere ends one whose release never reached the hook.
                    Some(Event::PressElsewhere)
                }
            }
            WM_MOUSEMOVE
                if DRAG.with(|d| d.get()) == Drag::Pressed
                    && beyond_drag(m.pt, unpack(PRESS.load(Ordering::SeqCst)), (DRAG_X.load(Ordering::SeqCst), DRAG_Y.load(Ordering::SeqCst))) =>
            {
                Some(Event::MoveBeyond)
            }
            WM_RBUTTONUP => Some(Event::Release),
            _ => None,
        };
        match event.map(advance) {
            Some(Act::Swallow) => return LRESULT(1),
            // Moves are never taken. The replay, not the move, ends the press: a release the hook sees before the
            // replay runs then gets our menu. Extra posts do nothing.
            Some(Act::PostReplay) => {
                // SendInput inside the callback would re-enter the hook: the hook thread sends it.
                let _ = PostThreadMessageW(HOOK_THREAD.load(Ordering::SeqCst), WM_REPLAY, WPARAM(0), LPARAM(PRESS.load(Ordering::SeqCst)));
            }
            Some(Act::Menu) => {
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
    // cannot tell: a press the hook took never reaches the async key state. A release the user makes while this runs
    // was queued before the replay and reaches the hook first; `step` then takes the replay back.
    let Some(now) = cursor_pos() else { return };
    if advance(Event::Replay) != Act::SendReplay {
        return;
    }
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
    if SendInput(&[goto(press), mouse(0, 0, down), goto(now)], size_of::<INPUT>() as i32) != 3 {
        // Blocked (by another desktop or a higher integrity window): no tagged press will come to end the replay.
        advance(Event::Forget);
    }
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
            // The shell opens its own window on Win+E whatever the registry says (seen on build 26300), so the key
            // is taken here, in any window.
            WM_KEYDOWN | WM_SYSKEYDOWN if is_win_e(vk as u16, held(VK_LWIN) || held(VK_RWIN), held(VK_SHIFT), held(VK_CONTROL), held(VK_MENU)) => {
                SWALLOWED_KEY.store(vk, Ordering::SeqCst);
                let target = HWND(TARGET.load(Ordering::SeqCst) as *mut _);
                let _ = PostMessageW(Some(target), WM_DESKTOP_WIN_E, WPARAM(0), LPARAM(0));
                return LRESULT(1);
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

/// Win+E with no other modifier (Win+Shift+E and the like stay the system's).
fn is_win_e(vk: u16, win: bool, shift: bool, ctrl: bool, alt: bool) -> bool {
    vk == VK_E.0 && win && !shift && !ctrl && !alt
}

/// Win+E: starts rexplorer the way the File Explorer icon does (no argument: the start folder, or a hand-off to the
/// window already open).
unsafe fn open_file_manager(hwnd: HWND) {
    // The shell never saw the E, and it opens Start when Win goes up with nothing pressed in between: a key no
    // program uses is pressed for it to see.
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0xE8), dwFlags: flags, dwExtraInfo: REPLAY_TAG, ..Default::default() } },
    };
    SendInput(&[key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)], size_of::<INPUT>() as i32);
    // The new process inherits the right to come to the front from the one that has it.
    take_foreground(hwnd);
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).spawn();
    }
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

/// Whether a menu request posted at tick `posted` is too old to answer at tick `now` (more than 2 s; the tick
/// count wraps every 49.7 days, and a time just ahead of `now` is not old).
fn stale(posted: u32, now: u32) -> bool {
    now.wrapping_sub(posted) as i32 > 2000
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
        // Clicks taken while Explorer hung must not come out later as a run of menus.
        WM_DESKTOP_MENU | WM_DESKTOP_KEYMENU | WM_DESKTOP_WIN_E if stale(GetMessageTime() as u32, windows::Win32::System::SystemInformation::GetTickCount()) => LRESULT(0),
        WM_DESKTOP_WIN_E => {
            open_file_manager(hwnd);
            LRESULT(0)
        }
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
    let mut look: Vec<Entry> = ICON_SIZES.iter().enumerate().map(|(i, (t, s))| Entry { checked: *s == size, ..Entry::new(ICON + i, tr(t), 0) }).collect();
    look.push(Entry::sep());
    look.extend(ARRANGE.iter().enumerate().map(|(i, (t, f))| Entry { checked: view.has_flag(*f).unwrap_or(false), ..Entry::new(ARRANGE_ID + i, tr(t), 0) }));
    let sort = SORT_KEYS.iter().enumerate().map(|(i, (t, _))| Entry::new(SORT + i, tr(t), 0)).collect();
    vec![
        Entry { sub: look, ..Entry::new(0, tr("menu.list.view"), 0xE8A9) },
        Entry { sub: sort, ..Entry::new(0, tr("menu.list.sort_by"), 0xE8CB) },
        Entry::sep(),
        Entry::new(REFRESH, tr("menu.list.refresh"), 0xE72C),
        Entry::new(PASTE, tr("menu.list.paste"), 0xE77F).enabled(commands::clipboard_has_files()),
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
    // The key that opened the menu has been released by now; if the hook missed that release (another hook took it),
    // the next such key in any window must not be taken.
    SWALLOWED_KEY.store(0, Ordering::SeqCst);
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
    // The hook checked `list` is the shell's own; it is checked again here, as the window may have changed since. A
    // list that does not answer gives no selection and no menu.
    if !desktop::is_desktop_list(list) {
        return;
    }
    // The timed read first: it is the liveness probe (a hung list fails at once), before `find`'s untimed COM calls.
    let Some(sel) = DesktopView::selection(list) else { return };
    let Ok(view) = DesktopView::find() else { return };
    let (index, pt) = match at {
        At::Point(pt) => {
            let index = DesktopView::index_at(list, pt);
            let _ = view.select_for_menu(index, &sel);
            (index, pt)
        }
        // A key acts on the selection as it is, with nothing under a pointer to select; the menu opens at the focused
        // icon, as in Explorer (the first selected one when the focus is elsewhere).
        At::Keyboard => {
            let index = DesktopView::focused(list).filter(|f| sel.contains(f)).or(sel.first().copied());
            let at_icon = index.and_then(|i| view.item_point(list, i).ok()).map(|p| POINT { x: p.x + 8, y: p.y + 8 });
            (index, at_icon.or_else(|| cursor_pos()).unwrap_or_default())
        }
    };
    let Ok(root) = ShellFolder::desktop() else { return };
    let items: Vec<ShellItem> = match index {
        Some(_) => {
            let Some(sel) = DesktopView::selection(list) else { return };
            sel.into_iter().filter_map(|i| view.item(i).ok()).map(|pidl| ShellItem { parent: root.folder.clone(), pidl }).collect()
        }
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
        // In Explorer, so its Ctrl+Z on the desktop brings the icons back; here only when it cannot.
        Some(Ok(Picked::Delete)) => {
            let paths: Vec<String> = items.iter().map(ShellItem::parsing_name).collect();
            view.delete_selection(&paths).or_else(|_| menu.map_or(Ok(()), |m| m.invoke_verb(hwnd, "delete", false)))
        }
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
    fn a_request_older_than_two_seconds_is_dropped() {
        assert!(!stale(1000, 1000));
        assert!(!stale(1000, 3000));
        assert!(stale(1000, 3001));
        // Across the tick count's wrap.
        assert!(!stale(u32::MAX - 500, 1000));
        assert!(stale(u32::MAX - 500, 1600));
        // A message time a tick ahead of the clock read after it.
        assert!(!stale(1016, 1000));
    }

    #[test]
    fn menu_keys() {
        assert!(is_win_e(VK_E.0, true, false, false, false));
        for (vk, win, shift, ctrl, alt) in [(VK_E.0, false, false, false, false), (VK_E.0, true, true, false, false), (VK_E.0, true, false, true, false), (VK_E.0, true, false, false, true), (VK_F10.0, true, false, false, false)] {
            assert!(!is_win_e(vk, win, shift, ctrl, alt), "{vk} {win} {shift} {ctrl} {alt}");
        }
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

    fn run(events: &[Event]) -> (Drag, Vec<Act>) {
        let mut s = Drag::Idle;
        let acts = events
            .iter()
            .map(|&e| {
                let (next, act) = step(s, e);
                s = next;
                act
            })
            .collect();
        (s, acts)
    }

    #[test]
    fn a_click_gets_our_menu_and_a_drag_is_replayed() {
        use {Act::*, Event::*};
        assert_eq!(run(&[PressOnList, Release]), (Drag::Idle, vec![Swallow, Menu]));
        // Replayed once however many moves posted it; the replay's press and moves pass, and so does the release.
        let drag = [PressOnList, MoveBeyond, MoveBeyond, Replay, Replay, TaggedMove, TaggedPress, TaggedMove, Release];
        assert_eq!(run(&drag), (Drag::Idle, vec![Swallow, PostReplay, PostReplay, SendReplay, Pass, Pass, Pass, Pass, Pass]));
        // Released before the posted replay ran: the release got the menu, the replay sends nothing.
        assert_eq!(run(&[PressOnList, MoveBeyond, Release, Replay]), (Drag::Idle, vec![Swallow, PostReplay, Menu, Pass]));
        assert_eq!(run(&[PressOnList, PressElsewhere, Release]).0, Drag::Idle);
        assert_eq!(run(&[PressOnList, Forget, Replay]), (Drag::Idle, vec![Swallow, Pass, Pass]));
    }

    #[test]
    fn a_release_that_overtakes_the_replay_takes_the_replay_back() {
        use {Act::*, Event::*};
        // The user's release was queued while the hook thread was sending: it reaches the hook first, so the tagged
        // press and the moves around it are taken and no button is left down.
        let events = [PressOnList, MoveBeyond, Replay, Release, TaggedMove, TaggedPress, TaggedMove];
        assert_eq!(run(&events), (Drag::Idle, vec![Swallow, PostReplay, SendReplay, Pass, Swallow, Swallow, Swallow]));
        // After that, other tagged input and the next click are back to normal.
        let (s, _) = run(&events);
        assert_eq!(step(s, TaggedMove), (Drag::Idle, Pass));
        assert_eq!(step(s, PressOnList), (Drag::Pressed, Swallow));
    }

    #[test]
    fn drag_box() {
        let (press, drag) = (POINT { x: 100, y: 100 }, (4, 4));
        assert!(!beyond_drag(POINT { x: 104, y: 96 }, press, drag));
        assert!(beyond_drag(POINT { x: 105, y: 100 }, press, drag));
        assert!(beyond_drag(POINT { x: 100, y: 95 }, press, drag));
    }
}
