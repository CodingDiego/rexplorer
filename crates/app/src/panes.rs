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
    FOLDERID_Desktop, FOLDERID_Profile, FOLDERID_PublicDesktop, ILIsEqual, IShellFolder, IShellFolder2, KF_FLAG_DEFAULT,
    SHGFI_ICON, SHGetKnownFolderPath,
};
use windows::core::{GUID, Interface};

/// lparam = Box<Pidl> picked in the tree.
pub(crate) const WM_TREE_SELECT: u32 = WM_APP + 3;
/// lparam = Box<SidebarRows> built by the sidebar worker.
pub(crate) const WM_SIDEBAR_ROWS: u32 = WM_APP + 8;
/// Gap between the side panes and the cards, and the cards' margin from the window edges; it is the splitter.
const SPLIT: i32 = 8;
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
    /// Index for index with the sidebar's rows.
    pub sidebar_kinds: Vec<Kind>,
    /// The list item under the pointer and whether a WM_MOUSELEAVE is pending for it.
    pub hover: Option<usize>,
    pub hover_tracking: bool,
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
    commands::update(app);
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
    let _ = MoveWindow(a.home, left, top, (right - left).max(0), h, true);
    a.panes.list_rect = RECT { left, top, right: right.max(left), bottom: top + h };
    if let Some(b) = &a.browser {
        let _ = b.set_rect(a.panes.list_rect);
        // Explorer's own window paints itself: a (jagged) region is the only way to round it.
        theme::round_corners(b.hwnd(), theme::card_radius(dpi));
    }
}

// ---- sidebar ---------------------------------------------------------------------------------------

/// One sidebar worker at a time (the 5 s refresh must not pile up behind a slow drive).
static SIDEBAR_BUSY: AtomicBool = AtomicBool::new(false);
/// A refresh was asked for while the worker ran (a pin right after the timer's refresh): run once more.
static SIDEBAR_AGAIN: AtomicBool = AtomicBool::new(false);

/// What a sidebar row is, for its right-click menu.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Kind {
    Place,
    /// A profile bookmark, by its label.
    Bookmark(String),
    /// A folder pinned to Quick Access.
    Pinned,
    /// A frequent folder of Quick Access (not pinned).
    Frequent,
}

/// WM_SIDEBAR_ROWS payload: the rows and, index for index, their kinds.
#[derive(Default)]
pub(crate) struct SidebarRows {
    pub rows: Vec<Row>,
    pub kinds: Vec<Kind>,
}

impl SidebarRows {
    /// Adds a row (loading its icon) unless its label is empty or, with `dedupe`, its target is already listed.
    fn add(&mut self, section: &'static str, label: String, target: String, pidl: &Pidl, kind: Kind, dedupe: bool) {
        if label.is_empty() || (dedupe && self.rows.iter().any(|r| r.target.eq_ignore_ascii_case(&target))) {
            return;
        }
        self.rows.push(Row { section, label, target, icon: small_icon(pidl) });
        self.kinds.push(kind);
    }
}

/// Builds the sidebar rows on a worker: Home, Favorites (bookmarks, then Quick Access), This PC, Devices, Cloud
/// and Network. Icons are loaded there too; the rows arrive as WM_SIDEBAR_ROWS.
pub(crate) unsafe fn refresh_sidebar(app: *mut App) {
    if SIDEBAR_BUSY.swap(true, Ordering::SeqCst) {
        SIDEBAR_AGAIN.store(true, Ordering::SeqCst);
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
            for i in rows.rows.iter().filter_map(|r| r.icon) {
                let _ = unsafe { DestroyIcon(i) };
            }
        }
    });
}

/// After our own pin or unpin: rebuild now, and once more a second later in case the shell updates late.
pub(crate) unsafe fn sidebar_changed(app: *mut App) {
    refresh_sidebar(app);
    SetTimer(Some((*app).hwnd), TIMER_SIDEBAR, 1000, None);
}

pub(crate) unsafe fn on_sidebar_rows(app: *mut App, rows: Box<SidebarRows>) {
    let SidebarRows { rows, kinds } = *rows;
    (*app).panes.sidebar_kinds = kinds;
    sidebar::set_rows((*app).sidebar, rows);
    let a = &*app;
    if let Some(t) = a.tabs.get(a.active) {
        sidebar::select(a.sidebar, t.loading.as_ref().map_or(&t.name, |l| &l.parsing));
    }
    if SIDEBAR_AGAIN.swap(false, Ordering::SeqCst) {
        refresh_sidebar(app);
    }
}

