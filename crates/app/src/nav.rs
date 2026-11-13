//! Tabs and navigation: each tab owns its source (a shell listing, an archive, a search or the embedded browser)
//! and its history; the single ListView shows the active tab. Loads run on worker threads that post WM_ENUM_DONE.

use super::*;
use crate::source::Source;
use crate::ui::{Entry, hinted};
use rexplorer_core::i18n::{tr, trf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use windows::Win32::Foundation::{E_ABORT, FILETIME, SYSTEMTIME};
use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_IFIRSTDAYOFWEEK, LOCALE_RETURN_NUMBER};
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime};
use windows::Win32::UI::Shell::Common::SHCOLSTATE_TYPE_DATE;
use windows::Win32::UI::Shell::{FOLDERID_Downloads, SHGetNameFromIDList, SHParseDisplayName, SIGDN, SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_NORMALDISPLAY};

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
    /// Search results the tab keeps (`Tab::kept`, by the search's generation); once dropped, the folder searched.
    Search(u64, Pidl),
}

/// Search results a tab left, as they were: Back shows them again without searching.
pub(crate) struct Kept {
    pub listing: crate::source::SearchListing,
    pub top: i32,
    pub selected: Vec<String>,
}

/// Search results a tab keeps for its history; older ones go back to their folder.
const KEPT: usize = 4;

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
    /// The worker left the listing unsorted, ungrouped and without icons: the UI thread sorts it once it holds it.
    pub unsorted: bool,
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
    /// The shell column the rows are grouped by (None: no groups); `groups` holds (header, first row), in row order.
    pub group: Option<u32>,
    pub groups: Vec<(String, usize)>,
    /// The shown columns in display order as (shell index, width in 96-DPI pixels, 0 = the shell's default); empty
    /// until the user changes them.
    pub layout: Vec<(u32, i32)>,
    /// The view this folder was last left in (None: the last one chosen anywhere).
    pub view: Option<views::View>,
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
    /// Places left, with the label the history menu shows.
    pub back: Vec<(Target, String)>,
    pub forward: Vec<(Target, String)>,
    /// Entries a multi-step jump went past; the next landing puts them on the other stack in order.
    pub skipped: Vec<(Target, String)>,
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
    /// Search results in this tab's history, oldest first, at most KEPT.
    pub kept: Vec<Kept>,
    /// A delete the user made here, for the reload that follows it.
    pub deleted: Option<Deleted>,
}

/// A delete of the selection the user made in a folder. The reload after it selects the row that took the deleted
/// rows' place, as Explorer does; any other reload leaves nothing selected when the selected items are gone (another
/// program's rename, an editor's save by replace, an undo): a row the user never chose must not take their next key.
pub(crate) struct Deleted {
    folder: String,
    names: Vec<String>,
    /// Parsing name of the row that takes the selection (`row_taking_place`).
    next: Option<String>,
    /// When the delete returned; None while it runs (its confirmation and progress pump messages, so reloads land).
    at: Option<Instant>,
}

/// How long a finished delete waits for its reload.
const DELETED_WAIT: Duration = Duration::from_secs(10);

impl Deleted {
    /// The delete returned: kept only when one of its items is known to be gone. A cancel in the confirmation
    /// deletes nothing and disarms; so does a delete of items that are not files (nothing tells they went).
    fn over(mut self, now: Instant, gone: impl Fn(&str) -> bool) -> Option<Self> {
        self.at = Some(now);
        self.names.iter().any(|n| gone(n)).then_some(self)
    }

    /// The row a finished load of `folder` selects when the listing no longer holds the selection it replaced
    /// (`replaced`): only a reload of the folder deleted in, soon after the delete, of a selection of deleted rows.
    /// Never on Home or Quick Access, whose page orders the items its own way (the list behind it would select a
    /// row the page does not show next to the deleted one).
    fn next_for(self, refresh: bool, folder: &str, home: bool, replaced: &[String], now: Instant) -> Option<String> {
        let recent = self.at.is_some_and(|at| now.duration_since(at) < DELETED_WAIT);
        let names: std::collections::HashSet<&str> = self.names.iter().map(String::as_str).collect();
        let ours = refresh && !home && recent && self.folder.eq_ignore_ascii_case(folder) && !replaced.is_empty() && replaced.iter().all(|r| names.contains(r.as_str()));
        if ours { self.next } else { None }
    }
}

/// The row that takes the selection after the `selected` rows (ascending) go: the first survivor after the focused
/// one (the first selected when the focus is elsewhere), else the last survivor before it; None when none survives.
pub(crate) fn row_taking_place(len: usize, selected: &[usize], focused: Option<usize>) -> Option<usize> {
    let at = focused.filter(|f| selected.binary_search(f).is_ok()).or_else(|| selected.first().copied())?;
    let survives = |i: &usize| selected.binary_search(i).is_err();
    (at + 1..len).find(survives).or_else(|| (0..at.min(len)).rev().find(survives))
}

/// Arms the active tab for the reload after the user deletes `rows` of its folder (any delete route calls this
/// before the delete runs and `delete_over` after); returns the tab, which the delete's dialogs may move.
pub(crate) unsafe fn delete_starts(app: *mut App, rows: &[usize]) -> Arc<AtomicU64> {
    let list = (*app).list;
    let focused = usize::try_from(SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_FOCUSED as isize))).0).ok();
    let tab = cur(app);
    tab.deleted = match &tab.source {
        Some(Source::Shell(l)) => Some(Deleted {
            folder: l.folder.parsing_name.clone(),
            names: rows.iter().filter_map(|&i| l.items.get(i)).map(|i| i.parsing_name()).collect(),
            next: row_taking_place(l.items.len(), rows, focused).and_then(|i| l.items.get(i)).map(|i| i.parsing_name()),
            at: None,
        }),
        _ => None,
    };
    tab.latest.clone()
}

/// The delete `delete_starts` armed has returned; a cancelled one disarms (`Deleted::over`).
pub(crate) unsafe fn delete_over(app: *mut App, id: &Arc<AtomicU64>) {
    let Some(t) = find_tab((*app).tabs.iter().map(|x| &x.latest), id) else { return };
    let tab = &mut (&mut (*app).tabs)[t];
    let gone = |n: &str| ui::is_fs_path(n) && matches!(std::path::Path::new(n).try_exists(), Ok(false));
    tab.deleted = tab.deleted.take().and_then(|d| d.over(Instant::now(), gone));
}

impl Tab {
    /// The history entry for the place this tab leaves. Search results stop and are kept with the tab's scroll and
    /// selection (saved by the caller); anything else reloads from its location.
    pub(crate) fn leave(&mut self, old: Source) -> (Target, String) {
        let label = old.title();
        match old {
            Source::Search(mut s) => {
                s.stop();
                let target = Target::Search(s.generation, s.root.clone());
                self.kept.push(Kept { listing: s, top: self.top, selected: std::mem::take(&mut self.selected) });
                if self.kept.len() > KEPT {
                    self.kept.remove(0);
                }
                (target, label)
            }
            other => (other.location(), label),
        }
    }
}

pub(crate) struct Loading {
    pub name: String,
    pub parsing: String,
    since: Instant,
    history: History,
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

/// Loads `target` into tab `t` on a worker thread. On this (UI) thread only `destination` looks at the target, and
/// it parses nothing but `shell:`/`::` names: a path to an unreachable share cannot block the window.
#[allow(clippy::needless_borrow)] // the explicit borrow of a raw-pointer field silences rustc's dangerous_implicit_autorefs
pub(crate) unsafe fn navigate(app: *mut App, t: usize, target: Target, history: History) {
    let _op = crate::watchdog::op("navigate");
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
        tab.deleted = None;
        let (name, parsing) = destination(&target);
        tabs::set_title((*app).strip, t, &name);
        tab.loading = Some(Loading { name, parsing, since: Instant::now(), history });
        if t == (*app).active {
            present_loading(app);
        }
    }
    let tab = &mut (&mut (*app).tabs)[t];
    // A new navigation stops a search still running in this tab; its results stay until the load lands.
    if let Some(Source::Search(s)) = &mut tab.source {
        s.stop();
    }
    // The worker sorts, as on_enum_done would: a refresh keeps the tab's sort, anything else the folder's remembered
    // one, else load() picks the default.
    let sort = match &tab.source {
        Some(Source::Shell(l)) if history == History::Refresh => Some((l.sort_col, l.sort_desc, l.group)),
        _ => None,
    };
    let saved = (*app).session.folders.clone();
    let latest = tab.latest.clone();
    let hwnd = (*app).hwnd.0 as isize;
    let show_hidden = (*app).profile.show_hidden;
    // Details' first screen from the top; a refresh keeps its scroll position, so it (and theme icons) load as painted.
    let prefill = if history == History::Refresh || views::icon_theme_active(app) || (*app).views.view.px() > 0 {
        0
    } else {
        SendMessageW((*app).list, LVM_GETCOUNTPERPAGE, None, None).0.max(0) as usize + 1
    };
    std::thread::spawn(move || {
        let _com = ComInit::mta();
        let superseded = || latest.load(Ordering::Relaxed) != generation;
        let result = load(target, show_hidden, sort, &saved, prefill, &superseded);
        // A newer load of this tab would make on_enum_done drop this one anyway.
        if superseded() {
            return;
        }
        let _ = ENUM_DONE.post(hwnd, WM_ENUM_DONE, Box::new(EnumDone { generation, history, result }));
    });
}

/// Finished loads, for WM_ENUM_DONE.
pub(crate) static ENUM_DONE: crate::mailbox::Mailbox<Box<EnumDone>> = crate::mailbox::Mailbox::new();

