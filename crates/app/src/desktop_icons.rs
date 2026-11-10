//! `rexplorer --desktop-icons`: the desktop's icons in a window of rexplorer's own (opt-in, primary monitor).
//!
//! A bottom top-level window over the primary work area sits directly above Explorer's desktop host and is never
//! made a child of the shell's windows, so a hang here cannot freeze the real desktop and the window dies with this
//! process. It hosts a plain (not owner-data) icon-view ListView, because only that control takes free item
//! positions. Items are the desktop root's children as the shell merges them (user and public Desktop, namespace
//! items); each sits on a grid cell, and the cells are kept in desktop-icons.txt next to the session file.
//! Everything shell-related (menus, rename, delete, drop) goes through the same shell calls the file list uses.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::mem::size_of;
use std::path::PathBuf;

use rexplorer_core::com::ComInit;
use rexplorer_core::dragdrop;
use rexplorer_core::folder::ShellFolder;
use rexplorer_core::i18n::tr;
use rexplorer_core::import_explorer;
use rexplorer_core::item::{ShellItem, split_icon};
use rexplorer_core::menu::{ContextMenu, Picked};
use rexplorer_core::notify::ChangeWatch;
use rexplorer_core::settings::Settings;
use rexplorer_core::undo;
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, POINTL, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, DeleteObject, FillRect, GetMonitorInfoW, GetSysColorBrush, HDC, HFONT, InvalidateRect, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
    PaintDesktop, ScreenToClient, CreateFontIndirectW, LOGFONTW, COLOR_DESKTOP, COLOR_WINDOWTEXT,
};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::IDropTarget;
use windows::Win32::System::SystemServices::SFGAO_CANRENAME;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext, SystemParametersInfoForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, SetFocus, VIRTUAL_KEY, VK_CONTROL, VK_DELETE, VK_F2, VK_F5, VK_SHIFT};
use windows::Win32::UI::Shell::{DefSubclassProc, SHGetImageList, SetWindowSubclass, SHIL_EXTRALARGE, FOLDERID_Desktop, FOLDERID_PublicDesktop};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, Result, w};

use crate::ui::{self, Entry};

/// The window's class; install closes a running view by it.
const CLASS: PCWSTR = w!("RexDesktopIcons");
const WM_DESK_CHANGE: u32 = WM_APP + 0x70;
const TIMER_RELOAD: usize = 1;
const BG_ID: usize = 0xD35;

// Own entries of the background menu (`ui::build_menu` offsets them past the shell's).
const REFRESH: usize = 1;
const PASTE: usize = 2;
const ARRANGE_NAME: usize = 3;
const CLOSE: usize = 4;

// SystemParametersInfo codes WM_SETTINGCHANGE carries (named by what changed, not by the setter).
const WORKAREA_CHANGED: usize = 0x2F;
const WALLPAPER_CHANGED: usize = 0x14;

/// A grid cell: (column, row).
type Pos = (u32, u32);

// ---- pure layout and file logic ---------------------------------------------------------------------------------

/// Columns and rows that fit `area` at `spacing` (one cell is the ListView's item spacing); at least 1x1.
fn grid_dims(area: RECT, spacing: (i32, i32)) -> (u32, u32) {
    let (cx, cy) = (spacing.0.max(1), spacing.1.max(1));
    (((area.right - area.left) / cx).max(1) as u32, ((area.bottom - area.top) / cy).max(1) as u32)
}

fn cell_origin(cell: Pos, area: RECT, spacing: (i32, i32)) -> POINT {
    POINT { x: area.left + cell.0 as i32 * spacing.0, y: area.top + cell.1 as i32 * spacing.1 }
}

/// The cell nearest to `pt`, clamped into the grid.
fn snap(pt: POINT, area: RECT, spacing: (i32, i32), dims: (u32, u32)) -> Pos {
    let axis = |v: i32, origin: i32, step: i32, n: u32| {
        let step = step.max(1);
        (v - origin + step / 2).div_euclid(step).clamp(0, n as i32 - 1) as u32
    };
    (axis(pt.x, area.left, spacing.0, dims.0), axis(pt.y, area.top, spacing.1, dims.1))
}

/// First free cell, column by column from the top left; columns are unbounded (an overfull desktop continues past
/// the right edge and the list scrolls).
fn first_free(taken: &HashSet<Pos>, rows: u32) -> Pos {
    let rows = rows.max(1);
    (0u32..).flat_map(|c| (0..rows).map(move |r| (c, r))).find(|p| !taken.contains(p)).unwrap_or((0, 0))
}

/// A cell for every key: a saved one inside the grid and not yet taken is kept, the rest fill the gaps in key order.
fn arrange(keys: &[String], saved: &HashMap<String, Pos>, dims: (u32, u32)) -> Vec<Pos> {
    let mut taken = HashSet::new();
    let mut cells: Vec<Option<Pos>> = keys
        .iter()
        .map(|k| {
            let c = saved.get(k).copied().filter(|c| c.0 < dims.0 && c.1 < dims.1 && !taken.contains(c))?;
            taken.insert(c);
            Some(c)
        })
        .collect();
    for c in cells.iter_mut().filter(|c| c.is_none()) {
        let f = first_free(&taken, dims.1);
        taken.insert(f);
        *c = Some(f);
    }
    cells.into_iter().flatten().collect()
}

/// `moving` items shift by `delta` (clamped into the grid); a target held by an unmoved item or an earlier mover
/// gives the first free cell instead. The movers' old cells count as free.
fn move_cells(cells: &[Pos], moving: &[usize], delta: (i32, i32), dims: (u32, u32)) -> Vec<Pos> {
    let mut taken: HashSet<Pos> = cells.iter().enumerate().filter(|(i, _)| !moving.contains(i)).map(|(_, c)| *c).collect();
    let mut out = cells.to_vec();
    for &i in moving {
        let Some(&(c, r)) = cells.get(i) else { continue };
        let to = (
            (c as i64 + delta.0 as i64).clamp(0, dims.0 as i64 - 1) as u32,
            (r as i64 + delta.1 as i64).clamp(0, dims.1 as i64 - 1) as u32,
        );
        let to = if taken.contains(&to) { first_free(&taken, dims.1) } else { to };
        taken.insert(to);
        out[i] = to;
    }
    out
}

const HEADER: &str = "# rexplorer desktop icons";