/// Right-click on a sidebar row: "Unpin from Home" on a pinned Quick Access folder, "Pin to Home" on a frequent one,
/// "Remove bookmark" on a bookmark.
pub(crate) unsafe fn sidebar_menu(app: *mut App, index: usize) {
    let kind = (&(*app).panes.sidebar_kinds).get(index).cloned();
    let text = match &kind {
        Some(Kind::Pinned) => "Unpin from Home",
        Some(Kind::Frequent) => "Pin to Home",
        Some(Kind::Bookmark(_)) => "Remove bookmark",
        _ => return,
    };
    let target = sidebar::target((*app).sidebar, index);
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    if ui::popup(app, &[(1, text.to_string())], &[], pt.x, pt.y) != 1 {
        return;
    }
    match kind {
        Some(Kind::Pinned) => quick_access_verb(app, &target, "unpinfromhome"),
        Some(Kind::Frequent) => quick_access_verb(app, &target, "pintohome"),
        Some(Kind::Bookmark(label)) => {
            views::remove_profile_key(&(*app).profile.name, &format!("bookmark.{label}"));
            (*app).profile.bookmarks.retain(|(l, _)| *l != label);
            sidebar_changed(app);
        }
        _ => {}
    }
}

/// A canonical verb ("unpinfromhome", "pintohome") on the Quick Access item for `target`; beeps when the shell
/// refuses it (the item left Quick Access meanwhile).
unsafe fn quick_access_verb(app: *mut App, target: &str, verb: &str) {
    let hwnd = (*app).hwnd;
    let done = (|| {
        let qa = ShellFolder::parse(&format!("shell:{}", nav::QUICK_ACCESS)).ok()?;
        let items = qa.enumerate().ok()?;
        let item = items.iter().find(|i| i.parsing_name().eq_ignore_ascii_case(target))?;
        ContextMenu::for_items(&qa, &[item], hwnd).ok()?.invoke_verb(hwnd, verb, false).ok()
    })();
    if done.is_some() {
        sidebar_changed(app);
    } else {
        let _ = MessageBeep(MB_OK);
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

/// SFGAO_FILESYSTEM (the app does not pull in System_SystemServices for one constant).
const SFGAO_FILESYSTEM: u32 = 0x4000_0000;

/// Places on the desktop root that are neither devices nor shown elsewhere in the sidebar: This PC, Network,
/// Control Panel, Libraries, Gallery (the root also lists Home and the Recycle Bin, which are rows already).
const NOT_DEVICES: [&str; 5] = [
    ui::THIS_PC,
    "::{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}",
    "::{26EE0668-A00A-44D7-9371-BEB064C98683}",
    "::{031E4825-7B94-4DC3-B131-E946B44C8DD5}",
    "::{E88865EA-0E1C-4E20-9AA6-EDCD0212C87C}",
];

/// `path` is `dir` or inside it (case-insensitive).
fn is_under(path: &str, dir: &str) -> bool {
    let (p, d) = (path.to_lowercase(), dir.to_lowercase().trim_end_matches('\\').to_string());
    p == d || p.starts_with(&format!("{d}\\"))
}

fn sidebar_rows(bookmarks: &[(String, String)]) -> SidebarRows {
    let mut b = SidebarRows::default();
    if let Ok(home) = ShellFolder::parse(settings::HOME_START) {
        b.add("", home.name.clone(), home.parsing_name.clone(), &home.pidl, Kind::Place, false);
    }
    for (label, target) in bookmarks {
        if let Ok(f) = ShellFolder::parse(target) {
            b.add("Favorites", label.clone(), f.parsing_name.clone(), &f.pidl, Kind::Bookmark(label.clone()), true);
        }
    }
    // Quick Access in the shell's order and names: the pinned known folders, the Recycle Bin, the user's pins.
    if let Ok(qa) = ShellFolder::parse(&format!("shell:{}", nav::QUICK_ACCESS)) {
        for item in qa.enumerate().unwrap_or_default().iter().filter(|i| i.is_folder()) {
            let kind = if item.property_bool(&home::IS_PINNED) == Some(false) { Kind::Frequent } else { Kind::Pinned };
            b.add("Favorites", item.display_name(), item.parsing_name(), &qa.child_pidl(item), kind, true);
        }
    }
    let cloud = ui::cloud_roots();
    if let Ok(pc) = ShellFolder::parse("shell:MyComputerFolder") {
        // Drives, phones and other devices; not the user folders This PC shows too.
        let mut items: Vec<ShellItem> = pc.enumerate().unwrap_or_default();
        items.retain(|i| {
            let p = i.parsing_name();
            i.attributes(SFGAO_FILESYSTEM) == 0 || (p.len() == 3 && p.ends_with(":\\"))
        });
        items.sort_by_key(|i| i.parsing_name());
        for d in &items {
            b.add("This PC", d.display_name(), d.parsing_name(), &pc.child_pidl(d), Kind::Place, false);
        }
        // Devices: what the desktop root adds besides files and the shell's own places (Phone Link phones, Linux).
        if let Some(root) = pc.parent() {
            let desktops: Vec<String> = [FOLDERID_Desktop, FOLDERID_PublicDesktop].iter().filter_map(known_folder).collect();
            let profile = known_folder(&FOLDERID_Profile).unwrap_or_default();
            for item in root.enumerate().unwrap_or_default().iter().filter(|i| i.is_folder() && !i.is_hidden()) {
                let (p, label) = (item.parsing_name(), item.display_name());
                let skip = NOT_DEVICES.iter().any(|n| p.eq_ignore_ascii_case(n))
                    || desktops.iter().any(|d| is_under(&p, d))
                    || p.eq_ignore_ascii_case(&profile)
                    || cloud.iter().any(|(l, _)| l.eq_ignore_ascii_case(&label));
                if !skip {
                    b.add("Devices", label, p, &root.child_pidl(item), Kind::Place, true);
                }
            }
        }
    }
    for (label, path) in cloud {
        if let Ok(f) = ShellFolder::parse(path) {
            b.add("Cloud", label.clone(), f.parsing_name.clone(), &f.pidl, Kind::Place, false);
        }
    }
    if let Ok(f) = ShellFolder::parse("shell:NetworkPlacesFolder") {
        b.add("Network", f.name.clone(), f.parsing_name.clone(), &f.pidl, Kind::Place, false);
    }
    b
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
        stop_hover(app);
    }
}

/// The hovered list item changed (None: the pointer left or is over empty space).
pub(crate) unsafe fn set_hover(app: *mut App, item: Option<usize>) {
    if (*app).panes.hover == item {
        return;
    }
    (*app).panes.hover = item;
    let _ = KillTimer(Some((*app).hwnd), TIMER_HOVER);
    if item.is_some() && (*app).session.show_preview {
        SetTimer(Some((*app).hwnd), TIMER_HOVER, (*app).profile.hover_preview_ms, None);
    }
}

/// Selection change and navigation cancel a pending prefetch.
pub(crate) unsafe fn stop_hover(app: *mut App) {
    (*app).panes.hover = None;
    let _ = KillTimer(Some((*app).hwnd), TIMER_HOVER);
}

/// TIMER_HOVER: warm the preview cache for the item still under the pointer. The work runs on a thread; nothing is shown.
pub(crate) unsafe fn prefetch_hovered(app: *mut App) {
    let Some(i) = (*app).panes.hover else { return };
    let Some(item) = listing(app).and_then(|l| l.items.get(i).map(|it| (l, it))) else { return };
    let (l, item) = item;
    let parsing = item.parsing_name();
    if item.is_folder() || !ui::is_fs_path(&parsing) {
        return;
    }
    preview_pane::prefetch(PathBuf::from(&parsing), l.folder.child_pidl(item), preview_pane::request_size((*app).panes.preview));
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
        let SidebarRows { rows, kinds } = sidebar_rows(&[("My downloads".to_string(), "shell:Downloads".to_string())]);
        let names: Vec<(&str, &str)> = rows.iter().map(|r| (r.section, r.target.as_str())).collect();
        assert_eq!(rows.len(), kinds.len());
        assert_eq!((rows[0].section, rows[0].target.as_str()), ("", nav::HOME), "Home first, without a header: {names:?}");
        assert_eq!((rows[1].label.as_str(), &kinds[1]), ("My downloads", &Kind::Bookmark("My downloads".into())), "then bookmarks");
        let downloads = rows[1].target.to_lowercase();
        assert_eq!(rows.iter().filter(|r| r.target.to_lowercase() == downloads).count(), 1, "Quick Access does not repeat it");
        let recycle = "::{645FF040-5081-101B-9F08-00AA002F954E}";
        let bin = rows.iter().position(|r| r.target.eq_ignore_ascii_case(recycle)).expect("Recycle Bin from Quick Access");
        assert_eq!((rows[bin].section, &kinds[bin]), ("Favorites", &Kind::Pinned));
        assert!(rows.iter().any(|r| r.section == "This PC" && r.target == "C:\\"), "drives: {names:?}");
        assert!(!rows.iter().any(|r| r.section == "This PC" && r.target.len() > 3 && ui::is_fs_path(&r.target)), "no user folders under This PC");
        assert!(rows.iter().filter(|r| r.section == "Devices").all(|r| !NOT_DEVICES.contains(&r.target.as_str())));
        assert_eq!(rows.last().map(|r| r.section), Some("Network"));
        assert!(rows.iter().all(|r| r.icon.is_some()));
        for i in rows.iter().filter_map(|r| r.icon) {
            let _ = unsafe { DestroyIcon(i) };
        }
    }

    #[test]
    fn under_a_directory() {
        assert!(is_under(r"C:\Users\v\Desktop\x", r"c:\users\v\desktop"));
        assert!(is_under(r"C:\Users\v\Desktop", r"C:\Users\v\Desktop\"));
        assert!(!is_under(r"C:\Users\v\Desktop2", r"C:\Users\v\Desktop"));
        assert!(!is_under("::{B2B4A4D1-2754-4140-A2EB-9A76D9D7CDC6}", r"C:\Users\v\Desktop"));
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
