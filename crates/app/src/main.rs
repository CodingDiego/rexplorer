//! The window: tabs, a header bar (buttons, breadcrumb path bar, search field), the places sidebar beside a virtual
//! ListView card, and a status bar.
//! All shell logic lives in rexplorer-core; this crate is plumbing and drawing.
#![cfg_attr(not(test), windows_subsystem = "windows")]
#![allow(unsafe_op_in_unsafe_fn)]
// A shell, COM or window failure must degrade, not panic (#28); tests may still unwrap.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

#[cfg(test)]
mod acc_test;
mod archive_view;
#[allow(dead_code)] // parts of the API are for later steps
mod breadcrumb;
mod commands;
mod connect;
mod desktop;
mod desktop_icons;
mod edit;
mod fit;
mod folder_options;
mod home;
#[allow(dead_code)] // parts of the API are for later steps
mod instance;
mod keys;
mod list;
mod mailbox;
mod nav;
mod palette;
mod panes;
mod preview_pane;
mod rtl;
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
mod watchdog;

use std::ffi::c_void;
use std::mem::size_of;
use std::path::PathBuf;
use std::time::{Duration, Instant};

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
    SetWindowSubclass, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Error, HSTRING, PCWSTR, PWSTR, Result, w};

use nav::{Tab, cur, go};
use source::Source;

const WM_ENUM_DONE: u32 = WM_APP + 1;
const WM_SHELL_CHANGE: u32 = WM_APP + 2;
/// The update check found a newer release: its version waits in UPDATE_FOUND.
const WM_UPDATE_FOUND: u32 = WM_APP + 11;
static UPDATE_FOUND: mailbox::Mailbox<String> = mailbox::Mailbox::new();
/// A breadcrumb dropdown's folders are listed: they wait in ui::CRUMB_ITEMS.
const WM_CRUMB_ITEMS: u32 = WM_APP + 13;
/// That version, for the status bar and the More menu.
static UPDATE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
const TIMER_REFRESH: usize = 1;
const TIMER_STATUS: usize = 2;
const TIMER_SESSION: usize = 3;
const TIMER_PREVIEW: usize = 4;
/// The pointer has rested on one list item long enough to prefetch its preview.
const TIMER_HOVER: usize = 10;
const TIMER_TREE: usize = 5;
/// The folder tree follows the folder just shown, after the listing has painted: navigation never waits for it.
const TIMER_TREE_SYNC: usize = 13;
const TIMER_SIDEBAR: usize = 6;
/// A navigation has run long enough to show the skeleton.
const TIMER_SKELETON: usize = 7;
/// The skeleton's shade alternates.
const TIMER_PULSE: usize = 8;
/// Typing in the Search box has paused.
pub(crate) const TIMER_SEARCH: usize = 9;
/// A shell change notification, debounced and at most one reload per second.
const TIMER_CHANGE: usize = 11;
/// A column was resized or dragged: read the header a moment later and remember it.
const TIMER_COLUMNS: usize = 12;
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
/// Details, Large icons, Thumbnails: CMD_VIEW1, +1, +2.
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
/// More > Install as default file manager... / Uninstall...
const CMD_INSTALL: u16 = 50;
const CMD_UNINSTALL: u16 = 51;
/// More > Folder options...
const CMD_FOLDER_OPTIONS: u16 = 52;
/// Ctrl+Shift+N, Ctrl+Enter, F11, Ctrl+I.
const CMD_NEW_FOLDER: u16 = 53;
const CMD_OPEN_TAB: u16 = 54;
const CMD_FULLSCREEN: u16 = 55;
const CMD_INVERT: u16 = 56;
/// F6 / Shift+F6: keyboard focus to the next / previous pane.
const CMD_FOCUS_NEXT: u16 = 57;
const CMD_FOCUS_PREV: u16 = 58;
/// Ctrl+N: a separate window on the current folder.
const CMD_NEW_WINDOW: u16 = 59;
/// Extra large, medium and small icons: no default chord for the first two.
const CMD_VIEW_XL: u16 = 60;
const CMD_VIEW_MEDIUM: u16 = 61;
const CMD_VIEW_SMALL: u16 = 62;
/// List, Tiles and Content: Ctrl+Shift+5, 7, 8.
const CMD_VIEW_LIST: u16 = 65;
const CMD_VIEW_TILES: u16 = 66;
const CMD_VIEW_CONTENT: u16 = 67;
/// Ctrl+Z / Ctrl+Y: undo and redo rexplorer's own file operations.
const CMD_UNDO: u16 = 63;
const CMD_REDO: u16 = 64;
/// Alt+Shift+P: the details pane in the preview pane's place.
const CMD_TOGGLE_DETAILS: u16 = 68;
/// Ctrl+Shift+C: the selected paths as text. Ctrl+Num+: size all columns to fit.
const CMD_COPY_PATH: u16 = 69;
const CMD_FIT_COLUMNS: u16 = 70;
/// Opens the release page the update notice names.
const CMD_UPDATE: u16 = 71;
/// Paste shortcut: .lnk files to the clipboard's files, in the current folder.
const CMD_PASTE_SHORTCUT: u16 = 72;
/// Ctrl+Shift+Enter: the selected program through the shell's runas verb.
const CMD_RUN_AS: u16 = 73;
/// F4: the address bar with its history dropdown.
const CMD_ADDRESS_HISTORY: u16 = 74;
/// Two crashes this close together (seconds) send the folder to Windows Explorer instead of a third start.
const CRASH_GUARD_SECS: u64 = 60;

const fn accel(flags: u8, key: VIRTUAL_KEY, cmd: u16) -> ACCEL {
    ACCEL { fVirt: ACCEL_VIRT_FLAGS(FVIRTKEY.0 | flags), key: key.0, cmd }
}

