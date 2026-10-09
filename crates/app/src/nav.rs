//! Tabs and navigation: each tab owns its listing and history; the single ListView shows the active tab.
//! Enumeration runs on worker threads that post WM_ENUM_DONE back to the window.

use super::*;

pub(crate) enum Target {
    Name(String),
    Pidl(Pidl),
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum History {
    Push,
    Back,
    Forward,
    Refresh,
}

/// One breadcrumb segment, root first.
pub(crate) struct Crumb {
    pub name: String,
    pub parsing: String,
    pub pidl: Pidl,
}

struct Loaded {
    folder: ShellFolder,
    items: Vec<ShellItem>,
    columns: Vec<Column>,
    crumbs: Vec<Crumb>,
    icon: (usize, i32),
}

pub(crate) struct EnumDone {
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

pub(crate) struct Listing {
    pub folder: ShellFolder,
    pub items: Vec<ShellItem>,
    pub columns: Vec<Column>,
    pub icons: Vec<i32>,
    /// Shell column index, not header position.
    pub sort_col: u32,
    pub sort_desc: bool,
}

pub(crate) struct Tab {
    /// Parsing name of the folder shown (or requested, before the first load finishes); goes to the session.
    pub name: String,
    pub listing: Option<Listing>,
    pub back: Vec<Pidl>,
    pub forward: Vec<Pidl>,
    /// Only the active tab watches its folder.
    pub watch: Option<ChangeWatch>,
    /// Generation of the load in flight for this tab, 0 when none.
    pub pending: u64,
    pub top: i32,
    pub selected: Vec<String>,
    /// Parsing name to select once the first load lands (`--select`).
    pub select_after: Option<String>,
    pub crumbs: Vec<Crumb>,
    /// System image list and index of the folder's small icon, for the breadcrumb.
    pub icon: (usize, i32),
}

/// How the active tab's listing is put into the ListView.
enum Show {
    /// A new folder: select `0` (by parsing name) if given, else focus the first item.
    Fresh(Vec<String>),
    /// Same folder again (refresh or tab switch): keep the scroll position and selection.
    Restore { top: i32, selected: Vec<String>, columns: bool },
}

pub(crate) unsafe fn cur<'a>(app: *mut App) -> &'a mut Tab {
    let a = &mut *app;
    &mut a.tabs[a.active]
}

pub(crate) unsafe fn listing<'a>(app: *mut App) -> Option<&'a Listing> {
    let a = &*app;
    a.tabs.get(a.active).and_then(|t| t.listing.as_ref())
}

// ---- loading ----------------------------------------------------------------------------------

/// Loads `target` into tab `t` on a worker thread.
pub(crate) unsafe fn navigate(app: *mut App, t: usize, target: Target, history: History) {
    (*app).generation += 1;
    let generation = (*app).generation;
    let Some(tab) = (&mut (*app).tabs).get_mut(t) else { return };
    tab.pending = generation;
    let hwnd = (*app).hwnd.0 as isize;
    std::thread::spawn(move || {
        let _com = ComInit::mta();
        let done = Box::new(EnumDone { generation, history, result: load(target) });
        let _ = PostMessageW(Some(HWND(hwnd as *mut c_void)), WM_ENUM_DONE, WPARAM(0), LPARAM(Box::into_raw(done) as isize));
    });
}

/// Navigates the active tab.
pub(crate) unsafe fn go(app: *mut App, target: Target, history: History) {
    navigate(app, (*app).active, target, history);
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
    let crumbs = crumbs_of(&folder);
    let icon = folder_icon(&folder.pidl);
    Ok(Loaded { folder, items, columns, crumbs, icon })
}

fn folder_icon(pidl: &Pidl) -> (usize, i32) {
    let mut fi = SHFILEINFOW::default();
    let flags = SHGFI_PIDL | SHGFI_SYSICONINDEX | SHGFI_SMALLICON;
    let himl = unsafe {
        SHGetFileInfoW(PCWSTR(pidl.as_ptr() as *const u16), FILE_FLAGS_AND_ATTRIBUTES(0), Some(&mut fi), size_of::<SHFILEINFOW>() as u32, flags)
    };
    (himl, fi.iIcon)
}

/// The folder and its ancestors up to (not including) the desktop root, root first.
fn crumbs_of(folder: &ShellFolder) -> Vec<Crumb> {
    let crumb = |f: &ShellFolder| Crumb { name: f.name.clone(), parsing: f.parsing_name.clone(), pidl: f.pidl.clone() };
    let mut out = vec![crumb(folder)];
    let mut next = folder.parent();
    while let Some(f) = next {
        if f.pidl.is_empty() {
            break;
        }
        out.push(crumb(&f));
        next = f.parent();
    }
    out.reverse();
    out
}

pub(crate) unsafe fn on_enum_done(app: *mut App, done: Box<EnumDone>) {
    let Some(t) = (*app).tabs.iter().position(|x| x.pending == done.generation) else { return };
    (&mut (*app).tabs)[t].pending = 0;
    let hwnd = (*app).hwnd;
    let list = (*app).list;
    let active = t == (*app).active;
    let loaded = match done.result {
        Err(LoadError { name, err, exists }) => {
            // Decide about the tab before the message box: it pumps messages and other loads may land meanwhile.
            if (&mut (*app).tabs)[t].listing.is_none() && (*app).tabs.len() > 1 {
                close_tab(app, t);
            } else if let Some(l) = listing(app) {
                set_text((*app).addr, &l.folder.editing_name);
            }
            if !exists || open_in_explorer(&name).is_err() {
                message(hwnd, &format!("Cannot open {name}\n\n{err}"));
            }
            return;
        }
        Ok(loaded) => loaded,
    };

    let refresh = done.history == History::Refresh;
    let tab = &mut (&mut (*app).tabs)[t];
    let old = tab.listing.take();
    let (mut sort_col, mut sort_desc) = (0, false);
    let mut keep: Vec<String> = Vec::new();
    let mut top = 0;
    if let Some(old) = &old {
        match done.history {
            History::Push => {
                tab.back.push(old.folder.pidl.clone());
                tab.forward.clear();
            }
            History::Back => tab.forward.push(old.folder.pidl.clone()),
            History::Forward => tab.back.push(old.folder.pidl.clone()),
            History::Refresh => {
                sort_col = old.sort_col;
                sort_desc = old.sort_desc;
                if active {
                    keep = list::selected_indices(list).iter().filter_map(|&i| old.items.get(i)).map(|i| i.parsing_name()).collect();
                    top = SendMessageW(list, LVM_GETTOPINDEX, None, None).0 as i32;
                } else {
                    keep = std::mem::take(&mut tab.selected);
                    top = tab.top;
                }
            }
        }
    }
    drop(old);
    if let Some(sel) = tab.select_after.take() {
        keep = vec![sel];
    }

    let mut l = Listing {
        icons: vec![ICON_UNKNOWN; loaded.items.len()],
        folder: loaded.folder,
        items: loaded.items,
        columns: loaded.columns,
        sort_col,
        sort_desc,
    };
    sort_listing(&mut l);
    tab.name = l.folder.parsing_name.clone();
    tab.crumbs = loaded.crumbs;
    tab.icon = loaded.icon;
    tabs::set_title((*app).strip, t, &l.folder.name);
    if !refresh {
        (*app).recent.push(&l.folder.parsing_name);
        (*app).recent.save();
    }
    tab.listing = Some(l);
    if active {
        show(app, if refresh { Show::Restore { top, selected: keep, columns: false } } else { Show::Fresh(keep) });
    } else {
        tab.top = top;
        tab.selected = keep;
    }
    session_changed(app);
}

/// Puts the active tab into the list, address bar, breadcrumb, title and toolbar.
unsafe fn show(app: *mut App, mode: Show) {
    let hwnd = (*app).hwnd;
    let list = (*app).list;
    let tab = cur(app);
    tab.watch = None;
    let Some(l) = &tab.listing else {
        SendMessageW(list, LVM_SETITEMCOUNT, Some(WPARAM(0)), Some(LPARAM(0)));
        set_columns(app, &[]);
        set_text((*app).addr, &tab.name);
        set_text(hwnd, "rexplorer");
        ui::set_crumbs(app);
        update_chrome(app);
        return;
    };

    if !(*app).image_list_set {
        if let Some(first) = l.items.first() {
            let (himl, _) = first.icon_index(&l.folder.pidl);
            if himl != 0 {
                SendMessageW(list, LVM_SETIMAGELIST, Some(WPARAM(LVSIL_SMALL as usize)), Some(LPARAM(himl as isize)));
                (*app).image_list_set = true;
            }
        }
    }
    if !matches!(mode, Show::Restore { columns: false, .. }) {
        set_columns(app, &l.columns);
    }
    set_text((*app).addr, &l.folder.editing_name);
    set_text(hwnd, &format!("{} - rexplorer", l.folder.name));
    ui::set_crumbs(app);
    tab.watch = ChangeWatch::register(hwnd, &l.folder, WM_SHELL_CHANGE);

    let count = l.items.len();
    // The list asks for item text synchronously from here on, so the listing must be in place first.
    SendMessageW(list, LVM_SETITEMCOUNT, Some(WPARAM(count)), Some(LPARAM(LVSICF_NOSCROLL as isize)));
    list::set_item_state(list, usize::MAX, LIST_VIEW_ITEM_STATE_FLAGS(0), LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0));
    let select = |names: &[String]| -> Option<usize> {
        let mut first = None;
        if !names.is_empty() {
            for (i, item) in l.items.iter().enumerate() {
                let p = item.parsing_name();
                if names.iter().any(|n| n.eq_ignore_ascii_case(&p)) {
                    list::set_item_state(list, i, LVIS_SELECTED, LVIS_SELECTED);
                    first.get_or_insert(i);
                }
            }
        }
        first
    };
    match mode {
        Show::Fresh(names) => {
            let i = select(&names).unwrap_or(0);
            if count > 0 {
                SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(i)), Some(LPARAM(0)));
                list::set_item_state(list, i, LVIS_FOCUSED, LVIS_FOCUSED);
            }
        }
        Show::Restore { top, selected, .. } => {
            if let Some(i) = select(&selected) {
                list::set_item_state(list, i, LVIS_FOCUSED, LVIS_FOCUSED);
            }
            let per_page = SendMessageW(list, LVM_GETCOUNTPERPAGE, None, None).0 as i32;
            let last = (top + per_page - 1).min(count as i32 - 1);
            if last >= 0 {
                SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(last as usize)), Some(LPARAM(0)));
                SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(top.max(0) as usize)), Some(LPARAM(0)));
            }
        }
    }
    update_chrome(app);
}

