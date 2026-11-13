//! What a tab shows: a shell folder, the inside of a .rar archive, search results, or the embedded Explorer
//! view for folders that exist but cannot be listed. One small set of functions dispatches over them.

use super::*;
use crate::archive_view::ArchiveListing;
use crate::mailbox::Mailbox;
use rexplorer_core::i18n::{tr, trf};
use crate::nav::{Crumb, Listing, Loaded, Show, Target, cur, go, source};
use rexplorer_core::archive::{self, Entry};
use rexplorer_core::browser::{self, EmbeddedBrowser};
use rexplorer_core::query::Query;
use rexplorer_core::search::{self, Hit};
use rexplorer_core::usn::{self, VolumeIndex};
use rexplorer_core::wsearch;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows::Win32::UI::Shell::Common::{ITEMIDLIST, SHCOLSTATE_TYPE_STR};
use windows::Win32::UI::Shell::{
    BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW, ILClone, ILIsEqual, ILRemoveLastID, IShellFolder,
    SHBindToParent, SHBrowseForFolderW, SHGFI_USEFILEATTRIBUTES, SHGetNameFromIDList, SHParseDisplayName, SIGDN_FILESYSPATH,
};
use windows::core::Interface;

/// Search results are in SEARCH_BATCH.
pub(crate) const WM_SEARCH_BATCH: u32 = WM_APP + 4;
pub(crate) static SEARCH_BATCH: Mailbox<Box<SearchBatch>> = Mailbox::new();
/// The embedded browser finished a navigation; where to is in BROWSER_NAV.
pub(crate) const WM_BROWSER_NAV: u32 = WM_APP + 5;
pub(crate) static BROWSER_NAV: Mailbox<Box<Pidl>> = Mailbox::new();
/// A worker failed (archive extraction): the text in WORKER_ERROR is shown as an error box.
pub(crate) const WM_WORKER_ERROR: u32 = WM_APP + 6;
pub(crate) static WORKER_ERROR: Mailbox<String> = Mailbox::new();
/// Most results the volume index returns per search.
const USN_LIMIT: usize = 5000;

/// Volume indexes per drive letter. Each drive has its own slot, so a build (seconds) stalls only searches of that
/// drive, which then share its index instead of building their own.
pub(crate) type UsnCache = Arc<Mutex<HashMap<char, Arc<Mutex<Option<Arc<VolumeIndex>>>>>>>;

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
    /// Packed system icon index of the file itself (see `ShellItem::icon_index`).
    pub icon: i32,
    pub bytes: Option<u64>,
    /// The containing folder's absolute PIDL, to parse it again on the UI thread.
    pub folder_pidl: Pidl,
}

pub(crate) struct SearchListing {
    pub needle: String,
    pub root: Pidl,
    pub root_parsing: String,
    pub root_editing: String,
    pub root_crumbs: Vec<Crumb>,
    pub rows: Vec<SearchRow>,
    pub icons: Vec<i32>,
    /// Sorted column and whether descending; None is arrival order.
    pub sort: Option<(usize, bool)>,
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
    Column { index, title: title.to_string(), width_chars, right_align, kind: SHCOLSTATE_TYPE_STR }
}

fn parent_pidl(p: &Pidl) -> Option<Pidl> {
    if p.is_empty() {
        return None;
    }
    let mut p = p.clone();
    unsafe { ILRemoveLastID(Some(p.as_mut_ptr())) }.as_bool().then_some(p)
}

/// Folder options > Show file extensions: off hides the extension in the Name column of files.
pub(crate) static SHOW_EXT: AtomicBool = AtomicBool::new(true);

/// `a.tar.gz` -> `a.tar`; a leading dot is part of the name (`.gitignore` stays).
fn strip_ext(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    }
}

/// Name column text of `e` under Show file extensions, and what a rename puts after the edited text to make the
/// whole file name (None: the text is the shell's display name, which renames as it is).
pub(crate) fn name_cell(e: &ShellItem) -> (String, Option<String>) {
    thread_local!(static NEVER: std::cell::RefCell<HashMap<String, bool>> = Default::default());
    let shown = e.in_folder_name();
    if named_folder(e) {
        return (shown, None);
    }
    let never = |ext: &str| NEVER.with_borrow_mut(|m| *m.entry(ext.to_lowercase()).or_insert_with(|| rexplorer_core::item::never_shows_ext(ext)));
    file_name_cell(shown, &e.file_name(), SHOW_EXT.load(Ordering::Relaxed), never)
}

/// The shell's display name already follows Explorer's own "hide extensions": the extension comes from the file
/// name and is cut or added only where the display name disagrees with `show`.
fn file_name_cell(shown: String, file: &str, show: bool, never_shown: impl FnOnce(&str) -> bool) -> (String, Option<String>) {
    let ext = &file[strip_ext(file).len()..];
    let has = !ext.is_empty() && shown.ends_with(ext);
    if !show && has {
        (shown[..shown.len() - ext.len()].to_string(), Some(ext.to_string()))
    } else if show && !ext.is_empty() && !has && file.strip_suffix(ext) == Some(shown.as_str()) && !never_shown(ext) {
        (file.to_string(), Some(String::new()))
    } else {
        (shown, None)
    }
}

/// A folder by name: archives (.zip) are folders to the shell but files by name.
fn named_folder(e: &ShellItem) -> bool {
    let (folder, stream) = (windows::Win32::System::SystemServices::SFGAO_FOLDER.0, windows::Win32::System::SystemServices::SFGAO_STREAM.0);
    let attrs = e.attributes(folder | stream);
    attrs & folder != 0 && attrs & stream == 0
}

