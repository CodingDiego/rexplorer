//! What a tab shows: a shell folder, the inside of a .rar archive, search results, or the embedded Explorer
//! view for folders that exist but cannot be listed. One small set of functions dispatches over them.

use super::*;
use crate::archive_view::ArchiveListing;
use crate::nav::{Crumb, Listing, Loaded, Show, Target, cur, go, source};
use rexplorer_core::archive::{self, Entry};
use rexplorer_core::browser::{self, EmbeddedBrowser};
use rexplorer_core::search::{self, Hit};
use rexplorer_core::usn::{self, VolumeIndex};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW, ILClone, ILIsEqual, ILRemoveLastID, IShellFolder,
    SHBindToParent, SHBrowseForFolderW, SHGFI_USEFILEATTRIBUTES, SHGetPathFromIDListW, SHParseDisplayName,
};
use windows::core::Interface;

/// lparam = Box<SearchBatch>.
pub(crate) const WM_SEARCH_BATCH: u32 = WM_APP + 4;
/// lparam = Box<Pidl>: the embedded browser finished a navigation.
pub(crate) const WM_BROWSER_NAV: u32 = WM_APP + 5;
/// lparam = Box<String>: a worker failed (archive extraction); shown as an error box.
pub(crate) const WM_WORKER_ERROR: u32 = WM_APP + 6;
/// Most results the volume index returns per search.
const USN_LIMIT: usize = 5000;

pub(crate) type UsnCache = Arc<Mutex<HashMap<char, Arc<VolumeIndex>>>>;

pub(crate) enum Source {
    Shell(Listing),
    Archive(ArchiveView),
    Search(SearchListing),
    /// The folder exists but cannot be listed: the embedded Explorer view shows it.
    Browser(BrowserPlace),
}

/// One directory of an archive, rows cached (the listing sorts on every `rows()` call).
pub(crate) struct ArchiveView {
    pub listing: ArchiveListing,
    pub rows: Vec<Entry>,
    pub icons: Vec<i32>,
}

pub(crate) struct SearchRow {
    pub item: ShellItem,
    pub name: String,
    pub folder: String,
    pub kind: String,
    pub size: String,
    pub is_folder: bool,
}

pub(crate) struct SearchListing {
    pub needle: String,
    pub root: Pidl,
    pub root_parsing: String,
    pub root_editing: String,
    pub root_crumbs: Vec<Crumb>,
    pub rows: Vec<SearchRow>,
    pub icons: Vec<i32>,
    pub generation: u64,
    pub running: bool,
    cancel: Arc<AtomicBool>,
}

impl SearchListing {
    pub fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.running = false;
    }
}

impl Drop for SearchListing {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub(crate) struct BrowserPlace {
    pub pidl: Pidl,
    pub name: String,
    pub parsing: String,
    pub editing: String,
}

impl BrowserPlace {
    pub fn of(f: &ShellFolder) -> BrowserPlace {
        BrowserPlace { pidl: f.pidl.clone(), name: f.name.clone(), parsing: f.parsing_name.clone(), editing: f.editing_name.clone() }
    }
}

pub(crate) struct SearchBatch {
    generation: u64,
    rows: Vec<SearchRow>,
    done: bool,
}

fn col(index: u32, title: &str, width_chars: i32, right_align: bool) -> Column {
    Column { index, title: title.to_string(), width_chars, right_align }
}

fn parent_pidl(p: &Pidl) -> Option<Pidl> {
    if p.is_empty() {
        return None;
    }
    let mut p = p.clone();
    unsafe { ILRemoveLastID(Some(p.as_mut_ptr())) }.as_bool().then_some(p)
}

impl Source {
    pub fn len(&self) -> usize {
        match self {
            Source::Shell(l) => l.items.len(),
            Source::Archive(a) => a.rows.len(),
            Source::Search(s) => s.rows.len(),
            Source::Browser(_) => 0,
        }
    }