const ACCELS: [ACCEL; 47] = [
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
    accel(FALT.0 | FSHIFT.0, VK_P, CMD_TOGGLE_DETAILS),
    accel(FCONTROL.0, VK_F, CMD_SEARCH),
    accel(FCONTROL.0, VK_D, CMD_PIN),
    accel(FCONTROL.0 | FSHIFT.0, VK_1, CMD_VIEW_XL),
    accel(FCONTROL.0 | FSHIFT.0, VK_2, CMD_VIEW1 + 1),
    accel(FCONTROL.0 | FSHIFT.0, VK_3, CMD_VIEW_MEDIUM),
    accel(FCONTROL.0 | FSHIFT.0, VK_4, CMD_VIEW_SMALL),
    accel(FCONTROL.0 | FSHIFT.0, VK_5, CMD_VIEW_LIST),
    accel(FCONTROL.0 | FSHIFT.0, VK_6, CMD_VIEW1),
    accel(FCONTROL.0 | FSHIFT.0, VK_7, CMD_VIEW_TILES),
    accel(FCONTROL.0 | FSHIFT.0, VK_8, CMD_VIEW_CONTENT),
    accel(FCONTROL.0 | FSHIFT.0, VK_N, CMD_NEW_FOLDER),
    accel(0, VK_F4, CMD_ADDRESS_HISTORY),
    accel(FCONTROL.0, VK_E, CMD_SEARCH),
    accel(0, VK_F3, CMD_SEARCH),
    accel(FCONTROL.0, VK_RETURN, CMD_OPEN_TAB),
    accel(FCONTROL.0 | FSHIFT.0, VK_RETURN, CMD_RUN_AS),
    accel(0, VK_F11, CMD_FULLSCREEN),
    accel(FCONTROL.0, VK_I, CMD_INVERT),
    accel(0, VK_F6, CMD_FOCUS_NEXT),
    accel(FSHIFT.0, VK_F6, CMD_FOCUS_PREV),
    accel(FCONTROL.0, VK_N, CMD_NEW_WINDOW),
    accel(FCONTROL.0, VK_Z, CMD_UNDO),
    accel(FCONTROL.0, VK_Y, CMD_REDO),
    accel(FCONTROL.0 | FSHIFT.0, VK_C, CMD_COPY_PATH),
    accel(FCONTROL.0, VK_ADD, CMD_FIT_COLUMNS),
    accel(0, VK_TAB, CMD_FOCUS_NEXT),
    accel(FSHIFT.0, VK_TAB, CMD_FOCUS_PREV),
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
    /// The parsing name of the item the list's label edit was opened on; its commit renames only that item.
    renaming: Option<String>,
    /// F11: the window placement and style to return to, while full screen.
    fullscreen: Option<(WINDOWPLACEMENT, isize)>,
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
    /// Spaces the reloads that shell change notifications cause.
    change_gate: session::Debounce,
    /// A change arrived while minimized: reload once when the window is restored.
    stale: bool,
    /// TIMER_CHANGE is armed: a notification does not restart it (SetTimer would), so a folder written every few
    /// milliseconds still reloads.
    change_armed: bool,
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
    /// A `--new-window` window: it saves no session, placement or recents, so the first window's stay.
    secondary: bool,
    /// The clipboard's cut files (lowercased parsing names): their rows draw faded until the clipboard changes.
    cut: std::collections::HashSet<String>,
    /// The row the item stage decided to fade, so its cells (sub-item stage) follow without asking the shell again.
    dim_row: Option<usize>,
    /// Undo and redo of the file operations made here (Ctrl+Z / Ctrl+Y).
    undo: rexplorer_core::undo::Log,
}