fn format_positions(items: &[(String, Pos)]) -> String {
    let mut s = format!("{HEADER}\n");
    for (key, (c, r)) in items {
        s.push_str(&format!("{c}\t{r}\t{key}\n"));
    }
    s
}

/// Lines `col<TAB>row<TAB>key`; anything else is skipped, the first entry of a key wins, keys are lowercased.
fn parse_positions(text: &str) -> HashMap<String, Pos> {
    let mut out = HashMap::new();
    if text.len() > 1_000_000 {
        return out;
    }
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with('#') {
            continue;
        }
        let mut f = line.splitn(3, '\t');
        let (Some(c), Some(r), Some(key)) = (f.next(), f.next(), f.next()) else { continue };
        let num = |s: &str| s.parse::<u32>().ok().filter(|n| *n <= 10_000);
        let (Some(c), Some(r)) = (num(c), num(r)) else { continue };
        if key.is_empty() || key.chars().any(char::is_control) {
            continue;
        }
        out.entry(key.to_lowercase()).or_insert((c, r));
    }
    out
}

/// Whether the desktop shows an item. `hide_value` is the DWORD under HideDesktopIcons\NewStartPanel named for the
/// item's CLSID (1 hides, 0 shows); None when absent.
fn shows_on_desktop(parsing: &str, hidden: bool, show_hidden: bool, hide_value: Option<u32>) -> bool {
    let name = parsing.rsplit('\\').next().unwrap_or(parsing);
    if name.eq_ignore_ascii_case("desktop.ini") || (hidden && !show_hidden) {
        return false;
    }
    let Some(clsid) = parsing.strip_prefix("::") else { return true };
    match clsid.to_ascii_lowercase().as_str() {
        "{645ff040-5081-101b-9f08-00aa002f954e}" => hide_value != Some(1),
        "{20d04fe0-3aea-1069-a2d8-08002b30309d}"
        | "{59031a47-3f72-44a7-89c5-5595fe6b30ee}"
        | "{f02c1a0d-be21-4350-88b0-7367fc96ef3c}"
        | "{5399e694-6ce5-4d6c-8fce-1d8870fdcba0}" => hide_value == Some(0),
        _ => false,
    }
}

fn hide_value(parsing: &str) -> Option<u32> {
    let clsid = parsing.strip_prefix("::")?;
    import_explorer::dword(r"Software\Microsoft\Windows\CurrentVersion\Explorer\HideDesktopIcons\NewStartPanel", clsid)
}

fn positions_path() -> PathBuf {
    crate::session::path().with_file_name("desktop-icons.txt")
}

// ---- state --------------------------------------------------------------------------------------------------------

struct Desk {
    list: HWND,
    items: Vec<ShellItem>,
    keys: Vec<String>,
    cells: Vec<Pos>,
    /// Held for their Drop, which deregisters them.
    _watches: Vec<ChangeWatch>,
    last_saved: String,
}

/// An icon drag in progress: where it began (list client), the item under it and the dragged items.
#[derive(Clone)]
struct DragStart {
    start: POINT,
    anchor: usize,
    moving: Vec<usize>,
}

thread_local! {
    static STATE: RefCell<Option<Desk>> = const { RefCell::new(None) };
    /// The shell menu being shown: its owner-drawn and submenu messages are forwarded to it.
    static ACTIVE: RefCell<Option<ContextMenu>> = const { RefCell::new(None) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
    /// True only while `place` moves the window; every other z-order change is dropped.
    static PLACING: Cell<bool> = const { Cell::new(false) };
    static DRAG: RefCell<Option<DragStart>> = const { RefCell::new(None) };
    static FONT: Cell<isize> = const { Cell::new(0) };
    /// Ctrl+Z / Ctrl+Y here: this process has no file list, so it keeps its own log.
    static UNDO: RefCell<undo::Log> = RefCell::new(undo::Log::default());
}

/// Runs `f` on the state; None while it is borrowed or not set up yet. Never hold it across a call that can pump
/// messages (menus, invoke, drag, rename, message boxes): clone what is needed, release, then call.
fn desk<R>(f: impl FnOnce(&mut Desk) -> R) -> Option<R> {
    STATE.with(|s| s.try_borrow_mut().ok().and_then(|mut s| s.as_mut().map(f)))
}

/// The list's client area, its item spacing and the resulting grid.
unsafe fn metrics(list: HWND) -> (RECT, (i32, i32), (u32, u32)) {
    let mut area = RECT::default();
    let _ = GetClientRect(list, &mut area);
    let s = SendMessageW(list, LVM_GETITEMSPACING, Some(WPARAM(0)), None).0 as u32;
    let spacing = ((s & 0xFFFF) as i32, (s >> 16) as i32);
    (area, spacing, grid_dims(area, spacing))
}

unsafe fn put_cells(list: HWND, cells: &[Pos]) {
    let (area, spacing, _) = metrics(list);
    for (i, c) in cells.iter().enumerate() {
        let pt = cell_origin(*c, area, spacing);
        SendMessageW(list, LVM_SETITEMPOSITION32, Some(WPARAM(i)), Some(LPARAM(&pt as *const _ as isize)));
    }
}

/// Writes the positions through a temp file when they changed since the last write.
fn save() {
    let Some((keys, cells, last)) = desk(|d| (d.keys.clone(), d.cells.clone(), d.last_saved.clone())) else { return };
    let text = format_positions(&keys.into_iter().zip(cells).collect::<Vec<_>>());
    // No items (the shell not ready yet) never wipes the saved layout; stale lines are harmless.
    if text == last || text == format_positions(&[]) {
        return;
    }
    let path = positions_path();
    let tmp = path.with_extension("txt.tmp");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::write(&tmp, &text).and_then(|_| std::fs::rename(&tmp, &path)).is_ok() {
        desk(|d| d.last_saved = text);
    }
}

fn load_saved() -> HashMap<String, Pos> {
    let path = positions_path();
    match std::fs::metadata(&path) {
        Ok(m) if m.len() <= 1_000_000 => std::fs::read_to_string(&path).map(|t| parse_positions(&t)).unwrap_or_default(),
        _ => HashMap::new(),
    }
}

// ---- window -------------------------------------------------------------------------------------------------------

pub(crate) fn run() -> Result<()> {
    unsafe {
        let _once = CreateMutexW(None, true, w!("Local\\rexplorer-desktop-icons"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _com = ComInit::ole();
        let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_LISTVIEW_CLASSES });
        crate::folder_options::sync(Settings::load().active_profile());
        let hinst = GetModuleHandleW(None)?.into();
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst, lpszClassName: CLASS, ..Default::default() });
        let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, CLASS, &windows::core::HSTRING::from(tr("desk.icons_name")), WS_POPUP | WS_CLIPCHILDREN, 0, 0, 0, 0, None, None, Some(hinst), None)?;
        let style = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(LVS_ICON | LVS_EDITLABELS | LVS_SHOWSELALWAYS | LVS_SHAREIMAGELISTS);
        let list = CreateWindowExW(WINDOW_EX_STYLE(0), WC_LISTVIEWW, PCWSTR::null(), style, 0, 0, 0, 0, Some(hwnd), None, Some(hinst), None)?;
        let _ = windows::Win32::UI::Controls::SetWindowTheme(list, w!("Explorer"), PCWSTR::null());
        SendMessageW(list, LVM_SETEXTENDEDLISTVIEWSTYLE, Some(WPARAM(LVS_EX_TRANSPARENTSHADOWTEXT as usize)), Some(LPARAM(LVS_EX_TRANSPARENTSHADOWTEXT as isize)));
        SendMessageW(list, LVM_SETBKCOLOR, None, Some(LPARAM(CLR_NONE as isize)));
        SendMessageW(list, LVM_SETTEXTBKCOLOR, None, Some(LPARAM(CLR_NONE as isize)));
        set_text_color(list);
        // The shell's shared 48 px system image list: the control never destroys it (LVS_SHAREIMAGELISTS), and its
        // reference is kept for the life of the process.
        if let Ok(himl) = SHGetImageList::<IImageList>(SHIL_EXTRALARGE as i32) {
            SendMessageW(list, LVM_SETIMAGELIST, Some(WPARAM(LVSIL_NORMAL as usize)), Some(LPARAM(windows::core::Interface::as_raw(&himl) as isize)));
            std::mem::forget(himl);
        }
        set_font(hwnd, list);
        let _ = SetWindowSubclass(list, Some(list_proc), BG_ID, 0);
        let mut watches = Vec::new();
        let roots = [ShellFolder::desktop().ok(), known(&FOLDERID_Desktop), known(&FOLDERID_PublicDesktop)];
        for f in roots.iter().flatten() {
            watches.extend(ChangeWatch::register(hwnd, f, WM_DESK_CHANGE));
        }
        STATE.with(|s| *s.borrow_mut() = Some(Desk { list, items: Vec::new(), keys: Vec::new(), cells: Vec::new(), _watches: watches, last_saved: String::new() }));
        place(hwnd);
        reload(hwnd);
        register_drop(list);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