/// Where a rename's selection of `e`'s Name cell ends: before the extension the cell shows, None for all of it.
pub(crate) fn rename_stem(e: &ShellItem) -> Option<usize> {
    if named_folder(e) {
        return None;
    }
    stem_len(&name_cell(e).0, &e.file_name())
}

/// UTF-16 length of `shown` before the extension of `file` (its file name), when `shown` ends with it: rename
/// selects only that, as Explorer does.
pub(crate) fn stem_len(shown: &str, file: &str) -> Option<usize> {
    let ext = &file[strip_ext(file).len()..];
    let stem = shown.strip_suffix(ext).filter(|s| !ext.is_empty() && !s.is_empty())?;
    Some(stem.encode_utf16().count())
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
            Source::Shell(l) => match (l.items.get(i), l.cells.get(i).and_then(|row| row.get_or_init(|| vec![std::cell::OnceCell::new(); l.columns.len().max(1)].into()).get(sub))) {
                (Some(e), Some(cell)) => cell.get().cloned().unwrap_or_else(|| {
                    let text = if sub == 0 { name_cell(e).0 } else { e.detail(l.columns[sub].index) };
                    // set, not get_or_init: a shell call that pumps messages may ask for this cell again meanwhile
                    let _ = cell.set(text.clone());
                    text
                }),
                _ => String::new(),
            },
            Source::Archive(a) => a.rows.get(i).map(|e| ArchiveListing::detail(e, sub)).unwrap_or_default(),
            Source::Search(s) => s
                .rows
                .get(i)
                .map(|r| match sub {
                    // shortcut: names follow Show file extensions as of the search; F5 runs it again after a change
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
                vec![col(0, tr("col.name"), 40, false), col(1, tr("col.date_modified"), 20, false), col(2, tr("col.type"), 20, false), col(3, tr("col.size"), 12, true)]
            }
            Source::Search(_) => vec![col(0, tr("col.name"), 40, false), col(1, tr("col.folder"), 60, false), col(2, tr("col.type"), 20, false), col(3, tr("col.size"), 12, true)],
            Source::Browser(_) => Vec::new(),
        }
    }

    /// `columns().len()` without building them (asked per tile paint).
    pub fn column_count(&self) -> usize {
        match self {
            Source::Shell(l) => l.columns.len(),
            Source::Archive(_) | Source::Search(_) => 4,
            Source::Browser(_) => 0,
        }
    }

    pub fn title(&self) -> String {
        match self {
            Source::Shell(l) => l.folder.name.clone(),
            Source::Archive(a) => a.listing.title(),
            Source::Search(s) => trf("tabs.search", &[("needle", &s.needle)]),
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
    Ok(Loaded { source: Source::Archive(ArchiveView { listing, rows, icons }), crumbs, icon, unsorted: false, parked: false })
}

/// System image list and small icon index for a name, by extension only (no disk access).
pub(crate) fn sys_icon(name: &str, dir: bool) -> (usize, i32) {
    let mut fi = SHFILEINFOW::default();
    let attr = if dir { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_NORMAL };
    let h = HSTRING::from(name);
    let flags = SHGFI_USEFILEATTRIBUTES | SHGFI_SYSICONINDEX | SHGFI_SMALLICON;
    let himl = unsafe { SHGetFileInfoW(PCWSTR(h.as_ptr()), attr, Some(&mut fi), size_of::<SHFILEINFOW>() as u32, flags) };
    (himl, rexplorer_core::icons::app_fallback(fi.iIcon, || name.to_string()))
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
    #[allow(clippy::type_complexity)] // a one-off tuple of borrows out of the match below
    let (cache, name, dir, shell, own): (&mut Vec<i32>, String, bool, Option<(&ShellItem, &Pidl)>, Option<i32>) = match &mut cur(app).source {
        Some(Source::Shell(l)) => {
            let Some(item) = l.items.get(i) else { return 0 };
            (&mut l.icons, String::new(), false, Some((item, &l.folder.pidl)), None)
        }
        Some(Source::Archive(a)) => {
            let Some(e) = a.rows.get(i) else { return 0 };
            (&mut a.icons, e.name.clone(), e.is_dir, None, None)
        }
        Some(Source::Search(s)) => {
            let Some(r) = s.rows.get(i) else { return 0 };
            (&mut s.icons, r.name.clone(), r.is_folder, None, Some(r.icon))
        }
        _ => return 0,
    };
    let Some(slot) = cache.get_mut(i) else { return 0 };
    if *slot == ICON_UNKNOWN {
        *slot = match shell {
            // shortcut: icon lookup runs on the UI thread, move it to a worker when a slow folder visibly stutters
            Some((item, folder)) => views::theme_icon(app, item).unwrap_or_else(|| item.icon_index(folder).1),
            // A search row has its file's own system icon already; the name only decides on a theme or on the list source.
            None => match own {
                Some(icon) => views::theme_icon_for(app, &name, dir).unwrap_or_else(|| {
                    if !(*app).image_list_set {
                        name_icon(app, &name, dir);
                    }
                    icon
                }),
                None => name_icon(app, &name, dir),
            },
        };
    }
    *slot
}

/// The archive popup: extract everything into a picked folder, or next to the archive.
pub(crate) unsafe fn archive_menu(app: *mut App, pt: POINT) {
    let Some(Source::Archive(a)) = source(app) else { return };
    let archive = a.listing.archive.clone();
    let entries = [(1, tr("menu.source.extract_all_to").to_string()), (2, tr("menu.source.extract_here").to_string())];
    let dest = match ui::popup(app, &entries, &[], pt.x, pt.y) {
        1 => pick_folder((*app).hwnd),
        2 => archive.parent().map(|p| p.to_path_buf()),
        _ => None,
    };
    let Some(dest) = dest else { return };
    run_job((*app).hwnd.0 as isize, archive, ArchiveJob::Extract(dest));
}

/// Work on an archive that may need its password: open a member with its app, or extract everything.
pub(crate) enum ArchiveJob {
    Open(Entry),
    Extract(PathBuf),
}

/// A job needs the archive's password; (archive, whether the one given was wrong, the job) waits in
/// ARCHIVE_PASSWORD. The UI thread asks for it and runs the job again.
pub(crate) const WM_ARCHIVE_PASSWORD: u32 = WM_APP + 12;
pub(crate) static ARCHIVE_PASSWORD: Mailbox<Box<(PathBuf, bool, ArchiveJob)>> = Mailbox::new();

/// Runs `job` on a worker; an error goes back to window `hwnd` as a message box, a missing or wrong password as
/// WM_ARCHIVE_PASSWORD.
pub(crate) fn run_job(hwnd: isize, archive: PathBuf, job: ArchiveJob) {
    std::thread::spawn(move || {
        let _com = ComInit::sta();
        let r = match &job {
            ArchiveJob::Open(e) => archive_view::open(&archive, "").and_then(|l| l.extract_for_open(e)).and_then(|p| instance::open_default_app(&p)),
            ArchiveJob::Extract(dest) => archive::extract_all(&archive, dest),
        };
        let Err(err) = r else { return };
        if let Some(wrong) = archive::password_needed(&err) {
            let _ = ARCHIVE_PASSWORD.post(hwnd, WM_ARCHIVE_PASSWORD, Box::new((archive, wrong, job)));
            return;
        }
        let text = match &job {
            ArchiveJob::Open(e) => trf("err.open", &[("name", &e.name), ("error", &err)]),
            ArchiveJob::Extract(_) => trf("err.extract", &[("name", &archive.display()), ("error", &err)]),
        };
        post_error(hwnd, text);
    });
}

/// WM_ARCHIVE_PASSWORD: asks, and runs the job again with the password typed; Cancel drops it.
#[allow(clippy::boxed_local)] // the Box is the posted message payload
pub(crate) unsafe fn on_archive_password(app: *mut App, ask: Box<(PathBuf, bool, ArchiveJob)>) {
    let (archive, wrong, job) = *ask;
    if let Some(pw) = ask_password(app, &archive, wrong) {
        archive::set_password(&archive, pw);
        run_job((*app).hwnd.0 as isize, archive, job);
    }
}

/// The themed password prompt for `archive` (`wrong`: the last one was refused). The typed text goes straight
/// into the `Password`, which zeroes it when dropped; None when cancelled or left empty.
pub(crate) unsafe fn ask_password(app: *mut App, archive: &Path, wrong: bool) -> Option<archive::Password> {
    let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let form = crate::connect::Form {
        title: tr("dlg.password_title").into(),
        note: Some(trf(if wrong { "dlg.password_wrong" } else { "dlg.password_note" }, &[("name", &name)])),
        labels: &["dlg.password_label"],
        masked: Some(0),
        ok: tr("dlg.password_ok").into(),
    };
    let text = crate::connect::form((*app).hwnd, &(*app).theme, &form)?.into_iter().next()?;
    let pw = archive::Password::from_wide(text);
    (!pw.is_empty()).then_some(pw)
}

/// `op` on this (UI) thread, asking for `archive`'s password while it needs one; None when the user cancels.
pub(crate) unsafe fn with_password<T>(app: *mut App, archive: &Path, op: impl Fn() -> Result<T>) -> Option<Result<T>> {
    loop {
        match op() {
            Err(e) if archive::password_needed(&e).is_some() => {
                let pw = ask_password(app, archive, archive::password_needed(&e) == Some(true))?;
                archive::set_password(archive, pw);
            }
            r => return Some(r),
        }
    }
}

unsafe fn pick_folder(owner: HWND) -> Option<PathBuf> {
    // The API's fixed MAX_PATH display-name buffer; the path itself comes from SHGetNameFromIDList below.
    let mut name = [0u16; 260];
    let title = HSTRING::from(tr("dlg.extract_title"));
    let bi = BROWSEINFOW {
        hwndOwner: owner,
        pszDisplayName: PWSTR(name.as_mut_ptr()),
        lpszTitle: PCWSTR(title.as_ptr()),
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        ..Default::default()
    };
    let p = SHBrowseForFolderW(&bi);
    if p.is_null() {
        return None;
    }
    let p = Pidl::from_raw(p);
    // Shell-allocated, so a path over 260 characters is not cut (SHGetPathFromIDListW's buffer is MAX_PATH).
    let path = SHGetNameFromIDList(p.as_ptr(), SIGDN_FILESYSPATH).ok()?;
    Some(PathBuf::from(rexplorer_core::com::take_cotaskmem_string(path)))
}

fn post_error(hwnd: isize, text: String) {
    let _ = WORKER_ERROR.post(hwnd, WM_WORKER_ERROR, text);
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
            run_job(hwnd.0 as isize, archive, ArchiveJob::Open(e));
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

/// The shell menu for search rows: rows of one folder get that folder (parsed again) as their menu's parent, rows of
/// several folders one menu over their absolute PIDLs.
pub(crate) unsafe fn search_menu(app: *mut App, rows: &[usize]) -> Option<ContextMenu> {
    let Some(Source::Search(s)) = source(app) else { return None };
    let picked: Vec<&SearchRow> = rows.iter().filter_map(|&i| s.rows.get(i)).collect();
    let first = picked.first()?;
    if picked.iter().any(|r| r.folder != first.folder) {
        let items: Vec<(&Pidl, &ShellItem)> = picked.iter().map(|r| (&r.folder_pidl, &r.item)).collect();
        return ContextMenu::for_scattered(&items, (*app).hwnd).ok();
    }
    let folder = ShellFolder::parse(&first.folder).ok()?;
    let items: Vec<&ShellItem> = picked.iter().map(|r| &r.item).collect();
    ContextMenu::for_items(&folder, &items, (*app).hwnd).ok()
}

// ---- search --------------------------------------------------------------------------------------

/// A re-run of search results (F5, the reload after an undo) keeps the order the user picked; a new search starts in
/// arrival order.
fn kept_sort(push: bool, old: Option<&Source>) -> Option<(usize, bool)> {
    match old {
        Some(Source::Search(s)) if !push => s.sort,
        _ => None,
    }
}

/// Starts searching tab `t`'s folder (or the folder its search covers) for `needle` on a worker thread.
/// `push` records the previous place in the history; a re-run (F5) does not.
#[allow(clippy::needless_borrow)] // the explicit borrow of a raw-pointer field silences rustc's dangerous_implicit_autorefs
pub(crate) unsafe fn start_search(app: *mut App, t: usize, needle: &str, push: bool) {
    let needle = needle.trim().to_string();
    // Search results this replaces are kept with where the list was (Tab::leave).
    if push && t == (*app).active && !needle.is_empty() {
        nav::save_view_state(app);
    }
    let Some(tab) = (&mut (*app).tabs).get_mut(t) else { return };
    let query = Query::parse(&needle);
    if query.is_empty() {
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
    // A load still in flight for this tab no longer lands here, nor does a change reload already armed, nor (for a
    // background tab) the watch on the folder it searches.
    tab.pending = 0;
    tab.loading = None;
    tab.dirty = false;
    tab.watch = None;
    let old = tab.source.take();
    let sort = kept_sort(push, old.as_ref());
    if let (true, Some(o)) = (push, old) {
        let left = tab.leave(o);
        tab.back.push(left);
        tab.forward.clear();
    } // else a previous search stops as it drops
    let cancel = Arc::new(AtomicBool::new(false));
    tab.crumbs = root_crumbs.clone();
    tab.crumbs.push(Crumb { name: trf("tabs.search", &[("needle", &needle)]), parsing: String::new(), pidl: None });
    tab.name = root_parsing.clone();
    let drive = root_parsing.chars().next().filter(|_| usn::is_elevated() && root_parsing.len() == 3 && root_parsing.ends_with(":\\"));
    let root_path = root_parsing.clone();
    let s = SearchListing {
        needle: needle.clone(),
        root: root.clone(),
        root_parsing,
        root_editing,
        root_crumbs,
        rows: Vec::new(),
        icons: Vec::new(),
        sort,
        generation,
        running: true,
        cancel: cancel.clone(),
    };
    tabs::set_title((*app).strip, t, &trf("tabs.search", &[("needle", &needle)]));
    tab.source = Some(Source::Search(s));
    if t == (*app).active {
        nav::show(app, Show::Fresh(Vec::new()));
        SetTimer(Some((*app).hwnd), TIMER_SKELETON, 120, None);
    }
    nav::session_changed(app);

    let hwnd = (*app).hwnd.0 as isize;
    let usn_cache = (*app).usn.clone();
    let scope = search::Scope { system: (*app).profile.search_system, archives: (*app).profile.search_archives };
    std::thread::spawn(move || {
        let _com = ComInit::mta();
        let post = |rows: Vec<SearchRow>, done: bool| {
            let _ = SEARCH_BATCH.post(hwnd, WM_SEARCH_BATCH, Box::new(SearchBatch { generation, rows, done }));
        };
        // The walk has no result limit of its own; stop it where the USN path stops.
        let mut n = 0usize;
        let mut sink = |hits: Vec<Hit>| {
            n += hits.len();
            if n >= USN_LIMIT {
                cancel.store(true, Ordering::Relaxed);
            }
            post(rows_of(hits), false)
        };
        let indexed = drive.is_some_and(|d| usn_search(&usn_cache, d, &query, &cancel, &mut sink).is_ok());
        if !indexed {
            // When the Windows Search index covers the folder, it answers, and only the folders its rules leave out
            // below (AppData in a profile) are walked. Archives are walked into, which the index does not do. Any
            // failure walks the whole folder, as without the index; what the index gave is not listed twice.
            let mut found = HashSet::new();
            let index = (ui::is_fs_path(&root_path) && !scope.archives).then(wsearch::Index::open).and_then(|r| r.ok()).filter(|ix| ix.covers(&root_path));
            let left_out = index.and_then(|ix| {
                ix.query(&root_path, &query, &cancel, &mut |paths| sink(index_hits(paths, &mut found))).and_then(|()| ix.excluded_below(&root_path)).ok()
            });
            let roots = match left_out {
                Some(dirs) => dirs.iter().filter_map(|d| ShellFolder::parse(d).ok()).map(|f| f.pidl).collect(),
                None => vec![root],
            };
            for r in &roots {
                let _ = search::walk(r, &query, scope, &cancel, &mut |hits| sink(new_hits(hits, &found)));
            }
        }
        post(Vec::new(), true);
    });
}

/// Index paths not seen yet, as hits; `found` remembers them so the walk does not list them again.
fn index_hits(paths: Vec<String>, found: &mut HashSet<String>) -> Vec<Hit> {
    paths.into_iter().filter(|p| found.insert(p.to_lowercase())).filter_map(|p| hit_for_path(Path::new(&p))).collect()
}

/// Walk hits the index did not already give.
fn new_hits(hits: Vec<Hit>, found: &HashSet<String>) -> Vec<Hit> {
    if found.is_empty() {
        return hits;
    }
    hits.into_iter().filter(|h| !found.contains(&h.item.parsing_name().to_lowercase())).collect()
}

/// Elevated search of a whole NTFS volume through its MFT index (built once per drive, kept for the session).
fn usn_search(cache: &UsnCache, drive: char, query: &Query, cancel: &AtomicBool, sink: &mut dyn FnMut(Vec<Hit>)) -> Result<()> {
    let slot = cache.lock().unwrap_or_else(|e| e.into_inner()).entry(drive).or_default().clone();
    let index = {
        let mut slot = slot.lock().unwrap_or_else(|e| e.into_inner());
        match &*slot {
            Some(i) => i.clone(),
            None if cancel.load(Ordering::Relaxed) => return Ok(()),
            None => slot.insert(Arc::new(usn::build(drive)?)).clone(),
        }
    };
    // A superseded search must not scan a whole index it no longer needs.
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    let mut batch = Vec::new();
    for path in index.search(query, USN_LIMIT, cancel) {
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
        Some(Hit { item: ShellItem { parent, pidl: Pidl::from_raw(child) }, folder: path.parent()?.display().to_string(), pidl: parent_pidl(&abs)? })
    }
}

/// Worker side: the text the list shows, computed once per hit.
fn rows_of(hits: Vec<Hit>) -> Vec<SearchRow> {
    // Hits of one walker batch share a parent folder: map its Size and Type columns once.
    #[allow(clippy::type_complexity)] // a one-off tuple of borrows out of the match below
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
            let is_folder = h.item.is_folder();
            SearchRow {
                name: name_cell(&h.item).0,
                kind: detail(type_col),
                size: detail(size_col),
                is_folder,
                icon: h.item.icon_index(&h.pidl).1,
                bytes: if is_folder { None } else { h.item.size() },
                folder: h.folder,
                folder_pidl: h.pidl,
                item: h.item,
            }
        })
        .collect()
}

/// WM_SEARCH_BATCH: rows join their tab's results; the active tab's list grows in place.
#[allow(clippy::boxed_local)] // the Box is the posted message payload
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
        if let Some((c, d)) = s.sort {
            sort_rows(s, c, d);
        }
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
        if let Some(Source::Search(s)) = source(app)
            && let Some((c, d)) = s.sort {
                sort_search(app, c, d);
            }
    }
    ui::update_status(app);
}