/// Navigates the active tab.
pub(crate) unsafe fn go(app: *mut App, target: Target, history: History) {
    navigate(app, (*app).active, target, history);
}

/// Where to go when `name` does not parse: Windows without Home (older Windows 10 builds) starts at This PC.
/// `%NAME%` references in a typed address, expanded as Explorer does; an unknown name stays as typed.
pub(crate) fn expand_env(s: &str, get: impl Fn(&str) -> Option<String>) -> String {
    let (mut out, mut rest) = (String::new(), s);
    while let Some(a) = rest.find('%') {
        let after = &rest[a + 1..];
        let Some(b) = after.find('%').filter(|&b| b > 0) else { break };
        match get(&after[..b]) {
            Some(v) => {
                out.push_str(&rest[..a]);
                out.push_str(&v);
            }
            None => out.push_str(&rest[..a + b + 2]),
        }
        rest = &after[b + 1..];
    }
    out.push_str(rest);
    out
}

fn fallback_of(name: &str) -> Option<&'static str> {
    name.eq_ignore_ascii_case(settings::HOME_START).then_some("shell:MyComputerFolder")
}

/// Worker side: the listing sorted by `sort` (shell column, descending, grouped by date; else `saved` for the
/// folder, else `default_sort`), the first `prefill` icons looked up.
/// Once `superseded` says a newer load of the tab started, gives up after enumerating (an error nobody reads).
fn load(target: Target, show_hidden: bool, sort: Option<(u32, bool, Option<u32>)>, saved: &[(String, session::FolderView)], prefill: usize, superseded: &dyn Fn() -> bool) -> std::result::Result<Loaded, LoadError> {
    if let Target::Name(n) = &target
        && let Some((archive, dir)) = archive_view::split_archive_path(n).filter(|(a, _)| a.is_file()) {
            return source::load_archive(&archive, &dir).map_err(|err| LoadError { name: n.clone(), err });
        }
    let folder = match target {
        Target::Name(n) => ShellFolder::parse(&n)
            .or_else(|e| fallback_of(&n).map_or(Err(e), ShellFolder::parse))
            .map_err(|err| LoadError { name: n, err })?,
        Target::Pidl(p) | Target::Search(_, p) => ShellFolder::from_pidl(&p).map_err(|err| LoadError { name: String::new(), err })?,
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
            None => return Ok(Loaded { source: Source::Browser(source::BrowserPlace::of(&folder)), crumbs, icon, unsorted: false }),
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
    let fv = saved.iter().find(|(k, _)| k.eq_ignore_ascii_case(&folder.parsing_name)).map(|(_, f)| f);
    let columns = match fv.filter(|f| !f.cols.is_empty()) {
        Some(f) => {
            let menu = folder.menu_columns();
            let offered: Vec<u32> = menu.iter().map(|c| c.index).collect();
            chosen_columns(&offered, &f.cols).into_iter().filter_map(|i| menu.iter().find(|c| c.index == i).cloned()).collect()
        }
        None => folder.columns(),
    };
    let sort = sort.or_else(|| fv.map(|f| (f.sort.0, f.sort.1, f.group))).unwrap_or_else(|| default_sort(&folder.parsing_name, &columns));
    let icons = vec![ICON_UNKNOWN; items.len()];
    let cells = vec![std::cell::OnceCell::new(); items.len()];
    let mut l = Listing { folder, items, columns, icons, cells, sort_col: sort.0, sort_desc: sort.1, group: sort.2, groups: Vec::new(), layout: fv.map_or(Vec::new(), |f| f.cols.clone()), view: fv.map(|f| views::View::parse(&f.view)), home: None, denied };
    if is_home(&l) {
        l.home = Some(home::build(&l.folder, &l.items));
    }
    // A library (Windows.Storage.Search) crashes on any property read made in this MTA (GetDetailsEx: an access
    // violation, not an error), and sorting, grouping and icons read properties: libraries and archive folders get
    // them on the UI thread instead.
    // shortcut: a big library sorts on the UI thread; give those an STA worker of their own if one stalls.
    let unsorted = l.home.is_none() && sorts_on_ui_thread(&l.folder.parsing_name);
    if unsorted {
        return Ok(Loaded { source: Source::Shell(l), crumbs, icon, unsorted });
    }
    // After the Home build: its tiles keep the enumeration order.
    unsafe { sort_listing(&mut l) };
    // Home's list is hidden, nothing asks it for icons.
    if l.home.is_none() {
        for (icon, item) in l.icons.iter_mut().zip(&l.items).take(prefill) {
            *icon = item.icon_index(&l.folder.pidl).1;
        }
    }
    Ok(Loaded { source: Source::Shell(l), crumbs, icon, unsorted })
}

/// The Libraries root, a library, or a folder inside an archive file (.zip, .cab): apartment-threaded folders whose
/// properties only the UI thread may read. This PC, the Recycle Bin, Network and Control Panel are not.
fn sorts_on_ui_thread(parsing_name: &str) -> bool {
    let lower = parsing_name.to_lowercase();
    lower.starts_with("::{031e4825-7b94-4dc3-b131-e946b44c8dd5}")
        || lower.contains(".library-ms")
        || (ui::is_fs_path(parsing_name) && !std::path::Path::new(parsing_name).is_dir() && std::path::Path::new(parsing_name).ancestors().any(|a| a.is_file()))
}

/// Downloads opens newest first (its first date column, descending) grouped by date; every other folder by name.
fn default_sort(parsing_name: &str, columns: &[Column]) -> (u32, bool, Option<u32>) {
    let trim = |s: &str| s.trim_end_matches('\\').to_lowercase();
    let downloads = panes::known_folder(&FOLDERID_Downloads).is_some_and(|d| trim(&d) == trim(parsing_name));
    match columns.iter().find(|c| c.kind == SHCOLSTATE_TYPE_DATE) {
        Some(c) if downloads => (c.index, true, Some(c.index)),
        _ => (0, false, None),
    }
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

/// The position of the tab whose `latest` counter is `id`, if it is still open (each tab has its own counter).
fn find_tab<'a>(mut latests: impl Iterator<Item = &'a Arc<AtomicU64>>, id: &Arc<AtomicU64>) -> Option<usize> {
    latests.position(|l| Arc::ptr_eq(l, id))
}

pub(crate) unsafe fn on_enum_done(app: *mut App, done: Box<EnumDone>) {
    let Some(mut t) = (*app).tabs.iter().position(|x| x.pending == done.generation) else { return };
    (&mut (*app).tabs)[t].pending = 0;
    let was_loading = (&mut (*app).tabs)[t].loading.take().is_some();
    let hwnd = (*app).hwnd;
    let list = (*app).list;
    let active = t == (*app).active;
    let mut result = done.result;
    // Bound on the loader's thread: Libraries and .zip folders answer only there until bound again here (a plain
    // directory answers on any thread). Unbound, every name would show empty: a failed load instead.
    let unbound = match &mut result {
        Ok(Loaded { source: Source::Shell(l), unsorted: true, .. }) => l.folder.adopt(&mut l.items).err().map(|err| LoadError { name: l.folder.parsing_name.clone(), err }),
        _ => None,
    };
    if let Some(e) = unbound {
        result = Err(e);
    }
    let loaded = match result {
        Err(LoadError { name, err }) => {
            // An archive with encrypted names: ask for its password and load it again. The prompt is modal over the
            // window; the tab is looked at again after it.
            let archive = archive_view::split_archive_path(&name).map(|(a, _)| a);
            let mut cancelled = false;
            if let (Some(wrong), Some(archive)) = (rexplorer_core::archive::password_needed(&err), archive) {
                let id = (&(*app).tabs)[t].latest.clone();
                let asked = source::ask_password(app, &archive, wrong);
                // The prompt pumps messages: a tab may have closed meanwhile and moved this one, or closed it.
                let Some(now) = find_tab((*app).tabs.iter().map(|x| &x.latest), &id) else { return };
                t = now;
                match asked {
                    Some(pw) => {
                        rexplorer_core::archive::set_password(&archive, pw);
                        navigate(app, t, Target::Name(name), done.history);
                        return;
                    }
                    None => cancelled = true,
                }
            }
            let active = t == (*app).active;
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
            } else if active && let Some(s) = &(&(*app).tabs)[t].source {
                set_address(app, &crate::connect::redact(&s.editing_name()));
            }
            if !cancelled {
                message(hwnd, &trf("err.open", &[("name", &crate::connect::redact(&name)), ("error", &err)]));
            }
            return;
        }
        Ok(loaded) => loaded,
    };
    let mut loaded = loaded;
    if let Source::Shell(l) = &mut loaded.source
        && loaded.unsorted
    {
        sort_listing(l);
    }

    let refresh = done.history == History::Refresh;
    // Read before the listing goes: in group view the list asks it (the groups callback) where each row is.
    let (shown, shown_top) = if refresh && active { (list::selected_indices(list), SendMessageW(list, LVM_GETTOPINDEX, None, None).0 as i32) } else { (Vec::new(), 0) };
    // Search results left for good are kept with where the list was (Tab::leave).
    if active && !refresh {
        save_view_state(app);
    }
    let tab = &mut (&mut (*app).tabs)[t];
    let old = tab.source.take();
    let left_archive = match &old {
        Some(Source::Archive(a)) => Some(a.listing.archive.clone()),
        _ => None,
    };
    let mut sort = None;
    let mut carry = None;
    let mut keep: Vec<String> = Vec::new();
    let mut top = 0;
    if let Some(old) = old {
        match done.history {
            History::Push => {
                let left = tab.leave(old);
                tab.back.push(left);
                tab.forward.clear();
                tab.skipped.clear();
            }
            History::Back => {
                let left = tab.leave(old);
                land(&mut tab.forward, left, std::mem::take(&mut tab.skipped));
            }
            History::Forward => {
                let left = tab.leave(old);
                land(&mut tab.back, left, std::mem::take(&mut tab.skipped));
            }
            History::Refresh => {
                if let Source::Shell(old) = &old {
                    sort = Some((old.sort_col, old.sort_desc, old.group));
                    carry = Some((old.columns.clone(), old.layout.clone(), old.view));
                    if active {
                        keep = shown.iter().filter_map(|&i| old.items.get(i)).map(|i| i.parsing_name()).collect();
                        top = shown_top;
                    } else {
                        keep = std::mem::take(&mut tab.selected);
                        top = tab.top;
                    }
                }
            }
        }
    }
    if let Some(sel) = tab.select_after.take() {
        keep = vec![sel];
    }
    let mut src = loaded.source;
    // One shot: any load of the tab ends the delete's wait, except a reload while the delete still runs.
    let deleted = if refresh && tab.deleted.as_ref().is_some_and(|d| d.at.is_none()) { None } else { tab.deleted.take() };
    let home = matches!(&src, Source::Shell(l) if is_home(l));
    let after_delete = deleted.filter(|_| active).and_then(|d| d.next_for(refresh, &src.parsing_name(), home, &keep, Instant::now()));

    if let Source::Shell(l) = &mut src {
        // A reload keeps the header, so the listing keeps the columns it shows (a column toggled while it ran), their
        // layout and the view.
        let mut regroup = false;
        if let Some((columns, layout, view)) = carry {
            regroup = columns.iter().map(|c| c.index).ne(l.columns.iter().map(|c| c.index));
            (l.columns, l.layout, l.view) = (columns, layout, view);
        }
        // load() sorted by the sort the tab had then; only a sort changed meanwhile (a header click) sorts again.
        if let Some((c, d, g)) = sort.filter(|&s| regroup || s != (l.sort_col, l.sort_desc, l.group)) {
            l.sort_col = c;
            l.sort_desc = d;
            l.group = g;
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
        if !(*app).secondary {
            (*app).recent.save();
            // Tests drive windows too, and the test scripts run on a scratch profile (they always set both
            // variables): neither may rewrite the user's jump list.
            let scratch = ["REXPLORER_REG_BASE", "REXPLORER_INSTALL_DIR"].iter().any(|v| std::env::var_os(v).is_some());
            if !cfg!(test) && !scratch {
                crate::refresh_jump_list((*app).recent.items().to_vec());
            }
        }
    }
    tab.source = Some(src);
    // An archive left for somewhere else: its password goes once no tab shows it (Back asks again).
    if let Some(a) = left_archive {
        forget_unshown(app, &a);
    }
    if active {
        show(app, if refresh { Show::Restore { top, selected: keep, columns: false } } else { Show::Fresh(keep) });
        // The deleted rows are gone for good only when none of them is selected again.
        if let Some(next) = after_delete.filter(|_| list::selected_indices(list).is_empty())
            && let Some(l) = listing(app)
            && let Some(&i) = selection_matches(&[next], l.items.iter().map(|i| i.parsing_name())).first()
        {
            list::select_after_delete(list, i, l.items.len());
        }
        commands::rename_new_item(app);
    } else {
        tab.top = top;
        tab.selected = keep;
    }
    crate::instance::signal_loaded(hwnd);
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
    list::set_groups(app, false);
    let hwnd = (*app).hwnd;
    let list = (*app).list;
    let tab = cur(app);
    let Some(src) = &tab.source else {
        source::leave_browser(app);
        views::reset(app, 0);
        SendMessageW(list, LVM_SETITEMCOUNT, Some(WPARAM(0)), Some(LPARAM(0)));
        panes::schedule_preview(app);
        set_columns(app, &[]);
        set_address(app, &crate::connect::redact(&tab.name));
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
        // A folder opens in the view it was left in; one with no memory in the last view chosen anywhere.
        if !is_home(l) {
            views::show_view(app, l.view.unwrap_or_else(|| views::View::parse(&(*app).session.view)));
        }
        if !(*app).image_list_set
            && let Some(first) = l.items.first() {
                let (himl, _) = first.icon_index(&l.folder.pidl);
                if himl != 0 {
                    views::set_small_source(app, HIMAGELIST(himl as isize));
                }
            }
    }
    if !matches!(mode, Show::Restore { columns: false, .. }) {
        set_columns(app, &src.columns());
        apply_layout(app);
    }
    // The shell's FTP folder keeps user:password in its names; nothing on screen shows the password.
    set_address(app, &crate::connect::redact(&src.editing_name()));
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
    list::set_groups(app, true);
    list::set_item_state(list, usize::MAX, LIST_VIEW_ITEM_STATE_FLAGS(0), LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0));
    if !matches!(mode, Show::Restore { columns: false, .. }) && (*app).panes.tree.is_some() {
        SetTimer(Some(hwnd), crate::TIMER_TREE_SYNC, 1, None);
    }
    let select = |names: &[String]| -> Option<usize> {
        let hits = match src {
            Source::Shell(l) => selection_matches(names, l.items.iter().map(|i| i.parsing_name())),
            Source::Search(s) => selection_matches(names, s.rows.iter().map(|r| r.item.parsing_name())),
            _ => return None,
        };
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
            if top <= 0 {
                // In group view ENSUREVISIBLE(0) leaves the first group's header above the view.
                SendMessageW(list, WM_VSCROLL, Some(WPARAM(SB_TOP.0 as usize)), None);
            } else if last >= 0 {
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
        home::HN_ACTIVATE => activate_or_tab(app, nm.index, GetKeyState(VK_CONTROL.0 as i32) < 0),
        home::HN_CONTEXT => list::show_menu(app, nm.index != usize::MAX, None),
        home::HN_CONTEXT_KEY => list::show_menu(app, nm.index != usize::MAX, Some(home::menu_point((*app).home))),
        home::HN_KEYDOWN => list::on_key(app, VIRTUAL_KEY(nm.index as u16)),
        home::HN_RENAME => list::rename_item(app, nm.index, &home::rename_text((*app).home), false),
        home::HN_DRAG => list::begin_drag(app),
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

/// Keys; the draw sites translate.
pub(crate) const DATE_GROUPS: [&str; 9] = [
    "group.date.future",
    "group.date.today",
    "group.date.yesterday",
    "group.date.earlier_week",
    "group.date.last_week",
    "group.date.earlier_month",
    "group.date.last_month",
    "group.date.earlier_year",
    "group.date.long_ago",
];

/// Days since 1601-01-01 to (year, month, day): Howard Hinnant's civil_from_days, moved to that epoch.
fn civil(day: i64) -> (i64, i64, i64) {
    let z = day - 134_774 + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + (m <= 2) as i64, m, doy - (153 * mp + 2) / 5 + 1)
}

/// The DATE_GROUPS index Explorer puts `day` in, seen from `today` (both days since 1601-01-01, which was a Monday);
/// the first rule that matches wins. `week_start` is the first day of the week, 0 = Monday.
fn date_group(day: i64, today: i64, week_start: i64) -> usize {
    let week = today - (today - week_start).rem_euclid(7);
    let ((y, m, _), (ty, tm, _)) = (civil(day), civil(today));
    let months = y * 12 + m - (ty * 12 + tm);
    match day {
        d if d > today => 0,
        d if d == today => 1,
        d if d == today - 1 => 2,
        d if d >= week => 3,
        d if d >= week - 7 => 4,
        _ if months == 0 => 5,
        _ if months == -1 => 6,
        _ if y == ty => 7,
        _ => 8,
    }
}

/// The local day of a UTC time, by the time zone's rules at that date (as Explorer shows it).
fn local_day(ft: FILETIME) -> Option<i64> {
    let (mut utc, mut local, mut out) = (SYSTEMTIME::default(), SYSTEMTIME::default(), FILETIME::default());
    unsafe {
        FileTimeToSystemTime(&ft, &mut utc).ok()?;
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
        SystemTimeToFileTime(&local, &mut out).ok()?;
    }
    Some(((out.dwHighDateTime as i64) << 32 | out.dwLowDateTime as i64) / 864_000_000_000)
}

/// The user's first day of the week, 0 = Monday.
fn week_start() -> i64 {
    let mut n = [0u16; 2];
    unsafe { GetLocaleInfoEx(PCWSTR::null(), LOCALE_IFIRSTDAYOFWEEK | LOCALE_RETURN_NUMBER, Some(&mut n)) };
    n[0] as i64 % 7
}

pub(crate) const SIZE_GROUPS: [&str; 8] = [
    "group.size.gigantic",
    "group.size.huge",
    "group.size.large",
    "group.size.medium",
    "group.size.small",
    "group.size.tiny",
    "group.size.empty",
    "group.unspecified",
];

/// The SIZE_GROUPS index Explorer puts an item in (0 is the largest, as 0 is the newest date).
fn size_group(folder: bool, bytes: Option<u64>) -> usize {
    match bytes {
        _ if folder => 7,
        None => 7,
        Some(0) => 6,
        Some(b) if b < 16 << 10 => 5,
        Some(b) if b < 1 << 20 => 4,
        Some(b) if b < 128 << 20 => 3,
        Some(b) if b < 1 << 30 => 2,
        Some(b) if b <= 4 << 30 => 1,
        Some(_) => 0,
    }
}

/// A name's group: its first letter, "0 - 9" for a digit, else "Other".
fn name_group(name: &str) -> String {
    match name.chars().next() {
        Some(c) if c.is_ascii_digit() => "0 - 9".to_string(),
        Some(c) if c.is_alphabetic() => c.to_uppercase().collect(),
        _ => tr("group.other").to_string(),
    }
}

/// Ranks for text group labels: (rank per label, the distinct labels in rank order). An empty label is "Unspecified";
/// that and "Other" go last.
fn text_ranks(labels: Vec<String>) -> (Vec<usize>, Vec<String>) {
    let (unspecified, other) = (tr("group.unspecified"), tr("group.other"));
    let labels: Vec<String> = labels.into_iter().map(|l| if l.is_empty() { unspecified.to_string() } else { l }).collect();
    let mut table = labels.clone();
    table.sort_by_cached_key(|l| (l == unspecified || l == other, l.to_lowercase()));
    table.dedup();
    (labels.iter().map(|l| table.iter().position(|t| t == l).unwrap_or(0)).collect(), table)
}

/// Whether the lowest group rank comes first: always, unless the rows are sorted by the group column in the order that
/// is not its natural one.
fn groups_lowest_first(same_col: bool, sort_desc: bool, desc_first: bool) -> bool {
    !same_col || sort_desc == desc_first
}

/// (rank per item, labels by rank) for grouping `items` by column `c`; None when the column has no usable key.
unsafe fn group_ranks(folder: &ShellFolder, c: &Column, items: &[ShellItem]) -> Option<(Vec<usize>, Vec<String>)> {
    if c.kind == SHCOLSTATE_TYPE_DATE {
        let key = folder.column_key(c.index)?;
        let (today, week) = (local_day(GetSystemTimeAsFileTime()).unwrap_or_default(), week_start());
        // An item without that date goes with the oldest.
        let ranks = items.iter().map(|it| it.property_time(&key).and_then(local_day).map_or(8, |d| date_group(d, today, week))).collect();
        return Some((ranks, DATE_GROUPS.iter().map(|s| tr(s).to_string()).collect()));
    }
    if panes::size_and_type_columns(&folder.folder).0 == Some(c.index) {
        let ranks = items.iter().map(|it| size_group(it.is_folder(), it.size())).collect();
        return Some((ranks, SIZE_GROUPS.iter().map(|s| tr(s).to_string()).collect()));
    }
    // shortcut: text groups read every row's cell on the UI thread; move to the worker if a big folder stalls
    Some(text_ranks(items.iter().map(|it| if c.index == 0 { name_group(&it.display_name()) } else { it.detail(c.index) }).collect()))
}

/// Stable-sorts the sorted `items` by group `rank`, the lowest rank first when `desc` (newest first), so a
/// folders-first order cannot split a group. Returns the groups as (rank, first row).
fn group_rows<T>(items: &mut Vec<T>, rank: impl Fn(&T) -> usize, desc: bool) -> Vec<(usize, usize)> {
    let mut ranked: Vec<(usize, T)> = items.drain(..).map(|t| (rank(&t), t)).collect();
    ranked.sort_by_key(|&(r, _)| if desc { r } else { usize::MAX - r });
    let mut groups: Vec<(usize, usize)> = Vec::new();
    for (i, (r, t)) in ranked.into_iter().enumerate() {
        if groups.last().is_none_or(|g| g.0 != r) {
            groups.push((r, i));
        }
        items.push(t);
    }
    groups
}

pub(crate) unsafe fn sort_listing(l: &mut Listing) {
    let folder = &l.folder;
    let col = l.sort_col;
    l.items.sort_by(|a, b| folder.compare(col, a, b));
    if l.sort_desc {
        l.items.reverse();
    }
    let gc = l.group.and_then(|g| l.columns.iter().find(|c| c.index == g));
    l.groups = match gc.and_then(|c| group_ranks(folder, c, &l.items).map(|r| (c, r))) {
        Some((c, (ranks, table))) => {
            let first = groups_lowest_first(c.index == col, l.sort_desc, c.descending_first());
            let mut paired: Vec<(usize, ShellItem)> = ranks.into_iter().zip(l.items.drain(..)).collect();
            let groups = group_rows(&mut paired, |p| p.0, first);
            l.items.extend(paired.into_iter().map(|p| p.1));
            groups.into_iter().map(|(r, f)| (table[r].clone(), f)).collect()
        }
        None => Vec::new(),
    };
    l.icons.fill(ICON_UNKNOWN);
    l.cells.fill(Default::default());
}

/// Remembers the active folder's view, sort, grouping and columns (on disk, a moment later).
pub(crate) unsafe fn remember_folder(app: *mut App) {
    let Some(l) = listing(app) else { return };
    if is_home(l) || cur(app).loading.is_some() {
        return;
    }
    let fv = session::FolderView { view: (*app).views.view.name().to_string(), sort: (l.sort_col, l.sort_desc), group: l.group, cols: l.layout.clone() };
    (*app).session.remember(&l.folder.parsing_name, fv);
    session_changed(app);
}

/// After the user resized or dragged a header: takes the header's widths and order into the listing and remembers them.
pub(crate) unsafe fn remember_layout(app: *mut App) {
    let layout = header_layout(app);
    if layout.is_empty() {
        return;
    }
    if let Some(Source::Shell(l)) = &mut cur(app).source {
        l.layout = layout;
    }
    remember_folder(app);
}

/// `layout` with column `index` shown or hidden: removed when there, else appended at its default width. Name stays.
pub(crate) fn toggled(layout: &[(u32, i32)], index: u32) -> Vec<(u32, i32)> {
    let mut out: Vec<(u32, i32)> = layout.iter().copied().filter(|c| index == 0 || c.0 != index).collect();
    if index != 0 && out.len() == layout.len() {
        out.push((index, 0));
    }
    out
}

/// The shell columns to show: Name first (the list reads it as header position 0), then each layout column the
/// folder offers, once, in layout order.
pub(crate) fn chosen_columns(available: &[u32], layout: &[(u32, i32)]) -> Vec<u32> {
    let mut out = vec![0];
    for &(i, _) in layout {
        if available.contains(&i) && !out.contains(&i) {
            out.push(i);
        }
    }
    out
}

/// Header positions in display order: the layout's columns that are shown, then any others as they are.
fn display_order(layout: &[(u32, i32)], shown: &[u32]) -> Vec<i32> {
    let mut out: Vec<i32> = Vec::new();
    for (i, _) in layout {
        if let Some(p) = shown.iter().position(|s| s == i).map(|p| p as i32).filter(|p| !out.contains(p)) {
            out.push(p);
        }
    }
    let rest: Vec<i32> = (0..shown.len() as i32).filter(|p| !out.contains(p)).collect();
    out.extend(rest);
    out
}

/// The layout the header shows: its positions in display order and each position's width.
fn layout_of(order: &[i32], shown: &[u32], widths96: &[i32]) -> Vec<(u32, i32)> {
    order
        .iter()
        .filter_map(|&o| Some((*shown.get(usize::try_from(o).ok()?)?, *widths96.get(o as usize)?)))
        .collect()
}

/// The active listing's layout as the header has it now; the listing's own outside Details (an icon view's header
/// is not there to ask).
pub(crate) unsafe fn header_layout(app: *mut App) -> Vec<(u32, i32)> {
    let Some(l) = listing(app) else { return Vec::new() };
    let list = (*app).list;
    let header = HWND(SendMessageW(list, LVM_GETHEADER, None, None).0 as *mut c_void);
    let n = l.columns.len();
    if (*app).views.view != views::View::Details || header.is_invalid() || SendMessageW(header, HDM_GETITEMCOUNT, None, None).0 as usize != n {
        return l.layout.clone();
    }
    let mut order = vec![0i32; n];
    SendMessageW(list, LVM_GETCOLUMNORDERARRAY, Some(WPARAM(n)), Some(LPARAM(order.as_mut_ptr() as isize)));
    let dpi = GetDpiForWindow((*app).hwnd).max(96) as i32;
    let widths: Vec<i32> = (0..n).map(|i| SendMessageW(list, LVM_GETCOLUMNWIDTH, Some(WPARAM(i)), None).0 as i32 * 96 / dpi).collect();
    let shown: Vec<u32> = l.columns.iter().map(|c| c.index).collect();
    layout_of(&order, &shown, &widths)
}

/// Puts the listing's remembered widths and order on the header (after set_columns).
pub(crate) unsafe fn apply_layout(app: *mut App) {
    let Some(l) = listing(app).filter(|l| !l.layout.is_empty()) else { return };
    let (list, dpi) = ((*app).list, GetDpiForWindow((*app).hwnd).max(96) as i32);
    let shown: Vec<u32> = l.columns.iter().map(|c| c.index).collect();
    for &(index, w) in &l.layout {
        if let (true, Some(pos)) = (w > 0, shown.iter().position(|&s| s == index)) {
            SendMessageW(list, LVM_SETCOLUMNWIDTH, Some(WPARAM(pos)), Some(LPARAM((w.max(20) * dpi / 96) as isize)));
        }
    }
    let order = display_order(&l.layout, &shown);
    SendMessageW(list, LVM_SETCOLUMNORDERARRAY, Some(WPARAM(order.len())), Some(LPARAM(order.as_ptr() as isize)));
}

pub(crate) unsafe fn set_columns(app: *mut App, columns: &[Column]) {
    let list = (*app).list;
    while SendMessageW(list, LVM_DELETECOLUMN, Some(WPARAM(0)), None).0 != 0 {}
    // Default widths are counted in characters, so they follow the text size.
    let dpi = theme::text_dpi(GetDpiForWindow((*app).hwnd)) as i32;
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
    list::set_sort_arrow(app);
}

/// Reloads the active tab; a search runs again. Nothing while a navigation is in flight: reloading the place being
/// left (a change in it, a tab switch) would cancel the navigation.
pub(crate) unsafe fn refresh(app: *mut App) {
    if cur(app).loading.is_some() {
        return;
    }
    // A reload would end a rename in progress (a new item's, right after its creation): wait for it, unless its item
    // is gone from the disk (deleted or renamed elsewhere): that edit ends unsaved (on_end_edit drops it too).
    if SendMessageW((*app).list, LVM_GETEDITCONTROL, None, None).0 != 0 {
        if (*app).renaming.as_deref().is_some_and(|p| ui::is_fs_path(p) && std::fs::symlink_metadata(p).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)) {
            // Forgotten first: the cancel still ends the edit with its text, and that commit must find nothing to rename.
            (*app).renaming = None;
            SendMessageW((*app).list, LVM_CANCELEDITLABEL, None, None);
        } else {
            SetTimer(Some((*app).hwnd), TIMER_REFRESH, 500, None);
            return;
        }
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
#[allow(clippy::needless_borrow)] // the explicit borrow of a raw-pointer field silences rustc's dangerous_implicit_autorefs
pub(crate) unsafe fn mark_changed(app: *mut App, items: &[Pidl]) -> bool {
    let hit = |t: &Tab| t.watch.as_ref().is_some_and(|w| items.iter().any(|p| w.concerns(p)));
    let all = !(*app).tabs.iter().any(hit);
    // Watched tabs only: a search, archive or browser tab has nothing a shell change makes stale.
    let marks = |t: &Tab| t.watch.is_some() && (all || hit(t));
    for t in (*app).tabs.iter_mut() {
        t.dirty |= marks(t);
    }
    (&(*app).tabs).get((*app).active).is_some_and(marks)
}

pub(crate) unsafe fn go_back(app: *mut App) {
    // Back while a new place is still loading returns to the place it left, as in Explorer: the chrome already shows
    // the new place, but its history entry is only pushed when the load lands, so a plain jump would skip a step.
    let tab = cur(app);
    if back_cancels_load(tab.loading.as_ref().map(|l| l.history), tab.source.is_some()) {
        return cancel_load(app);
    }
    jump(app, true, 1);
}

/// Back cancels the load in flight when that load is a step forward into a new place (not a refresh, Back or
/// Forward, whose history is already moved) and there is a place on screen to stay on.
fn back_cancels_load(loading: Option<History>, has_source: bool) -> bool {
    loading == Some(History::Push) && has_source
}

/// Drops the active tab's load in flight and shows the place it was leaving again, as a failed load does.
unsafe fn cancel_load(app: *mut App) {
    (*app).generation += 1;
    let t = (*app).active;
    let tab = cur(app);
    // The worker gives up, and a result that still arrives no longer matches the tab.
    tab.pending = 0;
    tab.latest.store((*app).generation, Ordering::Relaxed);
    tab.loading = None;
    if let Some(s) = &tab.source {
        tabs::set_title((*app).strip, t, &s.title());
    }
    save_view_state(app);
    let tab = cur(app);
    let (top, selected) = (tab.top, std::mem::take(&mut tab.selected));
    show(app, Show::Restore { top, selected, columns: true });
}

pub(crate) unsafe fn go_forward(app: *mut App) {
    jump(app, false, 1);
}

/// Goes `steps` entries back or forward. A load that fails loses the popped entries, as a single step always did.
pub(crate) unsafe fn jump(app: *mut App, back: bool, steps: usize) {
    let tab = cur(app);
    let Some(((target, _), skipped)) = take_steps(if back { &mut tab.back } else { &mut tab.forward }, steps) else { return };
    tab.skipped = skipped;
    if let Target::Search(id, _) = &target
        && let Some(k) = tab.kept.iter().position(|k| k.listing.generation == *id) {
            let kept = tab.kept.remove(k);
            return restore_search(app, kept, back);
        }
    go(app, target, if back { History::Back } else { History::Forward });
}

/// Back or Forward onto kept search results: they come back as they were left, without searching again.
unsafe fn restore_search(app: *mut App, kept: Kept, back: bool) {
    (*app).generation += 1;
    let t = (*app).active;
    let tab = cur(app);
    // A load still in flight for this tab no longer lands here.
    tab.pending = 0;
    tab.latest.store((*app).generation, Ordering::Relaxed);
    tab.loading = None;
    tab.dirty = false;
    save_view_state(app);
    let tab = cur(app);
    if let Some(old) = tab.source.take() {
        let left = tab.leave(old);
        let skipped = std::mem::take(&mut tab.skipped);
        land(if back { &mut tab.forward } else { &mut tab.back }, left, skipped);
    }
    let Kept { listing, top, selected } = kept;
    let title = trf("tabs.search", &[("needle", &listing.needle)]);
    tab.crumbs = listing.root_crumbs.clone();
    tab.crumbs.push(Crumb { name: title.clone(), parsing: String::new(), pidl: None });
    tab.name = listing.root_parsing.clone();
    tab.source = Some(Source::Search(listing));
    tabs::set_title((*app).strip, t, &title);
    show(app, Show::Restore { top, selected, columns: true });
    session_changed(app);
}

/// Pops `steps` entries: the last popped is the destination, the rest (pop order) are skipped. None, stack untouched, for 0 or too many.
pub(crate) fn take_steps<T>(stack: &mut Vec<T>, steps: usize) -> Option<(T, Vec<T>)> {
    if steps == 0 || steps > stack.len() {
        return None;
    }
    let mut s = stack.split_off(stack.len() - steps);
    s.reverse();
    let t = s.pop()?;
    Some((t, s))
}

/// The place left goes on the other stack, then the skipped ones, so they come back in order.
fn land<T>(other: &mut Vec<T>, left: T, skipped: Vec<T>) {
    other.push(left);
    other.extend(skipped);
}

/// Menu rows for a history stack: next destination first, `&` doubled (menus eat it), passwords redacted, at most 16.
fn history_entries(labels: &[&str]) -> Vec<(usize, String)> {
    labels.iter().rev().take(16).enumerate().map(|(k, l)| (k + 1, crate::connect::redact(l).replace('&', "&&"))).collect()
}

/// Right click on Back or Forward: the places that button would go through, newest first.
pub(crate) unsafe fn on_nav_context(app: *mut App, lparam: LPARAM) {
    let (x, y) = (lparam.0 as i16 as i32, (lparam.0 >> 16) as i16 as i32);
    if x == -1 && y == -1 {
        return;
    }
    let at = POINT { x, y };
    let hit = |cmd| toolbar::button_rect((*app).nav, cmd).filter(|r| windows::Win32::Graphics::Gdi::PtInRect(r, at).as_bool());
    let (back, r) = match (hit(CMD_BACK), hit(CMD_FORWARD)) {
        (Some(r), _) => (true, r),
        (_, Some(r)) => (false, r),
        _ => return,
    };
    let tab = cur(app);
    let stack = if back { &tab.back } else { &tab.forward };
    let labels: Vec<&str> = stack.iter().map(|(_, l)| l.as_str()).collect();
    let entries = history_entries(&labels);
    if entries.is_empty() {
        return;
    }
    let picked = ui::popup(app, &entries, &[], crate::rtl::menu_x(&r), r.bottom);
    if picked != 0 {
        jump(app, back, picked);
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
        Target::Pidl(_) | Target::Search(..) => String::new(),
    };
    let title = if name.is_empty() { "…".to_string() } else { name.clone() };
    (*app).tabs.push(Tab {
        name,
        source: None,
        back: Vec::new(),
        forward: Vec::new(),
        skipped: Vec::new(),
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
        kept: Vec::new(),
        deleted: None,
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
pub(crate) unsafe fn save_view_state(app: *mut App) {
    if (*app).skeleton {
        return;
    }
    let list = (*app).list;
    let tab = cur(app);
    tab.top = SendMessageW(list, LVM_GETTOPINDEX, None, None).0 as i32;
    tab.selected = match &tab.source {
        Some(Source::Shell(l)) => list::selected_indices(list).iter().filter_map(|&i| l.items.get(i)).map(|i| i.parsing_name()).collect(),
        Some(Source::Search(s)) => list::selected_indices(list).iter().filter_map(|&i| s.rows.get(i)).map(|r| r.item.parsing_name()).collect(),
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
        Target::Pidl(p) | Target::Search(_, p) => names(p.as_ptr()),
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

/// The address bar's text, left alone while it is open for typing (closing it puts the place back, ui::hide_edit).
unsafe fn set_address(app: *mut App, text: &str) {
    if !IsWindowVisible((*app).addr).as_bool() {
        set_text((*app).addr, text);
    }
}

/// The active tab is loading: its destination in the title, tab, breadcrumb, address and sidebar; "Loading..."
/// in the status bar; the skeleton once the load has taken longer than SKELETON_DELAY.
unsafe fn present_loading(app: *mut App) {
    let hwnd = (*app).hwnd;
    let tab = cur(app);
    let Some(l) = &tab.loading else { return };
    set_text(hwnd, &format!("{} - rexplorer", l.name));
    set_address(app, &crate::connect::redact(&l.parsing));
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
    list::set_groups(app, false);
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

/// A tab sent to another window that did not take it is waiting in RETURNED. The message carries no pointer: any
/// program on the desktop can post it.
pub(crate) const WM_TAB_RETURN: u32 = WM_APP + 14;
static RETURNED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The tabs handed back since the last call.
pub(crate) fn take_returned() -> Vec<String> {
    RETURNED.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
}

/// A tab dropped outside the window: onto another rexplorer window it moves there as a new tab, else it opens in a
/// window of its own. Only a place a command line can carry moves out (no FTP password, not empty); the last tab stays.
pub(crate) unsafe fn tear_tab(app: *mut App, t: usize) {
    let tabs = &(*app).tabs;
    let name = tabs.get(t).map(|t| t.name.clone()).unwrap_or_default();
    if tabs.len() < 2 || instance::new_window_args(&name).len() != 2 {
        let _ = MessageBeep(MB_OK);
        return;
    }
    // Where the button came up: the message being handled is the strip's WM_LBUTTONUP.
    let pos = GetMessagePos();
    let pt = ((pos & 0xFFFF) as i16 as i32, (pos >> 16) as i16 as i32);
    let at = WindowFromPoint(POINT { x: pt.0, y: pt.1 });
    match instance::other_main_window(at, (*app).hwnd).filter(|&w| instance::release_lands_on(w, pt)) {
        Some(other) => {
            // A busy window can take seconds to answer: the hand-off runs on a worker, and a refusal brings the tab back.
            let (other, own, payload) = (other.0 as isize, (*app).hwnd.0 as isize, instance::tab_payload(&name));
            close_tab(app, t);
            std::thread::spawn(move || {
                if !instance::send(HWND(other as *mut c_void), &payload) {
                    if let Ok(mut v) = RETURNED.lock() {
                        v.push(name);
                    }
                    let _ = unsafe { PostMessageW(Some(HWND(own as *mut c_void)), WM_TAB_RETURN, WPARAM(0), LPARAM(0)) };
                }
            });
        }
        None if instance::open_new_window(&name) => close_tab(app, t),
        None => {
            let _ = MessageBeep(MB_OK);
        }
    }
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
    // Members extracted to show them; another tab on the same archive extracts again as it needs. Its password
    // goes too, once no other tab shows the archive.
    if let Some(Source::Archive(a)) = &removed.source {
        let _ = std::fs::remove_dir_all(archive_view::show_dir(&a.listing.archive));
        forget_unshown(app, &a.listing.archive);
    }
    drop(removed);
    session_changed(app);
}

/// Drops the password typed for `archive` unless a tab still shows it.
unsafe fn forget_unshown(app: *mut App, archive: &std::path::Path) {
    let shown = (*app).tabs.iter().any(|t| matches!(&t.source, Some(Source::Archive(b)) if b.listing.archive.as_path() == archive));
    if !shown {
        rexplorer_core::archive::forget_password(archive);
    }
}

/// TN_MOVE: the strip already moved its title; the tab, the active index and the session follow.
pub(crate) unsafe fn tab_moved(app: *mut App, from: usize, to: usize) {
    let n = (*app).tabs.len();
    if from >= n || to >= n || from == to {
        return;
    }
    let t = (*app).tabs.remove(from);
    (*app).tabs.insert(to, t);
    (*app).active = tabs::moved((*app).active, from, to);
    session_changed(app);
}

const TAB_DUPLICATE: usize = 1;
const TAB_CLOSE: usize = 2;
const TAB_CLOSE_OTHERS: usize = 3;
const TAB_CLOSE_RIGHT: usize = 4;

/// Tabs that "Close other tabs" (others) or "Close tabs to the right" close around tab `i`, highest first so the indexes stay valid.
fn to_close(n: usize, i: usize, others: bool) -> Vec<usize> {
    (0..n).rev().filter(|&j| if others { j != i } else { j > i }).collect()
}

/// TN_CONTEXT: the tab's right-click menu.
#[allow(clippy::needless_borrow)] // the explicit borrow of a raw-pointer field silences rustc's dangerous_implicit_autorefs
pub(crate) unsafe fn tab_menu(app: *mut App, i: usize, at: Option<POINT>) {
    let n = (*app).tabs.len();
    let Some(name) = (&(*app).tabs).get(i).map(|t| t.name.clone()) else { return };
    let entries = [
        Entry::new(TAB_DUPLICATE, tr("tabs.duplicate"), 0).enabled(!name.is_empty()),
        Entry::sep(),
        Entry::new(TAB_CLOSE, &hinted("tabs.close", "Ctrl+W"), 0),
        Entry::new(TAB_CLOSE_OTHERS, tr("tabs.close_others"), 0).enabled(n > 1),
        Entry::new(TAB_CLOSE_RIGHT, tr("tabs.close_right"), 0).enabled(i + 1 < n),
    ];
    let pt = at.unwrap_or_else(|| {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        pt
    });
    match ui::pick_entries(app, &entries, pt.x, pt.y) {
        // Same place, without the history: the copy sits next to its source.
        Some(TAB_DUPLICATE) => {
            new_tab(app, Target::Name(name), true, None);
            let last = (*app).tabs.len() - 1;
            tabs::move_tab((*app).strip, last, i + 1);
            tab_moved(app, last, i + 1);
        }
        Some(TAB_CLOSE) => close_tab(app, i),
        Some(c @ (TAB_CLOSE_OTHERS | TAB_CLOSE_RIGHT)) => {
            // The kept tab is shown first, so closing the active one does not load a neighbour on the way.
            if c == TAB_CLOSE_OTHERS || (*app).active > i {
                switch_tab(app, i);
            }
            for j in to_close(n, i, c == TAB_CLOSE_OTHERS) {
                close_tab(app, j);
            }
        }
        _ => {}
    }
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

/// Ctrl+double-click opens a folder in a background tab. In single-click mode comctl32 may report a Ctrl+click as an
/// activation, which would steal multi-select, so that mode keeps its plain activation.
pub(crate) fn opens_tab(ctrl: bool, folder: bool, single_click: bool) -> bool {
    ctrl && folder && !single_click
}

pub(crate) unsafe fn activate_or_tab(app: *mut App, index: usize, ctrl: bool) {
    let folder = listing(app).and_then(|l| l.items.get(index)).is_some_and(|i| i.is_folder());
    if opens_tab(ctrl, folder, (*app).profile.single_click) {
        open_in_background(app, index);
    } else {
        source::activate(app, index);
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
    if (*app).secondary {
        return;
    }
    let a = &mut *app;
    let named: Vec<(usize, &Tab)> = a.tabs.iter().enumerate().filter(|(_, t)| !t.name.is_empty()).collect();
    a.session.active = named.iter().position(|(i, _)| *i == a.active).unwrap_or(0);
    a.session.tabs = named.iter().map(|(_, t)| t.name.clone()).collect();
    session::save(&a.session);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    /// The marker a delete of `sel` (rows of a..e in C:\d) leaves once it returned at `t0`, everything gone.
    fn deleted(sel: &[usize], focused: Option<usize>, t0: Instant) -> Option<Deleted> {
        let rows = ["a", "b", "c", "d", "e"].map(|n| format!(r"C:\d\{n}"));
        let d = Deleted {
            folder: r"C:\d".into(),
            names: sel.iter().map(|&i| rows[i].clone()).collect(),
            next: row_taking_place(rows.len(), sel, focused).map(|i| rows[i].clone()),
            at: None,
        };
        d.over(t0, |_| true)
    }

    #[test]
    fn a_reload_selects_the_next_row_only_after_the_users_own_delete() {
        let t0 = Instant::now();
        let soon = t0 + Duration::from_secs(1);
        let reload = |d: Option<Deleted>, replaced: &[&str]| d.and_then(|d| d.next_for(true, r"c:\D", false, &names(replaced), soon));
        assert_eq!(reload(deleted(&[1], Some(1), t0), &[r"C:\d\b"]), Some(r"C:\d\c".into()), "the next row");
        assert_eq!(reload(deleted(&[4], Some(4), t0), &[r"C:\d\e"]), Some(r"C:\d\d".into()), "the last row went: the new last row");
        assert_eq!(reload(None, &[r"C:\d\b"]), None, "another program removed the selected file: nothing");
        // Several rows: where the focused one was (the first selected when the focus is not on one of them).
        assert_eq!(reload(deleted(&[0, 3], Some(3), t0), &[r"C:\d\a", r"C:\d\d"]), Some(r"C:\d\e".into()));
        assert_eq!(reload(deleted(&[0, 1, 4], Some(2), t0), &[r"C:\d\a", r"C:\d\b", r"C:\d\e"]), Some(r"C:\d\c".into()));
        assert_eq!(row_taking_place(3, &[0, 1, 2], Some(1)), None, "nothing survives");
        // A reload while the delete ran took part of the selection; the rest is what the next one replaces.
        assert_eq!(reload(deleted(&[0, 3], Some(3), t0), &[r"C:\d\d"]), Some(r"C:\d\e".into()));
        assert_eq!(reload(deleted(&[1], Some(1), t0), &[r"C:\d\c"]), None, "the user selected another row since");
        assert_eq!(reload(deleted(&[1], Some(1), t0), &[]), None, "nothing was selected when the reload came");
        let other = deleted(&[1], Some(1), t0).and_then(|d| d.next_for(true, r"C:\elsewhere", false, &names(&[r"C:\d\b"]), soon));
        assert_eq!(other, None, "another folder");
        let late = deleted(&[1], Some(1), t0).and_then(|d| d.next_for(true, r"C:\d", false, &names(&[r"C:\d\b"]), t0 + DELETED_WAIT));
        assert_eq!(late, None, "no reload came soon after the delete");
        let running = Deleted { at: None, ..deleted(&[1], Some(1), t0).unwrap() };
        assert_eq!(running.next_for(true, r"C:\d", false, &names(&[r"C:\d\b"]), soon), None, "the delete still runs");
    }

    #[test]
    fn a_cancelled_delete_disarms() {
        let t0 = Instant::now();
        let armed = Deleted { at: None, ..deleted(&[1], Some(1), t0).unwrap() };
        // The confirmation was cancelled: the file is still there, so an unrelated reload later (another program
        // removing it) selects nothing.
        assert!(armed.over(t0, |_| false).is_none());
        let d = Deleted { at: None, ..deleted(&[1, 2], Some(1), t0).unwrap() };
        assert!(d.over(t0, |n| n.ends_with('c')).is_some(), "one of them went");
    }

    #[test]
    fn a_navigation_or_home_never_selects_after_a_delete() {
        let t0 = Instant::now();
        let soon = t0 + Duration::from_secs(1);
        let replaced = names(&[r"C:\d\b"]);
        // A navigation (and Back to the folder) consumes the marker without selecting.
        assert_eq!(deleted(&[1], Some(1), t0).and_then(|d| d.next_for(false, r"C:\d", false, &replaced, soon)), None);
        // On Home and Quick Access the list behind the page would select a row the page does not show there.
        let home = Deleted { folder: HOME.into(), ..deleted(&[1], Some(1), t0).unwrap() };
        assert_eq!(home.next_for(true, HOME, true, &replaced, soon), None);
    }

    #[test]
    fn find_tab_follows_the_tab_not_its_old_index() {
        let tabs: Vec<Arc<AtomicU64>> = (0..3).map(|_| Arc::default()).collect();
        let id = tabs[2].clone();
        assert_eq!(find_tab(tabs.iter(), &id), Some(2));
        let after_close: Vec<_> = tabs[1..].to_vec();
        assert_eq!(find_tab(after_close.iter(), &id), Some(1), "a tab before it closed");
        assert_eq!(find_tab(tabs[..2].iter(), &id), None, "it closed");
    }

    #[test]
    fn only_libraries_and_archives_sort_on_the_ui_thread() {
        let dir = std::env::temp_dir().join(format!("rexplorer-test-sorts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let zip = dir.join("a.zip");
        std::fs::write(&zip, b"PK\x05\x06").unwrap();
        let d = dir.to_string_lossy();
        assert!(!sorts_on_ui_thread(&d), "a plain directory");
        assert!(sorts_on_ui_thread(&zip.to_string_lossy()));
        assert!(sorts_on_ui_thread(&format!("{d}\\a.zip\\sub\\deeper")), "a folder inside the archive");
        assert!(sorts_on_ui_thread("::{031E4825-7B94-4DC3-B131-E946B44C8DD5}"));
        assert!(sorts_on_ui_thread("::{031E4825-7B94-4DC3-B131-E946B44C8DD5}\\Documents.library-ms"));
        assert!(sorts_on_ui_thread("C:\\Users\\u\\AppData\\Roaming\\Microsoft\\Windows\\Libraries\\Music.library-ms"));
        for loader in ["::{645FF040-5081-101B-9F08-00AA002F954E}", "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}", "::{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}", "::{26EE0668-A00A-44D7-9371-BEB064C98683}\\0"] {
            assert!(!sorts_on_ui_thread(loader), "{loader}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn history_jump_keeps_order() {
        let mut b = vec!["A", "B", "D", "E"];
        assert_eq!(take_steps(&mut b, 3), Some(("B", vec!["E", "D"])));
        assert_eq!(b, ["A"]);
        let mut f = vec![];
        land(&mut f, "C", vec!["E", "D"]);
        assert_eq!((f.pop(), f.pop(), f.pop()), (Some("D"), Some("E"), Some("C")));
        assert_eq!(take_steps(&mut b, 0), None);
        assert_eq!(take_steps(&mut b, 9), None);
        assert_eq!(b, ["A"]);
    }

    #[test]
    fn back_cancels_only_a_forward_step_in_flight() {
        assert!(back_cancels_load(Some(History::Push), true));
        // Nothing on screen to stay on, nothing loading, or a load whose history already moved: a normal Back.
        assert!(!back_cancels_load(Some(History::Push), false));
        assert!(!back_cancels_load(None, true));
        for h in [History::Back, History::Forward, History::Refresh] {
            assert!(!back_cancels_load(Some(h), true));
        }
    }

    #[test]
    fn history_menu_rows() {
        assert_eq!(history_entries(&["A", "R&D"]), [(1, "R&&D".to_string()), (2, "A".to_string())]);
        assert_eq!(history_entries(&["x"; 20]).len(), 16);
    }

    #[test]
    fn to_close_rules() {
        assert_eq!(to_close(4, 1, true), [3, 2, 0]);
        assert_eq!(to_close(4, 1, false), [3, 2]);
        assert!(to_close(4, 3, false).is_empty());
        assert!(to_close(1, 0, true).is_empty());
    }

    #[test]
    fn destinations_resolve_namespace_names_and_keep_typed_paths() {
        let _com = ComInit::sta();
        let (name, parsing) = destination(&Target::Name(format!("shell:{HOME}")));
        assert_eq!(parsing, HOME);
        assert!(!name.is_empty() && !name.starts_with("shell:"), "the shell's (localized) name: {name:?}");
        let pc = ShellFolder::parse("shell:MyComputerFolder").unwrap();
        assert_eq!(destination(&Target::Pidl(pc.pidl.clone())), (pc.name.clone(), pc.parsing_name.clone()));
        // Search results no longer kept go back to the folder searched.
        assert_eq!(destination(&Target::Search(9, pc.pidl.clone())), (pc.name.clone(), pc.parsing_name.clone()));
        let typed = r"C:\x\a.rar\docs".to_string();
        assert_eq!(destination(&Target::Name(typed.clone())), (typed.clone(), typed));
        let bad = "shell:NoSuchFolder".to_string();
        assert_eq!(destination(&Target::Name(bad.clone())), (bad.clone(), bad));
    }

    #[test]
    fn typed_addresses_expand_environment_names() {
        let get = |n: &str| match n.to_ascii_uppercase().as_str() {
            "TEMP" => Some(r"C:\T".to_string()),
            "USERPROFILE" => Some(r"C:\Users\u".to_string()),
            _ => None,
        };
        assert_eq!(expand_env("%TEMP%", get), r"C:\T");
        assert_eq!(expand_env(r"%userprofile%\Documents", get), r"C:\Users\u\Documents");
        assert_eq!(expand_env(r"%TEMP%\%USERPROFILE%", get), r"C:\T\C:\Users\u");
        assert_eq!(expand_env(r"%NOPE%\x", get), r"%NOPE%\x", "unknown names stay");
        assert_eq!(expand_env("100% sure", get), "100% sure");
        assert_eq!(expand_env("a%%b", get), "a%%b");
        assert_eq!(expand_env(r"C:\plain", get), r"C:\plain");
    }

    #[test]
    fn home_falls_back_to_this_pc_and_nothing_else_does() {
        assert_eq!(fallback_of(settings::HOME_START), Some("shell:MyComputerFolder"));
        assert_eq!(fallback_of(&settings::HOME_START.to_ascii_lowercase()), Some("shell:MyComputerFolder"));
        for other in [r"C:\", "shell:MyComputerFolder", HOME, r"\\server\share"] {
            assert_eq!(fallback_of(other), None, "{other}");
        }
    }

    #[test]
    fn destination_of_an_unreachable_share_touches_no_network() {
        // A non-routable address: any SMB or DNS attempt would take seconds before failing.
        let unc = r"\\10.255.255.1\share\folder".to_string();
        let t = Instant::now();
        assert_eq!(destination(&Target::Name(unc.clone())), (unc.clone(), unc));
        assert!(t.elapsed() < Duration::from_millis(200), "{:?}", t.elapsed());
    }

    #[test]
    fn ctrl_double_click_opens_folders_in_a_tab() {
        assert!(opens_tab(true, true, false));
        assert!(!opens_tab(true, false, false));
        assert!(!opens_tab(false, true, false));
        assert!(!opens_tab(true, true, true));
    }

    fn shell_listing(dir: &std::path::Path, sort: (u32, bool), prefill: usize) -> Listing {
        match load(Target::Name(dir.display().to_string()), true, Some((sort.0, sort.1, None)), &[], prefill, &|| false) {
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
        assert!(load(Target::Name(dir.display().to_string()), true, Some((0, false, None)), &[], 0, &|| true).is_err(), "superseded");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_sort_is_name_except_downloads_newest_first() {
        let _com = ComInit::mta();
        let l = shell_listing(&std::env::temp_dir(), (0, false), 0);
        let date = l.columns.iter().find(|c| c.kind == SHCOLSTATE_TYPE_DATE).expect("a date column").index;
        assert_eq!(default_sort(&std::env::temp_dir().display().to_string(), &l.columns), (0, false, None));
        let dl = panes::known_folder(&FOLDERID_Downloads).unwrap();
        assert_eq!(default_sort(&dl, &l.columns), (date, true, Some(date)));
        assert_eq!(default_sort(&format!("{}\\", dl.to_uppercase()), &l.columns), (date, true, Some(date)), "case and trailing backslash");
    }

    /// Days since 1601-01-01 of a date, as Windows counts them (independent of `civil`).
    fn day(y: u16, m: u16, d: u16) -> i64 {
        let mut ft = FILETIME::default();
        unsafe { SystemTimeToFileTime(&SYSTEMTIME { wYear: y, wMonth: m, wDay: d, ..Default::default() }, &mut ft) }.unwrap();
        ((ft.dwHighDateTime as i64) << 32 | ft.dwLowDateTime as i64) / 864_000_000_000
    }

    #[test]
    fn civil_epoch_is_monday_1601() {
        assert_eq!((day(1601, 1, 1), civil(0)), (0, (1601, 1, 1)));
        assert_eq!(day(2026, 10, 12) % 7, 0, "a Monday");
        for (y, m, d) in [(1601, 12, 31), (1700, 3, 1), (1900, 2, 28), (2000, 2, 29), (2024, 12, 31), (2026, 10, 13), (2028, 2, 29), (2100, 3, 1)] {
            assert_eq!(civil(day(y, m, d)), (y as i64, m as i64, d as i64));
        }
    }

    #[test]
    fn date_groups_like_explorer() {
        let today = day(2026, 10, 13); // a Tuesday
        let g = |y, m, d, week_start| tr(DATE_GROUPS[date_group(day(y, m, d), today, week_start)]);
        let monday = [
            ((2026, 10, 14), "In the future"),
            ((2026, 10, 13), "Today"),
            ((2026, 10, 12), "Yesterday"),
            ((2026, 10, 11), "Last week"),
            ((2026, 10, 5), "Last week"),
            ((2026, 10, 4), "Earlier this month"),
            ((2026, 10, 1), "Earlier this month"),
            ((2026, 9, 30), "Last month"),
            ((2026, 9, 1), "Last month"),
            ((2026, 8, 31), "Earlier this year"),
            ((2026, 1, 1), "Earlier this year"),
            ((2025, 12, 31), "A long time ago"),
        ];
        for ((y, m, d), want) in monday {
            assert_eq!(g(y, m, d, 0), want, "{y}-{m}-{d}");
        }
        // Weeks that start on Sunday: the 11th is this week's.
        assert_eq!(g(2026, 10, 11, 6), "Earlier this week");
        assert_eq!(g(2026, 10, 4, 6), "Last week");
        assert_eq!(g(2026, 10, 3, 6), "Earlier this month");
    }

    #[test]
    fn date_group_edges() {
        let g = |d, today| tr(DATE_GROUPS[date_group(d, today, 0)]);
        // Today starts the week: the day before is Yesterday, the one before that last week.
        assert_eq!(g(day(2026, 10, 11), day(2026, 10, 12)), "Yesterday");
        assert_eq!(g(day(2026, 10, 10), day(2026, 10, 12)), "Last week");
        let today = day(2026, 1, 2);
        assert_eq!(g(day(2025, 12, 31), today), "Earlier this week");
        assert_eq!(g(day(2025, 12, 22), today), "Last week");
        assert_eq!(g(day(2025, 12, 15), today), "Last month");
        assert_eq!(g(day(2025, 11, 30), today), "A long time ago");
        assert_eq!(g(day(2028, 2, 29), day(2028, 3, 1)), "Yesterday");
        assert_eq!(g(day(2028, 2, 1), day(2028, 3, 1)), "Last month");
    }

    #[test]
    fn group_rows_contiguous_stable() {
        let names = |rows: &[(&'static str, usize)]| rows.iter().map(|r| r.0).collect::<Vec<_>>();
        // The shell's order (folders F first) with each row's group rank; newest first is the lowest rank first.
        let mut rows = vec![("F1", 2), ("F2", 1), ("a", 1), ("b", 2), ("c", 8), ("d", 2)];
        assert_eq!(group_rows(&mut rows, |r| r.1, true), [(1, 0), (2, 2), (8, 5)]);
        assert_eq!(names(&rows), ["F2", "a", "F1", "b", "d", "c"]);
        let mut rows = vec![("F1", 2), ("F2", 1), ("c", 8), ("b", 2), ("a", 1)];
        assert_eq!(group_rows(&mut rows, |r| r.1, false), [(8, 0), (2, 1), (1, 3)]);
        assert_eq!(names(&rows), ["c", "F1", "b", "F2", "a"]);
        assert!(group_rows(&mut Vec::<u8>::new(), |_| 0, true).is_empty());
    }

    #[test]
    fn toggled_columns() {
        let base = [(0, 300), (3, 100)];
        assert_eq!(toggled(&base, 2), [(0, 300), (3, 100), (2, 0)]);
        assert_eq!(toggled(&base, 3), [(0, 300)]);
        assert_eq!(toggled(&base, 0), base);
    }

    #[test]
    fn chosen_columns_keep_name_first() {
        assert_eq!(chosen_columns(&[0, 1, 2, 3, 4, 10], &[(3, 0), (0, 0), (2, 0), (99, 0), (3, 0)]), [0, 3, 2]);
        assert_eq!(chosen_columns(&[0, 1], &[]), [0]);
    }

    #[test]
    fn layout_round_trip() {
        let layout = [(3, 100), (0, 300), (2, 90)];
        assert_eq!(display_order(&layout, &[0, 3, 2]), [1, 0, 2]);
        assert_eq!(layout_of(&[1, 0, 2], &[0, 3, 2], &[300, 100, 90]), layout);
        assert_eq!(display_order(&[(9, 1)], &[0, 3]), [0, 1]);
        assert_eq!(display_order(&[(3, 1), (3, 2)], &[0, 3]), [1, 0]);
    }

    #[test]
    fn size_groups_like_explorer() {
        let g = |folder, b| tr(SIZE_GROUPS[size_group(folder, b)]);
        assert_eq!(g(false, Some(0)), "Empty (0 KB)");
        for b in [1, 16 * 1024 - 1] {
            assert_eq!(g(false, Some(b)), "Tiny (0 - 16 KB)");
        }
        assert_eq!(g(false, Some(16 * 1024)), "Small (16 KB - 1 MB)");
        assert_eq!(g(false, Some(1 << 20)), "Medium (1 - 128 MB)");
        assert_eq!(g(false, Some(128 << 20)), "Large (128 MB - 1 GB)");
        assert_eq!(g(false, Some(1 << 30)), "Huge (1 - 4 GB)");
        assert_eq!(g(false, Some(4 << 30)), "Huge (1 - 4 GB)");
        assert_eq!(g(false, Some((4 << 30) + 1)), "Gigantic (> 4 GB)");
        assert_eq!(g(true, Some(0)), "Unspecified");
        assert_eq!(g(false, None), "Unspecified");
    }

    #[test]
    fn name_groups() {
        for (n, g) in [("alpha", "A"), ("Ébène", "É"), ("42.txt", "0 - 9"), ("_x", "Other"), ("", "Other")] {
            assert_eq!(name_group(n), g, "{n}");
        }
    }

    #[test]
    fn text_ranks_put_unspecified_last() {
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(text_ranks(s(&["Text Document", "", "JPEG image", "Text Document"])), (vec![1, 2, 0, 1], s(&["JPEG image", "Text Document", "Unspecified"])));
    }

    #[test]
    fn group_order_follows_the_group_column() {
        assert!(groups_lowest_first(false, false, false) && groups_lowest_first(false, true, true));
        assert!(groups_lowest_first(true, true, true));
        assert!(!groups_lowest_first(true, false, true));
        assert!(groups_lowest_first(true, false, false));
        assert!(!groups_lowest_first(true, true, false));
    }

    #[test]
    fn load_groups_by_date_size_and_name() {
        let _com = ComInit::mta();
        let dir = std::env::temp_dir().join(format!("rexplorer-nav-groups-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for n in ["new.txt", "old.txt"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let old = std::time::SystemTime::now() - Duration::from_secs(800 * 86_400);
        std::fs::File::options().write(true).open(dir.join("old.txt")).unwrap().set_modified(old).unwrap();
        let probe = shell_listing(&dir, (0, false), 0);
        let date = probe.columns.iter().find(|c| c.kind == SHCOLSTATE_TYPE_DATE).expect("a date column").index;
        let size = panes::size_and_type_columns(&probe.folder.folder).0.unwrap();
        let get = |sort| match load(Target::Name(dir.display().to_string()), true, Some(sort), &[], 0, &|| false) {
            Ok(Loaded { source: Source::Shell(l), .. }) => (
                l.items.iter().map(|i| i.display_name().trim_end_matches(".txt").to_string()).collect::<Vec<_>>(),
                l.groups.into_iter().collect::<Vec<_>>(),
            ),
            _ => panic!("a shell listing"),
        };
        let by_date = get((date, true, Some(date)));
        let none = get((date, true, None)).1;
        let by_name = get((0, false, Some(0)));
        let by_size = get((size, true, Some(size)));
        let _ = std::fs::remove_dir_all(&dir);
        let at = |g: &[(String, usize)]| g.iter().map(|(l, f)| (l.clone(), *f)).collect::<Vec<_>>();
        let want = |v: &[(&str, usize)]| v.iter().map(|(l, f)| (l.to_string(), *f)).collect::<Vec<_>>();
        assert_eq!(by_date.0, ["new", "sub", "old"], "the folder stays in its group");
        assert_eq!(at(&by_date.1), want(&[("Today", 0), ("A long time ago", 2)]));
        assert!(none.is_empty(), "off");
        assert_eq!(by_name.0, ["new", "old", "sub"]);
        assert_eq!(at(&by_name.1), want(&[("N", 0), ("O", 1), ("S", 2)]));
        assert_eq!(at(&by_size.1), want(&[("Tiny (0 - 16 KB)", 0), ("Unspecified", 2)]));
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
