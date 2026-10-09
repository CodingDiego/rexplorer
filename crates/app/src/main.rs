//! The window: an address bar over a virtual ListView. All logic lives in rexplorer-core.
#![cfg_attr(not(test), windows_subsystem = "windows")]
#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;
use std::mem::size_of;
use std::path::PathBuf;

use rexplorer_core::com::{ComInit, Pidl};
use rexplorer_core::fallback::open_in_explorer;
use rexplorer_core::menu::{ContextMenu, Picked};
use rexplorer_core::notify::ChangeWatch;
use rexplorer_core::{Column, ShellFolder, ShellItem};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateFontIndirectW, DeleteObject, HBRUSH, HFONT, InvalidateRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
    SystemParametersInfoForDpi,
};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Error, HSTRING, PCWSTR, PWSTR, Result, w};

const START_FOLDER: &str = "shell:MyComputerFolder";

const WM_ENUM_DONE: u32 = WM_APP + 1;
const WM_SHELL_CHANGE: u32 = WM_APP + 2;
const TIMER_REFRESH: usize = 1;
const ICON_UNKNOWN: i32 = i32::MIN;

const ID_ADDR: usize = 100;
const ID_LIST: usize = 101;

const CMD_BACK: u16 = 1;
const CMD_FORWARD: u16 = 2;
const CMD_UP: u16 = 3;
const CMD_REFRESH: u16 = 4;
const CMD_ADDRESS: u16 = 5;
const CMD_EXPLORER: u16 = 6;
const CMD_ENTER: u16 = 7;
const CMD_ESCAPE: u16 = 8;
const CMD_PROPERTIES: u16 = 9;

const fn accel(flags: u8, key: VIRTUAL_KEY, cmd: u16) -> ACCEL {
    ACCEL { fVirt: ACCEL_VIRT_FLAGS(FVIRTKEY.0 | flags), key: key.0, cmd }
}

const ACCELS: [ACCEL; 11] = [
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
];

enum Target {
    Name(String),
    Pidl(Pidl),
}

#[derive(Clone, Copy, PartialEq)]
enum History {
    Push,
    Back,
    Forward,
    Refresh,
}

struct Loaded {
    folder: ShellFolder,
    items: Vec<ShellItem>,
    columns: Vec<Column>,
}

struct EnumDone {
    generation: u64,
    history: History,
    result: std::result::Result<Loaded, LoadError>,
}

struct LoadError {
    name: String,
    err: Error,
    /// The folder exists but we cannot list it (a phone, an odd namespace extension): Explorer's job.
    exists: bool,
}

struct Listing {
    folder: ShellFolder,
    items: Vec<ShellItem>,
    columns: Vec<Column>,
    icons: Vec<i32>,
    /// Shell column index, not header position.
    sort_col: u32,
    sort_desc: bool,
}

struct App {
    hwnd: HWND,
    list: HWND,
    addr: HWND,
    font: HFONT,
    listing: Option<Listing>,
    back: Vec<Pidl>,
    forward: Vec<Pidl>,
    generation: u64,
    watch: Option<ChangeWatch>,
    active_menu: Option<ContextMenu>,
    image_list_set: bool,
}