/// Search rows by column `col` (0 name, 1 folder, 2 type, 3 size): folders first, then the column, then the name, natural order (StrCmpLogicalW, as Explorer).
fn cells_cmp(col: usize, a: (&str, &str, &str, Option<u64>, bool), b: (&str, &str, &str, Option<u64>, bool)) -> std::cmp::Ordering {
    let nat = |x: &str, y: &str| unsafe { windows::Win32::UI::Shell::StrCmpLogicalW(&HSTRING::from(x), &HSTRING::from(y)) }.cmp(&0);
    b.4.cmp(&a.4).then_with(|| match col {
        1 => nat(a.1, b.1),
        2 => nat(a.2, b.2),
        3 => a.3.cmp(&b.3),
        _ => std::cmp::Ordering::Equal,
    })
    .then_with(|| nat(a.0, b.0))
}

pub(crate) fn search_cmp(col: usize, a: &SearchRow, b: &SearchRow) -> std::cmp::Ordering {
    cells_cmp(col, (&a.name, &a.folder, &a.kind, a.bytes, a.is_folder), (&b.name, &b.folder, &b.kind, b.bytes, b.is_folder))
}

/// Stable, so batches arriving later do not flip equal rows; the icon cache goes with the order.
fn sort_rows(s: &mut SearchListing, col: usize, desc: bool) {
    s.rows.sort_by(|a, b| {
        let o = search_cmp(col, a, b);
        if desc { o.reverse() } else { o }
    });
    s.sort = Some((col, desc));
    s.icons.fill(ICON_UNKNOWN);
}

