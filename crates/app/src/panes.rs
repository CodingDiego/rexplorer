//! Side panes: the places sidebar (or, instead of it, the shell's folder tree) left of the list, the preview right
//! of it, and the splitters between them: the 8 px gaps around the content cards.
//! Pane visibility and widths live in the session (widths in 96-DPI pixels; the sidebar uses the tree's width).

use super::*;
use crate::nav::{Target, go, listing};
use crate::sidebar::{self, Row};
use rexplorer_core::tree::{self, TreePane};
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures, FOLDERID_Videos, ILIsEqual,
    IShellFolder, IShellFolder2, KF_FLAG_DEFAULT, SHGFI_ICON, SHGetKnownFolderPath,
};
use windows::core::{GUID, Interface};

/// lparam = Box<Pidl> picked in the tree.
pub(crate) const WM_TREE_SELECT: u32 = WM_APP + 3;
/// lparam = Box<Vec<sidebar::Row>> built by the sidebar worker.
pub(crate) const WM_SIDEBAR_ROWS: u32 = WM_APP + 8;
/// Gap between the side panes and the cards, and the cards' margin from the window edges; it is the splitter.
const SPLIT: i32 = 8;
/// Corner radius of the content cards.
const CARD_RADIUS: i32 = 10;
const MIN_WIDTH: i32 = 120;
const MAX_WIDTH: i32 = 600;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Split {
    Tree,
    Preview,
}

#[derive(Default)]
pub(crate) struct Panes {
    pub tree: Option<TreePane>,
    pub preview: HWND,
    /// Owned; the preview Edit borrows it.
    pub mono: HFONT,
    drag: Option<Split>,
    split_tree: RECT,
    split_preview: RECT,
    /// Where the list (or the embedded browser in its place) goes, in client coordinates.
    pub list_rect: RECT,
}

fn scale(px: i32, dpi: u32) -> i32 {
    px * dpi as i32 / 96
}

pub(crate) fn clamp_width(w: i32) -> i32 {
    w.clamp(MIN_WIDTH, MAX_WIDTH)
}

/// Creates the tree on first need; the selection callback runs inside the control, so it only posts.
pub(crate) unsafe fn ensure_tree(app: *mut App) {
    let a = &mut *app;
    if a.panes.tree.is_some() {
        return;
    }
    let hwnd = a.hwnd.0 as isize;
    let post = move |p: Pidl| {
        let boxed = Box::into_raw(Box::new(p));
        if unsafe { PostMessageW(Some(HWND(hwnd as *mut c_void)), WM_TREE_SELECT, WPARAM(0), LPARAM(boxed as isize)) }.is_err() {
            drop(unsafe { Box::from_raw(boxed) });
        }
    };
    match tree::create(a.hwnd, RECT::default(), Box::new(post)) {
        Ok(t) => a.panes.tree = Some(t),
        Err(_) => return,
    }
    theme_tree(app);
    sync_tree(app);
}

pub(crate) unsafe fn theme_tree(app: *mut App) {
    let a = &*app;
    let Some(t) = &a.panes.tree else { return };
    let h = t.hwnd();
    theme::apply_control(h, &a.theme);
    if let Ok(inner) = FindWindowExW(Some(h), None, WC_TREEVIEWW, PCWSTR::null()) {
        theme::apply_control(inner, &a.theme);
        SendMessageW(inner, TVM_SETBKCOLOR, Some(WPARAM(0)), Some(LPARAM(a.theme.background as isize)));
        SendMessageW(inner, TVM_SETTEXTCOLOR, Some(WPARAM(0)), Some(LPARAM(a.theme.text as isize)));
    }
}

/// Selects the active folder in the tree; folders the tree cannot show are ignored.
pub(crate) unsafe fn sync_tree(app: *mut App) {
    if let (Some(t), Some(l)) = (&(*app).panes.tree, listing(app)) {
        let _ = t.select(&l.folder.pidl);
    }
}