    /// Text of row `i`, column `sub` (header position).
    pub fn text(&self, i: usize, sub: usize) -> String {
        match self {
            Source::Shell(l) => match l.items.get(i) {
                Some(e) if sub == 0 => e.display_name(),
                Some(e) => l.columns.get(sub).map(|c| e.detail(c.index)).unwrap_or_default(),
                None => String::new(),
            },
            Source::Archive(a) => a.rows.get(i).map(|e| ArchiveListing::detail(e, sub)).unwrap_or_default(),
            Source::Search(s) => s
                .rows
                .get(i)
                .map(|r| match sub {
                    0 => r.name.clone(),
                    1 => r.folder.clone(),
                    2 => r.kind.clone(),
                    _ => r.size.clone(),
                })
                .unwrap_or_default(),
            Source::Browser(_) => String::new(),
        }
    }

    pub fn columns(&self) -> Vec<Column> {
        match self {
            Source::Shell(l) => l.columns.iter().map(|c| col(c.index, &c.title, c.width_chars, c.right_align)).collect(),
            Source::Archive(_) => {
                vec![col(0, "Name", 40, false), col(1, "Date modified", 20, false), col(2, "Type", 20, false), col(3, "Size", 12, true)]
            }
            Source::Search(_) => vec![col(0, "Name", 40, false), col(1, "Folder", 60, false), col(2, "Type", 20, false), col(3, "Size", 12, true)],
            Source::Browser(_) => Vec::new(),
        }
    }

    pub fn title(&self) -> String {
        match self {
            Source::Shell(l) => l.folder.name.clone(),
            Source::Archive(a) => a.listing.title(),
            Source::Search(s) => format!("Search: {}", s.needle),
            Source::Browser(b) => b.name.clone(),
        }
    }

    /// What the address bar shows; a search shows the folder it searches.
    pub fn editing_name(&self) -> String {
        match self {
            Source::Shell(l) => l.folder.editing_name.clone(),
            Source::Archive(a) => a.listing.editing_name(),
            Source::Search(s) => s.root_editing.clone(),
            Source::Browser(b) => b.editing.clone(),
        }
    }

    /// The name the session and the recent list keep; it loads this place again (a search: its folder).
    pub fn parsing_name(&self) -> String {
        match self {
            Source::Shell(l) => l.folder.parsing_name.clone(),
            Source::Archive(a) => a.listing.editing_name(),
            Source::Search(s) => s.root_parsing.clone(),
            Source::Browser(b) => b.parsing.clone(),
        }
    }

    /// What Ctrl+Shift+E opens in Explorer: an archive's own folder, else the place itself.
    pub fn explorer_name(&self) -> String {
        match self {
            Source::Archive(a) => a.listing.archive.parent().map(|p| p.display().to_string()).unwrap_or_default(),
            _ => self.parsing_name(),
        }
    }

    /// Where back/forward return to, and what a duplicated tab opens.
    pub fn location(&self) -> Target {
        match self {
            Source::Shell(l) => Target::Pidl(l.folder.pidl.clone()),
            Source::Archive(a) => Target::Name(a.listing.editing_name()),
            Source::Search(s) => Target::Pidl(s.root.clone()),
            Source::Browser(b) => Target::Pidl(b.pidl.clone()),
        }
    }