fn main() -> Result<()> {
    // Before any window, so the taskbar groups them and finds the jump list (no install needed).
    let _ = rexplorer_core::jumplist::set_process_app_id();
    // Before the usage and argument-error boxes, so they speak the user's language too. Reads settings.ini only (a
    // missing or unreadable file means `system`); nothing may call `tr` before this line: the first call freezes English.
    rexplorer_core::i18n::init(&settings::dir(), &Settings::load().language, &system_language());
    let args = match cli::parse(std::env::args().skip(1)) {
        Ok(a) if a.help => return {
            info_box(&cli::usage(), MB_ICONINFORMATION);
            Ok(())
        },
        Ok(a) => a,
        Err(e) => return {
            info_box(&format!("{e}\n\n{}", cli::usage()), MB_ICONERROR);
            Ok(())
        },
    };
    if args.forward {
        // Explorer arguments we do not handle (/idlist, URLs, apps): the real Explorer gets them verbatim.
        std::process::exit(if rexplorer_core::fallback::forward_command_line().is_ok() { 0 } else { 1 });
    }
    // Exit code of the install and register flows: 0 done, 1 failed, 2 cancelled.
    let code = |o: ui::InstallOutcome| match o {
        ui::InstallOutcome::Done => 0,
        ui::InstallOutcome::Cancelled => 2,
        ui::InstallOutcome::Failed => 1,
    };
    if args.register || args.unregister {
        std::process::exit(code(unsafe { ui::register_flow(None, args.unregister, args.quiet) }));
    }
    if args.install || args.uninstall {
        // Every install asks first (it replaces any running copy) unless --quiet; --uninstall stays unconfirmed.
        std::process::exit(code(unsafe { ui::install_flow(None, args.uninstall, !args.uninstall, args.quiet) }));
    }
    if args.import_explorer {
        std::process::exit(if unsafe { ui::import_explorer_flow(args.profile.as_deref(), args.overwrite) } { 0 } else { 1 });
    }
    // A folder window never runs elevated unless asked (see core::elevation); the registry flows above are HKCU-only.
    // The desktop icons view never stays elevated: its icons are files any program can replace.
    if (!args.elevated || args.desktop_icons) && rexplorer_core::elevation::is_elevated() && rexplorer_core::elevation::relaunch_unelevated().is_ok() {
        return Ok(());
    }
    if args.desktop {
        return desktop::run();
    }
    // Opt-in only: no instance claim, no crash relaunch and no session write for the icons view.
    if args.desktop_icons {
        return desktop_icons::run();
    }
    if args.recover && !crash::record_recovery(CRASH_GUARD_SECS) {
        // The second crash in a row: show the folder in Windows Explorer rather than crash again.
        let s = session::load();
        // A RAR entry name can hold a quote, which would inject Explorer arguments.
        let folder = s.tabs.get(s.active).filter(|f| !f.chars().any(|c| c == '"' || c.is_control())).map(|f| format!("\"{f}\"")).unwrap_or_default();
        let _ = rexplorer_core::fallback::run_explorer(&folder);
        return Ok(());
    }
    // A recovery launch must not hand off to the instance that is crashing.
    let _instance = if args.new_window { None } else { instance::claim() };
    if _instance.is_none() && !args.recover && !args.new_window
        && let Some(h) = instance::find_main_window()
            && instance::send(h, &cli::payload(&args)) {
                return Ok(());
            } // else UIPI dropped it: open our own window
    if !args.recover && !args.new_window {
        crash::reset_recoveries();
    }
    // The packaged context-menu commands are looked up now so the first menu has them.
    rexplorer_core::packaged::warm();
    std::thread::spawn(|| {
        ui::background_priority();
        rexplorer_core::install::cleanup_old();
    });
    // A crash in a separate window would otherwise reopen the first window's session.
    if !args.new_window {
        crash::install(&["--recover".to_string()]);
    }
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
        let profile = settings.active_profile().clone();
        folder_options::sync(&profile);
        let base = settings.theme(settings::resolve_theme_name(&profile.theme, rexplorer_core::import_explorer::apps_use_dark())).clone();
        // The starter PNGs never take contrast colors.
        views::ensure_starter_theme(&base);
        let theme = theme::effective_theme(&base, theme::high_contrast_on(), theme::sys_color);
        let brushes = theme::Brushes::new(&theme);
        let session = startup_session(&profile, args.profile.is_some());

        let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = w!("RexplorerMain");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hIcon: LoadIconW(Some(hinst), PCWSTR(std::ptr::without_provenance(1))).unwrap_or_default(),
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
            renaming: None,
            fullscreen: None,
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
            change_gate: session::Debounce::new(Duration::from_secs(1)),
            stale: false,
            change_armed: false,
            recent: Recent::load(dir.join("recent.txt")),
            panes: panes::Panes::default(),
            browser: None,
            browser_at: None,
            usn: Default::default(),
            skeleton: false,
            pulse: false,
            secondary: args.new_window,
            cut: Default::default(),
            dim_row: None,
            undo: Default::default(),
        }));
        let hwnd = CreateWindowExW(
            rtl::layout(),
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
        let _ = rexplorer_core::jumplist::set_window_app_id(hwnd, false);
        // Unmarked, later launches and Win+E still go to the first window; the system cascades this one.
        if !args.new_window {
            instance::mark(hwnd);
        }
        if args.new_window || !restore_placement(hwnd) {
            let _ = ShowWindow(hwnd, SW_SHOWDEFAULT);
        }

        open_startup_tabs(app, &args);
        if args.recover && crash::has_dumps() {
            let q = HSTRING::from(rexplorer_core::i18n::tr("dlg.crash.saved"));
            if MessageBoxW(Some(hwnd), PCWSTR(q.as_ptr()), w!("rexplorer"), MB_YESNO | MB_ICONINFORMATION | rtl::message_box()) == IDYES {
                nav::new_tab(app, nav::Target::Name(crash::crash_dir().to_string_lossy().into_owned()), true, None);
            }
        }
        watchdog::spawn(hwnd);
        if (*app).profile.check_updates {
            check_updates(hwnd);
        }

        let accel_table = CreateAcceleratorTableW(&accels(&settings.keys))?;
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            // Accelerators only for keys aimed at our own controls; the ListView's label editor, the palette and the
            // search box's Enter / Escape keep their keys. Key messages go to the focus window, so their target
            // is the focus (and a key posted to one control is not taken for another).
            let focus = msg.hwnd;
            // Esc while the address bar's suggestions show is theirs: it closes the list, and only the next one leaves the bar.
            let addr = focus == (*app).addr && !(msg.message == WM_KEYDOWN && msg.wParam.0 == VK_ESCAPE.0 as usize && edit::suggestions_open());
            // Enter on a Home filter pill picks the pill, not the entry selected before.
            let home = focus == (*app).home && !(msg.message == WM_KEYDOWN && msg.wParam.0 == VK_RETURN.0 as usize && home::on_pill(focus));
            let ours = focus == (*app).list || home || addr || focus == hwnd || pane_accel(app, &msg, focus);
            // Timed too: an accelerator sends its WM_COMMAND from inside TranslateAcceleratorW, not through dispatch.
            if ours && watchdog::timed(&msg, || TranslateAcceleratorW(hwnd, accel_table, &msg)) != 0 {
                continue;
            }
            let _ = TranslateMessage(&msg);
            watchdog::timed(&msg, || DispatchMessageW(&msg));
        }
    }
    // Last, after every teardown main does (COM above, the instance claim here): until then a crash keeps its record.
    drop(_instance);
    crash::finish();
    Ok(())
}

