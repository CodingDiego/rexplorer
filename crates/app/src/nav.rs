//! Tabs and navigation: each tab owns its source (a shell listing, an archive, a search or the embedded browser)
//! and its history; the single ListView shows the active tab. Loads run on worker threads that post WM_ENUM_DONE.

use super::*;
use crate::source::Source;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use windows::Win32::Foundation::E_ABORT;
use windows::Win32::UI::Shell::{SHGetNameFromIDList, SHParseDisplayName, SIGDN, SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_NORMALDISPLAY};

/// The shell's Home ("Inicio") and Quick Access, as parsing names.
pub(crate) const HOME: &str = "::{F874310E-B6B7-47DC-BC84-B9E6B38F5903}";
pub(crate) const QUICK_ACCESS: &str = "::{679F85CB-0220-4080-B29B-5540CC05AAB6}";
/// A load that takes longer than this shows the skeleton; faster ones never flash it.
const SKELETON_DELAY: Duration = Duration::from_millis(120);
const PULSE_MS: u32 = 400;

#[derive(Clone)]
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

/// One breadcrumb segment, root first. Segments inside an archive have no PIDL and navigate by `parsing`.
#[derive(Clone)]
pub(crate) struct Crumb {
    pub name: String,
    pub parsing: String,
    pub pidl: Option<Pidl>,
}

pub(crate) struct Loaded {
    pub source: Source,
    pub crumbs: Vec<Crumb>,
    pub icon: (usize, i32),
}

pub(crate) struct EnumDone {
    generation: u64,
    history: History,
    result: std::result::Result<Loaded, LoadError>,
}

/// A name that does not resolve (or an archive that cannot be read). Folders that exist but cannot be listed
/// are not errors: they load as `Source::Browser`.
struct LoadError {
    name: String,
    err: Error,
}

pub(crate) struct Listing {
    pub folder: ShellFolder,
    pub items: Vec<ShellItem>,
    pub columns: Vec<Column>,
    pub icons: Vec<i32>,
    /// Text per row and header position, filled when first asked for (the shell calls behind it are not cheap); a
    /// row's cells are allocated then too.
    pub cells: Vec<std::cell::OnceCell<Box<[std::cell::OnceCell<String>]>>>,
    /// Shell column index, not header position.
    pub sort_col: u32,
    pub sort_desc: bool,
    /// Home and Quick Access: what the Home page shows, built with the listing.
    pub home: Option<home::Content>,
    /// A filesystem folder the file system refuses to read (listed empty): why (the status bar says it).
    pub denied: Option<String>,
}

/// Home or Quick Access: shown on the Home page instead of the list.
pub(crate) fn is_home(l: &Listing) -> bool {
    [HOME, QUICK_ACCESS].iter().any(|h| l.folder.parsing_name.eq_ignore_ascii_case(h))
}

pub(crate) struct Tab {
    /// Parsing name of the place shown (or requested, before the first load finishes); goes to the session.
    pub name: String,
    pub source: Option<Source>,
    pub back: Vec<Target>,
    pub forward: Vec<Target>,
    /// Watches the folder from the tab's first show until it closes, so a tab switch need not reload.
    pub watch: Option<ChangeWatch>,
    /// A change was notified in the folder since its load started: showing the tab reloads it.
    pub dirty: bool,
    /// Generation of the load in flight for this tab, 0 when none.
    pub pending: u64,
    /// Generation of the newest load started for this tab; an older one still running gives up after enumerating.
    pub latest: Arc<AtomicU64>,
    pub top: i32,
    pub selected: Vec<String>,
    /// Parsing name to select once the first load lands (`--select`).
    pub select_after: Option<String>,
    /// Search to start once the first load lands (`--search`).
    pub search_after: Option<String>,
    pub crumbs: Vec<Crumb>,
    /// System image list and index of the folder's small icon, for the breadcrumb.
    pub icon: (usize, i32),
    /// Where a navigation in flight is going; the chrome shows it until the load lands.
    pub loading: Option<Loading>,
    /// The Home page's pill in this tab (not saved).
    pub home_filter: home::Filter,
}

pub(crate) struct Loading {
    pub name: String,
    pub parsing: String,
    since: Instant,
}

/// How the active tab's source is put into the ListView.
pub(crate) enum Show {
    /// A new place: select `0` (by parsing name) if given, else focus the first item.
    Fresh(Vec<String>),
    /// Same place again (refresh or tab switch): keep the scroll position and selection.
    Restore { top: i32, selected: Vec<String>, columns: bool },
}

pub(crate) unsafe fn cur<'a>(app: *mut App) -> &'a mut Tab {
    let a = &mut *app;
    &mut a.tabs[a.active]
}