    /// Up: the containing folder; at an archive's root its folder; from a search the folder searched.
    pub fn parent(&self) -> Option<Target> {
        match self {
            // Home and Quick Access sit on the desktop root; Up goes to This PC, as in Explorer.
            Source::Shell(l) if [nav::HOME, nav::QUICK_ACCESS].iter().any(|h| l.folder.parsing_name.eq_ignore_ascii_case(h)) => {
                Some(Target::Name(ui::THIS_PC.to_string()))
            }
            Source::Shell(l) => l.folder.parent_pidl().map(Target::Pidl),
            Source::Archive(a) => archive_parent(&a.listing.archive, &a.listing.dir).map(Target::Name),
            Source::Search(s) => Some(Target::Pidl(s.root.clone())),
            Source::Browser(b) => parent_pidl(&b.pidl).map(Target::Pidl),
        }
    }
}

// ---- archives ------------------------------------------------------------------------------------

/// Up from directory `dir` ("" = root) of `archive`: the parent directory, the archive's root, or its folder.
fn archive_parent(archive: &Path, dir: &str) -> Option<String> {
    match dir.rfind('\\') {
        Some(k) => Some(format!("{}\\{}", archive.display(), &dir[..k])),
        None if !dir.is_empty() => Some(archive.display().to_string()),
        None => archive.parent().map(|p| p.display().to_string()),
    }
}

/// Worker side: lists the archive and builds the crumbs (the archive's folder, the archive, its directories).
pub(crate) fn load_archive(archive: &Path, dir: &str) -> Result<Loaded> {
    let listing = archive_view::open(archive, dir)?;
    let rows: Vec<Entry> = listing.rows().into_iter().cloned().collect();
    let mut crumbs = archive.parent().and_then(|p| ShellFolder::parse(&p.display().to_string()).ok()).map(|f| nav::crumbs_of(&f)).unwrap_or_default();
    let mut path = archive.display().to_string();
    let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    crumbs.push(Crumb { name: name.clone(), parsing: path.clone(), pidl: None });
    for part in listing.dir.split('\\').filter(|p| !p.is_empty()) {
        path = format!("{path}\\{part}");
        crumbs.push(Crumb { name: part.to_string(), parsing: path.clone(), pidl: None });
    }
    let icon = sys_icon(&name, false);
    let icons = vec![ICON_UNKNOWN; rows.len()];
    Ok(Loaded { source: Source::Archive(ArchiveView { listing, rows, icons }), crumbs, icon })
}

/// System image list and small icon index for a name, by extension only (no disk access).
pub(crate) fn sys_icon(name: &str, dir: bool) -> (usize, i32) {
    let mut fi = SHFILEINFOW::default();
    let attr = if dir { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_NORMAL };
    let h = HSTRING::from(name);
    let flags = SHGFI_USEFILEATTRIBUTES | SHGFI_SYSICONINDEX | SHGFI_SMALLICON;
    let himl = unsafe { SHGetFileInfoW(PCWSTR(h.as_ptr()), attr, Some(&mut fi), size_of::<SHFILEINFOW>() as u32, flags) };
    (himl, fi.iIcon)
}

/// Icon-theme icon, else the system icon for the name (the system list goes into the ListView on first use).
unsafe fn name_icon(app: *mut App, name: &str, dir: bool) -> i32 {
    if let Some(i) = views::theme_icon_for(app, name, dir) {
        return i;
    }
    let (himl, index) = sys_icon(name, dir);
    if himl != 0 && !(*app).image_list_set {
        views::set_small_source(app, HIMAGELIST(himl as isize));
    }
    index
}

/// Details-view small icon of row `i` of the active tab, cached per row.
pub(crate) unsafe fn small_icon(app: *mut App, i: usize) -> i32 {
    let (cache, name, dir, shell): (&mut Vec<i32>, String, bool, Option<(&ShellItem, &Pidl)>) = match &mut cur(app).source {
        Some(Source::Shell(l)) => {
            let Some(item) = l.items.get(i) else { return 0 };
            (&mut l.icons, String::new(), false, Some((item, &l.folder.pidl)))
        }
        Some(Source::Archive(a)) => {
            let Some(e) = a.rows.get(i) else { return 0 };
            (&mut a.icons, e.name.clone(), e.is_dir, None)
        }
        Some(Source::Search(s)) => {
            let Some(r) = s.rows.get(i) else { return 0 };
            (&mut s.icons, r.name.clone(), r.is_folder, None)
        }
        _ => return 0,
    };
    let Some(slot) = cache.get_mut(i) else { return 0 };
    if *slot == ICON_UNKNOWN {
        *slot = match shell {
            // shortcut: icon lookup runs on the UI thread, move it to a worker when a slow folder visibly stutters
            Some((item, folder)) => views::theme_icon(app, item).unwrap_or_else(|| item.icon_index(folder).1),
            None => name_icon(app, &name, dir),
        };
    }
    *slot
}

/// The archive popup: extract everything into a picked folder, or next to the archive.
pub(crate) unsafe fn archive_menu(app: *mut App) {
    let Some(Source::Archive(a)) = source(app) else { return };
    let archive = a.listing.archive.clone();
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let entries = [(1, "Extract all to...".to_string()), (2, "Extract here".to_string())];
    let dest = match ui::popup(app, &entries, &[], pt.x, pt.y) {
        1 => pick_folder((*app).hwnd),
        2 => archive.parent().map(|p| p.to_path_buf()),
        _ => None,
    };
    let Some(dest) = dest else { return };
    let hwnd = (*app).hwnd.0 as isize;
    std::thread::spawn(move || {
        if let Err(e) = archive::extract_all(&archive, &dest) {
            post_error(hwnd, format!("Cannot extract {}\n\n{e}", archive.display()));
        }
    });
}

unsafe fn pick_folder(owner: HWND) -> Option<PathBuf> {
    let mut name = [0u16; 260];
    let bi = BROWSEINFOW {
        hwndOwner: owner,
        pszDisplayName: PWSTR(name.as_mut_ptr()),
        lpszTitle: w!("Extract all to"),
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        ..Default::default()
    };
    let p = SHBrowseForFolderW(&bi);
    if p.is_null() {
        return None;
    }
    let p = Pidl::from_raw(p);
    let mut path = [0u16; 260];
    if !SHGetPathFromIDListW(p.as_ptr(), &mut path).as_bool() {
        return None;
    }
    let len = path.iter().position(|&c| c == 0).unwrap_or(path.len());
    Some(PathBuf::from(String::from_utf16_lossy(&path[..len])))
}

fn post_error(hwnd: isize, text: String) {
    let b = Box::into_raw(Box::new(text));
    if unsafe { PostMessageW(Some(HWND(hwnd as *mut c_void)), WM_WORKER_ERROR, WPARAM(0), LPARAM(b as isize)) }.is_err() {
        drop(unsafe { Box::from_raw(b) });
    }
}

// ---- activation ----------------------------------------------------------------------------------

/// Enter / double-click on row `index`: open a folder (or a .rar) in place, or the file with its default app.
pub(crate) unsafe fn activate(app: *mut App, index: usize) {
    let hwnd = (*app).hwnd;
    match source(app) {
        Some(Source::Shell(l)) => {
            let Some(item) = l.items.get(index) else { return };
            let parsing = item.parsing_name();
            if archive::is_rar(Path::new(&parsing)) && ui::is_fs_path(&parsing) {
                go(app, Target::Name(parsing), nav::History::Push);
            } else if item.is_folder() {
                go(app, Target::Pidl(l.folder.child_pidl(item)), nav::History::Push);
            } else if let Ok(menu) = ContextMenu::for_items(&l.folder, &[item], hwnd) {
                // The shell shows its own error UI when a file cannot be opened.
                let _ = menu.invoke_default(hwnd);
            }
        }
        Some(Source::Archive(a)) => {
            let Some(e) = a.rows.get(index).cloned() else { return };
            let archive = a.listing.archive.clone();
            if e.is_dir {
                go(app, Target::Name(format!("{}\\{}", archive.display(), e.path)), nav::History::Push);
                return;
            }
            let h = hwnd.0 as isize;
            std::thread::spawn(move || {
                let _com = ComInit::sta();
                let opened = archive_view::open(&archive, "").and_then(|l| l.extract_for_open(&e)).and_then(|p| instance::open_default_app(&p));
                if let Err(err) = opened {
                    post_error(h, format!("Cannot open {}\n\n{err}", e.name));
                }
            });
        }
        Some(Source::Search(s)) => {
            let Some(r) = s.rows.get(index) else { return };
            let parsing = r.item.parsing_name();
            if r.is_folder || (archive::is_rar(Path::new(&parsing)) && ui::is_fs_path(&parsing)) {
                go(app, Target::Name(parsing), nav::History::Push);
            } else if let Some(menu) = list::selection_menu(app, &[index]) {
                let _ = menu.invoke_default(hwnd);
            }
        }
        _ => {}
    }
}

/// The shell menu for search rows: their folder is parsed again, so all rows must share one folder.
pub(crate) unsafe fn search_menu(app: *mut App, rows: &[usize]) -> Option<ContextMenu> {
    let Some(Source::Search(s)) = source(app) else { return None };
    let picked: Vec<&SearchRow> = rows.iter().filter_map(|&i| s.rows.get(i)).collect();
    let first = picked.first()?;
    if picked.iter().any(|r| r.folder != first.folder) {
        let _ = MessageBeep(MB_OK);
        return None;
    }
    let folder = ShellFolder::parse(&first.folder).ok()?;
    let items: Vec<&ShellItem> = picked.iter().map(|r| &r.item).collect();
    ContextMenu::for_items(&folder, &items, (*app).hwnd).ok()
}

// ---- search --------------------------------------------------------------------------------------

/// Starts searching tab `t`'s folder (or the folder its search covers) for `needle` on a worker thread.
/// `push` records the previous place in the history; a re-run (F5) does not.
pub(crate) unsafe fn start_search(app: *mut App, t: usize, needle: &str, push: bool) {
    let needle = needle.trim().to_string();
    let Some(tab) = (&mut (*app).tabs).get_mut(t) else { return };
    if needle.is_empty() {
        return;
    }
    let (root, root_parsing, root_editing, root_crumbs) = match &tab.source {
        Some(Source::Shell(l)) => (l.folder.pidl.clone(), l.folder.parsing_name.clone(), l.folder.editing_name.clone(), tab.crumbs.clone()),
        Some(Source::Search(s)) => (s.root.clone(), s.root_parsing.clone(), s.root_editing.clone(), s.root_crumbs.clone()),
        _ => {
            let _ = MessageBeep(MB_OK);
            return;
        }
    };
    (*app).generation += 1;
    let generation = (*app).generation;
    // A load still in flight for this tab no longer lands here.
    tab.pending = 0;
    tab.loading = None;
    let old = tab.source.take();
    if let (true, Some(o)) = (push, &old) {
        tab.back.push(o.location());
        tab.forward.clear();
    }
    drop(old); // stops a previous search
    let cancel = Arc::new(AtomicBool::new(false));
    tab.crumbs = root_crumbs.clone();
    tab.crumbs.push(Crumb { name: format!("Search: {needle}"), parsing: String::new(), pidl: None });
    tab.name = root_parsing.clone();
    let drive = (usn::is_elevated() && root_parsing.len() == 3 && root_parsing.ends_with(":\\")).then(|| root_parsing.chars().next().unwrap());
    let s = SearchListing {
        needle: needle.clone(),
        root: root.clone(),
        root_parsing,
        root_editing,
        root_crumbs,
        rows: Vec::new(),
        icons: Vec::new(),
        generation,
        running: true,
        cancel: cancel.clone(),
    };
    tabs::set_title((*app).strip, t, &format!("Search: {needle}"));
    tab.source = Some(Source::Search(s));
    if t == (*app).active {
        nav::show(app, Show::Fresh(Vec::new()));
        SetTimer(Some((*app).hwnd), TIMER_SKELETON, 120, None);
    }
    nav::session_changed(app);

    let hwnd = (*app).hwnd.0 as isize;
    let usn_cache = (*app).usn.clone();
    std::thread::spawn(move || {
        let _com = ComInit::mta();
        let post = |rows: Vec<SearchRow>, done: bool| {
            let b = Box::into_raw(Box::new(SearchBatch { generation, rows, done }));
            if unsafe { PostMessageW(Some(HWND(hwnd as *mut c_void)), WM_SEARCH_BATCH, WPARAM(0), LPARAM(b as isize)) }.is_err() {
                drop(unsafe { Box::from_raw(b) });
            }
        };
        let mut sink = |hits: Vec<Hit>| post(rows_of(hits), false);
        let indexed = drive.is_some_and(|d| usn_search(&usn_cache, d, &needle, &cancel, &mut sink).is_ok());
        if !indexed {
            let _ = search::walk(&root, &needle, &cancel, &mut sink);
        }
        post(Vec::new(), true);
    });
}

/// Elevated search of a whole NTFS volume through its MFT index (built once per drive, kept for the session).
fn usn_search(cache: &UsnCache, drive: char, needle: &str, cancel: &AtomicBool, sink: &mut dyn FnMut(Vec<Hit>)) -> Result<()> {
    let index = {
        let mut m = cache.lock().unwrap_or_else(|e| e.into_inner());
        match m.get(&drive) {
            Some(i) => i.clone(),
            None => {
                let i = Arc::new(usn::build(drive)?);
                m.insert(drive, i.clone());
                i
            }
        }
    };
    let mut batch = Vec::new();
    for path in index.search(needle, USN_LIMIT) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if let Some(hit) = hit_for_path(&path) {
            batch.push(hit);
        }
        if batch.len() >= 256 {
            sink(std::mem::take(&mut batch));
        }
    }
    if !batch.is_empty() {
        sink(batch);
    }
    Ok(())
}