fn main() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _com = ComInit::sta();
        let icc = INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_LISTVIEW_CLASSES };
        let _ = InitCommonControlsEx(&icc);

        let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = w!("RexplorerMain");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            // COLOR_WINDOW + 1, the classic system-brush encoding.
            hbrBackground: HBRUSH((5 + 1) as usize as *mut c_void),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);

        let app = Box::into_raw(Box::new(App {
            hwnd: HWND::default(),
            list: HWND::default(),
            addr: HWND::default(),
            font: HFONT::default(),
            listing: None,
            back: Vec::new(),
            forward: Vec::new(),
            generation: 0,
            watch: None,
            active_menu: None,
            image_list_set: false,
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

        let accel_table = CreateAcceleratorTableW(&ACCELS)?;
        let start = std::env::args().nth(1).unwrap_or_else(|| START_FOLDER.to_string());
        navigate(app, Target::Name(start), History::Push);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            // Accelerators only when our own controls have focus; the ListView's label editor keeps Enter/Escape.
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

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_CREATE {
        let cs = &*(lparam.0 as *const CREATESTRUCTW);
        let app = cs.lpCreateParams as *mut App;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as isize);
        (*app).hwnd = hwnd;
        if let Err(e) = create_children(app) {
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
        WM_SIZE => layout(app),
        WM_SETFOCUS => {
            let _ = SetFocus(Some((*app).list));
        }
        WM_NOTIFY => return on_notify(app, lparam),
        WM_COMMAND => on_command(app, (wparam.0 & 0xFFFF) as u16),
        WM_APPCOMMAND => {
            // APPCOMMAND_BROWSER_BACKWARD = 1, APPCOMMAND_BROWSER_FORWARD = 2 (mouse side buttons).
            match (lparam.0 as u32 >> 16) & !0xF000 {
                1 => go_back(app),
                2 => go_forward(app),
                _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
            }
            return LRESULT(1);
        }
        WM_ENUM_DONE => on_enum_done(app, Box::from_raw(lparam.0 as *mut EnumDone)),
        WM_SHELL_CHANGE => {
            ChangeWatch::acknowledge(wparam, lparam);
            SetTimer(Some(hwnd), TIMER_REFRESH, 150, None);
        }
        WM_TIMER if wparam.0 == TIMER_REFRESH => {
            let _ = KillTimer(Some(hwnd), TIMER_REFRESH);
            refresh(app);
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
            apply_font(app);
            if let Some(l) = &(*app).listing {
                set_columns(app, &l.columns);
            }
        }
        WM_DESTROY => {
            save_placement(hwnd);
            (*app).watch = None;
            PostQuitMessage(0);
        }
        _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
    }
    LRESULT(0)
}

unsafe fn create_children(app: *mut App) -> Result<()> {
    let hwnd = (*app).hwnd;
    let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
    (*app).addr = CreateWindowExW(
        WS_EX_CLIENTEDGE,
        WC_EDITW,
        PCWSTR::null(),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        0,
        0,
        0,
        0,
        Some(hwnd),
        Some(HMENU(ID_ADDR as *mut c_void)),
        Some(hinst),
        None,
    )?;
    let list_style = LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS | LVS_EDITLABELS | LVS_SHAREIMAGELISTS;
    (*app).list = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        WC_LISTVIEWW,
        PCWSTR::null(),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(list_style),
        0,
        0,
        0,
        0,
        Some(hwnd),
        Some(HMENU(ID_LIST as *mut c_void)),
        Some(hinst),
        None,
    )?;
    let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_HEADERDRAGDROP;
    SendMessageW((*app).list, LVM_SETEXTENDEDLISTVIEWSTYLE, Some(WPARAM(0)), Some(LPARAM(ex as isize)));
    let _ = SetWindowTheme((*app).list, w!("Explorer"), PCWSTR::null());
    apply_font(app);
    Ok(())
}

unsafe fn apply_font(app: *mut App) {
    let dpi = GetDpiForWindow((*app).hwnd);
    let mut ncm = NONCLIENTMETRICSW { cbSize: size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
    if SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS.0, ncm.cbSize, Some(&mut ncm as *mut _ as *mut c_void), 0, dpi).is_err() {
        return;
    }
    let font = CreateFontIndirectW(&ncm.lfMessageFont);
    if !(*app).font.is_invalid() {
        let _ = DeleteObject((*app).font.into());
    }
    (*app).font = font;
    for h in [(*app).list, (*app).addr] {
        SendMessageW(h, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
    }
}

unsafe fn layout(app: *mut App) {
    let mut rc = RECT::default();
    let _ = GetClientRect((*app).hwnd, &mut rc);
    let dpi = GetDpiForWindow((*app).hwnd) as i32;
    let pad = 4 * dpi / 96;
    let h = 24 * dpi / 96;
    let _ = MoveWindow((*app).addr, pad, pad, rc.right - 2 * pad, h, true);
    let _ = MoveWindow((*app).list, 0, h + 2 * pad, rc.right, rc.bottom - h - 2 * pad, true);
}

// ---- navigation -------------------------------------------------------------------------------

fn navigate(app: *mut App, target: Target, history: History) {
    unsafe {
        (*app).generation += 1;
        let generation = (*app).generation;
        let hwnd = (*app).hwnd.0 as isize;
        std::thread::spawn(move || {
            let _com = ComInit::mta();
            let done = Box::new(EnumDone { generation, history, result: load(target) });
            let _ = PostMessageW(Some(HWND(hwnd as *mut c_void)), WM_ENUM_DONE, WPARAM(0), LPARAM(Box::into_raw(done) as isize));
        });
    }
}

fn load(target: Target) -> std::result::Result<Loaded, LoadError> {
    let folder = match target {
        Target::Name(n) => ShellFolder::parse(&n).map_err(|err| LoadError { name: n, err, exists: false })?,
        Target::Pidl(p) => ShellFolder::from_pidl(&p).map_err(|err| LoadError { name: String::new(), err, exists: false })?,
    };
    let mut items = folder
        .enumerate()
        .map_err(|err| LoadError { name: folder.parsing_name.clone(), err, exists: true })?;
    items.retain(|i| !i.is_hidden());
    let columns = folder.columns();
    Ok(Loaded { folder, items, columns })
}

unsafe fn on_enum_done(app: *mut App, done: Box<EnumDone>) {
    if done.generation != (*app).generation {
        return;
    }
    let hwnd = (*app).hwnd;
    let list = (*app).list;
    let loaded = match done.result {
        Err(LoadError { name, err, exists }) => {
            if !exists || open_in_explorer(&name).is_err() {
                message(hwnd, &format!("Cannot open {name}\n\n{err}"));
            }
            if let Some(l) = &(*app).listing {
                set_text((*app).addr, &l.folder.editing_name);
            }
            return;
        }
        Ok(loaded) => loaded,
    };

    let refresh = done.history == History::Refresh;
    let old = (*app).listing.take();
    let (mut sort_col, mut sort_desc) = (0, false);
    let mut keep_selected: Vec<String> = Vec::new();
    let top = SendMessageW(list, LVM_GETTOPINDEX, None, None).0 as i32;
    if let Some(old) = &old {
        match done.history {
            History::Push => {
                (*app).back.push(old.folder.pidl.clone());
                (*app).forward.clear();
            }
            History::Back => (*app).forward.push(old.folder.pidl.clone()),
            History::Forward => (*app).back.push(old.folder.pidl.clone()),
            History::Refresh => {
                sort_col = old.sort_col;
                sort_desc = old.sort_desc;
                keep_selected = selected_indices(list).iter().map(|&i| old.items[i].parsing_name()).collect();
            }
        }
    }
    drop(old);

    let mut listing = Listing {
        icons: vec![ICON_UNKNOWN; loaded.items.len()],
        folder: loaded.folder,
        items: loaded.items,
        columns: loaded.columns,
        sort_col,
        sort_desc,
    };
    sort_listing(&mut listing);

    if !(*app).image_list_set {
        if let Some(first) = listing.items.first() {
            let (himl, _) = first.icon_index(&listing.folder.pidl);
            if himl != 0 {
                SendMessageW(list, LVM_SETIMAGELIST, Some(WPARAM(LVSIL_SMALL as usize)), Some(LPARAM(himl as isize)));
                (*app).image_list_set = true;
            }
        }
    }
    if !refresh {
        set_columns(app, &listing.columns);
    }
    set_text((*app).addr, &listing.folder.editing_name);
    set_text(hwnd, &format!("{} - rexplorer", listing.folder.name));
    (*app).watch = ChangeWatch::register(hwnd, &listing.folder, WM_SHELL_CHANGE);

    let count = listing.items.len();
    // The list asks for item text synchronously from here on, so the listing must be in place first.
    (*app).listing = Some(listing);
    SendMessageW(list, LVM_SETITEMCOUNT, Some(WPARAM(count)), Some(LPARAM(LVSICF_NOSCROLL as isize)));

    if refresh {
        if !keep_selected.is_empty() {
            let l = (*app).listing.as_ref().unwrap();
            for (i, item) in l.items.iter().enumerate() {
                if keep_selected.contains(&item.parsing_name()) {
                    set_item_state(list, i, LVIS_SELECTED);
                }
            }
        }
        let per_page = SendMessageW(list, LVM_GETCOUNTPERPAGE, None, None).0 as i32;
        let last = (top + per_page - 1).min(count as i32 - 1);
        if last >= 0 {
            SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(last as usize)), Some(LPARAM(0)));
            SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(top.max(0) as usize)), Some(LPARAM(0)));
        }
    } else if count > 0 {
        SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(0)), Some(LPARAM(0)));
        set_item_state(list, 0, LVIS_FOCUSED);
    }
}