/// WM_TREE_SELECT. Only the user's picks navigate: the tree also reports its own initial selection and our
/// sync, so the tree must have the focus and the pick must differ from the current folder.
pub(crate) unsafe fn on_tree_select(app: *mut App, pidl: Box<Pidl>) {
    let Some(t) = &(*app).panes.tree else { return };
    let (tree, focus) = (t.hwnd(), GetFocus());
    if focus != tree && !IsChild(tree, focus).as_bool() {
        return;
    }
    let Some(l) = listing(app) else { return };
    if ILIsEqual(l.folder.pidl.as_ptr(), pidl.as_ptr()).as_bool() {
        return;
    }
    go(app, Target::Pidl(*pidl), nav::History::Push);
}

pub(crate) unsafe fn create_preview(app: *mut App) -> Result<()> {
    let a = &mut *app;
    a.panes.preview = preview_pane::create(a.hwnd, ID_PREVIEW)?;
    preview_pane::set_theme(a.panes.preview, &a.theme);
    Ok(())
}

/// Called from ui::apply_font: the preview borrows the UI font and owns nothing.
pub(crate) unsafe fn set_preview_fonts(app: *mut App) {
    let a = &mut *app;
    let mono = theme::mono_font(GetDpiForWindow(a.hwnd), 10);
    preview_pane::set_fonts(a.panes.preview, a.font, mono);
    if !a.panes.mono.is_invalid() {
        let _ = DeleteObject(a.panes.mono.into());
    }
    a.panes.mono = mono;
}

/// More > Folder tree: the tree replaces the sidebar; turning it off brings the sidebar back.
pub(crate) unsafe fn toggle_tree(app: *mut App) {
    let s = &mut (*app).session;
    s.show_tree = !s.show_tree;
    s.show_sidebar |= !s.show_tree;
    if s.show_tree {
        ensure_tree(app);
    }
    show_left(app);
}

/// Ctrl+B: hides whichever left pane is showing, or shows the sidebar.
pub(crate) unsafe fn toggle_sidebar(app: *mut App) {
    let s = &mut (*app).session;
    if s.show_tree || s.show_sidebar {
        s.show_tree = false;
        s.show_sidebar = false;
    } else {
        s.show_sidebar = true;
    }
    show_left(app);
}

/// Shows the left pane the session asks for (tree, else sidebar, else none) and lays out.
pub(crate) unsafe fn show_left(app: *mut App) {
    let s = &(*app).session;
    let (tree, side) = (s.show_tree, !s.show_tree && s.show_sidebar);
    if let Some(t) = &(*app).panes.tree {
        let _ = ShowWindow(t.hwnd(), if tree { SW_SHOW } else { SW_HIDE });
    }
    let _ = ShowWindow((*app).sidebar, if side { SW_SHOW } else { SW_HIDE });
    ui::layout(app);
    nav::session_changed(app);
}

pub(crate) unsafe fn toggle_preview(app: *mut App) {
    let a = &mut *app;
    a.session.show_preview = !a.session.show_preview;
    let show = a.session.show_preview;
    let _ = ShowWindow(a.panes.preview, if show { SW_SHOW } else { SW_HIDE });
    ui::layout(app);
    if show {
        update_preview(app);
    } else {
        // Releases a hosted preview handler.
        preview_pane::show((*app).panes.preview, None);
    }
    nav::session_changed(app);
}