/// Header click on the search results: sorts the active tab's rows and keeps selection and focus on the same items.
pub(crate) unsafe fn sort_search(app: *mut App, col: usize, desc: bool) {
    let list = (*app).list;
    // A rename names its row by position until LVN_ENDLABELEDIT is done, and the edit survives a row count change:
    // a sort now would put the new name on another file.
    // shortcut: rows that arrive during a rename stay unsorted at the end until the next batch or header click.
    if SendMessageW(list, LVM_GETEDITCONTROL, None, None).0 != 0 {
        return;
    }
    let Some(Source::Search(s)) = &mut cur(app).source else { return };
    // The sort moves the rows but not their PIDLs, so a PIDL's address is the row's identity.
    let key = |r: &SearchRow| r.item.pidl.as_ptr() as usize;
    let carry = list::snapshot(list, s.rows.len(), |i| s.rows.get(i).map(key));
    sort_rows(s, col, desc);
    views::reset(app, s.rows.len());
    list::restore(list, &carry, s.rows.iter().map(key));
    let _ = InvalidateRect(Some(list), None, true);
    list::set_sort_arrow(app);
}

fn remove_indices<T>(v: &mut Vec<T>, gone: &[usize]) {
    let mut gone = gone.to_vec();
    gone.sort_unstable_by(|a, b| b.cmp(a));
    gone.dedup();
    let len = v.len();
    for i in gone.into_iter().filter(|&i| i < len) {
        v.remove(i);
    }
}