fn known(id: &windows::core::GUID) -> Option<ShellFolder> {
    ShellFolder::parse(&rexplorer_core::install::known_folder(id).ok()?.display().to_string()).ok()
}

unsafe fn set_text_color(list: HWND) {
    let color = if crate::theme::high_contrast_on() { crate::theme::sys_color(COLOR_WINDOWTEXT) } else { 0x00FF_FFFF };
    SendMessageW(list, LVM_SETTEXTCOLOR, None, Some(LPARAM(color as isize)));
}

/// The icon-title font at the window's DPI (read only).
unsafe fn set_font(hwnd: HWND, list: HWND) {
    let mut lf = LOGFONTW::default();
    let ok = SystemParametersInfoForDpi(SPI_GETICONTITLELOGFONT.0, size_of::<LOGFONTW>() as u32, Some(&mut lf as *mut _ as *mut c_void), 0, GetDpiForWindow(hwnd)).is_ok();
    if !ok {
        return;
    }
    let font = CreateFontIndirectW(&lf);
    SendMessageW(list, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
    let old = FONT.replace(font.0 as isize);
    if old != 0 {
        let _ = DeleteObject(HFONT(old as *mut _).into());
    }
}

/// The wallpaper, as the desktop paints it, behind the transparent list; the desktop color if that fails.
unsafe extern "system" fn list_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    if msg == WM_ERASEBKGND {
        let dc = HDC(wparam.0 as *mut c_void);
        if !PaintDesktop(dc).as_bool() {
            let mut r = RECT::default();
            let _ = GetClientRect(hwnd, &mut r);
            FillRect(dc, &r, GetSysColorBrush(COLOR_DESKTOP));
        }
        return LRESULT(1);
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

/// The top-level Progman / WorkerW that holds the desktop's icon view (read only lookups).
unsafe fn desktop_host() -> Option<HWND> {
    unsafe extern "system" fn each(h: HWND, found: LPARAM) -> windows::core::BOOL {
        let mut buf = [0u16; 32];
        let n = GetClassNameW(h, &mut buf).max(0) as usize;
        let class = String::from_utf16_lossy(&buf[..n]);
        if matches!(class.as_str(), "Progman" | "WorkerW") && FindWindowExW(Some(h), None, w!("SHELLDLL_DefView"), PCWSTR::null()).is_ok() {
            *(found.0 as *mut Option<HWND>) = Some(h);
            return false.into();
        }
        true.into()
    }
    let mut found: Option<HWND> = None;
    let _ = EnumWindows(Some(each), LPARAM(&mut found as *mut _ as isize));
    found
}

/// The primary work area; one pixel short of the monitor when they are the same (auto-hide taskbar), so the shell
/// never takes the focused view for a full-screen app.
fn view_rect(work: RECT, monitor: RECT) -> RECT {
    if work == monitor { RECT { bottom: work.bottom - 1, ..work } } else { work }
}

/// Where `place` puts the view in the z-order: Some(after) for SetWindowPos, None to leave it. `prev` is the window
/// right above the desktop host and whether it is topmost.
fn insert_after(prev: Option<(HWND, bool)>, ours: HWND, shown: bool) -> Option<HWND> {
    match prev {
        Some((p, _)) if p == ours => None,
        Some((p, false)) => Some(p),
        // After a topmost window the view would turn topmost (Show desktop can raise the host to the top). Without a
        // usable host a shown view keeps its place and a new one goes to the very bottom, never over the user's apps.
        _ if shown => None,
        _ => Some(HWND_BOTTOM),
    }
}

/// Covers the primary work area, directly above the desktop host and below every other window. The only place the
/// z-order is set; WM_WINDOWPOSCHANGING drops every other change.
unsafe fn place(hwnd: HWND) {
    let Some(list) = desk(|d| d.list) else { return };
    let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    if !GetMonitorInfoW(mon, &mut mi).as_bool() {
        return;
    }
    let r = view_rect(mi.rcWork, mi.rcMonitor);
    let prev = desktop_host()
        .and_then(|host| GetWindow(host, GW_HWNDPREV).ok())
        .map(|p| (p, GetWindowLongW(p, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0));
    let (after, flags) = match insert_after(prev, hwnd, IsWindowVisible(hwnd).as_bool()) {
        Some(a) => (Some(a), SWP_NOACTIVATE),
        None => (None, SWP_NOACTIVATE | SWP_NOZORDER),
    };
    PLACING.set(true);
    let _ = SetWindowPos(hwnd, after, r.left, r.top, r.right - r.left, r.bottom - r.top, flags);
    PLACING.set(false);
    let _ = MoveWindow(list, 0, 0, r.right - r.left, r.bottom - r.top, true);
}

// ---- items --------------------------------------------------------------------------------------------------------

unsafe fn selected(list: HWND) -> Vec<usize> {
    crate::list::selected_indices(list)
}

unsafe fn editing(list: HWND) -> bool {
    SendMessageW(list, LVM_GETEDITCONTROL, None, None).0 != 0
}

unsafe fn reload(hwnd: HWND) {
    let Some(list) = desk(|d| d.list) else { return };
    // A label edit, a drag or a menu is on: its item indexes must not move under it.
    if BUSY.get() || DRAG.with(|d| d.borrow().is_some()) || editing(list) {
        SetTimer(Some(hwnd), TIMER_RELOAD, 500, None);
        return;
    }
    let Ok(root) = ShellFolder::desktop() else { return };
    let show_hidden = Settings::load().active_profile().show_hidden;
    // A failed enumeration changes nothing: an empty list would overwrite the saved positions.
    let Ok(all) = root.enumerate() else { return };
    let mut items: Vec<ShellItem> = all
        .into_iter()
        .filter(|i| {
            let p = i.parsing_name();
            shows_on_desktop(&p, i.is_hidden(), show_hidden, hide_value(&p))
        })
        .collect();
    items.sort_by(|a, b| root.compare(0, a, b));
    let keys: Vec<String> = items.iter().map(|i| i.parsing_name().to_lowercase()).collect();
    let (_, _, dims) = metrics(list);
    // Known items keep their cells; the first load reads the file.
    let (old_keys, old_cells) = desk(|d| (d.keys.clone(), d.cells.clone())).unwrap_or_default();
    let saved: HashMap<String, Pos> = if old_keys.is_empty() { load_saved() } else { old_keys.iter().cloned().zip(old_cells).collect() };
    let cells = arrange(&keys, &saved, dims);
    let keep = |index: isize| usize::try_from(index).ok().and_then(|i| old_keys.get(i).cloned());
    let focus = keep(SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_FOCUSED as isize))).0);
    let chosen: HashSet<String> = selected(list).into_iter().filter_map(|i| old_keys.get(i).cloned()).collect();

    SendMessageW(list, WM_SETREDRAW, Some(WPARAM(0)), None);
    SendMessageW(list, LVM_DELETEALLITEMS, None, None);
    for (i, item) in items.iter().enumerate() {
        let (icon, overlay) = split_icon(item.icon_index(&root.pidl).1);
        let mut text: Vec<u16> = crate::source::name_cell(item).0.encode_utf16().chain(std::iter::once(0)).collect();
        let lv = LVITEMW {
            mask: LVIF_TEXT | LVIF_IMAGE | LVIF_STATE,
            iItem: i as i32,
            pszText: windows::core::PWSTR(text.as_mut_ptr()),
            iImage: icon,
            state: LIST_VIEW_ITEM_STATE_FLAGS(overlay << 8),
            stateMask: LVIS_OVERLAYMASK,
            ..Default::default()
        };
        SendMessageW(list, LVM_INSERTITEMW, None, Some(LPARAM(&lv as *const _ as isize)));
    }
    put_cells(list, &cells);
    for (i, k) in keys.iter().enumerate() {
        if chosen.contains(k) {
            crate::list::set_item_state(list, i, LVIS_SELECTED, LVIS_SELECTED);
        }
        if focus.as_ref() == Some(k) {
            crate::list::set_item_state(list, i, LVIS_FOCUSED, LVIS_FOCUSED);
        }
    }
    SendMessageW(list, WM_SETREDRAW, Some(WPARAM(1)), None);
    let _ = InvalidateRect(Some(list), None, true);
    desk(|d| {
        d.items = items;
        d.keys = keys;
        d.cells = cells;
    });
    save();
}