/// The opt-in update check, on a worker: it posts WM_UPDATE_FOUND only when a newer release is out.
fn check_updates(hwnd: HWND) {
    let hwnd = hwnd.0 as isize;
    std::thread::spawn(move || {
        ui::background_priority();
        if let Some(v) = rexplorer_core::update::check(&settings::dir(), env!("CARGO_PKG_VERSION")) {
            let _ = UPDATE_FOUND.post(hwnd, WM_UPDATE_FOUND, v);
        }
    });
}

/// Rebuilds the taskbar jump list on a worker: Quick Access's frequent folders and `recent`. A call made while one
/// runs leaves its list for that worker, so a burst of navigations rebuilds it once or twice, not once each.
fn refresh_jump_list(recent: Vec<String>) {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    static NEXT: Mutex<Option<Vec<String>>> = Mutex::new(None);
    static BUSY: AtomicBool = AtomicBool::new(false);
    let next = || NEXT.lock().unwrap_or_else(|e| e.into_inner()).take();
    *NEXT.lock().unwrap_or_else(|e| e.into_inner()) = Some(recent);
    if BUSY.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(move || {
        ui::background_priority();
        let _com = ComInit::sta();
        let exe = std::env::current_exe().unwrap_or_default();
        let tr = |k: &'static str| rexplorer_core::i18n::tr(k).to_string();
        let labels = rexplorer_core::jumplist::Labels { frequent: tr("menu.jump.frequent"), recent: tr("menu.jump.recent"), new_window: tr("menu.jump.new_window") };
        loop {
            while let Some(recent) = next() {
                if let Err(e) = rexplorer_core::jumplist::update(&exe, &panes::frequent_folders(), &recent, &labels) {
                    crash::log(&format!("jump list: {e}"));
                }
            }
            BUSY.store(false, Ordering::Release);
            // A call between the last take and the store saw BUSY and left its list: take the work back for it.
            if NEXT.lock().unwrap_or_else(|e| e.into_inner()).is_none() || BUSY.swap(true, Ordering::AcqRel) {
                break;
            }
        }
    });
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
    s.show_details &= !s.show_preview;
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
    // The embedded Explorer view keeps Ctrl+Z / Ctrl+Y for its own undo.
    let undo_key = GetKeyState(VK_CONTROL.0 as i32) < 0 && matches!(VIRTUAL_KEY(msg.wParam.0 as u16), VK_Z | VK_Y);
    let panes = (*app).panes.tree.as_ref().is_some_and(|t| inside(t.hwnd())) || (!undo_key && source::browser_hwnd(app).is_some_and(inside));
    let bars = [(*app).nav, (*app).tools, (*app).cmdbar].contains(&focus);
    // The preview's text, reached by a click, is no trap: Tab and F6 leave it. Its other keys stay its own.
    let leaves_preview = matches!(VIRTUAL_KEY(msg.wParam.0 as u16), VK_TAB | VK_F6) && inside((*app).panes.preview);
    bars || leaves_preview || focus == (*app).search || focus == (*app).sidebar || focus == (*app).strip || focus == (*app).crumbs || (panes && !edit_keeps_tab(msg))
}

/// Tab in an Edit (a rename in the tree or the embedded view) is the Edit's: it commits and edits the next item.
unsafe fn edit_keeps_tab(msg: &MSG) -> bool {
    let mut class = [0u16; 16];
    let n = GetClassNameW(msg.hwnd, &mut class).max(0) as usize;
    VIRTUAL_KEY(msg.wParam.0 as u16) == VK_TAB && String::from_utf16_lossy(&class[..n]).eq_ignore_ascii_case("Edit")
}

fn accels(overrides: &[(String, String)]) -> Vec<ACCEL> {
    let digits = [VK_1, VK_2, VK_3, VK_4, VK_5, VK_6, VK_7, VK_8, VK_9];
    keys::apply(&ACCELS, overrides).into_iter().chain(digits.iter().enumerate().map(|(i, &k)| accel(FCONTROL.0, k, CMD_TAB1 + i as u16))).collect()
}

