//! The command bar under the header, as Explorer has it: New v | Cut, Copy, Paste, Rename, Share, Delete | Sort v, View v |
//! More, and Preview at the right edge. Each command is a shell verb on the selection or the folder background, or
//! a menu of them; the buttons are enabled for what the active tab can do right now.

use super::*;
use crate::nav::{cur, listing};
use rexplorer_core::undo::Op;
use std::collections::HashSet;
use windows::Win32::System::DataExchange::{IsClipboardFormatAvailable, RegisterClipboardFormatW};

pub(crate) unsafe fn create(hwnd: HWND) -> Result<HWND> {
    let bar = toolbar::create(
        hwnd,
        ID_CMDBAR,
        &[
            (0xE710, CMD_NEW, "New folder or file"),
            toolbar::SEPARATOR,
            (0xE8C6, CMD_CUT, "Cut (Ctrl+X)"),
            (0xE8C8, CMD_COPY, "Copy (Ctrl+C)"),
            (0xE77F, CMD_PASTE, "Paste (Ctrl+V)"),
            (0xE8AC, CMD_RENAME, "Rename (F2)"),
            (0xE72D, CMD_SHARE, "Share"),
            (0xE74D, CMD_DELETE, "Delete (Del)"),
            toolbar::SEPARATOR,
            (0xE8CB, CMD_SORT, "Sort"),
            (0xE8A9, CMD_VIEW_MENU, "View and panes"),
            toolbar::SEPARATOR,
            (0xE712, CMD_MORE, "More"),
            toolbar::SPACER,
            (0xE7B3, CMD_TOGGLE_PREVIEW, "Preview pane (Alt+P)"),
        ],
    )?;
    toolbar::set_label(bar, CMD_NEW, "New", true);
    toolbar::set_label(bar, CMD_SORT, "Sort", true);
    toolbar::set_label(bar, CMD_VIEW_MENU, "View", true);
    toolbar::set_label(bar, CMD_TOGGLE_PREVIEW, "Preview", false);
    Ok(bar)
}

/// Files on the clipboard: CF_HDROP (15) or the shell's own "Shell IDList Array".
pub(crate) unsafe fn clipboard_has_files() -> bool {
    let idlist = RegisterClipboardFormatW(w!("Shell IDList Array"));
    IsClipboardFormatAvailable(15).is_ok() || (idlist != 0 && IsClipboardFormatAvailable(idlist).is_ok())
}

/// Whether the active tab is a shell listing that has finished loading (the only place Paste can act).
unsafe fn shell_ready(app: *mut App) -> bool {
    let tab = cur(app);
    tab.loading.is_none() && matches!(tab.source, Some(Source::Shell(_)))
}

/// Paste's enabled state alone, for a clipboard change: skips the selection and attribute reads of `update`.
pub(crate) unsafe fn update_paste(app: *mut App) {
    if (*app).tabs.is_empty() {
        return;
    }
    toolbar::set_enabled((*app).cmdbar, CMD_PASTE, shell_ready(app) && clipboard_has_files());
}

/// Enabled states for the active tab: nothing while it loads; selection verbs in a shell listing (Copy and Delete
/// in search results too, which act on each file's own folder); Paste with files on the clipboard; New and Sort
/// not on the Home page.
pub(crate) unsafe fn update(app: *mut App) {
    let a = &*app;
    if a.tabs.is_empty() {
        return;
    }
    let tab = cur(app);
    let search = tab.loading.is_none() && matches!(tab.source, Some(Source::Search(_)));
    let shell = shell_ready(app);
    let home = IsWindowVisible(a.home).as_bool();
    let sel = SendMessageW(a.list, LVM_GETSELECTEDCOUNT, None, None).0 as usize;
    let bar = a.cmdbar;
    for (cmd, on) in [
        (CMD_NEW, shell && !home),
        (CMD_CUT, shell && sel > 0),
        (CMD_COPY, (shell || search) && sel > 0),
        (CMD_PASTE, shell && clipboard_has_files()),
        (CMD_RENAME, (shell || search) && sel == 1),
        (CMD_SHARE, shell && sel > 0),
        (CMD_DELETE, (shell || search) && sel > 0 && list::can_delete(app, &list::selected_indices(a.list))),
        (CMD_SORT, shell && !home),
    ] {
        toolbar::set_enabled(bar, cmd, on);
    }
    toolbar::set_checked(bar, CMD_TOGGLE_PREVIEW, a.session.show_preview);
}

