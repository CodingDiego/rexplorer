//! The window: tabs, a nav row (buttons + breadcrumb address bar) over a virtual ListView, and a status bar.
//! All shell logic lives in rexplorer-core; this crate is plumbing and drawing.
#![cfg_attr(not(test), windows_subsystem = "windows")]
#![allow(unsafe_op_in_unsafe_fn)]

mod archive_view;
#[allow(dead_code)] // parts of the API are for later steps
mod breadcrumb;
mod connect;
#[allow(dead_code)] // parts of the API are for later steps
mod instance;
mod list;
mod nav;
mod palette;
mod preview_pane;
mod session;
mod statusbar;
#[allow(dead_code)] // parts of the API are for later steps
mod tabs;
mod theme;
mod thumbs;
mod toolbar;
mod ui;

use std::ffi::c_void;
use std::mem::size_of;
use std::path::PathBuf;
use std::time::Duration;

use rexplorer_core::cli;
use rexplorer_core::com::{ComInit, Pidl};
use rexplorer_core::crash;
use rexplorer_core::fallback::open_in_explorer;
use rexplorer_core::fuzzy::{Candidate, Recent};
use rexplorer_core::menu::{ContextMenu, Picked};
use rexplorer_core::notify::ChangeWatch;
use rexplorer_core::register;
use rexplorer_core::settings::{self, Profile, Settings, Theme};
use rexplorer_core::{Column, ShellFolder, ShellItem};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{DeleteObject, HDC, HFONT, InvalidateRect};
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{
    DefSubclassProc, SHFILEINFOW, SHGFI_PIDL, SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGetFileInfoW, SetWindowSubclass,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Error, HSTRING, PCWSTR, PWSTR, Result, w};

use nav::{Tab, cur, go, listing};

const WM_ENUM_DONE: u32 = WM_APP + 1;
const WM_SHELL_CHANGE: u32 = WM_APP + 2;
const TIMER_REFRESH: usize = 1;
const TIMER_STATUS: usize = 2;
const TIMER_SESSION: usize = 3;
const ICON_UNKNOWN: i32 = i32::MIN;

const ID_ADDR: usize = 100;
const ID_LIST: usize = 101;
const ID_TABS: usize = 102;
const ID_NAV: usize = 103;
const ID_TOOLS: usize = 104;
const ID_CRUMBS: usize = 105;
const ID_STATUS: usize = 106;
const ID_PALETTE: usize = 107;

const CMD_BACK: u16 = 1;
const CMD_FORWARD: u16 = 2;
const CMD_UP: u16 = 3;
const CMD_REFRESH: u16 = 4;
const CMD_ADDRESS: u16 = 5;
const CMD_EXPLORER: u16 = 6;
const CMD_ENTER: u16 = 7;
const CMD_ESCAPE: u16 = 8;
const CMD_PROPERTIES: u16 = 9;
const CMD_NEW_TAB: u16 = 10;
const CMD_CLOSE_TAB: u16 = 11;
const CMD_NEXT_TAB: u16 = 12;
const CMD_PREV_TAB: u16 = 13;
const CMD_PALETTE: u16 = 14;
const CMD_TERMINAL: u16 = 15;
const CMD_VIEW: u16 = 16;
const CMD_CONNECT: u16 = 17;
const CMD_MORE: u16 = 18;
const CMD_REGISTER: u16 = 19;
const CMD_UNREGISTER: u16 = 20;
const CMD_SETTINGS: u16 = 21;
const CMD_ABOUT: u16 = 22;
/// Ctrl+1..Ctrl+9 send CMD_TAB1..CMD_TAB1 + 8.
const CMD_TAB1: u16 = 31;

const fn accel(flags: u8, key: VIRTUAL_KEY, cmd: u16) -> ACCEL {
    ACCEL { fVirt: ACCEL_VIRT_FLAGS(FVIRTKEY.0 | flags), key: key.0, cmd }
}