/// After a delete: drops the `rows` whose file is gone (a cancelled delete keeps them). Only file system rows are
/// stat'ed, so no pass over network paths. A delete made outside rexplorer shows up after F5.
pub(crate) unsafe fn prune_search(app: *mut App, rows: &[usize]) {
    let list = (*app).list;
    let focused = usize::try_from(SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_FOCUSED as isize))).0).ok();
    let Some(Source::Search(s)) = &mut cur(app).source else { return };
    let mut gone: Vec<usize> = rows
        .iter()
        .copied()
        .filter(|&i| {
            s.rows.get(i).is_some_and(|r| {
                let p = r.item.parsing_name();
                ui::is_fs_path(&p) && !Path::new(&p).exists()
            })
        })
        .collect();
    gone.sort_unstable();
    gone.dedup();
    if gone.is_empty() {
        return;
    }
    let next = row_after_pruning(s.rows.len(), &gone, focused);
    remove_indices(&mut s.rows, &gone);
    let len = s.rows.len();
    s.icons = vec![ICON_UNKNOWN; len];
    SendMessageW(list, LVM_SETITEMCOUNT, Some(WPARAM(len)), Some(LPARAM(LVSICF_NOSCROLL as isize)));
    list::set_item_state(list, usize::MAX, LIST_VIEW_ITEM_STATE_FLAGS(0), LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0));
    if let Some(i) = next {
        list::select_after_delete(list, i, len);
    }
    views::reset(app, len);
    ui::update_status(app);
}