/// Share: the Windows share sheet for the selected files. Folders cannot be shared (as in Explorer): beeps.
pub(crate) unsafe fn share(app: *mut App) {
    let rows = list::selected_indices((*app).list);
    let paths: Vec<String> = listing(app)
        .map(|l| rows.iter().filter_map(|&i| l.items.get(i)).map(|i| i.parsing_name()).filter(|p| std::path::Path::new(p).is_file()).collect())
        .unwrap_or_default();
    if paths.is_empty() || share_files((*app).hwnd, paths).is_err() {
        let _ = MessageBeep(MB_OK);
    }
}

/// Opens the share sheet over `hwnd` for `paths`. The shell's own "Windows.ModernShare" verb fails from a desktop
/// app (0x80070578, invalid window), so this goes through IDataTransferManagerInterop as Microsoft documents.
unsafe fn share_files(hwnd: HWND, paths: Vec<String>) -> windows::core::Result<()> {
    use windows::ApplicationModel::DataTransfer::{DataRequestedEventArgs, DataTransferManager};
    use windows_collections::IIterable;
    use windows::Foundation::TypedEventHandler;
    use windows::Storage::{IStorageItem, StorageFile};
    use windows::Win32::UI::Shell::IDataTransferManagerInterop;
    use windows::core::{HSTRING, Interface, Ref};
    // The handler registered by the previous Share, removed so one request does not fill the sheet twice.
    static TOKEN: std::sync::Mutex<Option<i64>> = std::sync::Mutex::new(None);
    let interop = windows::core::factory::<DataTransferManager, IDataTransferManagerInterop>()?;
    let manager: DataTransferManager = unsafe { interop.GetForWindow(hwnd)? };
    if let Some(t) = TOKEN.lock().unwrap().take() {
        let _ = manager.RemoveDataRequested(t);
    }
    let handler = TypedEventHandler::new(move |_, args: Ref<DataRequestedEventArgs>| {
        let data = args.ok()?.Request()?.Data()?;
        let title = match &paths[..] {
            [one] => std::path::Path::new(one).file_name().map_or(one.clone(), |n| n.to_string_lossy().into_owned()),
            many => format!("{} items", many.len()),
        };
        data.Properties()?.SetTitle(&HSTRING::from(title))?;
        let mut items = Vec::new();
        for p in &paths {
            items.push(Some(StorageFile::GetFileFromPathAsync(&HSTRING::from(p.as_str()))?.join()?.cast::<IStorageItem>()?));
        }
        data.SetStorageItemsReadOnly(&IIterable::<IStorageItem>::from(items))
    });
    *TOKEN.lock().unwrap() = Some(manager.DataRequested(&handler)?);
    unsafe { interop.ShowShareUIForWindow(hwnd) }
}

/// Below the button that sends `cmd`.
unsafe fn anchor(app: *mut App, cmd: u16) -> (i32, i32) {
    let r = toolbar::button_rect((*app).cmdbar, cmd).unwrap_or_default();
    (r.left, r.bottom)
}

/// New v: "Folder", then the shell's own New submenu (its file types). The new item is renamed in place once the
/// change watch's refresh shows it.
pub(crate) unsafe fn new_menu(app: *mut App) {
    let hwnd = (*app).hwnd;
    let Some(menu) = listing(app).and_then(|l| ContextMenu::for_background(&l.folder, hwnd).ok()) else { return };
    // shortcut: an empty list when the New submenu cannot be found; "Folder" still works through the verb
    let types = menu.new_items().unwrap_or_default();
    let mut entries = vec![(1, "Folder".to_string())];
    if !types.is_empty() {
        entries.push((0, String::new()));
    }
    entries.extend(types.iter().enumerate().map(|(i, (_, text))| (i + 2, text.clone())));
    let (x, y) = anchor(app, CMD_NEW);
    let picked = ui::popup(app, &entries, &[], x, y);
    if picked == 0 {
        return;
    }
    add_item(app, &menu, |m| match picked {
        1 => m.invoke_verb(hwnd, "NewFolder", false),
        n => m.invoke_command(hwnd, types[n - 2].0),
    });
}

/// Ctrl+Shift+N: a new folder without the menu.
pub(crate) unsafe fn new_folder(app: *mut App) {
    let hwnd = (*app).hwnd;
    let Some(menu) = listing(app).and_then(|l| ContextMenu::for_background(&l.folder, hwnd).ok()) else { return };
    add_item(app, &menu, |m| m.invoke_verb(hwnd, "NewFolder", false));
}