const ACCELS: [ACCEL; 17] = [
    accel(FALT.0, VK_LEFT, CMD_BACK),
    accel(FALT.0, VK_RIGHT, CMD_FORWARD),
    accel(FALT.0, VK_UP, CMD_UP),
    accel(0, VK_F5, CMD_REFRESH),
    accel(FCONTROL.0, VK_L, CMD_ADDRESS),
    accel(FALT.0, VK_D, CMD_ADDRESS),
    accel(FCONTROL.0 | FSHIFT.0, VK_E, CMD_EXPLORER),
    accel(0, VK_RETURN, CMD_ENTER),
    accel(0, VK_ESCAPE, CMD_ESCAPE),
    accel(FALT.0, VK_RETURN, CMD_PROPERTIES),
    accel(FCONTROL.0, VK_R, CMD_REFRESH),
    accel(FCONTROL.0, VK_T, CMD_NEW_TAB),
    accel(FCONTROL.0, VK_W, CMD_CLOSE_TAB),
    accel(FCONTROL.0, VK_TAB, CMD_NEXT_TAB),
    accel(FCONTROL.0 | FSHIFT.0, VK_TAB, CMD_PREV_TAB),
    accel(FCONTROL.0, VK_P, CMD_PALETTE),
    accel(FCONTROL.0, VK_OEM_3, CMD_TERMINAL),
];

pub(crate) struct App {
    hwnd: HWND,
    list: HWND,
    addr: HWND,
    strip: HWND,
    nav: HWND,
    tools: HWND,
    crumbs: HWND,
    status: HWND,
    palette: HWND,
    /// Owned copy of the current folder's icon shown in the breadcrumb.
    crumb_icon: HICON,
    font: HFONT,
    theme: Theme,
    brushes: theme::Brushes,
    profile: Profile,
    tabs: Vec<Tab>,
    active: usize,
    generation: u64,
    active_menu: Option<ContextMenu>,
    image_list_set: bool,
    session: session::Session,
    debounce: session::Debounce,
    recent: Recent,
}