/// Toolbar enabled states and the status bar, after anything that changes the active tab.
pub(crate) unsafe fn update_chrome(app: *mut App) {
    let tab = cur(app);
    let nav = (*app).nav;
    toolbar::set_enabled(nav, CMD_BACK, !tab.back.is_empty());
    toolbar::set_enabled(nav, CMD_FORWARD, !tab.forward.is_empty());
    toolbar::set_enabled(nav, CMD_UP, tab.listing.as_ref().is_some_and(|l| l.folder.parent_pidl().is_some()));
    ui::update_status(app);
}

pub(crate) unsafe fn sort_listing(l: &mut Listing) {
    let folder = &l.folder;
    let col = l.sort_col;
    l.items.sort_by(|a, b| folder.compare(col, a, b));
    if l.sort_desc {
        l.items.reverse();
    }
    l.icons.fill(ICON_UNKNOWN);
}

pub(crate) unsafe fn set_columns(app: *mut App, columns: &[Column]) {
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

pub(crate) unsafe fn refresh(app: *mut App) {
    if let Some(l) = listing(app) {
        go(app, Target::Pidl(l.folder.pidl.clone()), History::Refresh);
    }
}

pub(crate) unsafe fn go_back(app: *mut App) {
    if let Some(p) = cur(app).back.pop() {
        go(app, Target::Pidl(p), History::Back);
    }
}

pub(crate) unsafe fn go_forward(app: *mut App) {
    if let Some(p) = cur(app).forward.pop() {
        go(app, Target::Pidl(p), History::Forward);
    }
}

pub(crate) unsafe fn go_up(app: *mut App) {
    if let Some(p) = listing(app).and_then(|l| l.folder.parent_pidl()) {
        go(app, Target::Pidl(p), History::Push);
    }
}

/// Opens a folder in place, or runs the default verb on a file.
pub(crate) unsafe fn activate(app: *mut App, index: usize) {
    let hwnd = (*app).hwnd;
    let (folder_target, menu) = {
        let Some(l) = listing(app) else { return };
        let Some(item) = l.items.get(index) else { return };
        if item.is_folder() {
            (Some(l.folder.child_pidl(item)), None)
        } else {
            (None, ContextMenu::for_items(&l.folder, &[item], hwnd).ok())
        }
    };
    if let Some(p) = folder_target {
        go(app, Target::Pidl(p), History::Push);
    } else if let Some(menu) = menu {
        // The shell shows its own error UI when a file cannot be opened.
        let _ = menu.invoke_default(hwnd);
    }
}

// ---- tabs ---------------------------------------------------------------------------------------

/// Adds a tab loading `target`; the first tab is always active.
pub(crate) unsafe fn new_tab(app: *mut App, target: Target, activate: bool, select_after: Option<String>) {
    let name = match &target {
        Target::Name(n) => n.clone(),
        Target::Pidl(_) => String::new(),
    };
    let title = if name.is_empty() { "…".to_string() } else { name.clone() };
    (*app).tabs.push(Tab {
        name,
        listing: None,
        back: Vec::new(),
        forward: Vec::new(),
        watch: None,
        pending: 0,
        top: 0,
        selected: Vec::new(),
        select_after,
        crumbs: Vec::new(),
        icon: (0, 0),
    });
    let t = (*app).tabs.len() - 1;
    tabs::add((*app).strip, &title);
    navigate(app, t, target, History::Push);
    if t == 0 {
        show_tab(app, 0, false);
    } else if activate {
        switch_tab(app, t);
    }
    session_changed(app);
}

/// A new tab at the active tab's folder (Ctrl+T, the strip's + button).
pub(crate) unsafe fn duplicate_tab(app: *mut App) {
    let target = match listing(app) {
        Some(l) => Target::Pidl(l.folder.pidl.clone()),
        None => Target::Name((*app).profile.start.clone()),
    };
    new_tab(app, target, true, None);
}

pub(crate) unsafe fn switch_tab(app: *mut App, t: usize) {
    let old = (*app).active;
    if t >= (*app).tabs.len() || t == old {
        return;
    }
    {
        let list = (*app).list;
        let tab = &mut (&mut (*app).tabs)[old];
        tab.watch = None;
        tab.top = SendMessageW(list, LVM_GETTOPINDEX, None, None).0 as i32;
        tab.selected = match &tab.listing {
            Some(l) => list::selected_indices(list).iter().filter_map(|&i| l.items.get(i)).map(|i| i.parsing_name()).collect(),
            None => Vec::new(),
        };
    }
    show_tab(app, t, true);
}

/// Makes `t` the active tab without saving the previous one's view state.
unsafe fn show_tab(app: *mut App, t: usize, refresh_after: bool) {
    (*app).active = t;
    tabs::set_active((*app).strip, t);
    let tab = cur(app);
    let (top, selected) = (tab.top, std::mem::take(&mut tab.selected));
    let loaded = tab.listing.is_some();
    show(app, Show::Restore { top, selected, columns: true });
    // Inactive tabs are not watched, so catch up on changes made while away.
    if loaded && refresh_after {
        refresh(app);
    }
    session_changed(app);
}

pub(crate) unsafe fn close_tab(app: *mut App, t: usize) {
    let n = (*app).tabs.len();
    if t >= n {
        return;
    }
    if n == 1 {
        let _ = DestroyWindow((*app).hwnd);
        return;
    }
    let active = (*app).active;
    let removed = (*app).tabs.remove(t);
    tabs::remove((*app).strip, t);
    if t < active {
        (*app).active = active - 1;
        tabs::set_active((*app).strip, active - 1);
    } else if t == active {
        show_tab(app, t.min(n - 2), true);
    }
    drop(removed);
    session_changed(app);
}

pub(crate) unsafe fn cycle_tab(app: *mut App, delta: isize) {
    let n = (*app).tabs.len() as isize;
    if n > 1 {
        switch_tab(app, ((*app).active as isize + delta).rem_euclid(n) as usize);
    }
}

/// Middle-click on a folder item: open it in a background tab.
pub(crate) unsafe fn open_in_background(app: *mut App, index: usize) {
    let Some(l) = listing(app) else { return };
    let Some(item) = l.items.get(index) else { return };
    if item.is_folder() {
        new_tab(app, Target::Pidl(l.folder.child_pidl(item)), false, None);
    }
}

/// Opens the paths (and the `--select` target) of command-line args or a forwarded payload, each in a new tab.
/// Returns false when the args name nothing to open.
pub(crate) unsafe fn open_args(app: *mut App, a: &cli::Args) -> bool {
    for p in &a.paths {
        new_tab(app, Target::Name(p.clone()), true, None);
    }
    if let Some(sel) = &a.select {
        let parent = std::path::Path::new(sel).parent().map(|p| p.display().to_string()).filter(|p| !p.is_empty());
        new_tab(app, Target::Name(parent.unwrap_or_else(|| sel.clone())), true, Some(sel.clone()));
    }
    !a.paths.is_empty() || a.select.is_some()
}

// ---- session ------------------------------------------------------------------------------------

/// Saves now, or within 2 s when a save just happened.
pub(crate) unsafe fn session_changed(app: *mut App) {
    if (*app).debounce.should_save(std::time::Instant::now()) {
        save_session(app);
    } else {
        SetTimer(Some((*app).hwnd), TIMER_SESSION, 2000, None);
    }
}

pub(crate) unsafe fn save_session(app: *mut App) {
    let a = &mut *app;
    let named: Vec<(usize, &Tab)> = a.tabs.iter().enumerate().filter(|(_, t)| !t.name.is_empty()).collect();
    a.session.active = named.iter().position(|(i, _)| *i == a.active).unwrap_or(0);
    a.session.tabs = named.iter().map(|(_, t)| t.name.clone()).collect();
    session::save(&a.session);
}