/// Places the left pane, the list card and the preview card in the band `top..bottom` of a client `w` wide.
/// The left pane runs to the window's edge (it is part of the frame); the cards keep a gap from everything.
pub(crate) unsafe fn layout(app: *mut App, top: i32, bottom: i32, w: i32) {
    let a = &mut *app;
    let dpi = GetDpiForWindow(a.hwnd);
    let (h, gap, min_list) = ((bottom - top).max(0), scale(SPLIT, dpi), scale(160, dpi));
    let s = &a.session;
    a.panes.split_tree = RECT::default();
    a.panes.split_preview = RECT::default();
    let mut right = w - gap;
    if s.show_preview {
        let pw = scale(s.preview_width, dpi).min((right - min_list - gap).max(0));
        right -= pw;
        let _ = MoveWindow(a.panes.preview, right, top, pw, h, true);
        theme::round_corners(a.panes.preview, scale(CARD_RADIUS, dpi));
        a.panes.split_preview = RECT { left: right - gap, top, right, bottom };
        right -= gap;
    }
    let pane = if s.show_tree { a.panes.tree.as_ref().map(|t| t.hwnd()) } else { s.show_sidebar.then_some(a.sidebar) };
    let mut left = gap;
    if pane.is_some() {
        let tw = scale(s.tree_width, dpi).min((right - min_list - gap).max(0));
        match (&a.panes.tree, s.show_tree) {
            (Some(t), true) => t.set_rect(RECT { left: 0, top, right: tw, bottom }),
            _ => {
                let _ = MoveWindow(a.sidebar, 0, top, tw, h, true);
            }
        }
        a.panes.split_tree = RECT { left: tw, top, right: tw + gap, bottom };
        left = tw + gap;
    }
    let _ = MoveWindow(a.list, left, top, (right - left).max(0), h, true);
    a.panes.list_rect = RECT { left, top, right: right.max(left), bottom: top + h };
    if let Some(b) = &a.browser {
        let _ = b.set_rect(a.panes.list_rect);
        theme::round_corners(b.hwnd(), scale(CARD_RADIUS, dpi));
    }
}

// ---- sidebar ---------------------------------------------------------------------------------------

/// One sidebar worker at a time (the 5 s refresh must not pile up behind a slow drive).
static SIDEBAR_BUSY: AtomicBool = AtomicBool::new(false);

/// Builds the sidebar rows on a worker: Favorites (bookmarks, then the known folders), This PC (drives),
/// Cloud and Network. Icons are loaded there too; the rows arrive as WM_SIDEBAR_ROWS.
pub(crate) unsafe fn refresh_sidebar(app: *mut App) {
    if SIDEBAR_BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    let bookmarks = (*app).profile.bookmarks.clone();
    let hwnd = (*app).hwnd.0 as isize;
    std::thread::spawn(move || {
        let _com = ComInit::mta();
        let rows = Box::new(sidebar_rows(&bookmarks));
        SIDEBAR_BUSY.store(false, Ordering::SeqCst);
        let p = Box::into_raw(rows);
        if unsafe { PostMessageW(Some(HWND(hwnd as *mut c_void)), WM_SIDEBAR_ROWS, WPARAM(0), LPARAM(p as isize)) }.is_err() {
            let rows = unsafe { Box::from_raw(p) };
            for i in rows.iter().filter_map(|r| r.icon) {
                let _ = unsafe { DestroyIcon(i) };
            }
        }
    });
}

pub(crate) unsafe fn on_sidebar_rows(app: *mut App, rows: Box<Vec<Row>>) {
    sidebar::set_rows((*app).sidebar, *rows);
    let a = &*app;
    if let Some(t) = a.tabs.get(a.active) {
        sidebar::select(a.sidebar, &t.name);
    }
}

fn small_icon(pidl: &Pidl) -> Option<HICON> {
    let mut fi = SHFILEINFOW::default();
    let flags = SHGFI_PIDL | SHGFI_ICON | SHGFI_SMALLICON;
    unsafe { SHGetFileInfoW(PCWSTR(pidl.as_ptr() as *const u16), FILE_FLAGS_AND_ATTRIBUTES(0), Some(&mut fi), size_of::<SHFILEINFOW>() as u32, flags) };
    (!fi.hIcon.is_invalid()).then_some(fi.hIcon)
}

fn known_folder(id: &GUID) -> Option<String> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const c_void));
        s
    }
}

