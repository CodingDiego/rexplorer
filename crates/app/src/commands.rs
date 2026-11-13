//! The command bar under the header, as Explorer has it: New v | Cut, Copy, Paste, Rename, Share, Delete | Sort v, View v |
//! More, and Preview at the right edge. Each command is a shell verb on the selection or the folder background, or
//! a menu of them; the buttons are enabled for what the active tab can do right now.

use super::*;
use crate::nav::{cur, listing};
use crate::ui::tip;
use rexplorer_core::i18n::{tr, trf};
use rexplorer_core::undo::Op;
use std::collections::HashSet;
use windows::Win32::System::DataExchange::{IsClipboardFormatAvailable, RegisterClipboardFormatW};

pub(crate) unsafe fn create(hwnd: HWND) -> Result<HWND> {
    let (cut, copy, paste, rename, delete, preview) =
        (tip("tb.cut", "Ctrl+X"), tip("tb.copy", "Ctrl+C"), tip("tb.paste", "Ctrl+V"), tip("tb.rename", "F2"), tip("tb.delete", "Del"), tip("tb.preview", "Alt+P"));
    let bar = toolbar::create(
        hwnd,
        ID_CMDBAR,
        &[
            (0xE710, CMD_NEW, tr("tb.new")),
            toolbar::SEPARATOR,
            (0xE8C6, CMD_CUT, &cut),
            (0xE8C8, CMD_COPY, &copy),
            (0xE77F, CMD_PASTE, &paste),
            (0xE8AC, CMD_RENAME, &rename),
            (0xE72D, CMD_SHARE, tr("tb.share")),
            (0xE74D, CMD_DELETE, &delete),
            toolbar::SEPARATOR,
            (0xE8CB, CMD_SORT, tr("tb.sort")),
            (0xE8A9, CMD_VIEW_MENU, tr("tb.view")),
            toolbar::SEPARATOR,
            (0xE712, CMD_MORE, tr("tb.more")),
            toolbar::SPACER,
            (0xE7B3, CMD_TOGGLE_PREVIEW, &preview),
        ],
    )?;
    let _ = SetWindowTextW(bar, &HSTRING::from(tr("tb.cmdbar_name"))); // the accessible name
    toolbar::set_label(bar, CMD_NEW, tr("tb.new_label"), true);
    toolbar::set_label(bar, CMD_SORT, tr("tb.sort_label"), true);
    toolbar::set_label(bar, CMD_VIEW_MENU, tr("tb.view_label"), true);
    toolbar::set_label(bar, CMD_TOGGLE_PREVIEW, tr("tb.preview_label"), false);
    toolbar::set_menu(bar, CMD_MORE);
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
/// in search results too, which act on each file's own folder); Paste with files on the clipboard; New not on the
/// Home page (Sort there orders its table).
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
        (CMD_SORT, shell),
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
    if let Some(t) = TOKEN.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = manager.RemoveDataRequested(t);
    }
    let handler = TypedEventHandler::new(move |_, args: Ref<DataRequestedEventArgs>| {
        let data = args.ok()?.Request()?.Data()?;
        let title = match &paths[..] {
            [one] => std::path::Path::new(one).file_name().map_or(one.clone(), |n| n.to_string_lossy().into_owned()),
            many => rexplorer_core::i18n::trn("status.items", many.len() as u64),
        };
        data.Properties()?.SetTitle(&HSTRING::from(title))?;
        let mut items = Vec::new();
        for p in &paths {
            items.push(Some(StorageFile::GetFileFromPathAsync(&HSTRING::from(p.as_str()))?.join()?.cast::<IStorageItem>()?));
        }
        data.SetStorageItemsReadOnly(&IIterable::<IStorageItem>::from(items))
    });
    *TOKEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(manager.DataRequested(&handler)?);
    unsafe { interop.ShowShareUIForWindow(hwnd) }
}

/// Below the button that sends `cmd`.
unsafe fn anchor(app: *mut App, cmd: u16) -> (i32, i32) {
    let r = toolbar::button_rect((*app).cmdbar, cmd).unwrap_or_default();
    (crate::rtl::menu_x(&r), r.bottom)
}