fn main() -> Result<()> {
    let args = match cli::parse(std::env::args().skip(1)) {
        Ok(a) if a.help => return Ok(info_box(&cli::usage(), MB_ICONINFORMATION)),
        Ok(a) => a,
        Err(e) => return Ok(info_box(&format!("{e}\n\n{}", cli::usage()), MB_ICONERROR)),
    };
    if args.register || args.unregister {
        unsafe { ui::register_flow(None, args.unregister) };
        return Ok(());
    }
    // A recovery launch must not hand off to the instance that is crashing.
    let _instance = instance::claim();
    if _instance.is_none() && !args.recover {
        if let Some(h) = instance::find_main_window() {
            instance::send(h, &cli::payload(&args));
            return Ok(());
        }
    }
    crash::install(&["--recover".to_string()], 10);

    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _com = ComInit::ole();
        let icc = INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_LISTVIEW_CLASSES };
        let _ = InitCommonControlsEx(&icc);

        let dir = settings::dir();
        Settings::ensure_files(&dir);
        let mut settings = Settings::load();
        if let Some(p) = &args.profile {
            settings.active = p.clone();
        }
        let profile = settings.active_profile().clone();
        let theme = settings.theme(&profile.theme).clone();
        let brushes = theme::Brushes::new(&theme);
        ui::probe_cloud_roots();

        let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = w!("RexplorerMain");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: brushes.background,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);

        let app = Box::into_raw(Box::new(App {
            hwnd: HWND::default(),
            list: HWND::default(),
            addr: HWND::default(),
            strip: HWND::default(),
            nav: HWND::default(),
            tools: HWND::default(),
            crumbs: HWND::default(),
            status: HWND::default(),
            palette: HWND::default(),
            crumb_icon: HICON::default(),
            font: HFONT::default(),
            theme,
            brushes,
            profile,
            tabs: Vec::new(),
            active: 0,
            generation: 0,
            active_menu: None,
            image_list_set: false,
            session: session::load(),
            debounce: session::Debounce::new(Duration::from_secs(2)),
            recent: Recent::load(dir.join("recent.txt")),
        }));
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("rexplorer"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1000,
            700,
            None,
            None,
            Some(hinst),
            Some(app as *const c_void),
        )?;
        if !restore_placement(hwnd) {
            let _ = ShowWindow(hwnd, SW_SHOWDEFAULT);
        }

        open_startup_tabs(app, &args);

        let accel_table = CreateAcceleratorTableW(&accels())?;
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            // Accelerators only when our own controls have focus; the ListView's label editor and the palette keep their keys.
            let focus = GetFocus();
            let ours = focus == (*app).list || focus == (*app).addr || focus == hwnd;
            if ours && TranslateAcceleratorW(hwnd, accel_table, &msg) != 0 {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

fn accels() -> Vec<ACCEL> {
    let digits = [VK_1, VK_2, VK_3, VK_4, VK_5, VK_6, VK_7, VK_8, VK_9];
    ACCELS.iter().copied().chain(digits.iter().enumerate().map(|(i, &k)| accel(FCONTROL.0, k, CMD_TAB1 + i as u16))).collect()
}

/// The session's tabs on a plain launch or --recover; otherwise the command-line folders; else the profile's start.
unsafe fn open_startup_tabs(app: *mut App, args: &cli::Args) {
    let plain = args.paths.is_empty() && args.select.is_none();
    let saved = std::mem::take(&mut (*app).session.tabs);
    if (args.recover || plain) && !saved.is_empty() {
        let active = (*app).session.active;
        for name in saved {
            nav::new_tab(app, nav::Target::Name(name), false, None);
        }
        nav::switch_tab(app, active.min((*app).tabs.len() - 1));
    } else if !nav::open_args(app, args) {
        nav::new_tab(app, nav::Target::Name((*app).profile.start.clone()), true, None);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_GETMINMAXINFO {
        let mmi = &mut *(lparam.0 as *mut MINMAXINFO);
        let dpi = GetDpiForWindow(hwnd) as i32;
        mmi.ptMinTrackSize = POINT { x: 640 * dpi / 96, y: 400 * dpi / 96 };
        return LRESULT(0);
    }
    if msg == WM_CREATE {
        let cs = &*(lparam.0 as *const CREATESTRUCTW);
        let app = cs.lpCreateParams as *mut App;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as isize);
        (*app).hwnd = hwnd;
        if let Err(e) = ui::create_children(app) {
            message(hwnd, &format!("Cannot create controls: {e}"));
            return LRESULT(-1);
        }
        return LRESULT(0);
    }
    let app = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if app.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    match msg {
        WM_SIZE => ui::layout(app),
        WM_SETFOCUS => {
            let _ = SetFocus(Some((*app).list));
        }
        WM_NOTIFY => return on_notify(app, lparam),
        WM_COMMAND => {
            let (id, code) = (wparam.0 & 0xFFFF, (wparam.0 >> 16) & 0xFFFF);
            if id == ID_ADDR {
                if code == EN_KILLFOCUS as usize {
                    ui::hide_edit(app);
                }
            } else {
                on_command(app, id as u16);
            }
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
            return LRESULT(theme::ctl_color(HDC(wparam.0 as *mut c_void), &(*app).theme, &(*app).brushes).0 as isize);
        }
        WM_COPYDATA => {
            if let Some(text) = instance::receive(lparam) {
                nav::open_args(app, &cli::parse_payload(&text));
                if IsIconic(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                let _ = SetForegroundWindow(hwnd);
                return LRESULT(1);
            }
            return LRESULT(0);
        }
        WM_APPCOMMAND => {
            // APPCOMMAND_BROWSER_BACKWARD = 1, APPCOMMAND_BROWSER_FORWARD = 2 (mouse side buttons).
            match (lparam.0 as u32 >> 16) & !0xF000 {
                1 => nav::go_back(app),
                2 => nav::go_forward(app),
                _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
            }
            return LRESULT(1);
        }
        WM_ENUM_DONE => nav::on_enum_done(app, Box::from_raw(lparam.0 as *mut nav::EnumDone)),
        WM_SHELL_CHANGE => {
            ChangeWatch::acknowledge(wparam, lparam);
            SetTimer(Some(hwnd), TIMER_REFRESH, 150, None);
        }
        WM_TIMER => {
            let _ = KillTimer(Some(hwnd), wparam.0);
            match wparam.0 {
                TIMER_REFRESH => nav::refresh(app),
                TIMER_STATUS => ui::update_status(app),
                TIMER_SESSION => nav::save_session(app),
                _ => {}
            }
        }
        WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR => {
            if let Some(menu) = &(*app).active_menu {
                if let Some(result) = menu.handle_message(msg, wparam, lparam) {
                    return result;
                }
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_DPICHANGED => {
            let r = &*(lparam.0 as *const RECT);
            let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            ui::apply_font(app);
            if let Some(l) = listing(app) {
                nav::set_columns(app, &l.columns);
            }
            ui::layout(app);
        }
        WM_DESTROY => {
            save_placement(hwnd);
            nav::save_session(app);
            for t in &mut (*app).tabs {
                t.watch = None;
            }
            PostQuitMessage(0);
        }
        _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
    }
    LRESULT(0)
}

unsafe fn on_notify(app: *mut App, lparam: LPARAM) -> LRESULT {
    let hdr = &*(lparam.0 as *const NMHDR);
    let from = hdr.hwndFrom;
    if from == (*app).list {
        return list::on_notify(app, lparam);
    }
    if from == (*app).strip {
        let nm = &*(lparam.0 as *const tabs::NmTab);
        match hdr.code {
            tabs::TN_SELECT => nav::switch_tab(app, nm.index),
            tabs::TN_CLOSE => nav::close_tab(app, nm.index),
            tabs::TN_NEW => nav::duplicate_tab(app),
            _ => {}
        }
    } else if from == (*app).crumbs {
        let nm = &*(lparam.0 as *const breadcrumb::NmCrumb);
        match hdr.code {
            breadcrumb::CN_CRUMB => {
                let tab = cur(app);
                if nm.index + 1 >= tab.crumbs.len() {
                    nav::refresh(app);
                } else if let Some(c) = tab.crumbs.get(nm.index) {
                    go(app, nav::Target::Pidl(c.pidl.clone()), nav::History::Push);
                }
            }
            breadcrumb::CN_EDIT => ui::show_edit(app),
            breadcrumb::CN_DROPDOWN => ui::crumb_dropdown(app, nm.index),
            _ => {}
        }
    } else if hdr.idFrom == ID_PALETTE && (hdr.code == palette::PN_PICK || hdr.code == palette::PN_CLOSE) {
        ui::on_palette(app, hdr);
    }
    LRESULT(0)
}

// ---- commands ----------------------------------------------------------------------------------

unsafe fn on_command(app: *mut App, cmd: u16) {
    let addr = (*app).addr;
    let list = (*app).list;
    match cmd {
        CMD_BACK => nav::go_back(app),
        CMD_FORWARD => nav::go_forward(app),
        CMD_UP => nav::go_up(app),
        CMD_REFRESH => nav::refresh(app),
        CMD_ADDRESS => ui::show_edit(app),
        CMD_EXPLORER => {
            if let Some(l) = listing(app) {
                let _ = open_in_explorer(&l.folder.parsing_name);
            }
        }
        CMD_ENTER => {
            if GetFocus() == addr {
                let text = window_text(addr);
                let current = listing(app).map(|l| l.folder.editing_name.clone()).unwrap_or_default();
                if text.trim().is_empty() || text == current {
                    nav::refresh(app);
                } else {
                    go(app, nav::Target::Name(text.trim().to_string()), nav::History::Push);
                }
                let _ = SetFocus(Some(list));
            } else {
                let i = SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_FOCUSED as isize))).0;
                if i >= 0 {
                    nav::activate(app, i as usize);
                }
            }
        }
        CMD_ESCAPE => {
            if GetFocus() == addr {
                // Losing focus hides the Edit and restores its text.
                let _ = SetFocus(Some(list));
            }
        }
        CMD_PROPERTIES => {
            if list::selected_indices(list).is_empty() {
                list::verb_on_background(app, "properties");
            } else {
                list::verb_on_selection(app, "properties", false);
            }
        }
        CMD_NEW_TAB => nav::duplicate_tab(app),
        CMD_CLOSE_TAB => nav::close_tab(app, (*app).active),
        CMD_NEXT_TAB => nav::cycle_tab(app, 1),
        CMD_PREV_TAB => nav::cycle_tab(app, -1),
        CMD_PALETTE => ui::open_palette(app),
        CMD_TERMINAL => ui::open_terminal(app),
        // Real views arrive in step 2, the connect dialog in step 3.
        CMD_VIEW | CMD_CONNECT => {}
        CMD_MORE => ui::more_menu(app),
        CMD_REGISTER | CMD_UNREGISTER => ui::register_flow(Some((*app).hwnd), cmd == CMD_UNREGISTER),
        CMD_SETTINGS => nav::new_tab(app, nav::Target::Name(settings::dir().display().to_string()), true, None),
        CMD_ABOUT => ui::about(app),
        c if (CMD_TAB1..CMD_TAB1 + 9).contains(&c) => {
            // Ctrl+9 is the last tab, as in browsers.
            let n = (*app).tabs.len();
            let i = if c == CMD_TAB1 + 8 { n - 1 } else { (c - CMD_TAB1) as usize };
            nav::switch_tab(app, i);
        }
        _ => {}
    }
}

// ---- small helpers -----------------------------------------------------------------------------

unsafe fn set_text(hwnd: HWND, text: &str) {
    let h = HSTRING::from(text);
    let _ = SetWindowTextW(hwnd, PCWSTR(h.as_ptr()));
}

unsafe fn window_text(hwnd: HWND) -> String {
    let len = GetWindowTextLengthW(hwnd);
    let mut buf = vec![0u16; len as usize + 1];
    let n = GetWindowTextW(hwnd, &mut buf);
    String::from_utf16_lossy(&buf[..n as usize])
}

unsafe fn message(hwnd: HWND, text: &str) {
    let h = HSTRING::from(text);
    MessageBoxW(Some(hwnd), PCWSTR(h.as_ptr()), w!("rexplorer"), MB_OK | MB_ICONERROR);
}

/// A message box before any window exists (--help, bad arguments).
fn info_box(text: &str, icon: MESSAGEBOX_STYLE) {
    let h = HSTRING::from(text);
    unsafe { MessageBoxW(None, PCWSTR(h.as_ptr()), w!("rexplorer"), MB_OK | icon) };
}

fn placement_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("rexplorer").join("window.txt"))
}

unsafe fn save_placement(hwnd: HWND) {
    let Some(path) = placement_path() else { return };
    let mut wp = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
    if GetWindowPlacement(hwnd, &mut wp).is_err() {
        return;
    }
    let r = wp.rcNormalPosition;
    let show = if wp.showCmd == SW_SHOWMINIMIZED.0 as u32 { SW_SHOWNORMAL.0 as u32 } else { wp.showCmd };
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let _ = std::fs::write(path, format!("{show} {} {} {} {}", r.left, r.top, r.right, r.bottom));
}

unsafe fn restore_placement(hwnd: HWND) -> bool {
    let Some(path) = placement_path() else { return false };
    let Ok(text) = std::fs::read_to_string(path) else { return false };
    let v: Vec<i32> = text.split_whitespace().filter_map(|s| s.parse().ok()).collect();
    if v.len() != 5 {
        return false;
    }
    let wp = WINDOWPLACEMENT {
        length: size_of::<WINDOWPLACEMENT>() as u32,
        showCmd: v[0] as u32,
        rcNormalPosition: RECT { left: v[1], top: v[2], right: v[3], bottom: v[4] },
        ..Default::default()
    };
    SetWindowPlacement(hwnd, &wp).is_ok()
}
