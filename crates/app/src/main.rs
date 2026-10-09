//! The window: tabs, a header bar (buttons, breadcrumb path bar, search field), the places sidebar beside a virtual
//! ListView card, and a status bar.
//! All shell logic lives in rexplorer-core; this crate is plumbing and drawing.
#![cfg_attr(not(test), windows_subsystem = "windows")]
#![allow(unsafe_op_in_unsafe_fn)]

mod archive_view;
#[allow(dead_code)] // parts of the API are for later steps
mod breadcrumb;
mod commands;
mod connect;
mod home;
#[allow(dead_code)] // parts of the API are for later steps
mod instance;
mod list;
mod nav;
mod palette;
mod panes;
mod preview_pane;
mod session;
mod sidebar;
mod source;
mod statusbar;
#[allow(dead_code)] // parts of the API are for later steps
mod tabs;
mod theme;
mod thumbs;
mod toolbar;
mod ui;
mod views;

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
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_FLAGS_AND_ATTRIBUTES};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{
    DefSubclassProc, SHFILEINFOW, SHGFI_PIDL, SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW,
    SetWindowSubclass,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Error, HSTRING, PCWSTR, PWSTR, Result, w};

use nav::{Tab, cur, go, listing};
use source::Source;

const WM_ENUM_DONE: u32 = WM_APP + 1;
const WM_SHELL_CHANGE: u32 = WM_APP + 2;
const TIMER_REFRESH: usize = 1;
const TIMER_STATUS: usize = 2;
const TIMER_SESSION: usize = 3;
const TIMER_PREVIEW: usize = 4;
const TIMER_TREE: usize = 5;
const TIMER_SIDEBAR: usize = 6;
/// A navigation has run long enough to show the skeleton.
const TIMER_SKELETON: usize = 7;
/// The skeleton's shade alternates.
const TIMER_PULSE: usize = 8;
const ICON_UNKNOWN: i32 = i32::MIN;

const ID_ADDR: usize = 100;
const ID_LIST: usize = 101;
const ID_TABS: usize = 102;
const ID_NAV: usize = 103;
const ID_TOOLS: usize = 104;
const ID_CRUMBS: usize = 105;
const ID_STATUS: usize = 106;
const ID_PALETTE: usize = 107;
const ID_PREVIEW: usize = 108;
const ID_SEARCH: usize = 109;
const ID_SIDEBAR: usize = 110;
const ID_CMDBAR: usize = 111;
const ID_HOME: usize = 112;

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
const CMD_TOGGLE_SIDEBAR: u16 = 23;
const CMD_TOGGLE_PREVIEW: u16 = 24;
const CMD_SEARCH: u16 = 28;
/// More > Folder tree: the tree instead of the sidebar.
const CMD_FOLDER_TREE: u16 = 30;
/// Ctrl+Shift+1..3 send CMD_VIEW1..CMD_VIEW1 + 2 (details, icons, thumbnails).
const CMD_VIEW1: u16 = 25;
/// Ctrl+1..Ctrl+9 send CMD_TAB1..CMD_TAB1 + 8.
const CMD_TAB1: u16 = 31;
/// Ctrl+D: pin the selected folders (or the current one) to Home.
const CMD_PIN: u16 = 40;
/// The command bar: New v, Cut, Copy, Paste, Rename, Share, Delete, Sort v, View v.
const CMD_NEW: u16 = 41;
const CMD_CUT: u16 = 42;
const CMD_COPY: u16 = 43;
const CMD_PASTE: u16 = 44;
const CMD_RENAME: u16 = 45;
const CMD_DELETE: u16 = 46;
const CMD_SORT: u16 = 47;
const CMD_VIEW_MENU: u16 = 48;
/// The command bar's Share: the Windows share sheet for the selected files.
const CMD_SHARE: u16 = 49;

const fn accel(flags: u8, key: VIRTUAL_KEY, cmd: u16) -> ACCEL {
    ACCEL { fVirt: ACCEL_VIRT_FLAGS(FVIRTKEY.0 | flags), key: key.0, cmd }
}