/// Runs `invoke` and arms the rename of the item it adds.
unsafe fn add_item(app: *mut App, menu: &ContextMenu, invoke: impl FnOnce(&ContextMenu) -> Result<()>) {
    let hwnd = (*app).hwnd;
    // The popup pumps messages: a refresh may have replaced the listing meanwhile.
    let Some(l) = listing(app) else { return };
    let before: HashSet<String> = l.items.iter().map(|i| i.parsing_name()).collect();
    let folder = l.folder.parsing_name.clone();
    match invoke(menu) {
        Ok(()) => {
            (*app).new_item = Some((folder, before));
            // The change watch may reload first; whichever reload starts cancels the other.
            SetTimer(Some(hwnd), TIMER_REFRESH, 300, None);
        }
        Err(_) => {
            let _ = MessageBeep(MB_OK);
        }
    }
}

/// After a load: the item New made has appeared, so select it and edit its name.
pub(crate) unsafe fn rename_new_item(app: *mut App) {
    let Some((folder, before)) = &(*app).new_item else { return };
    let Some(l) = listing(app) else { return };
    if !l.folder.parsing_name.eq_ignore_ascii_case(folder) {
        return;
    }
    let mut added = l.items.iter().enumerate().filter(|(_, it)| !before.contains(&it.parsing_name()));
    let Some((i, _)) = added.next() else { return };
    // Two new items: one is somebody else's (a download landing now), so undo must not claim either.
    if added.next().is_none() {
        (*app).undo.record(Op::Created(vec![l.items[i].parsing_name()]));
    }
    (*app).new_item = None;
    let list = (*app).list;
    let both = LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0);
    list::set_item_state(list, usize::MAX, LIST_VIEW_ITEM_STATE_FLAGS(0), both);
    list::set_item_state(list, i, both, both);
    SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(i)), Some(LPARAM(0)));
    list::begin_rename(app);
}

const SORT_ASC: usize = 0x100;
const SORT_DESC: usize = 0x101;
const GROUP_NONE: usize = 0x103;
/// Group by the column at position `id - GROUP_BASE`.
const GROUP_BASE: usize = 0x200;

/// Sort entries: the listing's columns (id = position + 1, the active one checked), then the direction, then
/// the grouping (none, or by a column; the sort stays).
/// (entries, checked); empty off a shell listing.
pub(crate) unsafe fn sort_entries(app: *mut App) -> (Vec<(usize, String)>, Vec<usize>) {
    let Some(l) = listing(app) else { return (Vec::new(), Vec::new()) };
    let mut entries: Vec<(usize, String)> = l.columns.iter().enumerate().map(|(i, c)| (i + 1, c.title.clone())).collect();
    entries.extend([(0, String::new()), (SORT_ASC, "Ascending".to_string()), (SORT_DESC, "Descending".to_string())]);
    entries.extend([(0, String::new()), (GROUP_NONE, "No groups".to_string())]);
    entries.extend(l.columns.iter().enumerate().map(|(p, c)| (GROUP_BASE + p, format!("Group by {}", c.title))));
    let active = l.columns.iter().position(|c| c.index == l.sort_col).map(|i| i + 1);
    let checked = active
        .into_iter()
        .chain([
            if l.sort_desc { SORT_DESC } else { SORT_ASC },
            l.group.and_then(|g| l.columns.iter().position(|c| c.index == g)).map_or(GROUP_NONE, |p| GROUP_BASE + p),
        ])
        .collect();
    (entries, checked)
}

/// Header right-click: the columns the folder offers, the shown ones checked; a pick shows or hides that column.
pub(crate) unsafe fn column_menu(app: *mut App, x: i32, y: i32) {
    if cur(app).loading.is_some() {
        return;
    }
    let Some(l) = nav::listing(app) else { return };
    let all = l.folder.menu_columns();
    let entries: Vec<(usize, String)> = all.iter().filter(|c| !c.title.is_empty()).map(|c| (1 + c.index as usize, c.title.clone())).collect();
    let checked: Vec<usize> = l.columns.iter().map(|c| 1 + c.index as usize).collect();
    let folder = l.folder.parsing_name.clone();
    let picked = ui::popup(app, &entries, &checked, x, y);
    // The popup pumps messages: a navigation that finished meanwhile shows another folder, whose columns differ.
    if picked != 0 && nav::listing(app).is_some_and(|l| l.folder.parsing_name == folder) {
        toggle_column(app, &all, picked as u32 - 1);
    }
}