unsafe fn sort_listing(l: &mut Listing) {
    let folder = &l.folder;
    let col = l.sort_col;
    l.items.sort_by(|a, b| folder.compare(col, a, b));
    if l.sort_desc {
        l.items.reverse();
    }
    l.icons.fill(ICON_UNKNOWN);
}

unsafe fn set_columns(app: *mut App, columns: &[Column]) {
    let list = (*app).list;
    while SendMessageW(list, LVM_DELETECOLUMN, Some(WPARAM(0)), None).0 != 0 {}
    let dpi = GetDpiForWindow((*app).hwnd) as i32;
    for (i, c) in columns.iter().enumerate() {
        let mut title: Vec<u16> = c.title.encode_utf16().collect();
        title.push(0);
        let mut cx = c.width_chars.max(8) * 7 * dpi / 96;
        if i == 0 {
            cx = cx.max(280 * dpi / 96);
        }
        let col = LVCOLUMNW {
            mask: LVCF_TEXT | LVCF_WIDTH | LVCF_FMT | LVCF_SUBITEM,
            fmt: if c.right_align { LVCFMT_RIGHT } else { LVCFMT_LEFT },
            cx,
            pszText: PWSTR(title.as_mut_ptr()),
            iSubItem: i as i32,
            ..Default::default()
        };
        SendMessageW(list, LVM_INSERTCOLUMNW, Some(WPARAM(i)), Some(LPARAM(&col as *const _ as isize)));
    }
}