/// A filesystem path as a shell item of its parent folder.
pub(crate) fn hit_for_path(path: &Path) -> Option<Hit> {
    unsafe {
        let wide = HSTRING::from(path.as_os_str());
        let mut abs: *mut ITEMIDLIST = std::ptr::null_mut();
        SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut abs, 0, None).ok()?;
        let abs = Pidl::from_raw(abs);
        let mut last: *mut ITEMIDLIST = std::ptr::null_mut();
        let parent: IShellFolder = SHBindToParent(abs.as_ptr(), Some(&mut last)).ok()?;
        let child = ILClone(last);
        if child.is_null() {
            return None;
        }
        Some(Hit { item: ShellItem { parent, pidl: Pidl::from_raw(child) }, folder: path.parent()?.display().to_string() })
    }
}

/// Worker side: the text the list shows, computed once per hit.
fn rows_of(hits: Vec<Hit>) -> Vec<SearchRow> {
    // Hits of one walker batch share a parent folder: map its Size and Type columns once.
    let mut last: Option<(*mut c_void, (Option<u32>, Option<u32>))> = None;
    hits.into_iter()
        .map(|h| {
            let key = h.item.parent.as_raw();
            let (size_col, type_col) = match last {
                Some((k, cols)) if k == key => cols,
                _ => {
                    let cols = panes::size_and_type_columns(&h.item.parent);
                    last = Some((key, cols));
                    cols
                }
            };
            let detail = |c: Option<u32>| c.map(|c| h.item.detail(c)).unwrap_or_default();
            SearchRow {
                name: h.item.display_name(),
                kind: detail(type_col),
                size: detail(size_col),
                is_folder: h.item.is_folder(),
                folder: h.folder,
                item: h.item,
            }
        })
        .collect()
}