/// The selected items, cloned so no state borrow is held while the shell works on them.
unsafe fn selected_items() -> Vec<ShellItem> {
    let Some(list) = desk(|d| d.list) else { return Vec::new() };
    let sel = selected(list);
    desk(|d| sel.iter().filter_map(|&i| d.items.get(i).cloned()).collect()).unwrap_or_default()
}

unsafe fn verb_on_selection(hwnd: HWND, verb: &str, shift: bool) {
    let items = selected_items();
    if items.is_empty() {
        return;
    }
    let Ok(root) = ShellFolder::desktop() else { return };
    let refs: Vec<&ShellItem> = items.iter().collect();
    if let Ok(menu) = ContextMenu::for_items(&root, &refs, hwnd) {
        let _ = menu.invoke_verb(hwnd, verb, shift);
    }
}

/// Files go to the Recycle Bin through IFileOperation (the shell's own confirmation applies); Shift+Delete and
/// namespace items keep the shell's verb.
unsafe fn delete_selection(hwnd: HWND, shift: bool) {
    let items = selected_items();
    let paths: Vec<String> = items.iter().map(|i| i.parsing_name()).filter(|p| ui::is_fs_path(p)).collect();
    if !shift && !items.is_empty() && paths.len() == items.len() {
        let done = undo::recycle(hwnd, &paths, false);
        UNDO.with(|u| u.borrow_mut().record(undo::Op::Recycled(done)));
    } else {
        verb_on_selection(hwnd, "delete", shift);
    }
}