/// The row, counted after the `gone` rows (ascending) are removed, that takes the selection: the folder view's rule
/// (`nav::row_taking_place`, from the focused row).
fn row_after_pruning(len: usize, gone: &[usize], focused: Option<usize>) -> Option<usize> {
    crate::nav::row_taking_place(len, gone, focused).map(|i| i - gone.partition_point(|&g| g < i))
}

/// A search row's absolute PIDL.
pub(crate) fn row_pidl(r: &SearchRow) -> Pidl {
    unsafe { Pidl::from_raw(windows::Win32::UI::Shell::ILCombine(Some(r.folder_pidl.as_ptr()), Some(r.item.pidl.as_ptr()))) }
}

/// Search row `i` as an item of its folder parsed again on this thread (the walk's folder objects belong to a worker apartment).
pub(crate) unsafe fn search_item(app: *mut App, i: usize) -> Option<ShellItem> {
    let Some(Source::Search(s)) = source(app) else { return None };
    let r = s.rows.get(i)?;
    let f = ShellFolder::from_pidl(&r.folder_pidl).ok()?;
    Some(ShellItem { parent: f.folder.clone(), pidl: r.item.pidl.clone() })
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

/// As-you-type searches start at this many characters; Enter searches any non-empty text. Below it a showing search
/// stays: going back to the folder would rewrite the box under the user's typing.
const MIN_TYPED: usize = 2;

/// TIMER_SEARCH: the box text settled. The box also changes when a tab shows its own state; that matches the
/// active search (or is empty with none), so only a real edit gets through.
pub(crate) unsafe fn on_search_timer(app: *mut App) {
    let text = window_text((*app).search);
    let text = text.trim();
    let (active, searching) = ((*app).active, matches!(&cur(app).source, Some(Source::Search(_))));
    let same = matches!(&cur(app).source, Some(Source::Search(s)) if s.needle == text);
    if text.is_empty() {
        if searching {
            cancel_search(app);
            match cur(app).back.is_empty() {
                false => nav::go_back(app),
                true => nav::go_up(app),
            }
        }
    } else if !same && text.chars().count() >= MIN_TYPED && matches!(cur(app).source, Some(Source::Shell(_) | Source::Search(_))) {
        start_search(app, active, text, !searching);
    }
}

/// The search box: Enter searches the active tab, Escape stops the search; both return to the list.
pub(crate) unsafe extern "system" fn search_subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, data: usize) -> LRESULT {
    let app = data as *mut App;
    match msg {
        WM_KEYDOWN if wparam.0 == VK_RETURN.0 as usize => {
            let text = window_text(hwnd);
            let _ = SetFocus(Some(ui::view_hwnd(app)));
            // A search's own place in the history is its root folder, already recorded when it started.
            let push = !matches!(cur(app).source, Some(Source::Search(_)));
            start_search(app, (*app).active, &text, push);
            LRESULT(0)
        }
        WM_KEYDOWN if wparam.0 == VK_ESCAPE.0 as usize => {
            let _ = KillTimer(Some((*app).hwnd), crate::TIMER_SEARCH);
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
            let _ = BROWSER_NAV.post(hwnd, WM_BROWSER_NAV, Box::new(p));
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
        message((*app).hwnd, &trf("err.open", &[("name", &parsing), ("error", &err)]));
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
#[allow(clippy::boxed_local)] // the Box is the posted message payload
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
    fn search_rows_sort_folders_first_then_column() {
        let rows = [
            ("a10.txt", "C:\\x", "Text", Some(10), false),
            ("a2.txt", "C:\\y", "Text", Some(30), false),
            ("sub", "C:\\x", "File folder", None, true),
            ("B.txt", "C:\\a", "Document", Some(20), false),
        ];
        let order = |col: usize| {
            let mut idx = [0usize, 1, 2, 3];
            idx.sort_by(|&i, &j| cells_cmp(col, rows[i], rows[j]));
            idx.map(|i| ["a", "b", "c", "d"][i]).join("")
        };
        assert_eq!(order(0), "cbad");
        assert_eq!(order(3), "cadb");
        assert_eq!(order(1), "cdab");
        assert_eq!(order(2), "cdba");
    }

    #[test]
    fn remove_indices_drops_each_once() {
        let mut v = vec!["a", "b", "c", "d"];
        remove_indices(&mut v, &[3, 1, 3, 9]);
        assert_eq!(v, ["a", "c"]);
    }

    #[test]
    fn search_results_select_like_a_folder_after_a_delete() {
        // Rows 1, 2 and 5 of 7 go, the focus on 5: the row after it (6) takes the place, which is row 3 once they are gone.
        let (gone, len) = ([1, 2, 5], 7);
        let next = row_after_pruning(len, &gone, Some(5));
        assert_eq!(next, Some(3));
        let mut rows: Vec<usize> = (0..len).collect();
        remove_indices(&mut rows, &gone);
        assert_eq!(rows[3], 6);
        // Focus elsewhere: from the first deleted row; the last rows gone: the survivor before them.
        assert_eq!(row_after_pruning(len, &gone, Some(0)), Some(1), "row 3, now second");
        assert_eq!(row_after_pruning(4, &[2, 3], Some(3)), Some(1));
        assert_eq!(row_after_pruning(2, &[0, 1], None), None);
    }

    #[test]
    fn name_cell_follows_show_extensions() {
        let cell = |shown: &str, file: &str, show: bool| file_name_cell(shown.to_string(), file, show, |e| e == ".lnk");
        let own = |t: &str, s: Option<&str>| (t.to_string(), s.map(str::to_string));
        // The shell shows extensions.
        assert_eq!(cell("report v1.2.txt", "report v1.2.txt", false), own("report v1.2", Some(".txt")));
        assert_eq!(cell("report v1.2.txt", "report v1.2.txt", true), own("report v1.2.txt", None));
        // The shell hides them: never cut into the name, and put the extension back when asked to show it.
        assert_eq!(cell("report v1.2", "report v1.2.txt", false), own("report v1.2", None));
        assert_eq!(cell("report v1.2", "report v1.2.txt", true), own("report v1.2.txt", Some("")));
        assert_eq!(cell("Chrome", "Chrome.lnk", true), own("Chrome", None));
        assert_eq!(cell("Recycled name", "$R1X2.txt", true), own("Recycled name", None));
        assert_eq!(cell(".gitignore", ".gitignore", false), own(".gitignore", None));
        assert!(rexplorer_core::item::never_shows_ext(".lnk") && !rexplorer_core::item::never_shows_ext(".txt"));
    }

    #[test]
    fn rename_selects_the_stem() {
        assert_eq!(stem_len("a.txt", "a.txt"), Some(1));
        assert_eq!(stem_len("archive.tar.gz", "archive.tar.gz"), Some(11));
        assert_eq!(stem_len("ñandú.txt", "ñandú.txt"), Some(5));
        assert_eq!((stem_len(".gitignore", ".gitignore"), stem_len("noext", "noext")), (None, None));
        // Extensions hidden: the text is all name, its dots included.
        assert_eq!((stem_len("report v1.2", "report v1.2.txt"), stem_len("Chrome", "Chrome.lnk")), (None, None));
        // The shell hides them and Show file extensions puts them back: the cell carries the extension.
        let (shown, _) = file_name_cell("report v1.2".into(), "report v1.2.txt", true, |_| false);
        assert_eq!(stem_len(&shown, "report v1.2.txt"), Some(11));
    }

    /// #34: a folder whose path is over 260 characters lists, and its file renames, through the shell. It lives in
    /// the app's tests because only this exe carries app.manifest's longPathAware; it needs LongPathsEnabled=1.
    /// %TEMP% with long names: the shell answers with them, and %TEMP% can be a short one (RUNNER~1 on CI).
    fn long_temp() -> std::path::PathBuf {
        let long = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        std::path::PathBuf::from(long.to_string_lossy().trim_start_matches(r"\\?\"))
    }

    #[test]
    fn lists_and_renames_in_a_folder_over_260_characters() {
        let _c = ComInit::sta();
        let root = long_temp().join(format!("rexplorer-test-long-{}", std::process::id()));
        let dir = (0..6).fold(root.clone(), |d, i| d.join(format!("{i}-{}", "x".repeat(50))));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let name = dir.to_str().unwrap();
        assert!(name.len() > 260, "{}", name.len());
        let (f, mut items) = rexplorer_core::folder::list(name).unwrap();
        assert_eq!(items.len(), 1);
        let renamed = items[0].rename(HWND::default(), "b.txt", true);
        let moved = dir.join("b.txt").is_file() && !dir.join("a.txt").exists();
        let new_name = items[0].parsing_name();
        // Onto a taken name: refused, both files kept.
        std::fs::write(dir.join("c.txt"), b"c").unwrap();
        let onto_c = items[0].rename(HWND::default(), "c.txt", true);
        let kept = (std::fs::read(dir.join("b.txt")).ok(), std::fs::read(dir.join("c.txt")).ok());
        let bad = items[0].rename(HWND::default(), r"..\escape.txt", true);
        // Trailing dots and spaces go, as in Explorer; what is left must be a usable file name.
        let trimmed = items[0].rename(HWND::default(), "d.txt. .", true).is_ok() && dir.join("d.txt").is_file();
        let refused = [".", "..", "...", " ", "CON", "nul.txt", "com1"].map(|n| items[0].rename(HWND::default(), n, true).is_err());
        let only = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect::<Vec<_>>();
        std::fs::remove_dir_all(&root).unwrap();
        renamed.unwrap();
        assert!(moved);
        assert_eq!(f.parsing_name, name);
        assert_eq!(new_name, dir.join("b.txt").to_str().unwrap(), "the item follows its new name");
        assert!(onto_c.is_err());
        assert_eq!(kept, (Some(b"x".to_vec()), Some(b"c".to_vec())));
        assert!(bad.is_err());
        assert!(trimmed);
        assert_eq!(refused, [true; 7]);
        assert_eq!(only, ["c.txt", "d.txt"]);
    }

    #[test]
    fn search_rows_from_walker_and_paths() {
        let _com = ComInit::mta();
        let dir = std::env::temp_dir().join(format!("rexplorer-source-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("needle-dir")).unwrap();
        std::fs::write(dir.join("needle.txt"), b"12345").unwrap();
        let root = ShellFolder::parse(dir.to_str().unwrap()).unwrap().pidl;
        let mut rows = Vec::new();
        search::walk(&root, &Query::parse("needle"), search::Scope::default(), &AtomicBool::new(false), &mut |h| rows.extend(rows_of(h))).unwrap();
        let by_path = hit_for_path(&dir.join("needle.txt")).map(|h| rows_of(vec![h]));
        std::fs::remove_dir_all(&dir).unwrap();
        // Folders first. The file's name follows the user's "hide extensions for known file types" setting.
        rows.sort_by(|a, b| b.is_folder.cmp(&a.is_folder).then(a.name.cmp(&b.name)));
        let names: Vec<(&str, bool)> = rows.iter().map(|r| (r.name.trim_end_matches(".txt"), r.is_folder)).collect();
        assert_eq!(names, [("needle-dir", true), ("needle", false)]);
        let file = &rows[1];
        // Type and size are localized shell text: only check that the shell filled them.
        assert!(!file.kind.is_empty() && !file.size.is_empty(), "kind={:?} size={:?}", file.kind, file.size);
        assert!(file.folder.ends_with(&format!("rexplorer-source-{}", std::process::id())), "{}", file.folder);
        let by_path = by_path.expect("path resolves to a shell item");
        assert_eq!(by_path[0].name, file.name);
        assert_eq!(by_path[0].size, file.size);
    }

    #[test]
    fn leaving_search_results_keeps_them_for_back() {
        let _com = ComInit::mta();
        let root = ShellFolder::parse(std::env::temp_dir().to_str().unwrap()).unwrap().pidl;
        let listing = |generation: u64| SearchListing {
            needle: "needle".into(),
            root: root.clone(),
            root_parsing: String::new(),
            root_editing: String::new(),
            root_crumbs: Vec::new(),
            rows: Vec::new(),
            icons: Vec::new(),
            sort: Some((1, true)),
            generation,
            running: true,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let mut tab = nav::Tab {
            name: String::new(),
            source: None,
            back: Vec::new(),
            forward: Vec::new(),
            skipped: Vec::new(),
            watch: None,
            dirty: false,
            pending: 0,
            latest: Arc::default(),
            top: 7,
            selected: vec![r"C:\x\needle.txt".into()],
            select_after: None,
            search_after: None,
            crumbs: Vec::new(),
            icon: (0, 0),
            loading: None,
            home_filter: crate::home::Filter::Recent,
            kept: Vec::new(),
            deleted: None,
        };
        let (target, label) = tab.leave(Source::Search(listing(1)));
        assert!(matches!(target, Target::Search(1, _)));
        assert_eq!(label, trf("tabs.search", &[("needle", &"needle")]));
        let k = &tab.kept[0];
        assert!(!k.listing.running && k.listing.cancel.load(Ordering::Relaxed), "a kept search is stopped");
        assert_eq!((k.top, k.selected.as_slice(), k.listing.sort), (7, [r"C:\x\needle.txt".to_string()].as_slice(), Some((1, true))));
        for g in 2..=6 {
            tab.leave(Source::Search(listing(g)));
        }
        let ids: Vec<u64> = tab.kept.iter().map(|k| k.listing.generation).collect();
        assert_eq!(ids, [3, 4, 5, 6], "the oldest go; Back to them reloads their folder");
        // A re-run keeps the picked order, a new search does not.
        let old = Source::Search(listing(7));
        assert_eq!((kept_sort(false, Some(&old)), kept_sort(true, Some(&old)), kept_sort(false, None)), (Some((1, true)), None, None));
    }

    #[test]
    fn index_and_walk_hits_merge_without_duplicates() {
        let _com = ComInit::mta();
        let dir = long_temp().join(format!("rexplorer-merge-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for n in ["needle-a.txt", "needle-b.txt"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let a = dir.join("needle-a.txt").display().to_string();
        let mut found = HashSet::new();
        // The index can name a file twice (case differs); it is one row.
        let from_index = index_hits(vec![a.clone(), a.to_uppercase()], &mut found);
        let root = ShellFolder::parse(dir.to_str().unwrap()).unwrap().pidl;
        let mut walked = Vec::new();
        search::walk(&root, &Query::parse("needle"), search::Scope::default(), &AtomicBool::new(false), &mut |h| walked.extend(new_hits(h, &found))).unwrap();
        let walked: Vec<String> = walked.iter().map(|h| h.item.parsing_name()).collect();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(from_index.len(), 1);
        assert_eq!(walked, [dir.join("needle-b.txt").display().to_string()], "the walk adds only what the index lacked");
    }
}