pub(crate) unsafe fn source<'a>(app: *mut App) -> Option<&'a Source> {
    let a = &*app;
    a.tabs.get(a.active).and_then(|t| t.source.as_ref())
}

/// The active tab's shell listing; None for archives, searches and the embedded browser.
pub(crate) unsafe fn listing<'a>(app: *mut App) -> Option<&'a Listing> {
    match source(app)? {
        Source::Shell(l) => Some(l),
        _ => None,
    }
}

// ---- loading ----------------------------------------------------------------------------------

/// Loads `target` into tab `t` on a worker thread.
pub(crate) unsafe fn navigate(app: *mut App, t: usize, target: Target, history: History) {
    (*app).generation += 1;
    crate::panes::stop_hover(app);
    let generation = (*app).generation;
    let Some(tab) = (&mut (*app).tabs).get_mut(t) else { return };
    tab.pending = generation;
    tab.latest.store(generation, Ordering::Relaxed);
    // A change notified from here on is not in this load.
    tab.dirty = false;
    if history != History::Refresh {
        (*app).new_item = None;
        let (name, parsing) = destination(&target);
        tabs::set_title((*app).strip, t, &name);
        tab.loading = Some(Loading { name, parsing, since: Instant::now() });
        if t == (*app).active {
            present_loading(app);
        }
    }
    let tab = &mut (&mut (*app).tabs)[t];
    // A new navigation stops a search still running in this tab; its results stay until the load lands.
    if let Some(Source::Search(s)) = &mut tab.source {
        s.stop();
    }
    // The worker sorts, as on_enum_done would: a refresh keeps the tab's sort, anything else starts by name.
    let sort = match &tab.source {
        Some(Source::Shell(l)) if history == History::Refresh => (l.sort_col, l.sort_desc),
        _ => (0, false),
    };
    let latest = tab.latest.clone();
    let hwnd = (*app).hwnd.0 as isize;
    let show_hidden = (*app).profile.show_hidden;
    // Details' first screen from the top; a refresh keeps its scroll position, so it (and theme icons) load as painted.
    let prefill = if history == History::Refresh || views::icon_theme_active(app) || (*app).views.view != views::View::Details {
        0
    } else {
        SendMessageW((*app).list, LVM_GETCOUNTPERPAGE, None, None).0.max(0) as usize + 1
    };
    std::thread::spawn(move || {
        let _com = ComInit::mta();
        let superseded = || latest.load(Ordering::Relaxed) != generation;
        let result = load(target, show_hidden, sort, prefill, &superseded);
        // A newer load of this tab would make on_enum_done drop this one anyway.
        if superseded() {
            return;
        }
        let done = Box::new(EnumDone { generation, history, result });
        let _ = PostMessageW(Some(HWND(hwnd as *mut c_void)), WM_ENUM_DONE, WPARAM(0), LPARAM(Box::into_raw(done) as isize));
    });
}

/// Navigates the active tab.
pub(crate) unsafe fn go(app: *mut App, target: Target, history: History) {
    navigate(app, (*app).active, target, history);
}