unsafe fn paste(hwnd: HWND) {
    let Ok(root) = ShellFolder::desktop() else { return };
    if let Ok(menu) = ContextMenu::for_background(&root, hwnd) {
        let _ = menu.invoke_verb(hwnd, "paste", false);
    }
}

unsafe fn begin_rename(list: HWND) {
    let i = SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_FOCUSED as isize))).0;
    if i >= 0 {
        SendMessageW(list, LVM_EDITLABELW, Some(WPARAM(i as usize)), None);
    }
}

unsafe fn arrange_by_name(list: HWND) {
    let Some(keys) = desk(|d| d.keys.clone()) else { return };
    let (_, _, dims) = metrics(list);
    let cells = arrange(&keys, &HashMap::new(), dims);
    put_cells(list, &cells);
    desk(|d| d.cells = cells);
    save();
}

// ---- notifications ------------------------------------------------------------------------------------------------

unsafe fn on_notify(hwnd: HWND, lparam: LPARAM) -> LRESULT {
    let hdr = &*(lparam.0 as *const NMHDR);
    let Some(list) = desk(|d| d.list) else { return LRESULT(0) };
    if hdr.hwndFrom != list {
        return LRESULT(0);
    }
    match hdr.code {
        LVN_ITEMACTIVATE => {
            let items = selected_items();
            let refs: Vec<&ShellItem> = items.iter().collect();
            if let (false, Ok(root)) = (refs.is_empty(), ShellFolder::desktop()) {
                // Folders open in the default file manager: the view never navigates.
                if let Ok(menu) = ContextMenu::for_items(&root, &refs, hwnd) {
                    let _ = menu.invoke_default(hwnd);
                }
            }
        }
        LVN_KEYDOWN => on_key(hwnd, list, VIRTUAL_KEY((*(lparam.0 as *const NMLVKEYDOWN)).wVKey)),
        LVN_BEGINLABELEDITW => {
            let i = (*(lparam.0 as *const NMLVDISPINFOW)).item.iItem;
            let item = desk(|d| usize::try_from(i).ok().and_then(|i| d.items.get(i).cloned())).flatten();
            let Some(item) = item.filter(|it| it.attributes(SFGAO_CANRENAME.0) & SFGAO_CANRENAME.0 != 0) else { return LRESULT(1) };
            if let Some(n) = crate::source::rename_stem(&item) {
                crate::list::select_stem_later(list, n);
            }
        }
        LVN_ENDLABELEDITW => {
            let di = &*(lparam.0 as *const NMLVDISPINFOW);
            if !di.item.pszText.is_null() {
                end_edit(hwnd, di.item.iItem, &di.item.pszText.to_string().unwrap_or_default());
            }
        }
        LVN_BEGINDRAG | LVN_BEGINRDRAG => begin_drag(list, &*(lparam.0 as *const NMLISTVIEW)),
        _ => {}
    }
    LRESULT(0)
}

unsafe fn on_key(hwnd: HWND, list: HWND, vk: VIRTUAL_KEY) {
    let held = |k: VIRTUAL_KEY| GetKeyState(k.0 as i32) < 0;
    let (ctrl, shift) = (held(VK_CONTROL), held(VK_SHIFT));
    match vk {
        VK_F2 => begin_rename(list),
        VK_DELETE => delete_selection(hwnd, shift),
        VK_F5 => {
            place(hwnd);
            reload(hwnd);
        }
        _ if ctrl && vk.0 == b'A' as u16 => crate::list::set_item_state(list, usize::MAX, LVIS_SELECTED, LVIS_SELECTED),
        _ if ctrl && vk.0 == b'C' as u16 => verb_on_selection(hwnd, "copy", false),
        _ if ctrl && vk.0 == b'X' as u16 => verb_on_selection(hwnd, "cut", false),
        _ if ctrl && vk.0 == b'V' as u16 => paste(hwnd),
        _ if ctrl && (vk.0 == b'Z' as u16 || vk.0 == b'Y' as u16) => undo_step(hwnd, vk.0 == b'Y' as u16),
        _ => {}
    }
}

/// Reverses the newest entry, as the file list's Ctrl+Z / Ctrl+Y do.
unsafe fn undo_step(hwnd: HWND, redo: bool) {
    let Some(entry) = UNDO.with(|u| u.borrow_mut().take(redo)) else {
        let _ = windows::Win32::System::Diagnostics::Debug::MessageBeep(MB_OK);
        return;
    };
    // No borrow of the log across the shell's UI, which pumps messages.
    let done = undo::reverse(hwnd, entry);
    UNDO.with(|u| u.borrow_mut().put(redo, done));
    SetTimer(Some(hwnd), TIMER_RELOAD, 150, None);
}

/// The edit box's text becomes the item's name, by the same rule as the file list (`name_cell`'s extension suffix);
/// the shell asks before replacing anything and shows its own error for a clash.
unsafe fn end_edit(hwnd: HWND, index: i32, name: &str) {
    let Ok(i) = usize::try_from(index) else { return };
    let Some(mut target) = desk(|d| d.items.get(i).cloned()).flatten() else { return };
    let (shown, suffix) = crate::source::name_cell(&target);
    if name.is_empty() || name == shown {
        return;
    }
    let before = target.parsing_name();
    // The shell's rename UI pumps messages; no reload may renumber the items under index `i` meanwhile.
    let was_busy = BUSY.replace(true);
    let done = match suffix {
        Some(ext) => target.rename(hwnd, &format!("{name}{ext}"), true),
        None => target.rename(hwnd, name, false),
    };
    BUSY.set(was_busy);
    if done.is_ok() {
        UNDO.with(|u| u.borrow_mut().record(undo::Op::Rename { from: before, to: target.parsing_name() }));
        // The renamed item keeps its cell: the reload finds it under its new key.
        let new_key = target.parsing_name().to_lowercase();
        desk(|d| {
            if let Some(k) = d.keys.get_mut(i) {
                *k = new_key;
            }
        });
    }
    SetTimer(Some(hwnd), TIMER_RELOAD, 150, None);
}

// ---- drag and drop ------------------------------------------------------------------------------------------------