unsafe fn refresh(app: *mut App) {
    if let Some(l) = &(*app).listing {
        navigate(app, Target::Pidl(l.folder.pidl.clone()), History::Refresh);
    }
}

unsafe fn go_back(app: *mut App) {
    if let Some(p) = (*app).back.pop() {
        navigate(app, Target::Pidl(p), History::Back);
    }
}

unsafe fn go_forward(app: *mut App) {
    if let Some(p) = (*app).forward.pop() {
        navigate(app, Target::Pidl(p), History::Forward);
    }
}

unsafe fn go_up(app: *mut App) {
    if let Some(p) = (*app).listing.as_ref().and_then(|l| l.folder.parent_pidl()) {
        navigate(app, Target::Pidl(p), History::Push);
    }
}

/// Opens a folder in place, or runs the default verb on a file.
unsafe fn activate(app: *mut App, index: usize) {
    let hwnd = (*app).hwnd;
    let (folder_target, menu) = {
        let Some(l) = &(*app).listing else { return };
        let Some(item) = l.items.get(index) else { return };
        if item.is_folder() {
            (Some(l.folder.child_pidl(item)), None)
        } else {
            (None, ContextMenu::for_items(&l.folder, &[item], hwnd).ok())
        }
    };
    if let Some(p) = folder_target {
        navigate(app, Target::Pidl(p), History::Push);
    } else if let Some(menu) = menu {
        // The shell shows its own error UI when a file cannot be opened.
        let _ = menu.invoke_default(hwnd);
    }
}