/// New v: "Folder", then the shell's own New submenu (its file types). The new item is renamed in place once the
/// change watch's refresh shows it.
pub(crate) unsafe fn new_menu(app: *mut App) {
    let hwnd = (*app).hwnd;
    let Some(menu) = listing(app).and_then(|l| ContextMenu::for_background(&l.folder, hwnd).ok()) else { return };
    // shortcut: an empty list when the New submenu cannot be found; "Folder" still works through the verb
    let types = menu.new_items().unwrap_or_default();
    let mut entries = vec![(1, tr("menu.new.folder").to_string())];
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
    // The popup pumps messages: a refresh may have replaced the listing meanwhile.
    let Some(before) = names_now(app) else { return };
    match invoke(menu) {
        // The change watch may reload first; whichever reload starts cancels the other.
        Ok(()) => new_item_made(app, before),
        Err(_) => {
            let _ = MessageBeep(MB_OK);
        }
    }
}

/// The current folder and what is in it now (lowercased parsing names), taken before a context menu's New runs: the
/// listing's items (on the Desktop root these include This PC and the Public Desktop's files, which no read_dir sees),
/// plus the disk for a folder on a fixed drive, since a change notification may reload the listing with the new item
/// while the menu is still open. Not read on other drives: this runs on the UI thread before every background menu.
pub(crate) unsafe fn names_now(app: *mut App) -> Option<(String, HashSet<String>)> {
    let l = listing(app)?;
    let folder = l.folder.parsing_name.clone();
    let disk = ui::is_fixed_drive(&folder).then(|| std::fs::read_dir(&folder).ok()).flatten();
    let on_disk = disk.into_iter().flatten().filter_map(|e| e.ok()).map(|e| e.path().to_string_lossy().into_owned());
    Some((folder, names_before(l.items.iter().map(|i| i.parsing_name()), on_disk)))
}

/// The lowercased union of what the listing shows and what the disk holds.
fn names_before(listed: impl Iterator<Item = String>, on_disk: impl Iterator<Item = String>) -> HashSet<String> {
    listed.chain(on_disk).map(|n| n.to_lowercase()).collect()
}

/// The context menu's New has made an item in `before`'s folder (from `names_now`): rename and record it as the
/// toolbar's New does, at the next load.
pub(crate) unsafe fn new_item_made(app: *mut App, before: (String, HashSet<String>)) {
    (*app).new_item = Some(before);
    SetTimer(Some((*app).hwnd), TIMER_REFRESH, 300, None);
}

/// After a load: the item New made has appeared, so select it and edit its name. New > Shortcut's stub is only
/// selected: its wizard, open now, names it.
pub(crate) unsafe fn rename_new_item(app: *mut App) {
    let Some((folder, before)) = &(*app).new_item else { return };
    let Some(l) = listing(app) else { return };
    if !l.folder.parsing_name.eq_ignore_ascii_case(folder) {
        return;
    }
    let mut added = l.items.iter().enumerate().filter(|(_, it)| !before.contains(&it.parsing_name().to_lowercase()));
    // A New that made nothing must not stay armed: a later download would be renamed and claimed for undo.
    let Some((i, _)) = added.next() else {
        (*app).new_item = None;
        return;
    };
    let path = l.items[i].parsing_name();
    let shortcut = path.to_lowercase().ends_with(".lnk");
    // Two new items: one is somebody else's (a download landing now), so undo must not claim either.
    if added.next().is_none() && !shortcut {
        (*app).undo.record(Op::created(vec![path]));
    }
    (*app).new_item = None;
    let list = (*app).list;
    let both = LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0);
    list::set_item_state(list, usize::MAX, LIST_VIEW_ITEM_STATE_FLAGS(0), both);
    list::set_item_state(list, i, both, both);
    SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(i)), Some(LPARAM(0)));
    if !shortcut {
        list::begin_rename(app);
    }
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
    // Home: its table's columns and the two directions; no groups.
    if IsWindowVisible((*app).home).as_bool() {
        let (cols, sort) = home::sort((*app).home);
        let mut entries: Vec<(usize, String)> = cols.into_iter().enumerate().map(|(i, t)| (i + 1, t)).collect();
        entries.extend([(0, String::new()), (SORT_ASC, tr("menu.sort.ascending").to_string()), (SORT_DESC, tr("menu.sort.descending").to_string())]);
        let checked = sort.map_or_else(Vec::new, |(c, desc)| vec![c + 1, if desc { SORT_DESC } else { SORT_ASC }]);
        return (entries, checked);
    }
    let Some(l) = listing(app) else { return (Vec::new(), Vec::new()) };
    let mut entries: Vec<(usize, String)> = l.columns.iter().enumerate().map(|(i, c)| (i + 1, c.title.clone())).collect();
    entries.extend([(0, String::new()), (SORT_ASC, tr("menu.sort.ascending").to_string()), (SORT_DESC, tr("menu.sort.descending").to_string())]);
    entries.extend([(0, String::new()), (GROUP_NONE, tr("menu.sort.no_groups").to_string())]);
    entries.extend(l.columns.iter().enumerate().map(|(p, c)| (GROUP_BASE + p, trf("menu.sort.group_by", &[("column", &c.title)]))));
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
    if resort
        && let Some(l) = nav::listing(app) {
            list::sort_by(app, l.sort_col, l.sort_desc);
        }
    let _ = InvalidateRect(Some((*app).list), None, true);
    nav::remember_folder(app);
}