unsafe fn begin_drag(list: HWND, nm: &NMLISTVIEW) {
    let moving = selected(list);
    let (Ok(anchor), false) = (usize::try_from(nm.iItem), moving.is_empty()) else { return };
    let items = selected_items();
    let Ok(root) = ShellFolder::desktop() else { return };
    let refs: Vec<&ShellItem> = items.iter().collect();
    let Ok(data) = dragdrop::data_object(&root, &refs, list) else { return };
    DRAG.with(|d| *d.borrow_mut() = Some(DragStart { start: nm.ptAction, anchor, moving }));
    // Modal. The returned effect is ignored: a rearrange ends with NONE, and nothing the drag does deletes sources.
    let _ = dragdrop::drag(&data, list);
    DRAG.with(|d| *d.borrow_mut() = None);
}

/// A drop of our own selection inside the view: the grabbed item lands on the nearest cell and the others follow.
unsafe fn rearrange(list: HWND, pt: POINTL) {
    let Some(drag) = DRAG.with(|d| d.borrow().clone()) else { return };
    let mut p = POINT { x: pt.x, y: pt.y };
    let _ = ScreenToClient(list, &mut p);
    let (area, spacing, dims) = metrics(list);
    let Some(cells) = desk(|d| d.cells.clone()) else { return };
    let Some(&anchor) = cells.get(drag.anchor) else { return };
    let o = cell_origin(anchor, area, spacing);
    let to = snap(POINT { x: o.x + p.x - drag.start.x, y: o.y + p.y - drag.start.y }, area, spacing, dims);
    let moved = move_cells(&cells, &drag.moving, (to.0 as i32 - anchor.0 as i32, to.1 as i32 - anchor.1 as i32), dims);
    put_cells(list, &moved);
    desk(|d| d.cells = moved);
    save();
}

unsafe fn register_drop(list: HWND) {
    let position: IDropTarget = dragdrop::position_target(Box::new(move |pt| unsafe { rearrange(list, pt) }));
    // One shell target per destination, kept while the pointer stays on it (DropForwarder compares them).
    let last: RefCell<Option<(String, IDropTarget)>> = RefCell::new(None);
    let pick = move |pt: POINTL| -> Option<IDropTarget> {
        let mut p = POINT { x: pt.x, y: pt.y };
        let (hit, dragging) = unsafe {
            let _ = ScreenToClient(list, &mut p);
            let mut ht = LVHITTESTINFO { pt: p, ..Default::default() };
            let hit = SendMessageW(list, LVM_HITTEST, None, Some(LPARAM(&mut ht as *mut _ as isize))).0;
            (usize::try_from(hit).ok(), DRAG.with(|d| d.borrow().is_some()))
        };
        let is_selected = |i: usize| unsafe { SendMessageW(list, LVM_GETITEMSTATE, Some(WPARAM(i)), Some(LPARAM(LVIS_SELECTED.0 as isize))).0 != 0 };
        let folder_item = hit.filter(|&i| !is_selected(i)).and_then(|i| desk(|d| d.items.get(i).cloned()).flatten()).filter(|it| it.is_folder());
        // Our own icons dropped anywhere but on another folder rearrange; they never run a shell move onto the desktop
        // root (that would move a public Desktop item into the user's).
        if dragging && folder_item.is_none() {
            return Some(position.clone());
        }
        let key = folder_item.as_ref().map_or(String::new(), |it| it.parsing_name());
        if let Some((k, t)) = &*last.borrow()
            && *k == key {
                return Some(t.clone());
            }
        let root = ShellFolder::desktop().ok()?;
        let target = match &folder_item {
            Some(it) => dragdrop::item_drop_target(&root, it, list),
            None => dragdrop::folder_drop_target(&root, list),
        }
        .ok()?;
        *last.borrow_mut() = Some((key, target.clone()));
        Some(target)
    };
    let _ = dragdrop::register(list, Box::new(pick), None::<Box<dyn Fn(POINTL, &IDataObject)>>);
}

// ---- menu ---------------------------------------------------------------------------------------------------------

unsafe fn show_menu(hwnd: HWND, lparam: LPARAM) {
    let Some(list) = desk(|d| d.list) else { return };
    if BUSY.replace(true) {
        return;
    }
    menu_for(hwnd, list, lparam);
    BUSY.set(false);
}

unsafe fn menu_for(hwnd: HWND, list: HWND, lparam: LPARAM) {
    let keyboard = lparam.0 == -1;
    let mut pt = POINT { x: lparam.0 as i16 as i32, y: (lparam.0 >> 16) as i16 as i32 };
    if keyboard {
        // At the focused item's label, as the file list does; the corner of the list without one.
        let i = SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_FOCUSED as isize))).0;
        let mut r = RECT { left: LVIR_LABEL as i32, ..Default::default() };
        let has = i >= 0 && SendMessageW(list, LVM_GETITEMRECT, Some(WPARAM(i as usize)), Some(LPARAM(&mut r as *mut _ as isize))).0 != 0;
        pt = if has { POINT { x: r.left, y: r.bottom } } else { POINT::default() };
        let _ = ClientToScreen(list, &mut pt);
    } else {
        let mut p = pt;
        let _ = ScreenToClient(list, &mut p);
        let mut ht = LVHITTESTINFO { pt: p, ..Default::default() };
        let hit = SendMessageW(list, LVM_HITTEST, None, Some(LPARAM(&mut ht as *mut _ as isize))).0;
        if let Ok(h) = usize::try_from(hit) {
            if !selected(list).contains(&h) {
                crate::list::set_item_state(list, usize::MAX, LIST_VIEW_ITEM_STATE_FLAGS(0), LVIS_SELECTED);
                crate::list::set_item_state(list, h, LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0), LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0));
            }
        } else {
            crate::list::set_item_state(list, usize::MAX, LIST_VIEW_ITEM_STATE_FLAGS(0), LVIS_SELECTED);
        }
    }
    let items = selected_items();
    let Ok(root) = ShellFolder::desktop() else { return };
    let refs: Vec<&ShellItem> = items.iter().collect();
    let menu = if refs.is_empty() { ContextMenu::for_background(&root, hwnd) } else { ContextMenu::for_items(&root, &refs, hwnd) };
    let Ok(menu) = menu else { return };
    let own = if refs.is_empty() {
        vec![
            Entry::new(REFRESH, tr("menu.list.refresh"), 0xE72C),
            Entry::new(PASTE, tr("menu.list.paste"), 0xE77F).enabled(crate::commands::clipboard_has_files()),
            Entry::new(ARRANGE_NAME, tr("desk.arrange_name"), 0xE8CB),
            Entry::sep(),
            Entry::new(CLOSE, tr("desk.close_icons"), 0xE711),
            Entry::sep(),
        ]
    } else {
        Vec::new()
    };
    let mut bitmaps = Vec::new();
    let hmenu = ui::build_menu(hwnd, &own, &mut bitmaps);
    let at = GetMenuItemCount(Some(hmenu)).max(0) as u32;
    let extended = GetKeyState(VK_SHIFT.0 as i32) < 0;
    crate::theme::set_menu_theme(import_explorer::apps_use_dark());
    ACTIVE.with(|a| *a.borrow_mut() = Some(menu));
    let picked = ACTIVE.with(|a| a.borrow().as_ref().map(|m| m.show_in(hwnd, hmenu, at, pt.x, pt.y, extended)));
    let menu = ACTIVE.with(|a| a.borrow_mut().take());
    let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(hmenu);
    for b in bitmaps {
        let _ = DeleteObject(b.into());
    }
    match picked {
        Some(Ok(Picked::Rename)) => begin_rename(list),
        // Shift held at the click deletes for good, as in Explorer and the file list (not Shift at the menu's opening).
        Some(Ok(Picked::Delete)) => delete_selection(hwnd, GetKeyState(VK_SHIFT.0 as i32) < 0),
        Some(Ok(Picked::Own(REFRESH))) => {
            place(hwnd);
            reload(hwnd);
        }
        Some(Ok(Picked::Own(PASTE))) => {
            let _ = menu.map(|m| m.invoke_verb(hwnd, "paste", false));
        }
        Some(Ok(Picked::Own(ARRANGE_NAME))) => arrange_by_name(list),
        Some(Ok(Picked::Own(CLOSE))) => {
            let _ = DestroyWindow(hwnd);
        }
        _ => {}
    }
}