const ACCELS: [ACCEL; 24] = [
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
    accel(FCONTROL.0, VK_B, CMD_TOGGLE_SIDEBAR),
    accel(FALT.0, VK_P, CMD_TOGGLE_PREVIEW),
    accel(FCONTROL.0, VK_F, CMD_SEARCH),
    accel(FCONTROL.0, VK_D, CMD_PIN),
    accel(FCONTROL.0 | FSHIFT.0, VK_1, CMD_VIEW1),
    accel(FCONTROL.0 | FSHIFT.0, VK_2, CMD_VIEW1 + 1),
    accel(FCONTROL.0 | FSHIFT.0, VK_3, CMD_VIEW1 + 2),
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
    search: HWND,
    sidebar: HWND,
    /// The command bar under the header.
    cmdbar: HWND,
    /// The Home page, shown in the list's place on Home and Quick Access.
    home: HWND,
    /// After New > ...: start renaming the item that appears in this folder (parsing name, names before).
    new_item: Option<(String, std::collections::HashSet<String>)>,
    /// The rounded field the window paints behind the search Edit.
    search_field: RECT,
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
    panes: panes::Panes,
    views: views::Views,
    /// The embedded Explorer view of the active tab, when it is in browser mode.
    browser: Option<rexplorer_core::browser::EmbeddedBrowser>,
    /// Where the embedded view is (or was last told to go).
    browser_at: Option<Pidl>,
    /// Volume indexes for elevated searches, built once per drive.
    usn: source::UsnCache,
    /// The list is empty and draws placeholder rows while the active tab loads (or searches with no results yet).
    skeleton: bool,
    /// Which of the skeleton's two shades is showing.
    pulse: bool,
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
    // Undocumented, for the smoke test: an access violation after this many ms exercises the recovery path. A
    // recovery launch inherits the variable and ignores it.
    if let Some(ms) = std::env::var("REXPLORER_CRASH_AFTER_MS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|_| !args.recover) {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            unsafe { windows::Win32::System::Diagnostics::Debug::RaiseException(0xC000_0005, 0, None) };
        });
    }

    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _com = ComInit::ole();
        let icc = INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_LISTVIEW_CLASSES };
        let _ = InitCommonControlsEx(&icc);
        // Creates the system image list here, before the workers ask it for folder icons: a worker's first
        // SHGFI_SYSICONINDEX call racing the UI thread's own first use comes back empty.
        let mut fi = SHFILEINFOW::default();
        SHGetFileInfoW(
            w!("folder"),
            FILE_ATTRIBUTE_DIRECTORY,
            Some(&mut fi),
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_USEFILEATTRIBUTES | SHGFI_SYSICONINDEX | SHGFI_SMALLICON,
        );

        let dir = settings::dir();
        Settings::ensure_files(&dir);
        let mut settings = Settings::load();
        if let Some(p) = &args.profile {
            settings.active = p.clone();
        }
        let mut profile = settings.active_profile().clone();
        // Home needs a recent Windows; elsewhere the default start is This PC.
        if profile.start.eq_ignore_ascii_case(settings::HOME_START) && ShellFolder::parse(&profile.start).is_err() {
            profile.start = "shell:MyComputerFolder".into();
        }
        let theme = settings.theme(&profile.theme).clone();
        let brushes = theme::Brushes::new(&theme);
        views::ensure_starter_theme(&theme);
        let session = startup_session(&profile, args.profile.is_some());

        let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = w!("RexplorerMain");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hIcon: LoadIconW(Some(hinst), PCWSTR(1 as *const u16)).unwrap_or_default(),
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
            search: HWND::default(),
            sidebar: HWND::default(),
            cmdbar: HWND::default(),
            home: HWND::default(),
            new_item: None,
            search_field: RECT::default(),
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
            views: views::Views::new(views::View::parse(&session.view)),
            session,
            debounce: session::Debounce::new(Duration::from_secs(2)),
            recent: Recent::load(dir.join("recent.txt")),
            panes: panes::Panes::default(),
            browser: None,
            browser_at: None,
            usn: Default::default(),
            skeleton: false,
            pulse: false,
        }));
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("rexplorer"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1000,
            700,
            None,
            None,
            Some(hinst),
            Some(app as *const c_void),
        )?;
        instance::mark(hwnd);
        if !restore_placement(hwnd) {
            let _ = ShowWindow(hwnd, SW_SHOWDEFAULT);
        }

        open_startup_tabs(app, &args);

        let accel_table = CreateAcceleratorTableW(&accels())?;
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            // Accelerators only for keys aimed at our own controls; the ListView's label editor, the palette and the
            // search box's Enter / Escape keep their keys. Key messages go to the focus window, so their target
            // is the focus (and a key posted to one control is not taken for another).
            let focus = msg.hwnd;
            let ours = focus == (*app).list || focus == (*app).home || focus == (*app).addr || focus == hwnd || pane_accel(app, &msg, focus);
            if ours && TranslateAcceleratorW(hwnd, accel_table, &msg) != 0 {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

/// Pane and view state: the saved session, or the profile on a first run or an explicit --profile.
fn startup_session(profile: &Profile, explicit_profile: bool) -> session::Session {
    let mut s = session::load();
    if explicit_profile || !session::path().exists() {
        s.view = profile.view.clone();
        s.show_tree = profile.show_tree;
        s.show_preview = profile.show_preview;
        s.tree_width = profile.tree_width;
        s.preview_width = profile.preview_width;
    }
    s.tree_width = panes::clamp_width(s.tree_width);
    s.preview_width = panes::clamp_width(s.preview_width);
    s
}

/// The tree, the embedded browser and the search box keep Enter and Escape; every other shortcut works while
/// they have the focus.
unsafe fn pane_accel(app: *mut App, msg: &MSG, focus: HWND) -> bool {
    if !matches!(msg.message, WM_KEYDOWN | WM_SYSKEYDOWN) || matches!(VIRTUAL_KEY(msg.wParam.0 as u16), VK_RETURN | VK_ESCAPE) {
        return false;
    }
    let inside = |h: HWND| focus == h || IsChild(h, focus).as_bool();
    focus == (*app).search || (*app).panes.tree.as_ref().is_some_and(|t| inside(t.hwnd())) || source::browser_hwnd(app).is_some_and(inside)
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
    nav::apply_search(app, args);
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
        WM_PAINT => ui::paint(app),
        WM_ACTIVATE => {
            // Drives come and go: the sidebar re-reads them every 5 s, only while the window is active.
            if (wparam.0 & 0xFFFF) as u32 != WA_INACTIVE {
                SetTimer(Some(hwnd), TIMER_SIDEBAR, 5000, None);
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_SETFOCUS => {
            let _ = SetFocus(Some(source::browser_hwnd(app).unwrap_or_else(|| ui::view_hwnd(app))));
        }
        WM_CLIPBOARDUPDATE => commands::update(app),
        WM_NOTIFY => return on_notify(app, lparam),
        WM_COMMAND => {
            let (id, code) = (wparam.0 & 0xFFFF, (wparam.0 >> 16) & 0xFFFF);
            if id == ID_ADDR {
                if code == EN_KILLFOCUS as usize {
                    ui::hide_edit(app);
                }
            } else if id != ID_SEARCH {
                on_command(app, id as u16);
            }
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
            return LRESULT(theme::ctl_color(HDC(wparam.0 as *mut c_void), &(*app).theme, &(*app).brushes).0 as isize);
        }
        WM_COPYDATA => {
            if let Some(text) = instance::receive(lparam) {
                let args = cli::parse_payload(&text);
                nav::open_args(app, &args);
                nav::apply_search(app, &args);
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
        source::WM_SEARCH_BATCH => source::on_search_batch(app, Box::from_raw(lparam.0 as *mut source::SearchBatch)),
        source::WM_BROWSER_NAV => source::on_browser_nav(app, Box::from_raw(lparam.0 as *mut Pidl)),
        source::WM_WORKER_ERROR => message(hwnd, &Box::from_raw(lparam.0 as *mut String)),
        panes::WM_SIDEBAR_ROWS => panes::on_sidebar_rows(app, Box::from_raw(lparam.0 as *mut panes::SidebarRows)),
        panes::WM_TREE_SELECT => panes::on_tree_select(app, Box::from_raw(lparam.0 as *mut Pidl)),
        views::WM_THUMB_DONE => views::on_thumb_done(app, wparam, lparam),
        WM_SETCURSOR => {
            if HWND(wparam.0 as *mut c_void) == hwnd && (lparam.0 & 0xFFFF) as u32 == HTCLIENT && panes::on_set_cursor(app) {
                return LRESULT(1);
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_LBUTTONDOWN => panes::on_button_down(app, lparam),
        WM_MOUSEMOVE => panes::on_mouse_move(app, lparam),
        WM_LBUTTONUP | WM_CAPTURECHANGED => panes::end_drag(app),
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
                TIMER_PREVIEW => panes::update_preview(app),
                TIMER_TREE => {
                    panes::ensure_tree(app);
                    ui::layout(app);
                }
                TIMER_SKELETON => nav::on_skeleton_timer(app),
                TIMER_PULSE => nav::on_pulse_timer(app),
                TIMER_SIDEBAR if GetActiveWindow() == hwnd => {
                    panes::refresh_sidebar(app);
                    SetTimer(Some(hwnd), TIMER_SIDEBAR, 5000, None);
                }
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
            views::rebuild_tall(app);
            if let Some(s) = nav::source(app) {
                nav::set_columns(app, &s.columns());
            }
            views::apply(app);
            ui::layout(app);
        }
        WM_DESTROY => {
            save_placement(hwnd);
            nav::save_session(app);
            for t in &mut (*app).tabs {
                t.watch = None;
                if let Some(Source::Search(s)) = &mut t.source {
                    s.stop();
                }
            }
            (*app).browser = None;
            let _ = windows::Win32::System::DataExchange::RemoveClipboardFormatListener(hwnd);
            rexplorer_core::dragdrop::unregister((*app).list);
            views::destroy(app);
            (*app).panes.tree = None;
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
    if from == (*app).home {
        nav::on_home_notify(app, &*(lparam.0 as *const home::NmHome));
        return LRESULT(0);
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
        // While a navigation is in flight the bar shows its destination, not the tab's crumbs.
        if cur(app).loading.is_some() {
            return LRESULT(0);
        }
        let nm = &*(lparam.0 as *const breadcrumb::NmCrumb);
        match hdr.code {
            breadcrumb::CN_CRUMB => {
                let tab = cur(app);
                if nm.index + 1 >= tab.crumbs.len() {
                    nav::refresh(app);
                } else if let Some(c) = tab.crumbs.get(nm.index) {
                    let target = match &c.pidl {
                        Some(p) => nav::Target::Pidl(p.clone()),
                        None => nav::Target::Name(c.parsing.clone()),
                    };
                    go(app, target, nav::History::Push);
                }
            }
            breadcrumb::CN_EDIT => ui::show_edit(app),
            breadcrumb::CN_DROPDOWN => ui::crumb_dropdown(app, nm.index),
            _ => {}
        }
    } else if from == (*app).sidebar && hdr.code == sidebar::SN_PICK {
        let nm = &*(lparam.0 as *const sidebar::NmSidebar);
        let target = sidebar::target(from, nm.index);
        if !target.is_empty() {
            go(app, nav::Target::Name(target), nav::History::Push);
        }
    } else if from == (*app).sidebar && hdr.code == sidebar::SN_CONTEXT {
        panes::sidebar_menu(app, (*(lparam.0 as *const sidebar::NmSidebar)).index);
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
            if let Some(s) = nav::source(app) {
                let _ = open_in_explorer(&s.explorer_name());
            }
        }
        CMD_ENTER => {
            if GetFocus() == addr {
                let text = window_text(addr);
                let current = nav::source(app).map(|s| s.editing_name()).unwrap_or_default();
                if text.trim().is_empty() || text == current {
                    nav::refresh(app);
                } else {
                    go(app, nav::Target::Name(text.trim().to_string()), nav::History::Push);
                }
                let _ = SetFocus(Some(ui::view_hwnd(app)));
            } else {
                let i = SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_FOCUSED as isize))).0;
                if i >= 0 {
                    source::activate(app, i as usize);
                }
            }
        }
        CMD_ESCAPE => {
            if GetFocus() == addr {
                // Losing focus hides the Edit and restores its text.
                let _ = SetFocus(Some(ui::view_hwnd(app)));
            } else {
                source::cancel_search(app);
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
        CMD_VIEW => views::set_view(app, (*app).views.view.next()),
        CMD_NEW => commands::new_menu(app),
        CMD_CUT => list::verb_on_selection(app, "cut", false),
        CMD_COPY => list::verb_on_selection(app, "copy", false),
        CMD_PASTE => list::verb_on_background(app, "paste"),
        CMD_RENAME => list::begin_rename(app),
        CMD_SHARE => commands::share(app),
        CMD_DELETE => list::verb_on_selection(app, "delete", GetKeyState(VK_SHIFT.0 as i32) < 0),
        CMD_SORT => commands::sort_menu(app),
        CMD_VIEW_MENU => commands::view_menu(app),
        CMD_VIEW1 => views::set_view(app, views::View::Details),
        c if c == CMD_VIEW1 + 1 => views::set_view(app, views::View::Icons),
        c if c == CMD_VIEW1 + 2 => views::set_view(app, views::View::Thumbnails),
        CMD_TOGGLE_SIDEBAR => panes::toggle_sidebar(app),
        CMD_FOLDER_TREE => panes::toggle_tree(app),
        CMD_TOGGLE_PREVIEW => panes::toggle_preview(app),
        CMD_CONNECT => {
            if let Some(url) = connect::dialog((*app).hwnd, &(*app).theme, (*app).font) {
                nav::new_tab(app, nav::Target::Name(url), true, None);
            }
        }
        CMD_SEARCH => {
            let _ = SetFocus(Some((*app).search));
            SendMessageW((*app).search, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        }
        CMD_MORE => ui::more_menu(app),
        CMD_REGISTER | CMD_UNREGISTER => ui::register_flow(Some((*app).hwnd), cmd == CMD_UNREGISTER),
        CMD_SETTINGS => nav::new_tab(app, nav::Target::Name(settings::dir().display().to_string()), true, None),
        CMD_ABOUT => ui::about(app),
        CMD_PIN => list::pin_to_home(app),
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