fn sidebar_rows(bookmarks: &[(String, String)]) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    // A place is listed once, under its first section (a bookmark to Downloads hides the known folder).
    let mut add = |section: &'static str, label: Option<&str>, name: &str| {
        let Ok(f) = ShellFolder::parse(name) else { return };
        if rows.iter().any(|r| r.target.eq_ignore_ascii_case(&f.parsing_name)) {
            return;
        }
        let label = label.map_or_else(|| f.name.clone(), str::to_string);
        rows.push(Row { section, label, target: f.parsing_name.clone(), icon: small_icon(&f.pidl) });
    };
    for (label, target) in bookmarks {
        add("Favorites", Some(label.as_str()), target);
    }
    let known = [FOLDERID_Desktop, FOLDERID_Downloads, FOLDERID_Documents, FOLDERID_Pictures, FOLDERID_Music, FOLDERID_Videos];
    for path in known.iter().filter_map(known_folder) {
        add("Favorites", None, &path);
    }
    if let Ok(pc) = ShellFolder::parse("shell:MyComputerFolder") {
        let mut drives: Vec<ShellItem> = pc.enumerate().unwrap_or_default();
        drives.retain(|i| {
            let p = i.parsing_name();
            p.len() == 3 && p.ends_with(":\\")
        });
        drives.sort_by_key(|i| i.parsing_name());
        for d in drives {
            let pidl = pc.child_pidl(&d);
            rows.push(Row { section: "This PC", label: d.display_name(), target: d.parsing_name(), icon: small_icon(&pidl) });
        }
    }
    let mut add = |section: &'static str, label: Option<&str>, name: &str| {
        let Ok(f) = ShellFolder::parse(name) else { return };
        let label = label.map_or_else(|| f.name.clone(), str::to_string);
        rows.push(Row { section, label, target: f.parsing_name.clone(), icon: small_icon(&f.pidl) });
    };
    for (label, path) in ui::cloud_roots() {
        add("Cloud", Some(label.as_str()), path);
    }
    add("Network", None, "shell:NetworkPlacesFolder");
    rows
}

fn hit(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

unsafe fn split_at(app: *mut App, x: i32, y: i32) -> Option<Split> {
    let p = &(*app).panes;
    if hit(&p.split_tree, x, y) {
        Some(Split::Tree)
    } else if hit(&p.split_preview, x, y) {
        Some(Split::Preview)
    } else {
        None
    }
}

/// WM_SETCURSOR over the main window's own client area: the resize cursor on a splitter.
pub(crate) unsafe fn on_set_cursor(app: *mut App) -> bool {
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = ScreenToClient((*app).hwnd, &mut pt);
    if split_at(app, pt.x, pt.y).is_some() {
        if let Ok(c) = LoadCursorW(None, IDC_SIZEWE) {
            SetCursor(Some(c));
        }
        return true;
    }
    false
}

fn point(lparam: LPARAM) -> (i32, i32) {
    (lparam.0 as i16 as i32, (lparam.0 >> 16) as i16 as i32)
}

pub(crate) unsafe fn on_button_down(app: *mut App, lparam: LPARAM) {
    let (x, y) = point(lparam);
    if let Some(s) = split_at(app, x, y) {
        (*app).panes.drag = Some(s);
        SetCapture((*app).hwnd);
    }
}

pub(crate) unsafe fn on_mouse_move(app: *mut App, lparam: LPARAM) {
    let a = &mut *app;
    let Some(split) = a.panes.drag else { return };
    let (x, _) = point(lparam);
    let dpi = GetDpiForWindow(a.hwnd) as i32;
    let mut rc = RECT::default();
    let _ = GetClientRect(a.hwnd, &mut rc);
    match split {
        Split::Tree => a.session.tree_width = clamp_width(x * 96 / dpi),
        Split::Preview => a.session.preview_width = clamp_width((rc.right - x) * 96 / dpi),
    }
    ui::layout(app);
}

/// WM_LBUTTONUP and WM_CAPTURECHANGED end a drag; the new width goes to the session.
pub(crate) unsafe fn end_drag(app: *mut App) {
    if (*app).panes.drag.take().is_some() {
        if GetCapture() == (*app).hwnd {
            let _ = ReleaseCapture();
        }
        nav::session_changed(app);
    }
}

// ---- preview -------------------------------------------------------------------------------------

/// Selection changed: refresh the preview once the selection settles.
pub(crate) unsafe fn schedule_preview(app: *mut App) {
    if (*app).session.show_preview {
        SetTimer(Some((*app).hwnd), TIMER_PREVIEW, 150, None);
    }
}

/// Shows the focused (else first) selected item, or nothing without a selection.
pub(crate) unsafe fn update_preview(app: *mut App) {
    if !(*app).session.show_preview {
        return;
    }
    let list = (*app).list;
    let subject = (|| {
        let l = listing(app)?;
        if SendMessageW(list, LVM_GETSELECTEDCOUNT, None, None).0 == 0 {
            return None;
        }
        let mut i = SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_FOCUSED as isize))).0;
        let selected = |i: isize| SendMessageW(list, LVM_GETITEMSTATE, Some(WPARAM(i as usize)), Some(LPARAM(LVIS_SELECTED.0 as isize))).0 != 0;
        if i < 0 || !selected(i) {
            i = SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_SELECTED as isize))).0;
        }
        let item = l.items.get(usize::try_from(i).ok()?)?;
        let parsing = item.parsing_name();
        let (_, type_col) = size_and_type_columns(&l.folder.folder);
        Some(preview_pane::Subject {
            path: ui::is_fs_path(&parsing).then(|| PathBuf::from(&parsing)),
            abs_pidl: l.folder.child_pidl(item),
            name: item.display_name(),
            is_folder: item.is_folder(),
            size: if item.is_folder() { None } else { item.size() },
            type_text: type_col.map(|c| item.detail(c)).unwrap_or_default(),
        })
    })();
    preview_pane::show((*app).panes.preview, subject);
}