// ---- list notifications ------------------------------------------------------------------------

unsafe fn on_notify(app: *mut App, lparam: LPARAM) -> LRESULT {
    let hdr = &*(lparam.0 as *const NMHDR);
    if hdr.hwndFrom != (*app).list {
        return LRESULT(0);
    }
    match hdr.code {
        LVN_GETDISPINFOW => {
            let di = &mut *(lparam.0 as *mut NMLVDISPINFOW);
            fill_dispinfo(app, &mut di.item);
        }
        LVN_COLUMNCLICK => {
            let nm = &*(lparam.0 as *const NMLISTVIEW);
            on_column_click(app, nm.iSubItem as usize);
        }
        LVN_ITEMACTIVATE => {
            let nm = &*(lparam.0 as *const NMITEMACTIVATE);
            if nm.iItem >= 0 {
                activate(app, nm.iItem as usize);
            }
        }
        NM_RCLICK => {
            let nm = &*(lparam.0 as *const NMITEMACTIVATE);
            show_menu(app, nm.iItem >= 0);
            return LRESULT(1);
        }
        LVN_KEYDOWN => {
            let nm = &*(lparam.0 as *const NMLVKEYDOWN);
            on_key(app, VIRTUAL_KEY(nm.wVKey));
        }
        LVN_ENDLABELEDITW => {
            let di = &*(lparam.0 as *const NMLVDISPINFOW);
            on_end_edit(app, &di.item);
        }
        LVN_ODFINDITEMW => {
            // Type-to-select: a virtual list holds no text, so it asks us. 0 would mean "item 0 matches".
            let nm = &*(lparam.0 as *const NMLVFINDITEMW);
            let Some(l) = &(*app).listing else { return LRESULT(-1) };
            if !nm.lvfi.flags.contains(LVFI_STRING) || nm.lvfi.psz.is_null() {
                return LRESULT(-1);
            }
            let needle = nm.lvfi.psz.to_string().unwrap_or_default();
            let wrap = nm.lvfi.flags.contains(LVFI_WRAP);
            let found = find_prefix(l.items.len(), nm.iStart.max(0) as usize, wrap, &needle, |i| l.items[i].display_name());
            return LRESULT(found.map_or(-1, |i| i as isize));
        }
        _ => {}
    }
    LRESULT(0)
}

fn find_prefix(n: usize, start: usize, wrap: bool, needle: &str, name: impl Fn(usize) -> String) -> Option<usize> {
    let start = start.min(n);
    let needle = needle.to_lowercase();
    (start..n).chain(if wrap { 0..start } else { 0..0 }).find(|&i| name(i).to_lowercase().starts_with(&needle))
}

unsafe fn fill_dispinfo(app: *mut App, item: &mut LVITEMW) {
    let Some(l) = &mut (*app).listing else { return };
    let i = item.iItem as usize;
    let Some(entry) = l.items.get(i) else { return };
    if item.mask.contains(LVIF_TEXT) {
        let text = if item.iSubItem == 0 {
            entry.display_name()
        } else {
            l.columns.get(item.iSubItem as usize).map(|c| entry.detail(c.index)).unwrap_or_default()
        };
        copy_text(&text, item.pszText, item.cchTextMax);
    }
    if item.mask.contains(LVIF_IMAGE) && item.iSubItem == 0 {
        if l.icons[i] == ICON_UNKNOWN {
            // shortcut: icon lookup runs on the UI thread, move it to a worker when a slow folder visibly stutters
            l.icons[i] = entry.icon_index(&l.folder.pidl).1;
        }
        item.iImage = l.icons[i];
    }
}