/// Worker side: the listing sorted by `sort` (shell column, descending), the first `prefill` icons looked up.
/// Once `superseded` says a newer load of the tab started, gives up after enumerating (an error nobody reads).
fn load(target: Target, show_hidden: bool, sort: (u32, bool), prefill: usize, superseded: &dyn Fn() -> bool) -> std::result::Result<Loaded, LoadError> {
    if let Target::Name(n) = &target {
        if let Some((archive, dir)) = archive_view::split_archive_path(n).filter(|(a, _)| a.is_file()) {
            return source::load_archive(&archive, &dir).map_err(|err| LoadError { name: n.clone(), err });
        }
    }
    let folder = match target {
        // Windows without Home: its start place is This PC.
        Target::Name(n) => ShellFolder::parse(&n)
            .or_else(|e| if n.eq_ignore_ascii_case(settings::HOME_START) { ShellFolder::parse("shell:MyComputerFolder") } else { Err(e) })
            .map_err(|err| LoadError { name: n, err })?,
        Target::Pidl(p) => ShellFolder::from_pidl(&p).map_err(|err| LoadError { name: String::new(), err })?,
    };
    let crumbs = crumbs_of(&folder);
    let icon = folder_icon(&folder.pidl);
    // A filesystem folder Windows refuses to read (C:\System Volume Information): why, for the status bar.
    let why = || ui::is_fs_path(&folder.parsing_name).then(|| std::fs::read_dir(&folder.parsing_name).err()).flatten().filter(|e| e.kind() == std::io::ErrorKind::PermissionDenied).map(|e| e.to_string());
    let (mut items, mut denied) = match folder.enumerate() {
        Ok(items) => (items, None),
        Err(_) => match why() {
            // An empty listing that says why: the embedded Explorer view could not read it either.
            Some(e) => (Vec::new(), Some(e)),
            // The folder exists but we cannot list it (a phone, an odd namespace extension): the embedded Explorer view.
            None => return Ok(Loaded { source: Source::Browser(source::BrowserPlace::of(&folder)), crumbs, icon }),
        },
    };
    if superseded() {
        return Err(LoadError { name: String::new(), err: E_ABORT.into() });
    }
    if !show_hidden {
        items.retain(|i| !i.is_hidden());
    }
    if items.is_empty() && denied.is_none() {
        denied = why();
    }
    let columns = folder.columns();
    let icons = vec![ICON_UNKNOWN; items.len()];
    let cells = vec![std::cell::OnceCell::new(); items.len()];
    let mut l = Listing { folder, items, columns, icons, cells, sort_col: sort.0, sort_desc: sort.1, home: None, denied };
    if is_home(&l) {
        l.home = Some(home::build(&l.folder, &l.items));
    }
    // After the Home build: its tiles keep the enumeration order.
    unsafe { sort_listing(&mut l) };
    // Home's list is hidden, nothing asks it for icons.
    if l.home.is_none() {
        for (icon, item) in l.icons.iter_mut().zip(&l.items).take(prefill) {
            *icon = item.icon_index(&l.folder.pidl).1;
        }
    }
    Ok(Loaded { source: Source::Shell(l), crumbs, icon })
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
pub(crate) fn crumbs_of(folder: &ShellFolder) -> Vec<Crumb> {
    let crumb = |f: &ShellFolder| Crumb { name: f.name.clone(), parsing: f.parsing_name.clone(), pidl: Some(f.pidl.clone()) };
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
    let was_loading = (&mut (*app).tabs)[t].loading.take().is_some();
    let hwnd = (*app).hwnd;
    let list = (*app).list;
    let active = t == (*app).active;
    let loaded = match done.result {
        Err(LoadError { name, err }) => {
            // Decide about the tab before the message box: it pumps messages and other loads may land meanwhile.
            if (&mut (*app).tabs)[t].source.is_none() && (*app).tabs.len() > 1 {
                close_tab(app, t);
            } else if was_loading {
                // The old place comes back: its items, title and crumbs.
                if let Some(s) = &(&(*app).tabs)[t].source {
                    tabs::set_title((*app).strip, t, &s.title());
                }
                if active {
                    save_view_state(app);
                    let tab = cur(app);
                    let (top, selected) = (tab.top, std::mem::take(&mut tab.selected));
                    show(app, Show::Restore { top, selected, columns: true });
                }
            } else if let Some(s) = source(app) {
                set_text((*app).addr, &s.editing_name());
            }
            message(hwnd, &format!("Cannot open {}\n\n{err}", crate::connect::redact(&name)));
            return;
        }
        Ok(loaded) => loaded,
    };

    let refresh = done.history == History::Refresh;
    let tab = &mut (&mut (*app).tabs)[t];
    let old = tab.source.take();
    let (mut sort_col, mut sort_desc) = (0, false);
    let mut keep: Vec<String> = Vec::new();
    let mut top = 0;
    if let Some(old) = &old {
        match done.history {
            History::Push => {
                tab.back.push(old.location());
                tab.forward.clear();
            }
            History::Back => tab.forward.push(old.location()),
            History::Forward => tab.back.push(old.location()),
            History::Refresh => {
                if let Source::Shell(old) = old {
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
    }
    drop(old);
    if let Some(sel) = tab.select_after.take() {
        keep = vec![sel];
    }

    let mut src = loaded.source;
    if let Source::Shell(l) = &mut src {
        // load() sorted by the sort the tab had then; only a sort changed meanwhile (a header click) sorts again.
        if (l.sort_col, l.sort_desc) != (sort_col, sort_desc) {
            l.sort_col = sort_col;
            l.sort_desc = sort_desc;
            sort_listing(l);
        }
    }
    let shell = matches!(src, Source::Shell(_));
    tab.name = src.parsing_name();
    tab.crumbs = loaded.crumbs;
    tab.icon = loaded.icon;
    tabs::set_title((*app).strip, t, &crate::connect::redact(&src.title()));
    if !refresh {
        (*app).recent.push(&crate::connect::redact(&tab.name)); // recents are saved to disk
        (*app).recent.save();
    }
    tab.source = Some(src);
    if active {
        show(app, if refresh { Show::Restore { top, selected: keep, columns: false } } else { Show::Fresh(keep) });
        commands::rename_new_item(app);
    } else {
        tab.top = top;
        tab.selected = keep;
    }
    let pending_search = if shell { (&mut (*app).tabs)[t].search_after.take() } else { None };
    if let Some(q) = pending_search {
        source::start_search(app, t, &q, true);
    }
    // A refresh reloads the same place; saving there made the settings folder refresh itself via session.txt.
    if !refresh {
        session_changed(app);
    }
}

/// Puts the active tab into the list (or the embedded browser), address bar, breadcrumb, title and toolbar; a
/// navigation still in flight then shows its destination over it.
pub(crate) unsafe fn show(app: *mut App, mode: Show) {
    stop_skeleton(app);
    show_source(app, mode);
    if cur(app).loading.is_some() {
        present_loading(app);
    }
}

unsafe fn show_source(app: *mut App, mode: Show) {
    let hwnd = (*app).hwnd;
    let list = (*app).list;
    let tab = cur(app);
    let Some(src) = &tab.source else {
        source::leave_browser(app);
        views::reset(app, 0);
        SendMessageW(list, LVM_SETITEMCOUNT, Some(WPARAM(0)), Some(LPARAM(0)));
        panes::schedule_preview(app);
        set_columns(app, &[]);
        set_text((*app).addr, &crate::connect::redact(&tab.name));
        set_text(hwnd, "rexplorer");
        ui::set_crumbs(app);
        sync_home(app);
        update_chrome(app);
        return;
    };

    match src {
        Source::Browser(b) => source::enter_browser(app, b),
        _ => source::leave_browser(app),
    }
    if let Source::Shell(l) = src {
        if !(*app).image_list_set {
            if let Some(first) = l.items.first() {
                let (himl, _) = first.icon_index(&l.folder.pidl);
                if himl != 0 {
                    views::set_small_source(app, HIMAGELIST(himl as isize));
                }
            }
        }
    }
    if !matches!(mode, Show::Restore { columns: false, .. }) {
        set_columns(app, &src.columns());
    }
    // The shell's FTP folder keeps user:password in its names; nothing on screen shows the password.
    set_text((*app).addr, &crate::connect::redact(&src.editing_name()));
    set_text(hwnd, &format!("{} - rexplorer", crate::connect::redact(&src.title())));
    // Rewriting text the user is typing would move the caret to the start and drop trailing spaces.
    let needle = if let Source::Search(s) = src { s.needle.as_str() } else { "" };
    if window_text((*app).search).trim() != needle {
        set_text((*app).search, needle);
    }
    ui::set_crumbs(app);
    sidebar::select((*app).sidebar, &tab.name);
    match src {
        // Afresh on every show, the old one dropped first: one kept through steady churn gets 3 s SHCNE_UPDATEDIR batches.
        Source::Shell(l) => {
            tab.watch = None;
            tab.watch = ChangeWatch::register(hwnd, &l.folder, WM_SHELL_CHANGE);
        }
        _ => tab.watch = None,
    }

    let count = src.len();
    views::reset(app, count);
    // The list asks for item text synchronously from here on, so the source must be in place first.
    SendMessageW(list, LVM_SETITEMCOUNT, Some(WPARAM(count)), Some(LPARAM(LVSICF_NOSCROLL as isize)));
    list::set_item_state(list, usize::MAX, LIST_VIEW_ITEM_STATE_FLAGS(0), LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0));
    if !matches!(mode, Show::Restore { columns: false, .. }) {
        panes::sync_tree(app);
    }
    let select = |names: &[String]| -> Option<usize> {
        let Source::Shell(l) = src else { return None };
        let hits = selection_matches(names, l.items.iter().map(|i| i.parsing_name()));
        for &i in &hits {
            list::set_item_state(list, i, LVIS_SELECTED, LVIS_SELECTED);
        }
        hits.first().copied()
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
    panes::schedule_preview(app);
    // The tree builds on this thread and can take a while (Quick Access, slow drives): only after the first
    // listing is on screen (WM_TIMER comes after paint).
    if (*app).session.show_tree && (*app).panes.tree.is_none() {
        SetTimer(Some(hwnd), TIMER_TREE, 1, None);
    }
    sync_home(app);
    update_chrome(app);
}

/// Indices of the items whose parsing name is one of `names`, stopping at the last match. Exact matches come
/// first (a case-sensitive folder can hold `a.txt` and `A.txt`); a name with none falls back to an ASCII
/// case-insensitive match on an item not already hit, so a rename that only changed case still reselects.
fn selection_matches(names: &[String], mut parsing: impl Iterator<Item = String>) -> Vec<usize> {
    let mut wanted: std::collections::HashSet<&str> = names.iter().map(String::as_str).collect();
    let mut seen = Vec::new(); // kept for the fallback pass: parsing a name is a shell call
    let mut hits = Vec::new();
    while !wanted.is_empty() {
        let Some(p) = parsing.next() else { break };
        if wanted.remove(p.as_str()) {
            hits.push(seen.len());
        }
        seen.push(p);
    }
    if !wanted.is_empty() {
        // The walk ran out of items, so `seen` is all of them.
        let mut loose: std::collections::HashSet<String> = wanted.iter().map(|n| n.to_ascii_lowercase()).collect();
        let exact: std::collections::HashSet<usize> = hits.iter().copied().collect();
        for (i, p) in seen.iter().enumerate() {
            if loose.is_empty() {
                break;
            }
            if !exact.contains(&i) && loose.remove(&p.to_ascii_lowercase()) {
                hits.push(i);
            }
        }
        hits.sort_unstable();
    }
    hits
}

// ---- the Home page ----------------------------------------------------------------------------------

/// Shows the Home page in the list's place when the active tab is Home or Quick Access (and not showing the
/// skeleton), with the list's selection; otherwise the list. The hidden list keeps the items and mirrors the
/// page's selection, so every command, the status bar and the preview work on the page as on the list.
pub(crate) unsafe fn sync_home(app: *mut App) {
    let shown = match source(app) {
        Some(Source::Shell(l)) if !(*app).skeleton => l.home.as_ref().map(|c| (l, c)),
        _ => None,
    };
    let Some((l, content)) = shown else { return show_home(app, false) };
    let index: std::collections::HashMap<String, usize> = l.items.iter().enumerate().map(|(i, it)| (it.parsing_name().to_lowercase(), i)).collect();
    let mut content = content.clone();
    for e in content.tiles.iter_mut().chain(content.rows.iter_mut()) {
        e.index = index.get(&e.key.to_lowercase()).copied().unwrap_or(usize::MAX);
    }
    let (h, list) = ((*app).home, (*app).list);
    home::set_filter(h, cur(app).home_filter);
    home::set_content(h, content);
    let focus = SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_FOCUSED as isize))).0;
    home::set_selection(h, &list::selected_indices(list), usize::try_from(focus).ok());
    show_home(app, true);
}

/// Swaps the Home page and the list (the embedded browser, when it shows, keeps the list hidden); the keyboard
/// focus follows.
unsafe fn show_home(app: *mut App, on: bool) {
    let (h, list) = ((*app).home, (*app).list);
    if IsWindowVisible(h).as_bool() == on {
        return;
    }
    let focus = GetFocus();
    let _ = ShowWindow(h, if on { SW_SHOW } else { SW_HIDE });
    if (*app).browser.is_none() {
        let _ = ShowWindow(list, if on { SW_HIDE } else { SW_SHOW });
    }
    if focus == h || focus == list {
        let _ = SetFocus(Some(ui::view_hwnd(app)));
    }
}

/// WM_NOTIFY from the Home page.
pub(crate) unsafe fn on_home_notify(app: *mut App, nm: &home::NmHome) {
    let list = (*app).list;
    match nm.hdr.code {
        home::HN_SELCHANGE => {
            let (sel, focus) = home::selection((*app).home);
            let none = LIST_VIEW_ITEM_STATE_FLAGS(0);
            list::set_item_state(list, usize::MAX, none, LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0));
            for i in sel {
                list::set_item_state(list, i, LVIS_SELECTED, LVIS_SELECTED);
            }
            if let Some(f) = focus {
                list::set_item_state(list, f, LVIS_FOCUSED, LVIS_FOCUSED);
            }
        }
        home::HN_ACTIVATE => source::activate(app, nm.index),
        home::HN_CONTEXT => list::show_menu(app, nm.index != usize::MAX, None),
        home::HN_CONTEXT_KEY => list::show_menu(app, nm.index != usize::MAX, Some(home::menu_point((*app).home))),
        home::HN_KEYDOWN => list::on_key(app, VIRTUAL_KEY(nm.index as u16)),
        home::HN_RENAME => list::rename_item(app, nm.index, &home::rename_text((*app).home), false),
        home::HN_FILTER => cur(app).home_filter = home::filter((*app).home),
        _ => {}
    }
}