/// Shows or hides shell column `index` on the active listing, keeping the widths and order the header has now.
unsafe fn toggle_column(app: *mut App, all: &[Column], index: u32) {
    let Some(l) = nav::listing(app) else { return };
    let mut base = nav::header_layout(app);
    if base.is_empty() {
        base = l.columns.iter().map(|c| (c.index, 0)).collect();
    }
    let new = nav::toggled(&base, index);
    let available: Vec<u32> = all.iter().map(|c| c.index).collect();
    let cols: Vec<Column> = nav::chosen_columns(&available, &new).into_iter().filter_map(|i| all.iter().find(|c| c.index == i).cloned()).collect();
    let mut resort = false;
    if let Some(source::Source::Shell(l)) = &mut cur(app).source {
        l.columns = cols;
        l.layout = new;
        // Rows hold cells sized for the old columns.
        l.cells.fill(Default::default());
        let shown = |i: u32| l.columns.iter().any(|c| c.index == i);
        if !shown(l.sort_col) {
            (l.sort_col, l.sort_desc) = (0, false);
            resort = true;
        }
        if l.group.is_some_and(|g| !shown(g)) {
            l.group = None;
            resort = true;
        }
    }
    if let Some(s) = nav::source(app) {
        nav::set_columns(app, &s.columns());
    }
    nav::apply_layout(app);
    if resort {
        if let Some(l) = nav::listing(app) {
            list::sort_by(app, l.sort_col, l.sort_desc);
        }
    }
    let _ = InvalidateRect(Some((*app).list), None, true);
    nav::remember_folder(app);
}

/// Applies a pick from `sort_entries`.
pub(crate) unsafe fn pick_sort(app: *mut App, id: usize) {
    let Some(l) = listing(app) else { return };
    let (cols, col, desc): (Vec<(u32, bool)>, u32, bool) = (l.columns.iter().map(|c| (c.index, c.descending_first())).collect(), l.sort_col, l.sort_desc);
    match id {
        0 => {}
        SORT_ASC => list::sort_by(app, col, false),
        SORT_DESC => list::sort_by(app, col, true),
        id if id == GROUP_NONE || id >= GROUP_BASE => {
            let Some(group) = (if id == GROUP_NONE { Some(None) } else { cols.get(id - GROUP_BASE).map(|c| Some(c.0)) }) else { return };
            if let Some(source::Source::Shell(l)) = &mut cur(app).source {
                l.group = group;
            }
            list::sort_by(app, col, desc);
        }
        n => {
            if let Some(&(c, first)) = cols.get(n - 1) {
                list::sort_by(app, c, if c == col { desc } else { first });
            }
        }
    }
}

/// Sort v on the command bar.
pub(crate) unsafe fn sort_menu(app: *mut App) {
    let (entries, checked) = sort_entries(app);
    if entries.is_empty() {
        return;
    }
    let (x, y) = anchor(app, CMD_SORT);
    pick_sort(app, ui::popup(app, &entries, &checked, x, y));
}

/// View entries: the three list views, then the panes, each checked when it is on. (entries, checked).
pub(crate) unsafe fn view_entries(app: *mut App) -> (Vec<(usize, String)>, Vec<usize>) {
    let s = &(*app).session;
    let entries = vec![
        (CMD_VIEW_XL as usize, "Extra large icons".to_string()),
        (CMD_VIEW1 as usize + 1, "Large icons\tCtrl+Shift+2".to_string()),
        (CMD_VIEW_MEDIUM as usize, "Medium icons".to_string()),
        (CMD_VIEW_SMALL as usize, "Small icons\tCtrl+Shift+4".to_string()),
        (CMD_VIEW1 as usize, "Details\tCtrl+Shift+1".to_string()),
        (CMD_VIEW1 as usize + 2, "Thumbnails\tCtrl+Shift+3".to_string()),
        (0, String::new()),
        (CMD_TOGGLE_SIDEBAR as usize, "Sidebar\tCtrl+B".to_string()),
        (CMD_FOLDER_TREE as usize, "Folder tree".to_string()),
        (CMD_TOGGLE_PREVIEW as usize, "Preview pane\tAlt+P".to_string()),
    ];
    let mut checked = vec![(*app).views.view.command() as usize];
    for (on, cmd) in [(s.show_sidebar && !s.show_tree, CMD_TOGGLE_SIDEBAR), (s.show_tree, CMD_FOLDER_TREE), (s.show_preview, CMD_TOGGLE_PREVIEW)] {
        if on {
            checked.push(cmd as usize);
        }
    }
    (entries, checked)
}

/// Applies a pick from `view_entries`.
pub(crate) unsafe fn pick_view(app: *mut App, id: usize) {
    match id {
        0 => {}
        // Sidebar while the tree shows: the sidebar comes back in its place (Ctrl+B would hide both).
        id if id == CMD_TOGGLE_SIDEBAR as usize && (*app).session.show_tree => panes::toggle_tree(app),
        id => on_command(app, id as u16),
    }
}

/// View v on the command bar.
pub(crate) unsafe fn view_menu(app: *mut App) {
    let (entries, checked) = view_entries(app);
    let (x, y) = anchor(app, CMD_VIEW_MENU);
    pick_view(app, ui::popup(app, &entries, &checked, x, y));
}