/// The session's tabs on --recover, or on a plain launch with restore_tabs; otherwise the command-line
/// folders; else the profile's start.
unsafe fn open_startup_tabs(app: *mut App, args: &cli::Args) {
    let saved = std::mem::take(&mut (*app).session.tabs);
    let names = !args.paths.is_empty() || args.select.is_some();
    if session::restores(args.recover, (*app).profile.restore_tabs, args.new_window, names) && !saved.is_empty() {
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
    // A window move or resize, a menu and a dialog box each run a message loop inside the message that began them.
    if matches!(msg, WM_ENTERSIZEMOVE | WM_ENTERMENULOOP | WM_ENTERIDLE) {
        watchdog::modal();
    }
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
            message(hwnd, &rexplorer_core::i18n::trf("err.controls", &[("error", &e)]));
            return LRESULT(-1);
        }
        return LRESULT(0);
    }
    let app = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if app.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    match msg {
        WM_SIZE => {
            ui::layout(app);
            if wparam.0 as u32 != SIZE_MINIMIZED && std::mem::take(&mut (*app).stale) {
                arm_change(app, 1);
            }
        }
        WM_PAINT => ui::paint(app),
        WM_ACTIVATE => {
            // Pins change in Explorer: activation re-reads the sidebar if it is over 5 s old, else it polls every 30 s
            // while the window is active (drives are announced by WM_DEVICECHANGE).
            if (wparam.0 & 0xFFFF) as u32 != WA_INACTIVE {
                SetTimer(Some(hwnd), TIMER_SIDEBAR, if panes::sidebar_stale(app) { 1 } else { 30_000 }, None);
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_DEVICECHANGE => {
            // DBT_DEVICEARRIVAL / DBT_DEVICEREMOVECOMPLETE: drives come and go without polling.
            if matches!(wparam.0, 0x8000 | 0x8004) {
                panes::sidebar_changed(app);
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_SETFOCUS => {
            let _ = SetFocus(Some(source::browser_hwnd(app).unwrap_or_else(|| ui::view_hwnd(app))));
        }
        WM_CLIPBOARDUPDATE => {
            commands::update_paste(app);
            list::refresh_cut(app);
        }
        WM_NOTIFY => return on_notify(app, lparam),
        WM_COMMAND => {
            let (id, code) = (wparam.0 & 0xFFFF, (wparam.0 >> 16) & 0xFFFF);
            if id == ID_ADDR {
                if code == EN_KILLFOCUS as usize {
                    ui::hide_edit(app);
                }
            } else if id == ID_SEARCH {
                if code == EN_CHANGE as usize {
                    SetTimer(Some(hwnd), TIMER_SEARCH, (*app).profile.search_delay_ms, None);
                }
            } else if id == ID_STATUS {
                // A click on the status bar's right text, where the update notice is.
                if UPDATE.get().is_some() {
                    on_command(app, CMD_UPDATE);
                }
            } else if is_command(code, (lparam.0 != 0).then(|| GetDlgCtrlID(HWND(lparam.0 as *mut c_void)) as usize)) {
                on_command(app, id as u16);
            }
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
            return LRESULT(theme::ctl_color(HDC(wparam.0 as *mut c_void), &(*app).theme, &(*app).brushes).0 as isize);
        }
        WM_COPYDATA => {
            // A sender on another protocol version gets 0 and opens its own window.
            if let Some(args) = instance::receive(lparam).and_then(|text| cli::parse_payload(&text)) {
                // A launch without folders (Win+E, the taskbar icon once installed) opens a tab at the start place.
                if !nav::open_args(app, &args) && args.search.is_none() {
                    nav::new_tab(app, nav::Target::Name((*app).profile.start.clone()), true, None);
                }
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
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_THEMECHANGED => {
            let setting = (msg == WM_SETTINGCHANGE && lparam.0 != 0).then(|| PCWSTR(lparam.0 as *const u16).to_string().ok()).flatten();
            if theme::is_color_change(msg, wparam.0, setting.as_deref()) {
                ui::refresh_theme(app);
            }
            // Text size has no setting name of its own here: re-read it on every broadcast (one registry read).
            if msg == WM_SETTINGCHANGE && theme::refresh_text_scale() {
                ui::rescale(app);
            }
            if msg == WM_SYSCOLORCHANGE {
                // Common controls cache system colors; they only refresh on this message.
                unsafe extern "system" fn forward(child: HWND, _: LPARAM) -> windows::core::BOOL {
                    unsafe { SendMessageW(child, WM_SYSCOLORCHANGE, None, None) };
                    windows::core::BOOL(1)
                }
                let _ = EnumChildWindows(Some(hwnd), Some(forward), LPARAM(0));
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_SYSCOMMAND if ui::is_bare_menu_key(wparam.0, lparam.0, GetKeyState(VK_SHIFT.0 as i32) < 0) => ui::focus_command_bar(app),
        WM_CONTEXTMENU if HWND(wparam.0 as *mut c_void) == (*app).nav => nav::on_nav_context(app, lparam),
        // Worker results: each message stands for one payload in a mailbox; the message itself carries none.
        WM_ENUM_DONE => {
            if let Some(d) = nav::ENUM_DONE.take(hwnd) {
                nav::on_enum_done(app, d);
            }
        }
        WM_CRUMB_ITEMS => {
            if let Some(c) = ui::CRUMB_ITEMS.take(hwnd) {
                ui::on_crumb_items(app, c);
            }
        }
        nav::WM_TAB_RETURN => {
            // The other window did not take the tab: it opens here again.
            for name in nav::take_returned() {
                let _ = MessageBeep(MB_OK);
                nav::new_tab(app, nav::Target::Name(name), true, None);
            }
        }
        WM_UPDATE_FOUND => {
            if let Some(v) = UPDATE_FOUND.take(hwnd) {
                let _ = UPDATE.set(v);
                ui::update_status(app);
            }
        }
        source::WM_SEARCH_BATCH => {
            if let Some(b) = source::SEARCH_BATCH.take(hwnd) {
                source::on_search_batch(app, b);
            }
        }
        source::WM_BROWSER_NAV => {
            if let Some(p) = source::BROWSER_NAV.take(hwnd) {
                source::on_browser_nav(app, p);
            }
        }
        source::WM_WORKER_ERROR => {
            if let Some(e) = source::WORKER_ERROR.take(hwnd) {
                message(hwnd, &e);
            }
        }
        source::WM_ARCHIVE_PASSWORD => {
            if let Some(a) = source::ARCHIVE_PASSWORD.take(hwnd) {
                source::on_archive_password(app, a);
            }
        }
        panes::WM_SIDEBAR_ROWS => {
            if let Some(r) = panes::SIDEBAR_ROWS.take(hwnd) {
                panes::on_sidebar_rows(app, r);
            }
        }
        panes::WM_TREE_SELECT => {
            if let Some(p) = panes::TREE_SELECT.take(hwnd) {
                panes::on_tree_select(app, p);
            }
        }
        panes::WM_QUICK_ACCESS_DONE => {
            if wparam.0 == 1 {
                panes::sidebar_changed(app);
            } else {
                let _ = MessageBeep(MB_OK);
            }
        }
        views::WM_THUMB_DONE => views::on_thumb_done(app),
        views::WM_ICONS_EXPORTED => views::on_icons_exported(app),
        watchdog::WM_HEARTBEAT => watchdog::beat(),
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
            // A change in a tab we are not showing only marks that tab; it reloads when shown.
            if nav::mark_changed(app, &ChangeWatch::acknowledge(wparam, lparam)) {
                if IsIconic(hwnd).as_bool() {
                    (*app).stale = true;
                } else if !(*app).change_armed {
                    arm_change(app, 150);
                }
            }
        }
        WM_TIMER => {
            let _ = KillTimer(Some(hwnd), wparam.0);
            if wparam.0 == TIMER_CHANGE {
                (*app).change_armed = false;
            }
            match wparam.0 {
                TIMER_REFRESH => nav::refresh(app),
                // Armed for the tab and folder active then: a clean one shown since has nothing to catch up on.
                TIMER_CHANGE if !nav::cur(app).dirty => {}
                TIMER_CHANGE => {
                    let (now, tab) = (Instant::now(), nav::cur(app));
                    // One change reload at a time (F5 and the rest still restart one): a change during it reloads once more after.
                    if tab.pending != 0 && tab.loading.is_none() {
                        arm_change(app, 500);
                    } else if (*app).change_gate.should_save(now) {
                        // Covers an in-app rename or new item whose own follow-up reload is still pending.
                        let _ = KillTimer(Some(hwnd), TIMER_REFRESH);
                        nav::refresh(app);
                    } else {
                        arm_change(app, (*app).change_gate.remaining(now).as_millis() as u32);
                    }
                }
                TIMER_STATUS => ui::update_status(app),
                TIMER_SESSION => nav::save_session(app),
                TIMER_COLUMNS => nav::remember_layout(app),
                TIMER_PREVIEW => panes::update_preview(app),
                TIMER_HOVER if GetForegroundWindow() == hwnd => panes::prefetch_hovered(app),
                TIMER_TREE => {
                    panes::ensure_tree(app);
                    ui::layout(app);
                }
                TIMER_TREE_SYNC => panes::sync_tree(app),
                TIMER_SKELETON => nav::on_skeleton_timer(app),
                TIMER_PULSE => nav::on_pulse_timer(app),
                TIMER_SEARCH => source::on_search_timer(app),
                TIMER_SIDEBAR if GetActiveWindow() == hwnd => {
                    if IsWindowVisible((*app).sidebar).as_bool() && !IsIconic(hwnd).as_bool() {
                        panes::refresh_sidebar(app);
                    }
                    SetTimer(Some(hwnd), TIMER_SIDEBAR, 30_000, None);
                }
                _ => {}
            }
        }
        WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR => {
            if let Some(menu) = &(*app).active_menu
                && let Some(result) = menu.handle_message(msg, wparam, lparam) {
                    return result;
                }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_DPICHANGED => {
            let r = &*(lparam.0 as *const RECT);
            let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            ui::rescale(app);
        }
        WM_DESTROY => {
            let _op = watchdog::op("close the window (save placement and session)");
            if !(*app).secondary {
                save_placement(hwnd, (*app).fullscreen.map(|f| f.0));
            }
            nav::save_session(app);
            for t in &mut (*app).tabs {
                t.watch = None;
                if let Some(Source::Search(s)) = &mut t.source {
                    s.stop();
                }
            }
            (*app).browser = None;
            let _ = windows::Win32::System::DataExchange::RemoveClipboardFormatListener(hwnd);
            for w in [(*app).list, (*app).sidebar, (*app).crumbs, (*app).strip, (*app).home] {
                rexplorer_core::dragdrop::unregister(w);
            }
            views::destroy(app);
            let _ = std::fs::remove_dir_all(archive_view::session_dir());
            rexplorer_core::archive::forget_passwords();
            (*app).panes.tree = None;
            // Workers that finish later fail to post and queue nothing.
            UPDATE_FOUND.forget(hwnd);
            nav::ENUM_DONE.forget(hwnd);
            ui::CRUMB_ITEMS.forget(hwnd);
            source::SEARCH_BATCH.forget(hwnd);
            source::BROWSER_NAV.forget(hwnd);
            source::WORKER_ERROR.forget(hwnd);
            source::ARCHIVE_PASSWORD.forget(hwnd);
            panes::SIDEBAR_ROWS.forget(hwnd);
            panes::TREE_SELECT.forget(hwnd);
            // The shell asks for a window's properties to be cleared before it goes.
            let _ = rexplorer_core::jumplist::set_window_app_id(hwnd, true);
            PostQuitMessage(0);
        }
        _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
    }
    LRESULT(0)
}

/// Arms TIMER_CHANGE for `ms` and records it, so a notification can tell it is already pending.
unsafe fn arm_change(app: *mut App, ms: u32) {
    (*app).change_armed = true;
    SetTimer(Some((*app).hwnd), TIMER_CHANGE, ms, None);
}

#[allow(clippy::needless_borrow)] // the explicit borrow of a raw-pointer field silences rustc's dangerous_implicit_autorefs
unsafe fn on_notify(app: *mut App, lparam: LPARAM) -> LRESULT {
    // rexplorer's own controls notify through `access::send_own`, so their structures are read only from such a send;
    // anything else here is the list's (comctl32's own notifications).
    if !rexplorer_core::access::own_pointer(lparam) {
        let hdr = &*(lparam.0 as *const NMHDR);
        return if hdr.hwndFrom == (*app).list { list::on_notify(app, lparam) } else { LRESULT(0) };
    }
    let hdr = &*(lparam.0 as *const NMHDR);
    let from = hdr.hwndFrom;
    if from == (*app).home {
        nav::on_home_notify(app, &*(lparam.0 as *const home::NmHome));
        return LRESULT(0);
    }
    if from == (*app).strip {
        let nm = &*(lparam.0 as *const tabs::NmTab);
        match hdr.code {
            tabs::TN_SELECT => nav::switch_tab(app, nm.index),
            tabs::TN_CLOSE => nav::close_tab(app, nm.index),
            tabs::TN_NEW => nav::new_start_tab(app),
            tabs::TN_MOVE => nav::tab_moved(app, nm.index, nm.to),
            tabs::TN_CONTEXT => nav::tab_menu(app, nm.index, None),
            tabs::TN_CONTEXT_KEY => nav::tab_menu(app, nm.index, Some(tabs::menu_point(from, nm.index))),
            tabs::TN_TEAR => nav::tear_tab(app, nm.index),
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
            breadcrumb::CN_DROPDOWN => ui::crumb_dropdown(app, nm.index, None),
            breadcrumb::CN_DROPDOWN_KEY => ui::crumb_dropdown(app, nm.index, Some(breadcrumb::menu_point(from, nm.index))),
            _ => {}
        }
    } else if from == (*app).sidebar && hdr.code == sidebar::SN_PICK {
        let nm = &*(lparam.0 as *const sidebar::NmSidebar);
        let target = sidebar::target(from, nm.index);
        if !target.is_empty() {
            go(app, nav::Target::Name(target), nav::History::Push);
        }
    } else if from == (*app).sidebar && hdr.code == sidebar::SN_CONTEXT {
        panes::sidebar_menu(app, (*(lparam.0 as *const sidebar::NmSidebar)).index, None);
    } else if from == (*app).sidebar && hdr.code == sidebar::SN_CONTEXT_KEY {
        let index = (*(lparam.0 as *const sidebar::NmSidebar)).index;
        panes::sidebar_menu(app, index, Some(sidebar::menu_point(from, index)));
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
        CMD_ADDRESS_HISTORY => ui::address_history(app),
        CMD_EXPLORER => {
            if let Some(s) = nav::source(app) {
                let _ = open_in_explorer(&s.explorer_name());
            }
        }
        CMD_ENTER => {
            if GetFocus() == addr {
                let text = window_text(addr);
                let current = nav::source(app).map(|s| s.editing_name()).unwrap_or_default();
                let searching = matches!(nav::source(app), Some(source::Source::Search(_)));
                if address_refreshes(&text, &current, searching) {
                    nav::refresh(app);
                } else {
                    go(app, nav::Target::Name(nav::expand_env(text.trim(), |n| std::env::var(n).ok())), nav::History::Push);
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
                // Losing focus hides the Edit and restores its text. An Esc that closes the suggestions never gets here.
                let _ = SetFocus(Some(ui::view_hwnd(app)));
            } else {
                if !source::cancel_search(app) {
                    list::cancel_cut(app);
                }
            }
        }
        CMD_PROPERTIES => {
            if list::selected_indices(list).is_empty() {
                list::verb_on_background(app, "properties");
            } else {
                list::verb_on_selection(app, "properties", false);
            }
        }
        CMD_NEW_TAB => nav::new_start_tab(app),
        CMD_CLOSE_TAB => nav::close_tab(app, (*app).active),
        CMD_NEXT_TAB => nav::cycle_tab(app, 1),
        CMD_PREV_TAB => nav::cycle_tab(app, -1),
        CMD_PALETTE => ui::open_palette(app),
        CMD_TERMINAL => ui::open_terminal(app),
        CMD_VIEW => views::set_view(app, (*app).views.view.next()),
        CMD_NEW => commands::new_menu(app),
        CMD_NEW_FOLDER => commands::new_folder(app),
        CMD_OPEN_TAB => {
            let i = SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_FOCUSED as isize))).0;
            // From the address bar Ctrl+Enter would open a list row the user is not looking at.
            if i >= 0 && GetFocus() != addr {
                nav::open_in_background(app, i as usize);
            }
        }
        CMD_FULLSCREEN => ui::toggle_fullscreen(app),
        CMD_INVERT => list::invert_selection(app),
        CMD_NEW_WINDOW => {
            if !instance::open_new_window(&cur(app).name) {
                let _ = MessageBeep(MB_OK);
            }
        }
        CMD_FOCUS_NEXT | CMD_FOCUS_PREV => ui::cycle_focus(app, cmd == CMD_FOCUS_PREV),
        CMD_CUT => list::verb_on_selection(app, "cut", false),
        CMD_COPY => list::verb_on_selection(app, "copy", false),
        CMD_COPY_PATH => list::copy_as_path(app),
        CMD_FIT_COLUMNS => list::fit_columns(app),
        CMD_PASTE => list::verb_on_background(app, "paste"),
        CMD_PASTE_SHORTCUT => list::paste_shortcut(app),
        CMD_RUN_AS => list::run_as_admin(app),
        CMD_UNDO | CMD_REDO => list::undo_step(app, cmd == CMD_REDO),
        CMD_RENAME => list::begin_rename(app),
        CMD_SHARE => commands::share(app),
        CMD_DELETE => list::verb_on_selection(app, "delete", GetKeyState(VK_SHIFT.0 as i32) < 0),
        CMD_SORT => commands::sort_menu(app),
        CMD_VIEW_MENU => commands::view_menu(app),
        CMD_TOGGLE_SIDEBAR => panes::toggle_sidebar(app),
        CMD_FOLDER_TREE => panes::toggle_tree(app),
        CMD_TOGGLE_PREVIEW => panes::toggle_right(app, panes::RightPane::Preview),
        CMD_TOGGLE_DETAILS => panes::toggle_right(app, panes::RightPane::Details),
        CMD_CONNECT => {
            if let Some(url) = connect::dialog((*app).hwnd, &(*app).theme) {
                nav::new_tab(app, nav::Target::Name(url), true, None);
            }
        }
        CMD_SEARCH => {
            let _ = SetFocus(Some((*app).search));
            SendMessageW((*app).search, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        }
        CMD_MORE => ui::more_menu(app),
        CMD_REGISTER | CMD_UNREGISTER => {
            ui::register_flow(Some((*app).hwnd), cmd == CMD_UNREGISTER, false);
        }
        CMD_INSTALL | CMD_UNINSTALL => {
            ui::install_flow(Some((*app).hwnd), cmd == CMD_UNINSTALL, true, false);
        }
        CMD_FOLDER_OPTIONS => folder_options::run(app),
        CMD_SETTINGS => nav::new_tab(app, nav::Target::Name(settings::dir().display().to_string()), true, None),
        CMD_ABOUT => ui::about(app),
        CMD_UPDATE => {
            let page = HSTRING::from(rexplorer_core::update::RELEASE_PAGE);
            ShellExecuteW(Some((*app).hwnd), w!("open"), &page, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        }
        CMD_PIN => list::pin_to_home(app),
        c if (CMD_TAB1..CMD_TAB1 + 9).contains(&c) => {
            // Ctrl+9 is the last tab, as in browsers.
            let n = (*app).tabs.len();
            let i = if c == CMD_TAB1 + 8 { n - 1 } else { (c - CMD_TAB1) as usize };
            nav::switch_tab(app, i);
        }
        _ => {
            if let Some(v) = views::View::of_command(cmd) {
                views::set_view(app, v)
            }
        }
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
    MessageBoxW(Some(hwnd), PCWSTR(h.as_ptr()), w!("rexplorer"), MB_OK | MB_ICONERROR | rtl::message_box());
}

/// The Windows UI language as a locale name (`es-ES`); `en` when the system gives none.
fn system_language() -> String {
    use windows::Win32::Globalization::{GetUserDefaultLocaleName, GetUserDefaultUILanguage, LCIDToLocaleName};
    let mut buf = [0u16; 85];
    let n = unsafe { LCIDToLocaleName(GetUserDefaultUILanguage() as u32, Some(&mut buf), 0) };
    let n = if n > 0 { n } else { unsafe { GetUserDefaultLocaleName(&mut buf) } };
    if n > 1 { String::from_utf16_lossy(&buf[..n as usize - 1]) } else { "en".into() }
}

/// A message box before any window exists (--help, bad arguments).
fn info_box(text: &str, icon: MESSAGEBOX_STYLE) {
    let h = HSTRING::from(text);
    unsafe { MessageBoxW(None, PCWSTR(h.as_ptr()), w!("rexplorer"), MB_OK | icon | rtl::message_box()) };
}

fn placement_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("rexplorer").join("window.txt"))
}

/// Saves `hwnd`'s placement, or `saved` (the one full screen returns to) when given.
unsafe fn save_placement(hwnd: HWND, saved: Option<WINDOWPLACEMENT>) {
    let Some(path) = placement_path() else { return };
    let mut wp = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
    if let Some(p) = saved {
        wp = p;
    } else if GetWindowPlacement(hwnd, &mut wp).is_err() {
        return;
    }
    let r = wp.rcNormalPosition;
    let show = if wp.showCmd == SW_SHOWMINIMIZED.0 as u32 { SW_SHOWNORMAL.0 as u32 } else { wp.showCmd };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
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

/// Enter in the address box refreshes on an empty box or the place's own name; on search results the name is the
/// searched folder's, and entering it goes to that folder (as in Explorer) instead of searching again.
fn address_refreshes(text: &str, current: &str, searching: bool) -> bool {
    text.trim().is_empty() || (text == current && !searching)
}

/// Whether a WM_COMMAND carries a command: a menu item (code 0) or accelerator (code 1) with no control, or a click
/// on one of our toolbars. Other controls forward their notifications here too (the list's label edit is id 1 =
/// CMD_BACK and sends EN_CHANGE per key), and none of those may run a command.
fn is_command(code: usize, control: Option<usize>) -> bool {
    match control {
        None => code <= 1,
        Some(id) => code == 0 && matches!(id, ID_NAV | ID_TOOLS | ID_CMDBAR),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_menus_accelerators_and_toolbars_send_commands() {
        use super::*;
        assert!(is_command(0, None));
        assert!(is_command(1, None));
        assert!(is_command(0, Some(ID_NAV)) && is_command(0, Some(ID_TOOLS)) && is_command(0, Some(ID_CMDBAR)));
        // The label edit (id 1): EN_UPDATE 0x400, EN_CHANGE 0x300, EN_SETFOCUS 0x100, EN_KILLFOCUS 0x200.
        for code in [0x100, 0x200, 0x300, 0x400] {
            assert!(!is_command(code, Some(1)));
        }
        assert!(!is_command(0, Some(1)));
        assert!(!is_command(0x300, Some(ID_NAV)));
    }

    #[test]
    fn address_enter_refreshes_only_the_place_shown() {
        use super::address_refreshes;
        assert!(address_refreshes("C:\\a", "C:\\a", false));
        assert!(address_refreshes("  ", "C:\\a", true));
        assert!(!address_refreshes("C:\\a", "C:\\a", true));
        assert!(!address_refreshes("C:\\b", "C:\\a", false));
    }

    #[test]
    fn system_language_is_a_locale_name() {
        let l = super::system_language();
        assert!(!l.is_empty() && l.is_ascii() && l.contains('-'), "{l}");
    }
}