/// Home's sort after Sort menu pick `id` over its `ncols` columns; a direction without a column sorts by name.
fn home_pick(id: usize, ncols: usize, sort: Option<(usize, bool)>) -> Option<(usize, bool)> {
    match id {
        SORT_ASC | SORT_DESC => Some((sort.map_or(0, |s| s.0), id == SORT_DESC)),
        n if (1..=ncols).contains(&n) => Some(home::next_sort(sort, n - 1, false)),
        _ => None,
    }
}

/// Applies a pick from `sort_entries`.
pub(crate) unsafe fn pick_sort(app: *mut App, id: usize) {
    let h = (*app).home;
    if IsWindowVisible(h).as_bool() {
        let (cols, sort) = home::sort(h);
        if let Some(picked) = home_pick(id, cols.len(), sort) {
            home::set_sort(h, Some(picked));
        }
        return;
    }
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

/// The View menu's list views, in Explorer's order, then Thumbnails (ours). The chords are the default ACCELS.
/// (command, name key, key hint); the hint is code so it survives translation.
const VIEW_MENU: [(u16, &str, &str); 9] = [
    (CMD_VIEW_XL, "menu.view.extra_large", "Ctrl+Shift+1"),
    (CMD_VIEW1 + 1, "menu.view.large", "Ctrl+Shift+2"),
    (CMD_VIEW_MEDIUM, "menu.view.medium", "Ctrl+Shift+3"),
    (CMD_VIEW_SMALL, "menu.view.small", "Ctrl+Shift+4"),
    (CMD_VIEW_LIST, "menu.view.list", "Ctrl+Shift+5"),
    (CMD_VIEW1, "menu.view.details", "Ctrl+Shift+6"),
    (CMD_VIEW_TILES, "menu.view.tiles", "Ctrl+Shift+7"),
    (CMD_VIEW_CONTENT, "menu.view.content", "Ctrl+Shift+8"),
    (CMD_VIEW1 + 2, "menu.view.thumbnails", ""),
];

/// View entries: the list views, then the panes, each checked when it is on. (entries, checked).
pub(crate) unsafe fn view_entries(app: *mut App) -> (Vec<(usize, String)>, Vec<usize>) {
    let s = &(*app).session;
    let mut entries: Vec<(usize, String)> = VIEW_MENU.iter().map(|&(id, key, hint)| (id as usize, if hint.is_empty() { tr(key).to_string() } else { ui::hinted(key, hint) })).collect();
    entries.extend([
        (0, String::new()),
        (CMD_TOGGLE_SIDEBAR as usize, ui::hinted("menu.view.sidebar", "Ctrl+B")),
        (CMD_FOLDER_TREE as usize, tr("menu.view.folder_tree").to_string()),
        (CMD_TOGGLE_PREVIEW as usize, ui::hinted("menu.view.preview_pane", "Alt+P")),
        (CMD_TOGGLE_DETAILS as usize, ui::hinted("menu.view.details_pane", "Alt+Shift+P")),
    ]);
    let mut checked = vec![(*app).views.view.command() as usize];
    for (on, cmd) in [(s.show_sidebar, CMD_TOGGLE_SIDEBAR), (s.show_tree, CMD_FOLDER_TREE), (s.show_preview, CMD_TOGGLE_PREVIEW), (s.show_details, CMD_TOGGLE_DETAILS)] {
        if on {
            checked.push(cmd as usize);
        }
    }
    (entries, checked)
}

/// Applies a pick from `view_entries`.
pub(crate) unsafe fn pick_view(app: *mut App, id: usize) {
    if id != 0 {
        on_command(app, id as u16);
    }
}

/// View v on the command bar.
pub(crate) unsafe fn view_menu(app: *mut App) {
    let (entries, checked) = view_entries(app);
    let (x, y) = anchor(app, CMD_VIEW_MENU);
    pick_view(app, ui::popup(app, &entries, &checked, x, y));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_sort_menu_picks() {
        assert_eq!(home_pick(SORT_DESC, 6, None), Some((0, true)), "a direction alone sorts by name");
        assert_eq!(home_pick(SORT_ASC, 6, Some((2, true))), Some((2, false)));
        assert_eq!(home_pick(2, 6, None), Some((1, true)), "Date modified starts newest first");
        assert_eq!(home_pick(2, 6, Some((1, false))), Some((1, false)), "the menu keeps the direction");
        assert_eq!((home_pick(7, 6, None), home_pick(0, 6, None), home_pick(GROUP_NONE, 6, None)), (None, None, None));
    }

    #[test]
    fn new_item_is_told_apart_on_the_desktop_root() {
        let listed = ["::{20D04FE0-3AEA-1069-A2D8-08002B30309D}", "C:\\Users\\Public\\Desktop\\Edge.lnk", "C:\\Users\\u\\Desktop\\a.txt"];
        // The stub landed before the menu returned, the listing has not reloaded yet: the disk sees it, the listing does not.
        let on_disk = ["c:\\users\\u\\desktop\\a.txt", "c:\\users\\u\\desktop\\stub.lnk"];
        let before = names_before(listed.iter().map(|s| s.to_string()), on_disk.iter().map(|s| s.to_string()));
        let after = [listed[0], listed[1], listed[2], "C:\\Users\\u\\Desktop\\Stub.lnk", "C:\\Users\\u\\Desktop\\Nuevo documento.txt"];
        let added: Vec<&str> = after.iter().copied().filter(|n| !before.contains(&n.to_lowercase())).collect();
        assert_eq!(added, ["C:\\Users\\u\\Desktop\\Nuevo documento.txt"]);
        // No disk read (a share): the listing alone still keeps This PC and the Public Desktop out.
        let before = names_before(listed.iter().map(|s| s.to_string()), std::iter::empty());
        assert_eq!(after.iter().filter(|n| !before.contains(&n.to_lowercase())).count(), 2);
    }

    #[test]
    fn view_menu_chords_match_accels() {
        let ids: Vec<u16> = VIEW_MENU.iter().map(|e| e.0).collect();
        assert_eq!(ids, [CMD_VIEW_XL, CMD_VIEW1 + 1, CMD_VIEW_MEDIUM, CMD_VIEW_SMALL, CMD_VIEW_LIST, CMD_VIEW1, CMD_VIEW_TILES, CMD_VIEW_CONTENT, CMD_VIEW1 + 2]);
        for (id, label, hint) in VIEW_MENU {
            match hint.strip_prefix("Ctrl+Shift+") {
                Some(n) => {
                    let key = VK_0.0 + n.parse::<u16>().unwrap();
                    assert!(ACCELS.iter().any(|a| a.fVirt.0 == FVIRTKEY.0 | FCONTROL.0 | FSHIFT.0 && a.key == key && a.cmd == id), "{label}");
                }
                None => assert!(hint.is_empty() && label == "menu.view.thumbnails", "{label}"),
            }
        }
    }
}