/// Shell column indices of System.Size and System.ItemTypeText (column titles are localized, keys are not).
pub(crate) fn size_and_type_columns(folder: &IShellFolder) -> (Option<u32>, Option<u32>) {
    // PKEY_Size is (STORAGE, 12), PKEY_ItemTypeText is (SUMMARY, 11).
    const STORAGE: GUID = GUID::from_u128(0xB725F130_47EF_101A_A5F1_02608C9EEBAC);
    const SUMMARY: GUID = GUID::from_u128(0x28636AA6_953D_11D2_B5D6_00C04FD918D0);
    let (mut size, mut kind) = (None, None);
    let Ok(f2) = folder.cast::<IShellFolder2>() else { return (None, None) };
    for c in 0..128 {
        let mut key = windows::Win32::Foundation::PROPERTYKEY::default();
        if unsafe { f2.MapColumnToSCID(c, &mut key) }.is_err() {
            break;
        }
        match (key.fmtid, key.pid) {
            (STORAGE, 12) => size = size.or(Some(c)),
            (SUMMARY, 11) => kind = kind.or(Some(c)),
            _ => {}
        }
        if size.is_some() && kind.is_some() {
            break;
        }
    }
    (size, kind)
}

/// "12 KB", "12.345 KB", "1,024 bytes" -> bytes. Separators are dropped whatever the locale.
// shortcut: a decimal size ("1,5 MB") reads as 15 MB; Explorer's Size column shows whole KB for files
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_rows_of_this_machine() {
        let _com = ComInit::mta();
        let rows = sidebar_rows(&[("Downloads".to_string(), "shell:Downloads".to_string())]);
        assert_eq!(rows[0].label, "Downloads", "bookmarks come first, under their own label");
        assert_eq!(rows.iter().filter(|r| r.section == "Favorites").count(), 6, "the known Downloads is not repeated");
        assert!(rows.iter().any(|r| r.section == "This PC" && r.target.len() == 3), "drives");
        assert_eq!(rows.last().map(|r| r.section), Some("Network"));
        assert!(rows.iter().all(|r| r.icon.is_some()));
        for i in rows.iter().filter_map(|r| r.icon) {
            let _ = unsafe { DestroyIcon(i) };
        }
    }

    #[test]
    fn widths_clamp() {
        assert_eq!(clamp_width(10), 120);
        assert_eq!(clamp_width(300), 300);
        assert_eq!(clamp_width(5000), 600);
    }

    #[test]
    fn storage_columns_of_a_filesystem_folder() {
        let _com = ComInit::sta();
        let f = ShellFolder::parse(&std::env::temp_dir().display().to_string()).unwrap();
        let (size, kind) = size_and_type_columns(&f.folder);
        assert!(size.is_some() && kind.is_some(), "size={size:?} type={kind:?}");
        assert_ne!(size, kind);
    }
}