unsafe fn on_column_click(app: *mut App, sub: usize) {
    let list = (*app).list;
    let Some(l) = &mut (*app).listing else { return };
    let Some(col) = l.columns.get(sub) else { return };
    if col.index == l.sort_col {
        l.sort_desc = !l.sort_desc;
    } else {
        l.sort_col = col.index;
        l.sort_desc = false;
    }
    sort_listing(l);
    let _ = InvalidateRect(Some(list), None, true);
}

unsafe fn show_menu(app: *mut App, on_item: bool) {
    let hwnd = (*app).hwnd;
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let menu = {
        let Some(l) = &(*app).listing else { return };
        if on_item {
            let sel = selected_indices((*app).list);
            let items: Vec<&ShellItem> = sel.iter().filter_map(|&i| l.items.get(i)).collect();
            if items.is_empty() {
                return;
            }
            ContextMenu::for_items(&l.folder, &items, hwnd)
        } else {
            ContextMenu::for_background(&l.folder, hwnd)
        }
    };
    let Ok(menu) = menu else { return };
    let extended = GetKeyState(VK_SHIFT.0 as i32) < 0;
    (*app).active_menu = Some(menu);
    let picked = (*app).active_menu.as_ref().unwrap().show(hwnd, pt.x, pt.y, extended);
    (*app).active_menu = None;
    if let Ok(Picked::Rename) = picked {
        begin_rename(app);
    }
}

unsafe fn on_key(app: *mut App, key: VIRTUAL_KEY) {
    let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
    let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
    match key {
        VK_BACK => go_up(app),
        VK_F2 => begin_rename(app),
        VK_DELETE => verb_on_selection(app, "delete", shift),
        VK_C if ctrl => verb_on_selection(app, "copy", false),
        VK_X if ctrl => verb_on_selection(app, "cut", false),
        VK_V if ctrl => verb_on_background(app, "paste"),
        VK_A if ctrl => set_item_state((*app).list, usize::MAX, LVIS_SELECTED),
        _ => {}
    }
}

unsafe fn verb_on_selection(app: *mut App, verb: &str, shift: bool) {
    let hwnd = (*app).hwnd;
    let menu = {
        let Some(l) = &(*app).listing else { return };
        let sel = selected_indices((*app).list);
        let items: Vec<&ShellItem> = sel.iter().filter_map(|&i| l.items.get(i)).collect();
        if items.is_empty() {
            return;
        }
        ContextMenu::for_items(&l.folder, &items, hwnd)
    };
    if let Ok(menu) = menu {
        let _ = menu.invoke_verb(hwnd, verb, shift);
    }
}

unsafe fn verb_on_background(app: *mut App, verb: &str) {
    let hwnd = (*app).hwnd;
    let menu = {
        let Some(l) = &(*app).listing else { return };
        ContextMenu::for_background(&l.folder, hwnd)
    };
    if let Ok(menu) = menu {
        let _ = menu.invoke_verb(hwnd, verb, false);
    }
}

unsafe fn begin_rename(app: *mut App) {
    let list = (*app).list;
    let i = SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_FOCUSED as isize))).0;
    if i >= 0 {
        let _ = SetFocus(Some(list));
        SendMessageW(list, LVM_EDITLABELW, Some(WPARAM(i as usize)), None);
    }
}

unsafe fn on_end_edit(app: *mut App, item: &LVITEMW) {
    if item.pszText.is_null() {
        return;
    }
    let name = item.pszText.to_string().unwrap_or_default();
    // Work on a copy: the shell may pump messages while renaming, and a refresh could replace the listing.
    let target = (*app).listing.as_ref().and_then(|l| l.items.get(item.iItem as usize).cloned());
    if let Some(mut target) = target {
        if !name.is_empty() && name != target.display_name() {
            let _ = target.rename((*app).hwnd, &name);
            SetTimer(Some((*app).hwnd), TIMER_REFRESH, 150, None);
        }
    }
}