/// WM_SEARCH_BATCH: rows join their tab's results; the active tab's list grows in place.
pub(crate) unsafe fn on_search_batch(app: *mut App, batch: Box<SearchBatch>) {
    let SearchBatch { generation, rows, done } = *batch;
    let found = (*app).tabs.iter().position(|t| matches!(&t.source, Some(Source::Search(s)) if s.generation == generation));
    let Some(t) = found else { return };
    let Some(Source::Search(s)) = &mut (&mut (*app).tabs)[t].source else { return };
    let before = s.rows.len();
    s.rows.extend(rows);
    s.icons.resize(s.rows.len(), ICON_UNKNOWN);
    if done {
        s.running = false;
    }
    let count = s.rows.len();
    if t != (*app).active {
        return;
    }
    let list = (*app).list;
    if count > 0 || done {
        nav::stop_skeleton(app);
    }
    if count != before {
        SendMessageW(list, LVM_SETITEMCOUNT, Some(WPARAM(count)), Some(LPARAM((LVSICF_NOINVALIDATEALL | LVSICF_NOSCROLL) as isize)));
        if before == 0 {
            list::set_item_state(list, 0, LVIS_FOCUSED, LVIS_FOCUSED);
        }
    }
    ui::update_status(app);
}

/// Escape: stop the active tab's search, keeping what it found.
pub(crate) unsafe fn cancel_search(app: *mut App) -> bool {
    match &mut cur(app).source {
        Some(Source::Search(s)) if s.running => {
            s.stop();
            ui::update_status(app);
            true
        }
        _ => false,
    }
}