// ---- messages -----------------------------------------------------------------------------------------------------

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // The z-order is only ever set by `place`: activation does not raise the view above other windows.
        WM_WINDOWPOSCHANGING => {
            if !PLACING.get() {
                (*(lparam.0 as *mut WINDOWPOS)).flags |= SWP_NOZORDER;
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_DESK_CHANGE => {
            let _ = ChangeWatch::acknowledge(wparam, lparam);
            SetTimer(Some(hwnd), TIMER_RELOAD, 300, None);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_RELOAD => {
            let _ = KillTimer(Some(hwnd), TIMER_RELOAD);
            reload(hwnd);
            LRESULT(0)
        }
        WM_SETTINGCHANGE if wparam.0 == WORKAREA_CHANGED => {
            place(hwnd);
            reload(hwnd);
            LRESULT(0)
        }
        WM_SETTINGCHANGE if wparam.0 == WALLPAPER_CHANGED => {
            let _ = desk(|d| InvalidateRect(Some(d.list), None, true));
            LRESULT(0)
        }
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE => {
            if let Some(list) = desk(|d| d.list) {
                set_text_color(list);
                let _ = InvalidateRect(Some(list), None, true);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED => {
            // The suggested rectangle is ignored: `place` covers the work area.
            if let Some(list) = desk(|d| d.list) {
                set_font(hwnd, list);
            }
            place(hwnd);
            reload(hwnd);
            LRESULT(0)
        }
        WM_ACTIVATE => {
            // Only on activation: SetFocus while being deactivated would take activation back.
            if let (true, Some(list)) = ((wparam.0 & 0xFFFF) as u32 != WA_INACTIVE, desk(|d| d.list)) {
                let _ = SetFocus(Some(list));
            }
            LRESULT(0)
        }
        WM_NOTIFY => on_notify(hwnd, lparam),
        WM_CONTEXTMENU => {
            show_menu(hwnd, lparam);
            LRESULT(0)
        }
        WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR => ACTIVE
            .with(|a| a.try_borrow().ok().and_then(|a| a.as_ref().and_then(|m| m.handle_message(msg, wparam, lparam))))
            .unwrap_or_else(|| DefWindowProcW(hwnd, msg, wparam, lparam)),
        WM_DESTROY => {
            if let Some(list) = desk(|d| d.list) {
                dragdrop::unregister(list);
            }
            let old = FONT.replace(0);
            if old != 0 {
                let _ = DeleteObject(HFONT(old as *mut _).into());
            }
            // Dropping the watches deregisters them.
            STATE.with(|s| s.borrow_mut().take());
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Closes a running view (before install replaces the exe). True if one was running.
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

    fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
        RECT { left: l, top: t, right: r, bottom: b }
    }

    fn keys(n: usize) -> Vec<String> {
        (0..n).map(|i| ((b'a' + i as u8) as char).to_string()).collect()
    }

    #[test]
    fn safety_no_hooks_parenting_or_input() {
        let src = include_str!("desktop_icons.rs");
        for bad in [concat!("SetPar", "ent("), concat!("SetWindows", "HookEx"), concat!("Send", "Input"), concat!("keybd", "_event"), concat!("mouse", "_event"), concat!("SPI_", "SET"), concat!("RegSet", "Value"), concat!("RegCreate", "Key")] {
            assert!(!src.contains(bad), "{bad}");
        }
        // Nor joins Explorer's input queue, subclasses or restyles a window it does not own, turns topmost or takes the foreground.
        for bad in [concat!("AttachThread", "Input"), concat!("SetWindow", "Long"), concat!("HWND_", "TOPMOST"), concat!("SetForeground", "Window")] {
            assert!(!src.contains(bad), "{bad}");
        }
    }

    #[test]
    fn view_rect_stays_off_full_screen() {
        let (work, mon) = (rect(0, 0, 1920, 1040), rect(0, 0, 1920, 1080));
        assert_eq!(view_rect(work, mon), work);
        // Auto-hide taskbar: the work area is the whole monitor.
        assert_eq!(view_rect(mon, mon), rect(0, 0, 1920, 1079));
    }

    #[test]
    fn insert_after_never_goes_topmost_or_over_apps() {
        let h = |n: usize| HWND(n as *mut c_void);
        let ours = h(9);
        assert_eq!(insert_after(Some((h(1), false)), ours, false), Some(h(1)));
        assert_eq!(insert_after(Some((h(1), false)), ours, true), Some(h(1)));
        assert_eq!(insert_after(Some((ours, false)), ours, true), None);
        // A topmost window right above the host, or no host: a shown view stays put, a new one goes to the bottom.
        assert_eq!(insert_after(Some((h(1), true)), ours, true), None);
        assert_eq!(insert_after(Some((h(1), true)), ours, false), Some(HWND_BOTTOM));
        assert_eq!(insert_after(None, ours, true), None);
        assert_eq!(insert_after(None, ours, false), Some(HWND_BOTTOM));
    }

    #[test]
    fn grid_dims_fit_and_floor() {
        assert_eq!(grid_dims(rect(0, 0, 1920, 1040), (75, 100)), (25, 10));
        assert_eq!(grid_dims(rect(0, 0, 10, 10), (75, 100)), (1, 1));
    }

    #[test]
    fn snap_and_cell_origin_agree() {
        let (area, sp, dims) = (rect(10, 20, 410, 320), (100, 100), (4, 3));
        for c in 0..4 {
            for r in 0..3 {
                assert_eq!(snap(cell_origin((c, r), area, sp), area, sp, dims), (c, r));
            }
        }
        let mid = 10 + 150;
        assert_eq!(snap(POINT { x: mid - 1, y: 20 }, area, sp, dims).0, 1);
        assert_eq!(snap(POINT { x: mid, y: 20 }, area, sp, dims).0, 2);
        assert_eq!(snap(POINT { x: -500, y: -500 }, area, sp, dims), (0, 0));
        assert_eq!(snap(POINT { x: 99999, y: 99999 }, area, sp, dims), (3, 2));
    }

    #[test]
    fn arrange_fills_columns_first() {
        assert_eq!(arrange(&keys(5), &HashMap::new(), (3, 2)), [(0, 0), (0, 1), (1, 0), (1, 1), (2, 0)]);
    }

    #[test]
    fn arrange_keeps_saved_and_fills_gaps() {
        let saved = HashMap::from([("b".to_string(), (2, 1))]);
        assert_eq!(arrange(&keys(3), &saved, (3, 2)), [(0, 0), (2, 1), (0, 1)]);
    }

    #[test]
    fn arrange_replaces_out_of_bounds_and_duplicates() {
        let out = HashMap::from([("a".to_string(), (9, 9))]);
        assert_eq!(arrange(&keys(1), &out, (3, 2)), [(0, 0)]);
        let dup = HashMap::from([("a".to_string(), (1, 1)), ("b".to_string(), (1, 1))]);
        assert_eq!(arrange(&keys(2), &dup, (3, 2)), [(1, 1), (0, 0)]);
    }

    #[test]
    fn arrange_overflows_to_the_right() {
        assert_eq!(arrange(&keys(7), &HashMap::new(), (3, 2))[6], (3, 0));
    }

    #[test]
    fn move_cells_shifts_and_avoids_taken() {
        let cells = [(0, 0), (0, 1), (1, 0)];
        assert_eq!(move_cells(&cells, &[0, 1], (1, 0), (3, 2)), [(0, 0), (1, 1), (1, 0)]);
    }

    #[test]
    fn move_cells_clamps_at_edges() {
        assert_eq!(move_cells(&[(2, 1)], &[0], (5, 5), (3, 2)), [(2, 1)]);
        assert_eq!(move_cells(&[(2, 1)], &[0], (-9, -9), (3, 2)), [(0, 0)]);
    }

    #[test]
    fn positions_round_trip() {
        let items: Vec<(String, Pos)> = [r"c:\users\x\desktop\a b.txt", r"c:\users\public\desktop\ñ.lnk", "::{645ff040-5081-101b-9f08-00aa002f954e}", r"\\server\share\f"]
            .iter()
            .enumerate()
            .map(|(i, k)| (k.to_string(), (i as u32, 2 * i as u32)))
            .collect();
        assert_eq!(parse_positions(&format_positions(&items)), items.iter().cloned().collect());
        assert_eq!(parse_positions("0\t1\tC:\\A\n").get(r"c:\a"), Some(&(0, 1)));
    }

    #[test]
    fn parse_positions_skips_garbage() {
        for bad in ["x", "1\t2", "-1\t2\tc:\\a", "1\t2\t", "99999\t1\ta", "1\t2\ta\u{7}", "# c\t1\tz"] {
            assert!(parse_positions(bad).is_empty(), "{bad:?}");
        }
        assert_eq!(parse_positions("1\t2\tC:\\A\r").get(r"c:\a"), Some(&(1, 2)));
        assert_eq!(parse_positions("1\t2\ta\n3\t4\ta\n").get("a"), Some(&(1, 2)));
        assert!(parse_positions(&"1\t2\ta\n".repeat(200_000)).is_empty());
    }

    #[test]
    fn shows_on_desktop_rules() {
        let bin = "::{645FF040-5081-101B-9F08-00AA002F954E}";
        assert!(shows_on_desktop(bin, false, false, None));
        assert!(!shows_on_desktop(bin, false, false, Some(1)));
        assert!(shows_on_desktop(bin, false, false, Some(0)));
        let pc = "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}";
        assert!(!shows_on_desktop(pc, false, false, None));
        assert!(shows_on_desktop(pc, false, false, Some(0)));
        assert!(!shows_on_desktop(pc, false, false, Some(1)));
        for other in ["::{59031a47-3f72-44a7-89c5-5595fe6b30ee}", "::{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}", "::{5399E694-6CE5-4D6C-8FCE-1D8870FDCBA0}"] {
            assert!(!shows_on_desktop(other, false, false, None) && shows_on_desktop(other, false, false, Some(0)), "{other}");
        }
        assert!(!shows_on_desktop("::{031E4825-7B94-4dc3-B131-E946B44C8DD5}", false, false, Some(0)));
        assert!(shows_on_desktop(&pc.to_lowercase(), false, false, Some(0)));
        assert!(!shows_on_desktop(r"C:\Users\x\Desktop\h.txt", true, false, None));
        assert!(shows_on_desktop(r"C:\Users\x\Desktop\h.txt", true, true, None));
        assert!(!shows_on_desktop(r"C:\Users\x\Desktop\DESKTOP.INI", false, true, None));
        assert!(shows_on_desktop(r"C:\Users\x\Desktop\a.txt", false, false, None));
    }
}