// ---- commands ----------------------------------------------------------------------------------

unsafe fn on_command(app: *mut App, cmd: u16) {
    let hwnd = (*app).hwnd;
    let addr = (*app).addr;
    let list = (*app).list;
    match cmd {
        CMD_BACK => go_back(app),
        CMD_FORWARD => go_forward(app),
        CMD_UP => go_up(app),
        CMD_REFRESH => refresh(app),
        CMD_ADDRESS => {
            let _ = SetFocus(Some(addr));
            SendMessageW(addr, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        }
        CMD_EXPLORER => {
            if let Some(l) = &(*app).listing {
                let _ = open_in_explorer(&l.folder.parsing_name);
            }
        }
        CMD_ENTER => {
            if GetFocus() == addr {
                let text = window_text(addr);
                let current = (*app).listing.as_ref().map(|l| l.folder.editing_name.clone()).unwrap_or_default();
                if text.trim().is_empty() || text == current {
                    refresh(app);
                } else {
                    navigate(app, Target::Name(text.trim().to_string()), History::Push);
                }
                let _ = SetFocus(Some(list));
            } else {
                let i = SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_FOCUSED as isize))).0;
                if i >= 0 {
                    activate(app, i as usize);
                }
            }
        }
        CMD_ESCAPE => {
            if GetFocus() == addr {
                if let Some(l) = &(*app).listing {
                    set_text(addr, &l.folder.editing_name);
                }
                let _ = SetFocus(Some(list));
            }
        }
        CMD_PROPERTIES => {
            if selected_indices(list).is_empty() {
                verb_on_background(app, "properties");
            } else {
                verb_on_selection(app, "properties", false);
            }
        }
        _ => {
            let _ = hwnd;
        }
    }
}

// ---- small helpers -----------------------------------------------------------------------------

unsafe fn selected_indices(list: HWND) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i: isize = -1;
    loop {
        i = SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(i as usize)), Some(LPARAM(LVNI_SELECTED as isize))).0;
        if i < 0 {
            break;
        }
        out.push(i as usize);
    }
    out
}

/// `index == usize::MAX` applies to every item.
unsafe fn set_item_state(list: HWND, index: usize, state: LIST_VIEW_ITEM_STATE_FLAGS) {
    let lvi = LVITEMW { state, stateMask: state, ..Default::default() };
    SendMessageW(list, LVM_SETITEMSTATE, Some(WPARAM(index)), Some(LPARAM(&lvi as *const _ as isize)));
}

unsafe fn copy_text(text: &str, dst: PWSTR, max: i32) {
    if dst.is_null() || max <= 0 {
        return;
    }
    let buf = std::slice::from_raw_parts_mut(dst.0, max as usize);
    let mut n = 0;
    for (d, c) in buf[..max as usize - 1].iter_mut().zip(text.encode_utf16()) {
        *d = c;
        n += 1;
    }
    buf[n] = 0;
}

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

#[cfg(test)]
mod tests {
    use super::find_prefix;

    #[test]
    fn type_to_select_searches_forward_then_wraps() {
        let names = ["sub", "Alpha", "beta", "c"];
        let name = |i: usize| names[i].to_string();
        assert_eq!(find_prefix(4, 2, true, "a", name), Some(1));
        assert_eq!(find_prefix(4, 2, false, "a", name), None);
        assert_eq!(find_prefix(4, 0, false, "BE", name), Some(2));
        assert_eq!(find_prefix(4, 4, true, "s", name), Some(0));
        assert_eq!(find_prefix(0, 0, true, "s", name), None);
    }
}