/// The search box: Enter searches the active tab, Escape stops the search; both return to the list.
pub(crate) unsafe extern "system" fn search_subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, data: usize) -> LRESULT {
    let app = data as *mut App;
    match msg {
        WM_KEYDOWN if wparam.0 == VK_RETURN.0 as usize => {
            let text = window_text(hwnd);
            let _ = SetFocus(Some(ui::view_hwnd(app)));
            start_search(app, (*app).active, &text, true);
            LRESULT(0)
        }
        WM_KEYDOWN if wparam.0 == VK_ESCAPE.0 as usize => {
            cancel_search(app);
            let _ = SetFocus(Some(ui::view_hwnd(app)));
            LRESULT(0)
        }
        // No beep for Enter and Escape in a single-line Edit.
        WM_CHAR if wparam.0 == 0x0D || wparam.0 == 0x1B => LRESULT(0),
        _ => DefSubclassProc(hwnd, msg, wparam, lparam),
    }
}

// ---- embedded browser ----------------------------------------------------------------------------

/// Shows `place` in the embedded Explorer view over the list; Explorer itself when the view cannot be created.
pub(crate) unsafe fn enter_browser(app: *mut App, place: &BrowserPlace) {
    let a = &mut *app;
    if a.browser.is_none() {
        // Creating the view blocks this thread for a moment: the skeleton stands in for it meanwhile.
        a.skeleton = true;
        SendMessageW(a.list, LVM_SETITEMCOUNT, Some(WPARAM(0)), Some(LPARAM(0)));
        let _ = InvalidateRect(Some(a.list), None, true);
        let _ = windows::Win32::Graphics::Gdi::UpdateWindow(a.list);
        a.skeleton = false;
        let hwnd = a.hwnd.0 as isize;
        // Runs inside the browser's own message handling: only post.
        let post = move |p: Pidl| {
            let b = Box::into_raw(Box::new(p));
            if unsafe { PostMessageW(Some(HWND(hwnd as *mut c_void)), WM_BROWSER_NAV, WPARAM(0), LPARAM(b as isize)) }.is_err() {
                drop(unsafe { Box::from_raw(b) });
            }
        };
        match browser::create(a.hwnd, a.panes.list_rect, Box::new(post)) {
            Ok(b) => a.browser = Some(b),
            Err(e) => return explorer_fallback(app, &place.parsing, e),
        }
    }
    let at = a.browser_at.as_ref().is_some_and(|p| ILIsEqual(p.as_ptr(), place.pidl.as_ptr()).as_bool());
    if !at {
        a.browser_at = Some(place.pidl.clone());
        if let Some(Err(e)) = a.browser.as_ref().map(|b| b.browse(&place.pidl)) {
            leave_browser(app);
            return explorer_fallback(app, &place.parsing, e);
        }
    }
    let _ = ShowWindow(a.list, SW_HIDE);
}