/// Toolbar enabled states and the status bar, after anything that changes the active tab.
pub(crate) unsafe fn update_chrome(app: *mut App) {
    let tab = cur(app);
    let nav = (*app).nav;
    toolbar::set_enabled(nav, CMD_BACK, !tab.back.is_empty());
    toolbar::set_enabled(nav, CMD_FORWARD, !tab.forward.is_empty());
    toolbar::set_enabled(nav, CMD_UP, tab.source.as_ref().is_some_and(|s| s.parent().is_some()));
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
    l.cells.fill(Default::default());
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

/// Reloads the active tab; a search runs again. Nothing while a navigation is in flight: reloading the place being
/// left (a change in it, a tab switch) would cancel the navigation.
pub(crate) unsafe fn refresh(app: *mut App) {
    if cur(app).loading.is_some() {
        return;
    }
    // A reload would end a rename in progress (a new item's, right after its creation): wait for it.
    if SendMessageW((*app).list, LVM_GETEDITCONTROL, None, None).0 != 0 {
        SetTimer(Some((*app).hwnd), TIMER_REFRESH, 500, None);
        return;
    }
    let target = match source(app) {
        Some(Source::Search(s)) => {
            let needle = s.needle.clone();
            source::start_search(app, (*app).active, &needle, false);
            return;
        }
        Some(s) => s.location(),
        None => return,
    };
    // This reload covers the changes notified so far.
    let _ = KillTimer(Some((*app).hwnd), crate::TIMER_CHANGE);
    (*app).change_armed = false;
    go(app, target, History::Refresh);
}

/// WM_SHELL_CHANGE: marks the tabs whose folder the notified `items` are about as changed. Returns whether the
/// active tab is one. A notification that names no item, or no watched folder, concerns every tab.
pub(crate) unsafe fn mark_changed(app: *mut App, items: &[Pidl]) -> bool {
    let hit = |t: &Tab| t.watch.as_ref().is_some_and(|w| items.iter().any(|p| w.concerns(p)));
    let all = !(&(*app).tabs).iter().any(hit);
    // Watched tabs only: a search, archive or browser tab has nothing a shell change makes stale.
    let marks = |t: &Tab| t.watch.is_some() && (all || hit(t));
    for t in (&mut (*app).tabs).iter_mut() {
        t.dirty |= marks(t);
    }
    (&(*app).tabs).get((*app).active).is_some_and(marks)
}

pub(crate) unsafe fn go_back(app: *mut App) {
    if let Some(p) = cur(app).back.pop() {
        go(app, p, History::Back);
    }
}

pub(crate) unsafe fn go_forward(app: *mut App) {
    if let Some(p) = cur(app).forward.pop() {
        go(app, p, History::Forward);
    }
}

pub(crate) unsafe fn go_up(app: *mut App) {
    if let Some(p) = source(app).and_then(|s| s.parent()) {
        go(app, p, History::Push);
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
        source: None,
        back: Vec::new(),
        forward: Vec::new(),
        watch: None,
        dirty: false,
        pending: 0,
        latest: Arc::default(),
        top: 0,
        selected: Vec::new(),
        select_after,
        search_after: None,
        crumbs: Vec::new(),
        icon: (0, 0),
        loading: None,
        home_filter: home::Filter::Recent,
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

/// Ctrl+T and the tab strip's +: a tab at the profile's start place, as in Explorer.
pub(crate) unsafe fn new_start_tab(app: *mut App) {
    new_tab(app, Target::Name((*app).profile.start.clone()), true, None);
}

pub(crate) unsafe fn switch_tab(app: *mut App, t: usize) {
    let old = (*app).active;
    if t >= (*app).tabs.len() || t == old {
        return;
    }
    save_view_state(app);
    show_tab(app, t, true);
}

/// Keeps the active tab's scroll position and selection in the tab, unless the skeleton already replaced them.
unsafe fn save_view_state(app: *mut App) {
    if (*app).skeleton {
        return;
    }
    let list = (*app).list;
    let tab = cur(app);
    tab.top = SendMessageW(list, LVM_GETTOPINDEX, None, None).0 as i32;
    tab.selected = match &tab.source {
        Some(Source::Shell(l)) => list::selected_indices(list).iter().filter_map(|&i| l.items.get(i)).map(|i| i.parsing_name()).collect(),
        _ => Vec::new(),
    };
}

// ---- loading state ------------------------------------------------------------------------------

/// Display and parsing name of where a navigation goes, without loading it: PIDLs and `shell:` / `::{CLSID}`
/// names resolve in the namespace (no disk access); anything else shows as typed.
fn destination(target: &Target) -> (String, String) {
    let names = |p: *const windows::Win32::UI::Shell::Common::ITEMIDLIST| unsafe {
        let get = |k: SIGDN| SHGetNameFromIDList(p, k).map(|s| rexplorer_core::com::take_cotaskmem_string(s)).unwrap_or_default();
        (get(SIGDN_NORMALDISPLAY), get(SIGDN_DESKTOPABSOLUTEPARSING))
    };
    match target {
        Target::Pidl(p) => names(p.as_ptr()),
        Target::Name(n) => {
            let lower = n.to_ascii_lowercase();
            if lower.starts_with("shell:") || lower.starts_with("::") {
                let wide = HSTRING::from(n.as_str());
                let mut abs = std::ptr::null_mut();
                if unsafe { SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut abs, 0, None) }.is_ok() && !abs.is_null() {
                    let p = unsafe { Pidl::from_raw(abs) };
                    let (name, parsing) = names(p.as_ptr());
                    if !name.is_empty() {
                        return (name, parsing);
                    }
                }
            }
            (crate::connect::redact(n), n.clone()) // the name is shown, the parsing name keeps the password for the load
        }
    }
}

/// The active tab is loading: its destination in the title, tab, breadcrumb, address and sidebar; "Loading..."
/// in the status bar; the skeleton once the load has taken longer than SKELETON_DELAY.
unsafe fn present_loading(app: *mut App) {
    let hwnd = (*app).hwnd;
    let tab = cur(app);
    let Some(l) = &tab.loading else { return };
    set_text(hwnd, &format!("{} - rexplorer", l.name));
    if !IsWindowVisible((*app).addr).as_bool() {
        set_text((*app).addr, &crate::connect::redact(&l.parsing));
    }
    sidebar::select((*app).sidebar, &l.parsing);
    let waited = l.since.elapsed();
    ui::set_crumbs(app);
    if waited >= SKELETON_DELAY {
        begin_skeleton(app);
    } else {
        SetTimer(Some(hwnd), TIMER_SKELETON, (SKELETON_DELAY - waited).as_millis() as u32 + 1, None);
    }
    ui::update_status(app);
}

/// TIMER_SKELETON: a load (or a search with nothing found yet) is still running in the active tab.
pub(crate) unsafe fn on_skeleton_timer(app: *mut App) {
    let tab = cur(app);
    if tab.loading.is_some() {
        present_loading(app);
    } else if matches!(&tab.source, Some(Source::Search(s)) if s.running && s.rows.is_empty()) {
        begin_skeleton(app);
    }
}

/// Empties the list (the old items stay in the tab) and lets it draw placeholder rows that pulse.
unsafe fn begin_skeleton(app: *mut App) {
    if (*app).skeleton {
        return;
    }
    save_view_state(app);
    (*app).skeleton = true;
    show_home(app, false);
    let list = (*app).list;
    views::reset(app, 0);
    SendMessageW(list, LVM_SETITEMCOUNT, Some(WPARAM(0)), Some(LPARAM(0)));
    panes::schedule_preview(app);
    SetTimer(Some((*app).hwnd), TIMER_PULSE, PULSE_MS, None);
    let _ = InvalidateRect(Some(list), None, true);
}

/// TIMER_PULSE: the skeleton alternates between two shades.
pub(crate) unsafe fn on_pulse_timer(app: *mut App) {
    if (*app).skeleton {
        (*app).pulse = !(*app).pulse;
        let _ = InvalidateRect(Some((*app).list), None, false);
        SetTimer(Some((*app).hwnd), TIMER_PULSE, PULSE_MS, None);
    }
}

pub(crate) unsafe fn stop_skeleton(app: *mut App) {
    if std::mem::take(&mut (*app).skeleton) {
        let _ = KillTimer(Some((*app).hwnd), TIMER_PULSE);
        let _ = InvalidateRect(Some((*app).list), None, true);
    }
}

/// Makes `t` the active tab without saving the previous one's view state.
unsafe fn show_tab(app: *mut App, t: usize, refresh_after: bool) {
    (*app).active = t;
    tabs::set_active((*app).strip, t);
    let tab = cur(app);
    let (top, selected) = (tab.top, std::mem::take(&mut tab.selected));
    // A watched folder reloads only if it changed while away. Virtual folders (Home) get no matching notifications,
    // shares may send none (WSL, WebDAV), and a tab whose load landed while away watches none or the folder it left:
    // those catch up on every show.
    let catch_up = matches!(&tab.source, Some(Source::Shell(l)) if tab.dirty || !tab.watch.as_ref().is_some_and(|w| w.is_on(&l.folder.pidl)) || !ui::is_fs_path(&l.folder.parsing_name) || ui::is_remote(&l.folder.parsing_name));
    show(app, Show::Restore { top, selected, columns: true });
    if catch_up && refresh_after {
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

/// `--search <text>`: searches the active tab now, or once its load lands. Call after the tabs exist.
pub(crate) unsafe fn apply_search(app: *mut App, a: &cli::Args) {
    let Some(q) = a.search.clone() else { return };
    if (*app).tabs.is_empty() {
        return;
    }
    let tab = cur(app);
    if tab.pending == 0 && matches!(tab.source, Some(Source::Shell(_) | Source::Search(_))) {
        source::start_search(app, (*app).active, &q, true);
    } else {
        tab.search_after = Some(q);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destinations_resolve_namespace_names_and_keep_typed_paths() {
        let _com = ComInit::sta();
        let (name, parsing) = destination(&Target::Name(format!("shell:{HOME}")));
        assert_eq!(parsing, HOME);
        assert!(!name.is_empty() && !name.starts_with("shell:"), "the shell's (localized) name: {name:?}");
        let pc = ShellFolder::parse("shell:MyComputerFolder").unwrap();
        assert_eq!(destination(&Target::Pidl(pc.pidl.clone())), (pc.name.clone(), pc.parsing_name.clone()));
        let typed = r"C:\x\a.rar\docs".to_string();
        assert_eq!(destination(&Target::Name(typed.clone())), (typed.clone(), typed));
        let bad = "shell:NoSuchFolder".to_string();
        assert_eq!(destination(&Target::Name(bad.clone())), (bad.clone(), bad));
    }

    fn shell_listing(dir: &std::path::Path, sort: (u32, bool), prefill: usize) -> Listing {
        match load(Target::Name(dir.display().to_string()), true, sort, prefill, &|| false) {
            Ok(Loaded { source: Source::Shell(l), .. }) => l,
            _ => panic!("a shell listing"),
        }
    }

    #[test]
    fn load_sorts_on_the_worker_and_prefills_icons_and_caches_cells() {
        let _com = ComInit::mta();
        let dir = std::env::temp_dir().join(format!("rexplorer-nav-load-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for n in ["c.txt", "a.txt", "b.txt"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let names = |l: &Listing| l.items.iter().map(|i| i.display_name().trim_end_matches(".txt").to_string()).collect::<Vec<_>>();
        let l = shell_listing(&dir, (0, false), 3);
        assert_eq!(names(&l), ["sub", "a", "b", "c"], "folders first, then by name");
        assert!(l.icons[..3].iter().all(|&i| i != ICON_UNKNOWN), "the first icons are looked up on the worker");
        assert_eq!(l.icons[3], ICON_UNKNOWN, "only as many as asked for");
        assert_eq!(shell_listing(&dir, (0, true), 0).icons, vec![ICON_UNKNOWN; 4], "no prefill for a refresh or an icon theme");
        assert_eq!(names(&shell_listing(&dir, (0, true), 0)), ["c", "b", "a", "sub"]);

        let src = Source::Shell(l);
        let Source::Shell(l) = &src else { unreachable!() };
        assert!(l.cells.iter().all(|row| row.get().is_none()), "rows wait until asked for");
        let name = src.text(1, 0);
        assert_eq!(src.text(1, 0), name);
        assert_eq!(l.cells[1].get().unwrap()[0].get(), Some(&name));
        assert!(l.cells[1].get().unwrap().get(1).is_none_or(|c| c.get().is_none()), "other cells wait until asked for");
        assert!(l.cells[2].get().is_none(), "other rows too");
        assert_eq!(src.text(9, 0), "");
        let mut src = src;
        let Source::Shell(l) = &mut src else { unreachable!() };
        unsafe { sort_listing(l) };
        assert!(l.cells[1].get().is_none(), "sorting moves the rows");
        assert!(load(Target::Name(dir.display().to_string()), true, (0, false), 0, &|| true).is_err(), "superseded");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn selection_matches_exact_first_then_ascii_case_insensitively() {
        let s = |v: &[&str]| v.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        let names = s(&["C:\\a.txt", "c:\\b.txt"]);
        let parsing = ["C:\\a.txt", "c:\\x", "c:\\b.txt", "never"].into_iter().enumerate().map(|(i, p)| {
            assert!(i < 3, "read past the last exact match");
            p.to_string()
        });
        assert_eq!(selection_matches(&names, parsing), [0, 2]);
        assert_eq!(selection_matches(&s(&["C:\\a.txt"]), s(&["C:\\A.txt", "C:\\a.txt"]).into_iter()), [1], "the exact one wins in a case-sensitive folder");
        assert_eq!(selection_matches(&s(&["C:\\a.txt"]), s(&["C:\\A.txt"]).into_iter()), [0], "a rename that only changed case still reselects");
        assert_eq!(selection_matches(&s(&["C:\\a.txt", "C:\\A.txt"]), s(&["C:\\a.txt"]).into_iter()), [0], "an item already hit is not taken twice");
        assert_eq!(selection_matches(&s(&["C:\\A.txt", "c:\\B.TXT"]), s(&["c:\\a.TXT", "c:\\x", "C:\\b.txt"]).into_iter()), [0, 2]);
        assert!(selection_matches(&["É".to_string()], ["é".to_string()].into_iter()).is_empty(), "ASCII only, as eq_ignore_ascii_case");
        assert!(selection_matches(&[], std::iter::once_with(|| -> String { panic!("no names, nothing to parse") })).is_empty());
    }
}