unsafe fn explorer_fallback(app: *mut App, parsing: &str, err: Error) {
    if open_in_explorer(parsing).is_err() {
        message((*app).hwnd, &format!("Cannot open {parsing}\n\n{err}"));
    }
}

/// Destroys the embedded view (if any) and shows the list again.
pub(crate) unsafe fn leave_browser(app: *mut App) {
    let a = &mut *app;
    a.browser_at = None;
    if a.browser.take().is_some() {
        let _ = ShowWindow(a.list, SW_SHOW);
    }
}

/// The embedded view's window, when there is one (keyboard focus and accelerators).
pub(crate) unsafe fn browser_hwnd(app: *mut App) -> Option<HWND> {
    (*app).browser.as_ref().map(EmbeddedBrowser::hwnd).filter(|h| !h.is_invalid())
}

/// WM_BROWSER_NAV: the user went somewhere inside the embedded view. Load it like any navigation: a folder our
/// list can show replaces the view, one it cannot stays in the view (already there, so it is not browsed again).
pub(crate) unsafe fn on_browser_nav(app: *mut App, pidl: Box<Pidl>) {
    let a = &mut *app;
    if a.browser.is_none() {
        return;
    }
    let same = |p: &Pidl| ILIsEqual(p.as_ptr(), pidl.as_ptr()).as_bool();
    let known = a.browser_at.as_ref().is_some_and(same);
    a.browser_at = Some((*pidl).clone());
    let shown = matches!(&cur(app).source, Some(Source::Browser(b)) if same(&b.pidl));
    if !known && !shown {
        go(app, Target::Pidl(*pidl), nav::History::Push);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_up_goes_dir_root_folder() {
        let a = Path::new(r"C:\x\a.rar");
        assert_eq!(archive_parent(a, r"docs\sub").as_deref(), Some(r"C:\x\a.rar\docs"));
        assert_eq!(archive_parent(a, "docs").as_deref(), Some(r"C:\x\a.rar"));
        assert_eq!(archive_parent(a, "").as_deref(), Some(r"C:\x"));
    }

    #[test]
    fn search_rows_from_walker_and_paths() {
        let _com = ComInit::mta();
        let dir = std::env::temp_dir().join(format!("rexplorer-source-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("needle-dir")).unwrap();
        std::fs::write(dir.join("needle.txt"), b"12345").unwrap();
        let root = ShellFolder::parse(dir.to_str().unwrap()).unwrap().pidl;
        let mut rows = Vec::new();
        search::walk(&root, "needle", &AtomicBool::new(false), &mut |h| rows.extend(rows_of(h))).unwrap();
        let by_path = hit_for_path(&dir.join("needle.txt")).map(|h| rows_of(vec![h]));
        std::fs::remove_dir_all(&dir).unwrap();
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<(&str, bool)> = rows.iter().map(|r| (r.name.as_str(), r.is_folder)).collect();
        assert_eq!(names, [("needle-dir", true), ("needle.txt", false)]);
        let file = &rows[1];
        // Type and size are localized shell text: only check that the shell filled them.
        assert!(!file.kind.is_empty() && !file.size.is_empty(), "kind={:?} size={:?}", file.kind, file.size);
        assert!(file.folder.ends_with(&format!("rexplorer-source-{}", std::process::id())), "{}", file.folder);
        let by_path = by_path.expect("path resolves to a shell item");
        assert_eq!(by_path[0].name, "needle.txt");
        assert_eq!(by_path[0].size, file.size);
    }
}
