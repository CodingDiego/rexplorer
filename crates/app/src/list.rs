//! The ListView: virtual items, sorting, shell context menus, keys, rename and middle-click.

use super::*;
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, DT_CENTER, DT_END_ELLIPSIS, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DrawTextW, FillRect, SelectObject, SetBkMode,
    SetTextColor, TRANSPARENT,
};
use crate::nav::{self, cur, listing, open_in_background, sort_listing, source};
use crate::source::Source;
use crate::ui::Entry;
use rexplorer_core::dragdrop;
use rexplorer_core::undo::{self, Op};
use std::cell::{Cell, RefCell};
use windows::Win32::Foundation::{E_NOINTERFACE, E_NOTIMPL, POINTL, S_OK};
use windows::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Ole::IDropTarget;
use windows::Win32::UI::Shell::{ILClone, ILFindLastID};
use windows::core::{GUID, HRESULT, IUnknown, Interface};

use rexplorer_core::i18n::{tr, trf};

pub(crate) unsafe fn on_notify(app: *mut App, lparam: LPARAM) -> LRESULT {
    let hdr = &*(lparam.0 as *const NMHDR);
    match hdr.code {
        LVN_GETDISPINFOW => {
            let di = &mut *(lparam.0 as *mut NMLVDISPINFOW);
            fill_dispinfo(app, &mut di.item);
        }
        // A click on a row's check box toggles its selection: a row is checked while it is selected.
        NM_CLICK if (*app).profile.item_checkboxes => {
            let nm = &*(lparam.0 as *const NMITEMACTIVATE);
            let mut hit = LVHITTESTINFO { pt: nm.ptAction, ..Default::default() };
            SendMessageW((*app).list, LVM_HITTEST, None, Some(LPARAM(&mut hit as *mut _ as isize)));
            if hit.iItem >= 0 && hit.flags.0 & LVHT_ONITEMSTATEICON.0 != 0 {
                let i = hit.iItem as usize;
                let selected = SendMessageW((*app).list, LVM_GETITEMSTATE, Some(WPARAM(i)), Some(LPARAM(LVIS_SELECTED.0 as isize))).0 != 0;
                set_item_state((*app).list, i, LIST_VIEW_ITEM_STATE_FLAGS(if selected { 0 } else { LVIS_SELECTED.0 }), LVIS_SELECTED);
            }
        }
        LVN_COLUMNCLICK => {
            let nm = &*(lparam.0 as *const NMLISTVIEW);
            on_column_click(app, nm.iSubItem as usize);
        }
        LVN_ITEMACTIVATE => {
            let nm = &*(lparam.0 as *const NMITEMACTIVATE);
            if nm.iItem >= 0 {
                nav::activate_or_tab(app, nm.iItem as usize, nm.uKeyFlags & LVKF_CONTROL != 0);
            }
        }
        LVN_ITEMCHANGED | LVN_ODSTATECHANGED => {
            SetTimer(Some((*app).hwnd), TIMER_STATUS, 100, None);
            panes::schedule_preview(app);
        }
        LVN_ODCACHEHINT => {
            let nm = &*(lparam.0 as *const NMLVCACHEHINT);
            views::on_cache_hint(app, nm.iFrom, nm.iTo);
        }
        LVN_BEGINDRAG | LVN_BEGINRDRAG => begin_drag(app),
        NM_CUSTOMDRAW => return LRESULT(draw_item(app, &mut *(lparam.0 as *mut NMLVCUSTOMDRAW)) as isize),
        NM_RCLICK => {
            let nm = &*(lparam.0 as *const NMITEMACTIVATE);
            show_menu(app, nm.iItem >= 0, None);
            return LRESULT(1);
        }
        LVN_KEYDOWN => {
            let nm = &*(lparam.0 as *const NMLVKEYDOWN);
            on_key(app, VIRTUAL_KEY(nm.wVKey));
        }
        // Only shell items and search results can be renamed.
        LVN_BEGINLABELEDITW => {
            // The ListView selects all the text after this returns, so the stem selection is posted behind it.
            // Tab and Shift+Tab in the edit are comctl32's own: commit, then edit the next / previous row.
            let i = (*(lparam.0 as *const NMLVDISPINFOW)).item.iItem as usize;
            let Some(item) = editable_item(app, i) else { return LRESULT(1) };
            (*app).renaming = Some(item.parsing_name());
            if let Some(n) = source::rename_stem(&item) {
                select_stem_later((*app).list, n);
            }
            return LRESULT(0);
        }
        LVN_ENDLABELEDITW => {
            let di = &*(lparam.0 as *const NMLVDISPINFOW);
            on_end_edit(app, &di.item);
        }
        LVN_ODFINDITEMW => {
            // Type-to-select: a virtual list holds no text, so it asks us. 0 would mean "item 0 matches".
            let nm = &*(lparam.0 as *const NMLVFINDITEMW);
            let Some(s) = source(app) else { return LRESULT(-1) };
            if !nm.lvfi.flags.contains(LVFI_STRING) || nm.lvfi.psz.is_null() {
                return LRESULT(-1);
            }
            let needle = nm.lvfi.psz.to_string().unwrap_or_default();
            let wrap = nm.lvfi.flags.contains(LVFI_WRAP);
            let found = find_prefix(s.len(), nm.iStart.max(0) as usize, wrap, &needle, |i| source::row_text(app, i, 0));
            return LRESULT(found.map_or(-1, |i| i as isize));
        }
        _ => {}
    }
    LRESULT(0)
}

/// Makes `list`'s label edit select only the first `n` UTF-16 units: the first select-all it gets (the list's own,
/// whenever it comes) becomes (0, n), and a posted select covers a select-all that already happened.
pub(crate) unsafe fn select_stem_later(list: HWND, n: usize) {
    let edit = HWND(SendMessageW(list, LVM_GETEDITCONTROL, None, None).0 as *mut c_void);
    let _ = SetWindowSubclass(edit, Some(stem_subclass), STEM_ID, n);
    let _ = PostMessageW(Some(edit), EM_SETSEL, WPARAM(0), LPARAM(n as isize));
}

const STEM_ID: usize = 0x57E;

/// One-shot: turns the first EM_SETSEL(0, -1) into (0, n), and goes away at that or at the first user input.
unsafe extern "system" fn stem_subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, n: usize) -> LRESULT {
    let select_all = msg == EM_SETSEL && wparam.0 == 0 && (lparam.0 == -1 || lparam.0 as u32 == u32::MAX);
    if select_all || matches!(msg, WM_KEYDOWN | WM_CHAR | WM_LBUTTONDOWN) {
        let _ = windows::Win32::UI::Shell::RemoveWindowSubclass(hwnd, Some(stem_subclass), STEM_ID);
    }
    if select_all {
        return DefSubclassProc(hwnd, msg, wparam, LPARAM(n as isize));
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

pub(crate) fn find_prefix(n: usize, start: usize, wrap: bool, needle: &str, name: impl Fn(usize) -> String) -> Option<usize> {
    let start = start.min(n);
    let needle = needle.to_lowercase();
    (start..n).chain(if wrap { 0..start } else { 0..0 }).find(|&i| name(i).to_lowercase().starts_with(&needle))
}

/// Type-ahead for panes that take WM_CHAR themselves: (the new typed buffer, the text to look for), the needle empty
/// for a control character or a space (ignored). A pause over a second restarts the buffer; the same letter repeated
/// looks for that one letter again, to cycle through the names starting with it.
pub(crate) fn type_ahead(buf: &str, last_ms: u32, now_ms: u32, ch: char) -> (String, String) {
    if ch.is_control() || ch == ' ' {
        return (buf.to_string(), String::new());
    }
    let mut typed = if now_ms.wrapping_sub(last_ms) > 1000 { String::new() } else { buf.to_string() };
    typed.push(ch);
    let needle = if typed.chars().all(|c| c == ch) { ch.to_string() } else { typed.clone() };
    (typed, needle)
}

/// Whether row `i` is a file on the clipboard's cut; free while nothing is cut.
unsafe fn is_cut(app: *mut App, i: usize) -> bool {
    !(*app).cut.is_empty() && listing(app).and_then(|l| l.items.get(i)).is_some_and(|it| (*app).cut.contains(&it.parsing_name().to_lowercase()))
}

/// WM_CLIPBOARDUPDATE: re-reads which files a cut put on the clipboard and repaints when that changed.
pub(crate) unsafe fn refresh_cut(app: *mut App) {
    let cut: std::collections::HashSet<String> = dragdrop::clipboard_cut_paths().iter().map(|p| p.to_lowercase()).collect();
    if cut != (*app).cut {
        (*app).cut = cut;
        let _ = InvalidateRect(Some((*app).list), None, false);
    }
}

unsafe fn fill_dispinfo(app: *mut App, item: &mut LVITEMW) {
    let i = item.iItem as usize;
    if item.mask.contains(LVIF_STATE) && item.iSubItem == 0 && item.stateMask.0 & LVIS_CUT.0 != 0 && is_cut(app, i) {
        item.state = LIST_VIEW_ITEM_STATE_FLAGS(item.state.0 | LVIS_CUT.0);
    }
    // With check boxes a row is checked while it is selected.
    if item.mask.contains(LVIF_STATE) && item.iSubItem == 0 && (*app).profile.item_checkboxes {
        let selected = SendMessageW((*app).list, LVM_GETITEMSTATE, Some(WPARAM(i)), Some(LPARAM(LVIS_SELECTED.0 as isize))).0 != 0;
        item.state = LIST_VIEW_ITEM_STATE_FLAGS((item.state.0 & !LVIS_STATEIMAGEMASK.0) | (if selected { 2 } else { 1 }) << 12);
    }
    let icon_view = (*app).views.view.px() > 0;
    if item.mask.contains(LVIF_IMAGE) && item.iSubItem == 0 && icon_view {
        item.iImage = views::image(app, i);
    }
    let Some(s) = source(app) else { return };
    if i >= s.len() {
        return;
    }
    if item.mask.contains(LVIF_COLUMNS) && item.iSubItem == 0 {
        put_tile_columns(item, &views::tile_columns((*app).views.view, s.column_count()));
    }
    if item.mask.contains(LVIF_TEXT) {
        copy_text(&source::row_text(app, i, item.iSubItem as usize), item.pszText, item.cchTextMax);
    }
    if item.mask.contains(LVIF_IMAGE) && item.iSubItem == 0 && !icon_view {
        item.iImage = views::tall_icon(app, source::small_icon(app, i));
    }
}

/// Fills a tile's extra-line request: the header positions, and zero formats when asked for. The list hands over room
/// for 20 of each.
fn put_tile_columns(item: &mut LVITEMW, cols: &[u32]) {
    if item.puColumns.is_null() {
        item.cColumns = 0;
        return;
    }
    let n = cols.len().min(20);
    // SAFETY: puColumns (and piColFmt when LVIF_COLFMT is asked) point at the list's 20-slot arrays.
    unsafe {
        for (k, &c) in cols[..n].iter().enumerate() {
            *item.puColumns.add(k) = c;
            if item.mask.contains(LVIF_COLFMT) && !item.piColFmt.is_null() {
                *item.piColFmt.add(k) = LIST_VIEW_ITEM_COLUMN_FORMAT_FLAGS(0);
            }
        }
    }
    item.cColumns = n as u32;
}

unsafe fn on_column_click(app: *mut App, sub: usize) {
    if let Some(Source::Search(s)) = &cur(app).source {
        let desc = match s.sort {
            Some((c, d)) if c == sub => !d,
            _ => sub == 3,
        };
        return source::sort_search(app, sub, desc);
    }
    let Some(Source::Shell(l)) = &cur(app).source else { return };
    let Some(col) = l.columns.get(sub) else { return };
    let desc = if col.index == l.sort_col { !l.sort_desc } else { col.descending_first() };
    sort_by(app, col.index, desc);
}

/// Sorts the active shell listing by shell column `col` (the header click, the command bar's Sort menu).
pub(crate) unsafe fn sort_by(app: *mut App, col: u32, desc: bool) {
    let list = (*app).list;
    let Some(Source::Shell(l)) = &mut cur(app).source else { return };
    // The list keeps selection and focus per row position: carry them over to the same items. The sort moves the
    // items but not their PIDLs, so a PIDL's address is the item's identity (exact, unlike a case-folded name).
    let carry = snapshot(list, l.items.len(), |i| l.items.get(i).map(|it| it.pidl.as_ptr() as usize));
    l.sort_col = col;
    l.sort_desc = desc;
    sort_listing(l);
    // Rows moved between groups: the list's groups follow before it is asked where any row is.
    set_groups(app, true);
    // Icon views keep images per row position: drop them, or thumbnails stay where their files were.
    views::reset(app, l.items.len());
    restore(list, &carry, l.items.iter().map(|it| it.pidl.as_ptr() as usize));
    let _ = InvalidateRect(Some(list), None, true);
    set_sort_arrow(app);
    nav::remember_folder(app);
}

/// Selection and focus of a list by row identity, to carry across a re-sort.
pub(crate) struct Carry {
    all: bool,
    selected: std::collections::HashSet<usize>,
    focused: Option<usize>,
}

/// The list keeps selection and focus per row position: `key` names the item at a position, and `restore` puts the
/// state back on the same keys. `n` is the row count.
pub(crate) unsafe fn snapshot(list: HWND, n: usize, key: impl Fn(usize) -> Option<usize>) -> Carry {
    let focus = SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_FOCUSED as isize))).0;
    // Ctrl+A then sort: every row stays selected whatever the order, so only the focus moves (collecting and
    // re-setting each row costs 0.15 s per 20k rows).
    let all = SendMessageW(list, LVM_GETSELECTEDCOUNT, None, None).0 as usize == n;
    let selected = if all { Default::default() } else { selected_indices(list).into_iter().filter_map(&key).collect() };
    Carry { all, selected, focused: usize::try_from(focus).ok().and_then(key) }
}

/// Puts the snapshot's state on the rows whose keys (in the new order) it names; the Shift+click anchor follows the
/// focused item, and that row stays in view.
pub(crate) unsafe fn restore(list: HWND, c: &Carry, keys: impl Iterator<Item = usize>) {
    let mask = if c.all { LVIS_FOCUSED.0 } else { LVIS_SELECTED.0 | LVIS_FOCUSED.0 };
    set_item_state(list, usize::MAX, LIST_VIEW_ITEM_STATE_FLAGS(0), LIST_VIEW_ITEM_STATE_FLAGS(mask));
    let mut row = None;
    for (i, p) in keys.enumerate() {
        if !c.all && c.selected.contains(&p) {
            set_item_state(list, i, LVIS_SELECTED, LVIS_SELECTED);
        }
        if c.focused == Some(p) {
            set_item_state(list, i, LVIS_FOCUSED, LVIS_FOCUSED);
            row = Some(i);
        }
    }
    if let Some(i) = row {
        SendMessageW(list, LVM_SETSELECTIONMARK, None, Some(LPARAM(i as isize)));
        SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(i)), Some(LPARAM(0)));
    }
}

/// Marks the sorted column's header item with the sort direction (draw_header paints the arrow).
pub(crate) unsafe fn set_sort_arrow(app: *mut App) {
    let header = HWND(SendMessageW((*app).list, LVM_GETHEADER, None, None).0 as *mut c_void);
    let sorted = match source(app) {
        Some(Source::Search(s)) => s.sort,
        _ => listing(app).and_then(|l| l.columns.iter().position(|c| c.index == l.sort_col).map(|p| (p, l.sort_desc))),
    };
    let n = SendMessageW(header, HDM_GETITEMCOUNT, None, None).0.max(0) as usize;
    for i in 0..n {
        let mut hi = HDITEMW { mask: HDI_FORMAT, ..Default::default() };
        SendMessageW(header, HDM_GETITEMW, Some(WPARAM(i)), Some(LPARAM(&mut hi as *mut _ as isize)));
        hi.fmt.0 &= !(HDF_SORTUP.0 | HDF_SORTDOWN.0);
        match sorted {
            Some((p, desc)) if p == i => hi.fmt.0 |= if desc { HDF_SORTDOWN.0 } else { HDF_SORTUP.0 },
            _ => {}
        }
        SendMessageW(header, HDM_SETITEMW, Some(WPARAM(i)), Some(LPARAM(&hi as *const _ as isize)));
    }
}

/// The active listing's groups on the list (none when `on` is false or it has none). Off before any item-count
/// change and on after it: the list lays out its groups by their item counts.
pub(crate) unsafe fn set_groups(app: *mut App, on: bool) {
    let on = on && (*app).views.view != views::View::List; // comctl32 has no groups in list mode
    let list = (*app).list;
    SendMessageW(list, LVM_REMOVEALLGROUPS, None, None);
    let (groups, rows) = listing(app).filter(|_| on).map_or((&[][..], 0), |l| (&l.groups[..], l.items.len()));
    // The callback is set only while groups show: left set without them, comctl32 (CLVItemStore::OnGetItem) reads a
    // group that is not there once the item count changes. Without it the list refuses groups (-1).
    if groups.is_empty() || !set_callback(app, true) || SendMessageW(list, LVM_ENABLEGROUPVIEW, Some(WPARAM(1)), None).0 < 0 {
        SendMessageW(list, LVM_ENABLEGROUPVIEW, Some(WPARAM(0)), None);
        set_callback(app, false);
        return;
    }
    for (id, (label, first)) in groups.iter().enumerate() {
        let end = groups.get(id + 1).map_or(rows, |g| g.1);
        let mut header: Vec<u16> = label.encode_utf16().chain([0]).collect();
        let g = LVGROUP {
            cbSize: size_of::<LVGROUP>() as u32,
            mask: LVGF_HEADER | LVGF_GROUPID | LVGF_ITEMS,
            pszHeader: PWSTR(header.as_mut_ptr()),
            iGroupId: id as i32,
            cItems: (end - *first) as u32,
            ..Default::default()
        };
        SendMessageW(list, LVM_INSERTGROUP, Some(WPARAM(usize::MAX)), Some(LPARAM(&g as *const _ as isize)));
    }
}

/// Sets (or clears) the virtual list's IOwnerDataCallback. Since Vista a virtual list ignores LVITEM.iGroupId and
/// asks that callback which rows each group holds; it goes in through the list's IListView (both undocumented).
unsafe fn set_callback(app: *mut App, on: bool) -> bool {
    const IID_ILISTVIEW: GUID = GUID::from_u128(0xE5B16AF2_3990_4681_A609_1F060CD14269); // Windows 7 and later
    let mut lv: *mut c_void = std::ptr::null_mut();
    SendMessageW((*app).list, LVM_FIRST + 189, Some(WPARAM(&IID_ILISTVIEW as *const GUID as usize)), Some(LPARAM(&mut lv as *mut _ as isize)));
    if lv.is_null() {
        return false;
    }
    let lv = IUnknown::from_raw(lv);
    // IListView::SetOwnerDataCallback: vtable slot 112 (IUnknown 3, IOleWindow 2, then 107 methods before it).
    let slot = (*(lv.as_raw() as *const *const *const c_void)).add(112).read();
    let set: unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT = std::mem::transmute(slot);
    let cb = if on { Box::into_raw(Box::new(Groups { vtbl: &GROUPS_VTBL, refs: Cell::new(1), app })) } else { std::ptr::null_mut() };
    let ok = set(lv.as_raw(), cb.cast()).is_ok();
    // The list keeps its own reference (and releases it when the callback is replaced or cleared).
    if on {
        groups_release(cb);
    }
    ok
}

/// IOwnerDataCallback {44C09D56-8D3B-419D-A462-7B956B105B47}, answering from the active listing's groups (runs of
/// rows).
#[repr(C)]
struct Groups {
    vtbl: &'static GroupsVtbl,
    refs: Cell<u32>,
    app: *mut App,
}

#[repr(C)]
struct GroupsVtbl {
    query_interface: unsafe extern "system" fn(*mut Groups, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut Groups) -> u32,
    release: unsafe extern "system" fn(*mut Groups) -> u32,
    get_item_position: unsafe extern "system" fn(*mut Groups, i32, *mut POINT) -> HRESULT,
    set_item_position: unsafe extern "system" fn(*mut Groups, i32, POINT) -> HRESULT,
    get_item_in_group: unsafe extern "system" fn(*mut Groups, i32, i32, *mut i32) -> HRESULT,
    get_item_group: unsafe extern "system" fn(*mut Groups, i32, i32, *mut i32) -> HRESULT,
    get_item_group_count: unsafe extern "system" fn(*mut Groups, i32, *mut i32) -> HRESULT,
    on_cache_hint: unsafe extern "system" fn(*mut Groups, LVITEMINDEX, LVITEMINDEX) -> HRESULT,
}

static GROUPS_VTBL: GroupsVtbl = GroupsVtbl {
    query_interface: groups_query,
    add_ref: groups_add_ref,
    release: groups_release,
    get_item_position: groups_position,
    set_item_position: groups_set_position,
    get_item_in_group: groups_item_in_group,
    get_item_group: groups_item_group,
    get_item_group_count: groups_item_group_count,
    on_cache_hint: groups_cache_hint,
};

unsafe extern "system" fn groups_query(this: *mut Groups, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    let ok = *iid == IUnknown::IID || *iid == GUID::from_u128(0x44C09D56_8D3B_419D_A462_7B956B105B47);
    *out = if ok { this.cast() } else { std::ptr::null_mut() };
    if !ok {
        return E_NOINTERFACE;
    }
    groups_add_ref(this);
    S_OK
}

unsafe extern "system" fn groups_add_ref(this: *mut Groups) -> u32 {
    (*this).refs.set((*this).refs.get() + 1);
    (*this).refs.get()
}

unsafe extern "system" fn groups_release(this: *mut Groups) -> u32 {
    let n = (*this).refs.get() - 1;
    (*this).refs.set(n);
    if n == 0 {
        drop(Box::from_raw(this));
    }
    n
}

unsafe extern "system" fn groups_position(_: *mut Groups, _: i32, _: *mut POINT) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn groups_set_position(_: *mut Groups, _: i32, _: POINT) -> HRESULT {
    E_NOTIMPL
}

/// The row of item `n` of group `g`.
unsafe extern "system" fn groups_item_in_group(this: *mut Groups, g: i32, n: i32, row: *mut i32) -> HRESULT {
    *row = group_runs((*this).app).get(g as usize).map_or(0, |r| r.1 as i32) + n;
    S_OK
}

/// The group of `row` (each row is in one group).
unsafe extern "system" fn groups_item_group(this: *mut Groups, row: i32, _occurrence: i32, g: *mut i32) -> HRESULT {
    *g = group_runs((*this).app).partition_point(|r| r.1 as i32 <= row).saturating_sub(1) as i32;
    S_OK
}

unsafe extern "system" fn groups_item_group_count(_: *mut Groups, _: i32, n: *mut i32) -> HRESULT {
    *n = 1;
    S_OK
}

unsafe extern "system" fn groups_cache_hint(_: *mut Groups, _: LVITEMINDEX, _: LVITEMINDEX) -> HRESULT {
    S_OK
}

/// The active listing's groups; none while the tab shows anything else (the list may ask before set_groups runs).
unsafe fn group_runs<'a>(app: *mut App) -> &'a [(String, usize)] {
    listing(app).map_or(&[], |l| &l.groups)
}

/// Drags the selection with the shell's data object; the shell performs the operation and the change watch refreshes.
pub(crate) unsafe fn begin_drag(app: *mut App) {
    let sel = selected_indices((*app).list);
    let data = match source(app) {
        Some(Source::Shell(l)) => {
            let items: Vec<&ShellItem> = sel.iter().filter_map(|&i| l.items.get(i)).collect();
            if items.is_empty() {
                return;
            }
            dragdrop::data_object(&l.folder, &items, (*app).hwnd)
        }
        // Rows come from many folders: by parsing name, through a shell item array.
        Some(Source::Search(s)) => {
            let paths: Vec<String> = sel.iter().filter_map(|&i| s.rows.get(i)).map(|r| r.item.parsing_name()).collect();
            if paths.is_empty() {
                return;
            }
            dragdrop::data_object_of_paths(&paths)
        }
        // RAR members are extracted first (a password is asked for if needed) and offered as copies of those files.
        Some(Source::Archive(a)) => {
            let picked: Vec<rexplorer_core::archive::Entry> = sel.iter().filter_map(|&i| a.rows.get(i)).cloned().collect();
            if picked.is_empty() {
                return;
            }
            let archive = a.listing.archive.clone();
            let extracted = source::with_password(app, &archive, || match source(app) {
                Some(Source::Archive(a)) => a.listing.extract_for_drag(&picked),
                _ => Err(windows::Win32::Foundation::E_ABORT.into()),
            });
            let paths = match extracted {
                Some(Ok(p)) => p,
                None => return,
                Some(Err(e)) => {
                    message((*app).hwnd, &trf("err.drag", &[("error", &e)]));
                    return;
                }
            };
            let strings: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
            match dragdrop::data_object_of_paths(&strings) {
                Ok(d) => {
                    crate::watchdog::modal(); // the drag runs its own loop until the drop
                    if let Err(e) = dragdrop::drag_with(&d, ui::view_hwnd(app), windows::Win32::System::Ole::DROPEFFECT_COPY) {
                        message((*app).hwnd, &trf("err.drag", &[("error", &e)]));
                    }
                }
                Err(e) => message((*app).hwnd, &trf("err.drag", &[("error", &e)])),
            }
            return;
        }
        _ => return,
    };
    let d = match data {
        Ok(d) => d,
        Err(e) => {
            rexplorer_core::crash::log(&format!("drag data object failed: {e}"));
            message((*app).hwnd, &trf("err.drag", &[("error", &e)]));
            return;
        }
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
    // Physical buttons: a button already up when the loop starts makes it drop at once where the pointer is.
    let button_down = GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 || GetAsyncKeyState(VK_RBUTTON.0 as i32) < 0;
    let start = std::time::Instant::now();
    let r = {
        // Home answers the shell's request for the drag image only inside this call.
        let _grant = crate::home::grant_drag_image(ui::view_hwnd(app));
        crate::watchdog::modal(); // the drag runs its own loop until the drop
        dragdrop::drag(&d, ui::view_hwnd(app))
    };
    // Every drag that moved nothing is logged, so the next live run can say why a target never got it.
    if let Some(note) = dragdrop::drag_note(&r, dragdrop::performed_effect(&d), button_down, start.elapsed().as_millis()) {
        rexplorer_core::crash::log(&note);
    }
    // A refused or cancelled drop is a success code (Ok, effect NONE) and shows nothing, as in Explorer.
    if let Err(e) = r {
        message((*app).hwnd, &trf("err.drag", &[("error", &e)]));
    }
}

/// The list row under the screen point, or -1.
unsafe fn row_at(list: HWND, pt: POINTL) -> isize {
    let mut p = POINT { x: pt.x, y: pt.y };
    let _ = ScreenToClient(list, &mut p);
    let mut ht = LVHITTESTINFO { pt: p, ..Default::default() };
    SendMessageW(list, LVM_HITTEST, None, Some(LPARAM(&mut ht as *mut _ as isize))).0
}

/// A paste or drop of `sources` into `dest`, read before the shell runs it (see `undo::plan_transfer`).
pub(crate) fn plan_transfer(sources: &[String], dest: &str) -> Vec<undo::Sent> {
    if !ui::is_fs_path(dest) {
        return Vec::new();
    }
    let temp = std::env::temp_dir().to_string_lossy().to_lowercase();
    let sources: Vec<String> = sources.iter().filter(|s| !extracted_for_transfer(&s.to_lowercase(), &temp)).cloned().collect();
    undo::plan_transfer(&sources, dest)
}

/// Files an archive was extracted to for a transfer (the zip folder's `%TEMP%\Temp1_x.zip`, our own archive
/// folders): once cleaned up, undo would take the copy for a move and put it back there. Other files under %TEMP%
/// are ordinary files. Both arguments lowercase.
fn extracted_for_transfer(path: &str, temp: &str) -> bool {
    let Some(rest) = path.strip_prefix(temp.trim_end_matches('\\')).and_then(|r| r.strip_prefix('\\')) else { return false };
    let top = rest.split('\\').next().unwrap_or("");
    let zip = top.strip_prefix("temp").and_then(|t| t.split_once('_')).is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    zip || top.starts_with("rexplorer-archive") || top.starts_with("rexplorer-show-")
}

/// Drops land on the folder item under the cursor, else on the current folder.
pub(crate) unsafe fn register_drop(app: *mut App) {
    let ptr = app as usize;
    // The forwarder re-enters a target only when pick returns a different object, so reuse it per (listing, item).
    let last: RefCell<Option<((String, isize), IDropTarget)>> = RefCell::new(None);
    let pick = move |pt: POINTL| -> Option<IDropTarget> {
        let app = ptr as *mut App;
        unsafe {
            let (list, hwnd) = ((*app).list, (*app).hwnd);
            let l = listing(app)?;
            let i = row_at(list, pt);
            let folder_item = usize::try_from(i).ok().and_then(|i| l.items.get(i)).filter(|it| it.is_folder());
            // By parsing name: a PIDL's address can be reused by a later folder's allocation.
            let key = (l.folder.parsing_name.clone(), if folder_item.is_some() { i } else { -1 });
            if let Some((k, t)) = &*last.borrow()
                && *k == key {
                    return Some(t.clone());
                }
            let target = match folder_item {
                Some(it) => dragdrop::item_drop_target(&l.folder, it, hwnd),
                None => dragdrop::folder_drop_target(&l.folder, hwnd),
            }
            .ok()?;
            *last.borrow_mut() = Some((key, target.clone()));
            Some(target)
        }
    };
    // Called before the shell runs the drop, while the destination names are still free.
    let on_drop = move |pt: POINTL, data: &IDataObject| unsafe {
        let app = ptr as *mut App;
        let l = listing(app)?;
        let dest = usize::try_from(row_at((*app).list, pt))
            .ok()
            .and_then(|i| l.items.get(i))
            .filter(|it| it.is_folder())
            .map_or_else(|| l.folder.parsing_name.clone(), |it| it.parsing_name());
        Some(record_drop(ptr, data, dest))
    };
    let _ = dragdrop::register((*app).list, Box::new(pick), Some(Box::new(on_drop)));
}

/// Plans a drop into `dest` before it runs; what it returns gives the undo entry once the target says what it did:
/// copies and moves as a transfer, shortcuts ("Create shortcuts here", Alt or a system folder) as the .lnk files
/// that appeared. A drop that did nothing (refused, cancelled, its right-drag menu dismissed) gives an empty entry,
/// which is not recorded.
pub(crate) fn plan_drop(data: &IDataObject, dest: String) -> impl FnOnce(windows::Win32::System::Ole::DROPEFFECT) -> Op + use<> {
    let pairs = plan_transfer(&dragdrop::hdrop_paths(data), &dest);
    let links = shortcut_names(&dest);
    move |effect| drop_entry(effect, pairs, || new_shortcuts(&dest, &links))
}

fn drop_entry(effect: windows::Win32::System::Ole::DROPEFFECT, pairs: Vec<undo::Sent>, links: impl FnOnce() -> Vec<String>) -> Op {
    use windows::Win32::System::Ole::{DROPEFFECT_LINK, DROPEFFECT_NONE};
    if effect == DROPEFFECT_NONE {
        Op::Transfer(Vec::new())
    } else if effect == DROPEFFECT_LINK {
        Op::created(links())
    } else {
        Op::Transfer(pairs)
    }
}

/// Records a drop into `dest` for undo (`plan_drop`).
pub(crate) fn record_drop(app: usize, data: &IDataObject, dest: String) -> dragdrop::AfterDrop {
    let op = plan_drop(data, dest);
    Box::new(move |effect| unsafe { (*(app as *mut App)).undo.record(op(effect)) })
}

/// The .lnk file names in `dir`.
fn shortcut_names(dir: &str) -> std::collections::HashSet<String> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Default::default() };
    rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.to_lowercase().ends_with(".lnk")).collect()
}

/// Full paths of the .lnk files in `dir` that `before` did not list.
fn new_shortcuts(dir: &str, before: &std::collections::HashSet<String>) -> Vec<String> {
    let mut v: Vec<String> = shortcut_names(dir).into_iter().filter(|n| !before.contains(n)).map(|n| std::path::Path::new(dir).join(n).to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

/// Drops on `window` go into the folder `at` names under the screen point: a key for it (its parsing name) and
/// where it is. One shell target per key, failures included, kept while the pointer stays on it.
pub(crate) unsafe fn register_folder_drop(app: *mut App, window: HWND, at: impl Fn(*mut App, POINTL) -> Option<(String, nav::Target)> + 'static) {
    let ptr = app as usize;
    // A key that does not resolve is cached too, or ShellFolder::parse would run on every DragOver.
    #[allow(clippy::type_complexity)] // a local drop-target cache, named once here
    let last: std::rc::Rc<RefCell<Option<(String, Option<IDropTarget>)>>> = Default::default();
    let seen = last.clone();
    let pick = move |pt: POINTL| -> Option<IDropTarget> {
        let app = ptr as *mut App;
        let (key, target) = at(app, pt)?;
        if let Some((k, t)) = &*last.borrow()
            && *k == key {
                return t.clone();
            }
        let pidl = match target {
            nav::Target::Pidl(p) | nav::Target::Search(_, p) => Some(p),
            nav::Target::Name(n) => rexplorer_core::ShellFolder::parse(&n).ok().map(|f| f.pidl),
        };
        let t = pidl.and_then(|p| unsafe { dragdrop::pidl_drop_target(&p, (*app).hwnd) }.ok());
        *last.borrow_mut() = Some((key, t.clone()));
        t
    };
    // The forwarder calls this only while a target is current, which is the one picked for the last key.
    let on_drop = move |_: POINTL, data: &IDataObject| {
        let dest = seen.borrow().as_ref().map(|(k, _)| k.clone())?;
        // A Home tile can be a file: a drop on it opens the file with the dropped items and copies nothing.
        std::path::Path::new(&dest).is_dir().then(|| record_drop(ptr, data, dest))
    };
    let _ = dragdrop::register(window, Box::new(pick), Some(Box::new(on_drop)));
}

/// Drop targets on the sidebar rows, the crumbs, the tabs and the Home tiles.
#[allow(clippy::needless_borrow)] // the explicit borrow of a raw-pointer field silences rustc's dangerous_implicit_autorefs
pub(crate) unsafe fn register_folder_drops(app: *mut App) {
    register_folder_drop(app, (*app).sidebar, |app, pt| unsafe {
        let s = (*app).sidebar;
        let t = sidebar::target(s, sidebar::row_at_screen(s, pt.x, pt.y)?);
        (!t.is_empty()).then(|| (t.clone(), nav::Target::Name(t)))
    });
    register_folder_drop(app, (*app).crumbs, |app, pt| unsafe {
        // The bar shows the destination while loading (as CN_CRUMB ignores clicks then).
        if cur(app).loading.is_some() {
            return None;
        }
        let c = cur(app).crumbs.get(breadcrumb::crumb_at_screen((*app).crumbs, pt.x, pt.y)?)?;
        // Archive crumbs have no pidl: no drop.
        Some((c.parsing.clone(), nav::Target::Pidl(c.pidl.clone()?)))
    });
    register_folder_drop(app, (*app).strip, |app, pt| unsafe {
        let hit = tabs::tab_at_screen((*app).strip, pt.x, pt.y);
        tabs::drag_hover((*app).strip, hit);
        let src = (&(*app).tabs).get(hit?)?.source.as_ref()?;
        // Archive and Search tabs switch on hover but take no drop.
        match src {
            Source::Shell(_) | Source::Browser(_) => Some((src.parsing_name(), src.location())),
            _ => None,
        }
    });
    register_folder_drop(app, (*app).home, |app, pt| unsafe {
        // The tile's parsing name, not Entry::abs (a child of the virtual Home folder).
        let key = home::tile_key_at_screen((*app).home, pt.x, pt.y)?;
        Some((key.clone(), nav::Target::Name(key)))
    });
}

/// Screen point for a keyboard-opened menu: under the focused item's label when it is in view, else the list's
/// top-left corner (as Explorer does).
unsafe fn menu_point(list: HWND, item: Option<usize>) -> POINT {
    let mut client = RECT::default();
    let _ = GetClientRect(list, &mut client);
    let mut r = RECT { left: LVIR_LABEL as i32, ..Default::default() };
    let labelled = item.is_some_and(|i| SendMessageW(list, LVM_GETITEMRECT, Some(WPARAM(i)), Some(LPARAM(&mut r as *mut _ as isize))).0 != 0);
    let mut pt = if labelled && r.bottom > client.top && r.top < client.bottom {
        POINT { x: r.left, y: r.bottom.min(client.bottom) }
    } else {
        POINT { x: client.left, y: client.top }
    };
    let _ = ClientToScreen(list, &mut pt);
    pt
}

/// The shell context menu for the selection (`on_item`) or the folder background, at `at` (screen) or the cursor.
/// On the Home page our own entry on folders is "Unpin from Home" (pinned) or "Pin to Home" (frequent); elsewhere
/// "Pin to Home".
pub(crate) unsafe fn show_menu(app: *mut App, on_item: bool, at: Option<POINT>) {
    // A previous menu's command may still be running (InvokeCommand pumps): its menu object must outlive it.
    if (*app).active_menu.is_some() {
        return;
    }
    let _op = crate::watchdog::op("context menu");
    let hwnd = (*app).hwnd;
    let pt = at.unwrap_or_else(|| {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        pt
    });
    if let Some(Source::Archive(_)) = source(app) {
        return source::archive_menu(app, pt);
    }
    if !on_item && listing(app).is_some_and(nav::is_home) {
        return home_menu(app, pt);
    }
    let sel = selected_indices((*app).list);
    let menu = if on_item {
        selection_menu(app, &sel)
    } else {
        listing(app).and_then(|l| ContextMenu::for_background(&l.folder, hwnd).ok())
    };
    let Some(mut menu) = menu else { return };
    let home = listing(app).is_some_and(nav::is_home);
    let folders = listing(app).is_some_and(|l| sel.iter().filter_map(|&i| l.items.get(i)).any(|i| i.is_folder()));
    // On Home a selection with a pinned folder offers Unpin; frequent folders offer Pin.
    let pinned = home
        && listing(app).is_some_and(|l| {
            let tiles = l.home.as_ref().map_or(&[][..], |c| &c.tiles[..]);
            let pinned_key = |k: &str| tiles.iter().any(|t| t.pinned && t.key.eq_ignore_ascii_case(k));
            sel.iter().filter_map(|&i| l.items.get(i)).any(|i| i.is_folder() && pinned_key(&i.parsing_name()))
        });
    // Pinning applies to the current folder (background) or to selected folders of a shell listing.
    // Our own entry above the shell's in the list's context menu; Unpin on the Home page's folders.
    let pin = if pinned { Entry::new(CMD_PIN as usize, tr("menu.list.unpin_home"), 0xE77A) } else { Entry::new(CMD_PIN as usize, &ui::hinted("menu.more.pin_home", "Ctrl+D"), 0xE718) };
    let mut own: Vec<Entry> = match (home, on_item) {
        (true, true) if folders => vec![pin],
        (true, _) => Vec::new(),
        (false, _) if listing(app).is_some() && (!on_item || folders) => vec![pin],
        _ => Vec::new(),
    };
    // The background menu starts as Explorer's does: View, Sort by | Refresh, Paste (and our pin), then the shell's.
    if !on_item && !home && listing(app).is_some() {
        let (ve, vc) = commands::view_entries(app);
        let (se, sc) = commands::sort_entries(app);
        own.splice(
            0..0,
            [
                Entry::submenu(tr("menu.list.view"), 0xE8A9, &ve, &vc, VIEW_BASE),
                Entry::submenu(tr("menu.list.sort_by"), 0xE8CB, &se, &sc, SORT_BASE),
                Entry::sep(),
                Entry::new(CMD_REFRESH as usize, &ui::hinted("menu.list.refresh", "F5"), 0xE72C),
                Entry::new(CMD_PASTE as usize, &ui::hinted("menu.list.paste", "Ctrl+V"), 0xE77F).enabled(commands::clipboard_has_files()),
                Entry::new(CMD_PASTE_SHORTCUT as usize, tr("menu.list.paste_shortcut"), 0).enabled(commands::clipboard_has_files()),
            ],
        );
        menu.hide_verb("paste");
        menu.hide_verb("pastelink");
    }
    // A program: Explorer's "Run as administrator" (the shell's runas verb) as our entry, with its shield.
    if on_item && !home && let [i] = sel[..] && listing(app).and_then(|l| l.items.get(i)).is_some_and(|it| runs_elevated(&it.file_name())) {
        own.insert(0, Entry::new(CMD_RUN_AS as usize, &ui::hinted("menu.list.run_as_admin", "Ctrl+Shift+Enter"), 0xEA18));
        menu.hide_verb("runas");
    }
    // Our pin entry stands for the shell's own, which would show the same words twice.
    if own.iter().any(|e| e.id == CMD_PIN as usize) {
        menu.hide_verb(if pinned { "unpinfromhome" } else { "pintohome" });
    }
    if !own.is_empty() {
        own.push(Entry::sep());
    }
    // The shell's Paste runs inside show_in, into the one selected folder or the background's folder: what it may
    // create is read now, while the names are still free.
    let paste_dest = listing(app).and_then(|l| match (on_item, sel.as_slice()) {
        (true, [i]) => l.items.get(*i).filter(|it| it.is_folder()).map(|it| it.parsing_name()),
        (true, _) => None,
        (false, _) => Some(l.folder.parsing_name.clone()),
    });
    // Reading the folder can hang on a slow share: only when there is something to paste.
    let paste_pairs = paste_dest.filter(|_| commands::clipboard_has_files()).map(|d| plan_transfer(&dragdrop::clipboard_paths(), &d)).unwrap_or_default();
    let mut bitmaps = Vec::new();
    let hmenu = ui::build_menu(hwnd, &own, &mut bitmaps);
    let at = GetMenuItemCount(Some(hmenu)).max(0) as u32;
    let extended = GetKeyState(VK_SHIFT.0 as i32) < 0;
    (*app).active_menu = Some(menu);
    // What New will be told apart from: the menu pumps messages, and the change watch may reload the listing with the
    // new item before the pick returns.
    let before = if on_item { None } else { commands::names_now(app) };
    let picked = (*app)
        .active_menu
        .as_ref()
        .map_or(Err(windows::Win32::Foundation::E_UNEXPECTED.into()), |m| m.show_in(hwnd, hmenu, at, pt.x, pt.y, extended));
    (*app).active_menu = None;
    let _ = DestroyMenu(hmenu);
    for b in bitmaps {
        let _ = DeleteObject(b.into());
    }
    match picked {
        Ok(Picked::Rename) => begin_rename(app),
        Ok(Picked::Own(_)) if home => {
            verb_on_selection(app, if pinned { "unpinfromhome" } else { "pintohome" }, false);
            panes::sidebar_changed(app);
            SetTimer(Some(hwnd), TIMER_REFRESH, 300, None);
        }
        Ok(Picked::Own(id)) if id >= SORT_BASE => commands::pick_sort(app, id - SORT_BASE),
        Ok(Picked::Own(id)) if id >= VIEW_BASE => commands::pick_view(app, id - VIEW_BASE),
        Ok(Picked::Own(id)) => on_command(app, id as u16),
        // As the Delete key: to the bin and into the undo log; Shift held at the click deletes for good, as in Explorer.
        Ok(Picked::Delete) => {
            verb_on_selection(app, "delete", GetKeyState(VK_SHIFT.0 as i32) < 0);
            if home {
                SetTimer(Some(hwnd), TIMER_REFRESH, 300, None);
            }
        }
        Ok(Picked::New) => {
            if let Some(before) = before {
                commands::new_item_made(app, before);
            }
        }
        Ok(Picked::Invoked(verb)) if verb.eq_ignore_ascii_case("paste") => {
            (*app).undo.record(Op::Transfer(paste_pairs));
            if home {
                SetTimer(Some(hwnd), TIMER_REFRESH, 300, None);
            }
        }
        // The shell's own entries can change Home (add to or remove from Favorites, remove from the list).
        Ok(Picked::Invoked(_)) if home => {
            SetTimer(Some(hwnd), TIMER_REFRESH, 300, None);
        }
        _ => {}
    }
}

const HOME_RECENT: usize = 0x3001;
const HOME_FREQUENT: usize = 0x3002;

/// Home's background menu: the shell offers none, so it is all ours.
unsafe fn home_menu(app: *mut App, pt: POINT) {
    let p = &(*app).profile;
    let entries = [
        Entry::new(CMD_REFRESH as usize, &ui::hinted("menu.list.refresh", "F5"), 0xE72C),
        Entry::sep(),
        Entry { checked: p.show_recent, ..Entry::new(HOME_RECENT, tr("menu.list.show_recent"), 0) },
        Entry { checked: p.show_frequent, ..Entry::new(HOME_FREQUENT, tr("menu.list.show_frequent"), 0) },
        Entry::sep(),
        Entry::new(CMD_FOLDER_OPTIONS as usize, tr("menu.more.folder_options"), 0xE713),
    ];
    match ui::pick_entries(app, &entries, pt.x, pt.y) {
        Some(HOME_RECENT) => folder_options::toggle(app, true),
        Some(HOME_FREQUENT) => folder_options::toggle(app, false),
        Some(id) => on_command(app, id as u16),
        None => {}
    }
}

/// Context-menu ids of the View and Sort by submenu entries: the command bar's ids plus these.
const VIEW_BASE: usize = 0x1000;
const SORT_BASE: usize = 0x2000;

/// Ctrl+D and the menus' "Pin to Home": the shell's canonical "pintohome" verb on the selected folders, or on the
/// current folder when nothing is selected. Beeps when there is nothing to pin or the shell refuses.
pub(crate) unsafe fn pin_to_home(app: *mut App) {
    let hwnd = (*app).hwnd;
    let menu = (|| {
        // While a navigation is in flight the listing is the place being left.
        if cur(app).loading.is_some() {
            return None;
        }
        let l = listing(app)?;
        let sel = selected_indices((*app).list);
        if sel.is_empty() {
            let parent = ShellFolder::from_pidl(&l.folder.parent_pidl()?).ok()?;
            let last = ILClone(ILFindLastID(l.folder.pidl.as_ptr()));
            if last.is_null() {
                return None;
            }
            let item = ShellItem { parent: parent.folder.clone(), pidl: Pidl::from_raw(last) };
            return ContextMenu::for_items(&parent, &[&item], hwnd).ok();
        }
        let folders: Vec<&ShellItem> = sel.iter().filter_map(|&i| l.items.get(i)).filter(|i| i.is_folder()).collect();
        if folders.is_empty() {
            return None;
        }
        ContextMenu::for_items(&l.folder, &folders, hwnd).ok()
    })();
    match menu.map(|m| m.invoke_verb(hwnd, "pintohome", false)) {
        Some(Ok(())) => panes::sidebar_changed(app),
        _ => {
            let _ = MessageBeep(MB_OK);
        }
    }
}

/// While the active tab loads: eight placeholder rows (an icon square, a name bar, two detail bars under the
/// second and third columns) in the selection color washed toward the surface, alternating between two shades.
unsafe fn draw_skeleton(app: *mut App, hdc: HDC) {
    let (list, t) = ((*app).list, &(*app).theme);
    let dpi = GetDpiForWindow(list) as i32;
    let sc = |v: i32| v * dpi / 96;
    let mut client = RECT::default();
    let _ = GetClientRect(list, &mut client);
    let w = client.right;
    let header = HWND(SendMessageW(list, LVM_GETHEADER, None, None).0 as *mut c_void);
    let mut top = client.top + sc(4);
    let mut cols = Vec::new();
    if !header.is_invalid() && IsWindowVisible(header).as_bool() {
        let mut hr = RECT::default();
        let _ = GetWindowRect(header, &mut hr);
        top += hr.bottom - hr.top;
        let n = SendMessageW(header, HDM_GETITEMCOUNT, None, None).0.max(0) as usize;
        for i in 0..n.min(3) {
            let mut r = RECT::default();
            SendMessageW(header, HDM_GETITEMRECT, Some(WPARAM(i)), Some(LPARAM(&mut r as *mut _ as isize)));
            cols.push(r);
        }
    }
    // Contrast: a steady system-color bar, no pulse.
    let color = t.contrast.map_or_else(|| theme::blend(t.selection, t.surface, if (*app).pulse { 0.75 } else { 0.5 }), |c| c.gray);
    let (row, pad) = (sc(28), sc(12));
    // Name bar up to the second column; detail bars at the second and third (or at 55% / 75% without a header).
    let name_right = cols.get(1).map_or(w, |c| c.left - pad);
    let details: Vec<(i32, i32)> = match (cols.get(1), cols.get(2)) {
        (Some(a), Some(b)) => vec![(a.left + pad, a.right - pad), (b.left + pad, b.right - pad)],
        _ => vec![(w * 55 / 100, w), (w * 75 / 100, w)],
    };
    for r in 0..8 {
        let cy = top + r * row + row / 2;
        let bar = |left: i32, right: i32, h: i32| RECT { left, top: cy - h / 2, right: right.max(left), bottom: cy + h / 2 };
        theme::fill_round(hdc, &bar(pad, pad + sc(16), sc(16)), color, sc(4));
        let x = pad + sc(28);
        theme::fill_round(hdc, &bar(x, (x + w * 40 / 100).min(name_right), sc(10)), color, sc(10));
        for &(left, right) in &details {
            theme::fill_round(hdc, &bar(left, (left + w * 15 / 100).min(right), sc(10)), color, sc(10));
        }
    }
}

/// Keys of the list, and the keys the Home page passes on.
pub(crate) unsafe fn on_key(app: *mut App, key: VIRTUAL_KEY) {
    let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
    let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
    match key {
        // Windows 11 Explorer: Backspace goes Back (Alt+Up goes up).
        VK_BACK => nav::go_back(app),
        VK_F2 if matches!(source(app), Some(Source::Shell(_) | Source::Search(_) | Source::Archive(_))) => begin_rename(app),
        VK_DELETE => verb_on_selection(app, "delete", shift),
        VK_C if ctrl && !shift => verb_on_selection(app, "copy", false),
        VK_X if ctrl => verb_on_selection(app, "cut", false),
        VK_V if ctrl => verb_on_background(app, "paste"),
        VK_A if ctrl => set_item_state((*app).list, usize::MAX, LVIS_SELECTED, LVIS_SELECTED),
        _ => {}
    }
}

/// File types Explorer offers "Run as administrator" on (their classes carry the runas verb).
fn runs_elevated(name: &str) -> bool {
    std::path::Path::new(name).extension().and_then(|e| e.to_str()).is_some_and(|e| ["exe", "bat", "cmd", "com", "msi", "msc"].iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Ctrl+Shift+Enter and the menu entry: the shell's runas verb on the one selected program; beeps otherwise.
pub(crate) unsafe fn run_as_admin(app: *mut App) {
    let rows = selected_indices((*app).list);
    let program = matches!(rows[..], [i] if listing(app).and_then(|l| l.items.get(i)).is_some_and(|it| runs_elevated(&it.file_name())));
    if !program {
        let _ = MessageBeep(MB_OK);
        return;
    }
    // A "No" at the consent prompt comes back as an error: nothing to report.
    if let Some(m) = selection_menu(app, &rows) {
        let _ = m.invoke_verb((*app).hwnd, "runas", false);
    }
}

/// Explorer's "Copy as path" text: each path in double quotes, one per line.
/// False for a name that would end the quotes or start a new line when pasted into a terminal; a real file system
/// path has neither, a folder inside an archive can.
fn pastable(path: &str) -> bool {
    !path.chars().any(|c| c == '"' || c.is_control())
}

fn quoted_paths(paths: &[String]) -> String {
    paths.iter().map(|p| format!("\"{p}\"")).collect::<Vec<_>>().join("\r\n")
}

/// Ctrl+Shift+C: the selected rows' paths (the folder's own when nothing is selected) as text on the clipboard.
pub(crate) unsafe fn copy_as_path(app: *mut App) {
    let rows = selected_indices((*app).list);
    let names: Vec<String> = match source(app) {
        Some(Source::Shell(l)) if !rows.is_empty() => rows.iter().filter_map(|&i| l.items.get(i)).map(|it| it.parsing_name()).collect(),
        Some(Source::Search(s)) if !rows.is_empty() => rows.iter().filter_map(|&i| s.rows.get(i)).map(|r| r.item.parsing_name()).collect(),
        Some(src) => vec![src.parsing_name()],
        None => Vec::new(),
    };
    // shortcut: Home's recent files without a path-shaped parsing name are skipped (beep); no core accessor for SIGDN_FILESYSPATH.
    let paths: Vec<String> = names.into_iter().filter(|p| ui::is_fs_path(p) && pastable(p)).collect();
    if paths.is_empty() || dragdrop::set_clipboard_text((*app).hwnd, &quoted_paths(&paths)).is_err() {
        let _ = MessageBeep(MB_OK);
    }
}

/// Ctrl+Num+: every column of the details view to the width of its widest text, then the layout is saved.
pub(crate) unsafe fn fit_columns(app: *mut App) {
    if (*app).views.view != views::View::Details || !IsWindowVisible((*app).list).as_bool() {
        return;
    }
    let header = HWND(SendMessageW((*app).list, LVM_GETHEADER, None, None).0 as *mut c_void);
    let n = SendMessageW(header, HDM_GETITEMCOUNT, None, None).0.max(0) as usize;
    for i in 0..n {
        SendMessageW((*app).list, LVM_SETCOLUMNWIDTH, Some(WPARAM(i)), Some(LPARAM(LVSCW_AUTOSIZE as isize)));
    }
    SetTimer(Some((*app).hwnd), crate::TIMER_COLUMNS, 100, None);
}

/// Esc: a cut shown in this folder stops being a cut. A cut made elsewhere is left alone.
pub(crate) unsafe fn cancel_cut(app: *mut App) {
    // The cached set may be older than the clipboard: never empty something copied elsewhere a moment ago.
    refresh_cut(app);
    let shown = listing(app).is_some_and(|l| (0..l.items.len()).any(|i| is_cut(app, i)));
    if shown {
        dragdrop::clear_clipboard((*app).hwnd);
        refresh_cut(app);
    }
}

/// The shell context menu for rows of the active tab: shell items, or search results (from one folder or several).
/// Copy and Cut in a file system folder, without the shell's verb (`dragdrop::to_clipboard`). False when it does not
/// apply or failed: the caller runs the verb instead.
unsafe fn to_clipboard(app: *mut App, rows: &[usize], cut: bool) -> bool {
    let Some(Source::Shell(l)) = source(app) else { return false };
    let items: Vec<&ShellItem> = rows.iter().filter_map(|&i| l.items.get(i)).collect();
    !items.is_empty() && std::path::Path::new(&l.folder.parsing_name).is_absolute() && dragdrop::to_clipboard(&l.folder, &items, (*app).hwnd, cut).is_ok()
}

pub(crate) unsafe fn selection_menu(app: *mut App, rows: &[usize]) -> Option<ContextMenu> {
    match source(app)? {
        Source::Shell(l) => {
            let items: Vec<&ShellItem> = rows.iter().filter_map(|&i| l.items.get(i)).collect();
            if items.is_empty() {
                return None;
            }
            ContextMenu::for_items(&l.folder, &items, (*app).hwnd).ok()
        }
        Source::Search(_) => source::search_menu(app, rows),
        _ => None,
    }
}

/// False when a selected shell item cannot be deleted, such as Home's OneDrive cloud recent files: the shell's
/// delete verb fails on them with 0x80004002. Home reports no SFGAO_CANDELETE even for its local files, which do
/// delete, so a file system item counts as deletable. A file system folder (its parsing name is a path) is not
/// checked: Ctrl+A in a huge folder would ask every item.
pub(crate) unsafe fn can_delete(app: *mut App, rows: &[usize]) -> bool {
    use windows::Win32::System::SystemServices::{SFGAO_CANDELETE, SFGAO_FILESYSTEM};
    let Some(Source::Shell(l)) = source(app) else { return true };
    if std::path::Path::new(&l.folder.parsing_name).is_absolute() {
        return true;
    }
    let mask = SFGAO_CANDELETE.0 | SFGAO_FILESYSTEM.0;
    rows.iter().filter_map(|&i| l.items.get(i)).all(|it| it.attributes(mask) & mask != 0)
}

/// File system paths of the selected rows, or None when a row is not a plain file or folder: a shell listing of a
/// real directory other than Home (one check, not one per row), or search results with drive or UNC paths.
unsafe fn selected_paths(app: *mut App, rows: &[usize]) -> Option<Vec<String>> {
    let paths: Vec<String> = match source(app)? {
        Source::Shell(l) if !nav::is_home(l) && std::path::Path::new(&l.folder.parsing_name).is_dir() => {
            rows.iter().filter_map(|&i| l.items.get(i)).map(|it| it.parsing_name()).collect()
        }
        Source::Search(s) => rows.iter().filter_map(|&i| s.rows.get(i)).map(|r| r.item.parsing_name()).filter(|p| ui::is_fs_path(p)).collect(),
        _ => return None,
    };
    (paths.len() == rows.len() && !paths.is_empty()).then_some(paths)
}

pub(crate) unsafe fn verb_on_selection(app: *mut App, verb: &str, shift: bool) {
    let hwnd = (*app).hwnd;
    let rows = selected_indices((*app).list);
    if verb == "delete" && !can_delete(app, &rows) {
        let _ = MessageBeep(MB_OK);
        return;
    }
    // Every delete of the selection (key, menu, command bar, palette) comes here: the reload after it selects the next row.
    let deleting = (verb == "delete").then(|| nav::delete_starts(app, &rows));
    let picks = if verb == "delete" { source::search_picks(app, &rows) } else { Vec::new() };
    // To the Recycle Bin through IFileOperation, so Ctrl+Z can restore it; Shift+Delete, Home and virtual items keep the shell's verb.
    if verb == "delete" && !shift
        && let Some(paths) = selected_paths(app, &rows) {
            let done = undo::recycle(hwnd, &paths, false);
            (*app).undo.record(Op::Recycled(done));
            if let Some(id) = &deleting {
                nav::delete_over(app, id);
            }
            source::prune_search(app, &picks);
            return;
        }
    if matches!(verb, "copy" | "cut") && to_clipboard(app, &rows, verb == "cut") {
        return;
    }
    if let Some(menu) = selection_menu(app, &rows) {
        let _ = menu.invoke_verb(hwnd, verb, shift);
        // The verb has finished when it returns (invoke_verb runs a delete with CMIC_MASK_NOASYNC); a no-op outside
        // search results.
        if verb == "delete" {
            source::prune_search(app, &picks);
        }
    }
    if let Some(id) = &deleting {
        nav::delete_over(app, id);
    }
}

pub(crate) unsafe fn verb_on_background(app: *mut App, verb: &str) {
    let hwnd = (*app).hwnd;
    let (menu, pairs) = {
        let Some(l) = listing(app) else { return };
        // Read before the shell runs it: only destination names free now.
        let pairs = if verb == "paste" { plan_transfer(&dragdrop::clipboard_paths(), &l.folder.parsing_name) } else { Vec::new() };
        let builtin = (verb == "paste" && std::path::Path::new(&l.folder.parsing_name).is_absolute()).then(|| ContextMenu::builtin(&l.folder, &[], hwnd));
        (builtin.unwrap_or_else(|| ContextMenu::for_background(&l.folder, hwnd)), pairs)
    };
    if let Ok(menu) = menu
        && menu.invoke_verb(hwnd, verb, false).is_ok() {
            (*app).undo.record(Op::Transfer(pairs));
        }
}

/// Paste shortcut: the shell makes and names (localized, numbered) a .lnk per clipboard item in this folder; in a
/// file system folder the new ones are found by difference and recorded for Ctrl+Z. Beeps when nothing was pasted.
pub(crate) unsafe fn paste_shortcut(app: *mut App) {
    let made = {
        let Some(l) = listing(app) else { return };
        let dir = std::path::Path::new(&l.folder.parsing_name).is_absolute().then(|| l.folder.parsing_name.clone());
        let before = dir.as_deref().map(shortcut_names);
        dragdrop::paste_link(&l.folder, (*app).hwnd).map(|()| dir.zip(before).map(|(d, b)| new_shortcuts(&d, &b)))
    };
    match made {
        Ok(Some(made)) => (*app).undo.record(Op::created(made)),
        Ok(None) => {}
        Err(_) => {
            let _ = MessageBeep(MB_OK);
        }
    }
}

/// Ctrl+Z / Ctrl+Y: reverses the newest entry of the log; in the address bar and the search box Ctrl+Z undoes
/// their typing instead (the accelerator table reaches them).
pub(crate) unsafe fn undo_step(app: *mut App, redo: bool) {
    let focus = GetFocus();
    if focus == (*app).addr || focus == (*app).search {
        if !redo {
            SendMessageW(focus, WM_UNDO, None, None);
        }
        return;
    }
    let Some(entry) = (*app).undo.take(redo) else {
        let _ = MessageBeep(MB_OK);
        return;
    };
    // The shell's progress UI pumps messages: no borrow of the log is held across it.
    let r = undo::reverse_all((*app).hwnd, entry);
    let any = !r.done.is_empty();
    // What the shell did not reach (it cancels the items after one it refused) stays for the next Ctrl+Z.
    (*app).undo.finish(redo, r);
    if any {
        SetTimer(Some((*app).hwnd), TIMER_REFRESH, 150, None);
    } else {
        let _ = MessageBeep(MB_OK);
    }
}

/// F2: edits the focused item's name in place (in the list, or on the Home page when it shows).
pub(crate) unsafe fn begin_rename(app: *mut App) {
    let list = (*app).list;
    let i = SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_FOCUSED as isize))).0;
    if i >= 0 && matches!(source(app), Some(Source::Archive(_))) {
        // unrar only reads archives and rexplorer never rewrites them; say so instead of doing nothing.
        let t = HSTRING::from(tr("err.archive_readonly"));
        MessageBoxW(Some((*app).hwnd), PCWSTR(t.as_ptr()), w!("rexplorer"), crate::rtl::message_box() | MB_OK | MB_ICONINFORMATION);
        return;
    }
    if i < 0 || !matches!(source(app), Some(Source::Shell(_) | Source::Search(_))) {
        return;
    }
    if IsWindowVisible((*app).home).as_bool() {
        home::begin_rename((*app).home, i as usize);
    } else {
        let _ = SetFocus(Some(list));
        SendMessageW(list, LVM_EDITLABELW, Some(WPARAM(i as usize)), None);
    }
}

/// Ctrl+I: flips every row's selection.
pub(crate) unsafe fn invert_selection(app: *mut App) {
    let list = (*app).list;
    if IsWindowVisible((*app).home).as_bool() {
        return;
    }
    // Select all in one message, then clear the old selection: messages per selected row, not per row (100k rows).
    let was = selected_indices(list);
    SendMessageW(list, WM_SETREDRAW, Some(WPARAM(0)), None);
    set_item_state(list, usize::MAX, LVIS_SELECTED, LVIS_SELECTED);
    for i in was {
        set_item_state(list, i, LIST_VIEW_ITEM_STATE_FLAGS(0), LVIS_SELECTED);
    }
    SendMessageW(list, WM_SETREDRAW, Some(WPARAM(1)), None);
    let _ = InvalidateRect(Some(list), None, true);
}

/// Row `i` of the list when it can be renamed: shell items and search results.
unsafe fn editable_item(app: *mut App, i: usize) -> Option<ShellItem> {
    match source(app) {
        Some(Source::Shell(l)) => l.items.get(i).cloned(),
        Some(Source::Search(_)) => source::search_item(app, i),
        _ => None,
    }
}

unsafe fn on_end_edit(app: *mut App, item: &LVITEMW) {
    let opened = (*app).renaming.take();
    if item.pszText.is_null() {
        return;
    }
    // The commit names a row by position; it goes to the item the edit was opened on, wherever it is now, and is
    // dropped when that item is gone (the folder changed under the edit).
    let n = source(app).map_or(0, |s| s.len());
    let row = opened.and_then(|o| edit_row(&o, item.iItem as usize, n, |i| editable_item(app, i).map(|it| it.parsing_name())));
    if let Some(i) = row {
        rename_item(app, i, &item.pszText.to_string().unwrap_or_default(), true);
    }
}

/// The row now holding the item a label edit was opened on (`opened`, a parsing name): `index` when it still does,
/// else wherever that item is among the `n` rows, None when it is gone.
pub(crate) fn edit_row(opened: &str, index: usize, n: usize, name: impl Fn(usize) -> Option<String>) -> Option<usize> {
    // Nothing remembered: an item without a parsing name would match it, and the commit would rename that item.
    if opened.is_empty() {
        return None;
    }
    if name(index).as_deref() == Some(opened) {
        return Some(index);
    }
    (0..n).find(|&i| name(i).as_deref() == Some(opened))
}

/// Renames item `index` of the active listing to `name` (the shell shows its own error UI).
pub(crate) unsafe fn rename_item(app: *mut App, index: usize, name: &str, cell: bool) {
    // Work on a copy: the shell may pump messages while renaming, and a refresh could replace the listing.
    let target = match source(app) {
        Some(Source::Search(_)) => source::search_item(app, index),
        _ => listing(app).and_then(|l| l.items.get(index).cloned()),
    };
    if let Some(mut target) = target {
        // Home tiles edit the display name; the list edits its Name column, which Show file extensions may change.
        let (shown, suffix) = if cell { source::name_cell(&target) } else { (target.display_name(), None) };
        if !name.is_empty() && name != shown {
            let before = target.parsing_name();
            let done = match suffix {
                Some(ext) => target.rename((*app).hwnd, &format!("{name}{ext}"), true),
                None => target.rename((*app).hwnd, name, false),
            };
            if done.is_ok()
                && let Some(op) = Op::rename(before.clone(), target.parsing_name()) {
                    (*app).undo.record(op);
                }
            // The rename's error and conflict dialogs pump messages: the listing may have changed, so the row is found
            // again by the old name, and nothing is written back when it is gone.
            // Search results are updated in place: a refresh would run the search again.
            if let Some(Source::Search(s)) = &mut cur(app).source {
                let row = edit_row(&before, index, s.rows.len(), |i| s.rows.get(i).map(|r| r.item.parsing_name()));
                if let (Ok(()), Some(i)) = (done, row) {
                    let r = &mut s.rows[i];
                    r.name = source::name_cell(&target).0;
                    r.item = target;
                    s.icons[i] = ICON_UNKNOWN;
                    SendMessageW((*app).list, LVM_REDRAWITEMS, Some(WPARAM(i)), Some(LPARAM(i as isize)));
                }
                return;
            }
            // The row asks the shell for its text again instead of showing the old name from the cache. The reload
            // waits while a Tab-chained edit is open, so the row takes the renamed item now (Shift+Tab edits it).
            if let Some(Source::Shell(l)) = &mut cur(app).source
                && let Some(i) = edit_row(&before, index, l.items.len(), |i| l.items.get(i).map(|it| it.parsing_name()))
            {
                if let Some(row) = l.cells.get_mut(i) {
                    row.take();
                }
                if done.is_ok() {
                    l.items[i] = target;
                }
            }
            SetTimer(Some((*app).hwnd), TIMER_REFRESH, 150, None);
        }
    }
}

/// Selected and hot rows (and icon tiles) as rounded pills: the pill is painted first, then the control draws the
/// item as unselected over it with a transparent text background.
unsafe fn draw_item(app: *mut App, cd: &mut NMLVCUSTOMDRAW) -> u32 {
    // A group's stages carry its id in dwItemSpec, not a row.
    if cd.dwItemType == LVCDI_GROUP {
        return if cd.nmcd.dwDrawStage == CDDS_PREPAINT { draw_group(app, cd) } else { CDRF_DODEFAULT };
    }
    match cd.nmcd.dwDrawStage {
        CDDS_PREPAINT => {
            // The Explorer style draws faint vertical lines between details columns (LVP_COLUMNDETAIL) before the
            // items; painting the whole client over them leaves a clean card.
            let b = CreateSolidBrush(COLORREF((*app).theme.surface));
            FillRect(cd.nmcd.hdc, &cd.nmcd.rc, b);
            let _ = DeleteObject(b.into());
            if (*app).skeleton {
                draw_skeleton(app, cd.nmcd.hdc);
            }
            CDRF_NOTIFYITEMDRAW | CDRF_NOTIFYPOSTPAINT
        }
        CDDS_POSTPAINT => {
            let list = (*app).list;
            theme::paint_card_corners(list, list, cd.nmcd.hdc, (*app).theme.background);
            CDRF_DODEFAULT
        }
        CDDS_ITEMPREPAINT => {
            let (list, t) = ((*app).list, &(*app).theme);
            let i = cd.nmcd.dwItemSpec;
            let state = SendMessageW(list, LVM_GETITEMSTATE, Some(WPARAM(i)), Some(LPARAM((LVIS_SELECTED.0 | LVIS_FOCUSED.0) as isize))).0 as u32;
            let selected = state & LVIS_SELECTED.0 != 0;
            let hot = cd.nmcd.uItemState.0 & CDIS_HOT.0 != 0;
            // The keyboard focus is drawn here, since CDIS_FOCUS is stripped below. Contrast: a rectangle. Other
            // themes: an outline on a focused row that is not selected (Ctrl+arrows, Ctrl+Space), once the keyboard
            // is in use (the system's focus-cue state, as for every other control).
            let focused = state & LVIS_FOCUSED.0 != 0 && GetFocus() == list;
            let focus = t.contrast.is_some() && focused;
            let ring = t.contrast.is_none() && focused && !selected && SendMessageW(list, WM_QUERYUISTATE, None, None).0 as u32 & UISF_HIDEFOCUS == 0;
            if selected || hot || focus || ring {
                // nmcd.rc is the item's rect; asking the list for it here (LVM_GETITEMRECT) stops it drawing the item.
                let r = cd.nmcd.rc;
                let mut client = RECT::default();
                let _ = GetClientRect(list, &mut client);
                let dpi = GetDpiForWindow(list) as i32;
                let (m, v) = (4 * dpi / 96, dpi / 96);
                // A details row spans the visible width, whatever the columns add up to.
                let details = (*app).views.view == views::View::Details;
                let (left, right) = if details { (client.left, client.right) } else { (r.left, r.right) };
                let pill = RECT { left: left + m, top: r.top + v, right: right - m, bottom: r.bottom - v };
                if selected || hot {
                    theme::fill_round(cd.nmcd.hdc, &pill, if selected { t.selection } else { theme::hover(t) }, 6 * dpi / 96);
                }
                if hot && !selected {
                    theme::hot_outline(cd.nmcd.hdc, &pill, t, 6 * dpi / 96, dpi as u32);
                }
                if focus {
                    let _ = windows::Win32::Graphics::Gdi::DrawFocusRect(cd.nmcd.hdc, &pill);
                }
                if ring {
                    theme::outline_round(cd.nmcd.hdc, &pill, t.text, 6 * dpi / 96, (dpi / 96).max(1));
                }
            }
            cd.nmcd.uItemState.0 &= !(CDIS_SELECTED.0 | CDIS_HOT.0 | CDIS_FOCUS.0);
            let dim = is_cut(app, i);
            (*app).dim_row = dim.then_some(i);
            let ink = if dim { t.secondary_text } else { t.text };
            cd.clrText = COLORREF(if selected { theme::on_selection(t, ink) } else { ink });
            cd.clrTextBk = COLORREF(CLR_NONE as u32);
            CDRF_NEWFONT | CDRF_NOTIFYSUBITEMDRAW
        }
        // Single-click underlining applies to item names only, as in Explorer: other columns get the plain font.
        s if s.0 == CDDS_ITEMPREPAINT.0 | CDDS_SUBITEM.0 => {
            cd.nmcd.uItemState.0 &= !(CDIS_SELECTED.0 | CDIS_HOT.0 | CDIS_FOCUS.0);
            let t = &(*app).theme;
            let ink = if (*app).dim_row == Some(cd.nmcd.dwItemSpec) { t.secondary_text } else { t.text };
            // Only a contrast theme changes the selected row's text: normal themes skip the query per subitem.
            let selected = t.contrast.is_some() && SendMessageW((*app).list, LVM_GETITEMSTATE, Some(WPARAM(cd.nmcd.dwItemSpec)), Some(LPARAM(LVIS_SELECTED.0 as isize))).0 != 0;
            cd.clrText = COLORREF(if selected { theme::on_selection(t, ink) } else { ink });
            cd.clrTextBk = COLORREF(CLR_NONE as u32);
            if cd.iSubItem > 0 {
                let font = SendMessageW((*app).list, WM_GETFONT, None, None).0;
                windows::Win32::Graphics::Gdi::SelectObject(cd.nmcd.hdc, windows::Win32::Graphics::Gdi::HGDIOBJ(font as *mut c_void));
            }
            CDRF_NEWFONT
        }
        _ => CDRF_DODEFAULT,
    }
}

/// A date group's header band (rcText): its label in the theme's text colour over a thin rule. The system draws
/// it for a light background whatever the theme.
unsafe fn draw_group(app: *mut App, cd: &NMLVCUSTOMDRAW) -> u32 {
    let Some((label, _)) = group_runs(app).get(cd.nmcd.dwItemSpec) else { return CDRF_DODEFAULT };
    let (list, t, dc, r) = ((*app).list, &(*app).theme, cd.nmcd.hdc, cd.rcText);
    let dpi = GetDpiForWindow(list) as i32;
    let pad = 12 * dpi / 96;
    let font = SendMessageW(list, WM_GETFONT, None, None).0;
    let old = SelectObject(dc, windows::Win32::Graphics::Gdi::HGDIOBJ(font as *mut c_void));
    SetBkMode(dc, TRANSPARENT);
    SetTextColor(dc, COLORREF(t.text));
    crate::theme::draw_text(dc, label, &mut RECT { left: r.left + pad, ..r }, DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX);
    SelectObject(dc, old);
    let b = CreateSolidBrush(COLORREF(t.border));
    FillRect(dc, &RECT { left: r.left + pad, top: r.bottom - dpi / 96, right: r.right - pad, bottom: r.bottom }, b);
    let _ = DeleteObject(b.into());
    CDRF_SKIPDEFAULT
}

/// The list's own timer that restores full-row select during a marquee (an id the ListView's own timers do not use).
const MARQUEE_TIMER: usize = 0x5258;

/// The header is 32 px high (96 DPI) whatever the font asks for.
/// Window property on the header: the hovered column + 1 (0 none). The header never reports CDIS_HOT to a
/// custom draw that skips its default painting, so the hover is tracked here (#79).
const HEADER_HOT: PCWSTR = w!("RexHeaderHot");

/// The header item a hit test names as hovered: + 1, or 0 for none (empty space right of the last column).
fn hot_slot(flags: u32, item: i32) -> usize {
    use windows::Win32::UI::Controls::HHT_NOWHERE;
    if item < 0 || flags & HHT_NOWHERE.0 != 0 { 0 } else { item as usize + 1 }
}

unsafe fn set_header_hot(hwnd: HWND, slot: usize) {
    use windows::Win32::UI::WindowsAndMessaging::{GetPropW, SetPropW};
    if GetPropW(hwnd, HEADER_HOT).0 as usize != slot {
        let _ = SetPropW(hwnd, HEADER_HOT, Some(windows::Win32::Foundation::HANDLE(slot as *mut c_void)));
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

pub(crate) unsafe extern "system" fn header_subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    use windows::Win32::UI::Controls::{HDHITTESTINFO, HDM_HITTEST, WM_MOUSELEAVE};
    use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
    use windows::Win32::UI::WindowsAndMessaging::RemovePropW;
    match msg {
        WM_MOUSEMOVE => {
            let mut ht = HDHITTESTINFO { pt: POINT { x: lparam.0 as i16 as i32, y: (lparam.0 >> 16) as i16 as i32 }, ..Default::default() };
            SendMessageW(hwnd, HDM_HITTEST, None, Some(LPARAM(&mut ht as *mut _ as isize)));
            set_header_hot(hwnd, hot_slot(ht.flags.0, ht.iItem));
            let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            let _ = TrackMouseEvent(&mut tme);
        }
        WM_MOUSELEAVE => set_header_hot(hwnd, 0),
        WM_NCDESTROY => {
            let _ = RemovePropW(hwnd, HEADER_HOT);
        }
        _ => {}
    }
    let r = DefSubclassProc(hwnd, msg, wparam, lparam);
    if msg == HDM_LAYOUT {
        let l = &*(lparam.0 as *const HDLAYOUT);
        let h = 32 * GetDpiForWindow(hwnd) as i32 / 96;
        (*l.pwpos).cy = h;
        (*l.prc).top = (*l.pwpos).y + h;
    }
    r
}

/// Middle-click support (the ListView ignores the middle button), the flat header and the card's rounded corners.
pub(crate) unsafe extern "system" fn subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, data: usize) -> LRESULT {
    // Only the header's custom draw; the label tooltip sends NM_CUSTOMDRAW too.
    if msg == WM_NOTIFY && (*(lparam.0 as *const NMHDR)).code == NM_CUSTOMDRAW
        && (*(lparam.0 as *const NMHDR)).hwndFrom.0 == SendMessageW(hwnd, LVM_GETHEADER, None, None).0 as *mut _
    {
        return LRESULT(draw_header(&(*(data as *mut App)).theme, &*(lparam.0 as *const NMCUSTOMDRAW)) as isize);
    }
    // The user finished resizing or dragging a column: read the header a moment later, once it has applied the change.
    if msg == WM_NOTIFY {
        let nm = &*(lparam.0 as *const NMHDR);
        if matches!(nm.code, HDN_ENDTRACKW | HDN_ENDDRAG | HDN_DIVIDERDBLCLICKW) && nm.hwndFrom.0 as usize == SendMessageW(hwnd, LVM_GETHEADER, None, None).0 as usize {
            SetTimer(Some((*(data as *mut App)).hwnd), crate::TIMER_COLUMNS, 100, None);
        }
    }
    if msg == WM_MBUTTONUP {
        let mut ht = LVHITTESTINFO { pt: POINT { x: lparam.0 as i16 as i32, y: (lparam.0 >> 16) as i16 as i32 }, ..Default::default() };
        let i = SendMessageW(hwnd, LVM_HITTEST, None, Some(LPARAM(&mut ht as *mut _ as isize))).0;
        if i >= 0 {
            open_in_background(data as *mut App, i as usize);
        }
        return LRESULT(0);
    }
    let app = data as *mut App;
    // A right click on the header: the column chooser (the header is the wParam, the point is in screen coordinates).
    if msg == WM_CONTEXTMENU && wparam.0 != 0 && wparam.0 == SendMessageW(hwnd, LVM_GETHEADER, None, None).0 as usize {
        crate::commands::column_menu(app, lparam.0 as i16 as i32, (lparam.0 >> 16) as i16 as i32);
        return LRESULT(0);
    }
    // Shift+F10 and the Menu key (a right click arrives as NM_RCLICK instead).
    if msg == WM_CONTEXTMENU && lparam.0 as i32 == -1 {
        let focus = SendMessageW(hwnd, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_FOCUSED as isize))).0;
        let on_item = focus >= 0 && SendMessageW(hwnd, LVM_GETITEMSTATE, Some(WPARAM(focus as usize)), Some(LPARAM(LVIS_SELECTED.0 as isize))).0 != 0;
        show_menu(app, on_item, Some(menu_point(hwnd, on_item.then_some(focus as usize))));
        return LRESULT(0);
    }
    // Explorer box-selects when a drag starts on an unselected row's other columns (a selected row drags from any
    // column); a full-row ListView would drag the item. So such a press reaches the ListView with full-row select
    // off: to it the press is on blank space and a drag draws a marquee. A timer that fires inside the marquee's
    // loop turns full-row back on, else the marquee would only take rows whose name it touches. A press released
    // without a drag runs again with full-row on, as a click on the row. shortcut: a drag from blank space right of
    // a name drags the item, unlike Explorer; measure the name's text width if that matters.
    let full = LVS_EX_FULLROWSELECT as usize;
    if msg == WM_TIMER && wparam.0 == MARQUEE_TIMER {
        let _ = KillTimer(Some(hwnd), MARQUEE_TIMER);
        SendMessageW(hwnd, LVM_SETEXTENDEDLISTVIEWSTYLE, Some(WPARAM(full)), Some(LPARAM(full as isize)));
        return LRESULT(0);
    }
    if msg == WM_LBUTTONDOWN && (*app).views.view == views::View::Details {
        let pt = POINT { x: lparam.0 as i16 as i32, y: (lparam.0 >> 16) as i16 as i32 };
        let mut ht = LVHITTESTINFO { pt, ..Default::default() };
        SendMessageW(hwnd, LVM_SUBITEMHITTEST, None, Some(LPARAM(&mut ht as *mut _ as isize)));
        let selected = ht.iItem >= 0 && SendMessageW(hwnd, LVM_GETITEMSTATE, Some(WPARAM(ht.iItem as usize)), Some(LPARAM(LVIS_SELECTED.0 as isize))).0 != 0;
        if ht.iItem >= 0 && ht.iSubItem > 0 && !selected {
            SendMessageW(hwnd, LVM_SETEXTENDEDLISTVIEWSTYLE, Some(WPARAM(full)), Some(LPARAM(0)));
            SetTimer(Some(hwnd), MARQUEE_TIMER, 1, None);
            let r = DefSubclassProc(hwnd, msg, wparam, lparam);
            let _ = KillTimer(Some(hwnd), MARQUEE_TIMER);
            SendMessageW(hwnd, LVM_SETEXTENDEDLISTVIEWSTYLE, Some(WPARAM(full)), Some(LPARAM(full as isize)));
            // Where the button came up: the last message the ListView's mouse loop took.
            let up = GetMessagePos();
            let mut down = pt;
            let _ = ClientToScreen(hwnd, &mut down);
            let dragged = (up as i16 as i32 - down.x).abs() > GetSystemMetrics(SM_CXDRAG)
                || ((up >> 16) as i16 as i32 - down.y).abs() > GetSystemMetrics(SM_CYDRAG);
            if dragged {
                return r;
            }
            // The ListView's click handling waits for a button-up; the first pass took the real one.
            let mut queued = MSG::default();
            if !PeekMessageW(&mut queued, Some(hwnd), WM_LBUTTONUP, WM_LBUTTONUP, PM_NOREMOVE).as_bool() {
                let _ = PostMessageW(Some(hwnd), WM_LBUTTONUP, WPARAM(wparam.0 & !(windows::Win32::System::SystemServices::MK_LBUTTON.0 as usize)), lparam);
            }
            return DefSubclassProc(hwnd, msg, wparam, lparam);
        }
    }
    if msg == WM_MOUSEMOVE {
        if !(*app).panes.hover_tracking {
            let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            (*app).panes.hover_tracking = TrackMouseEvent(&mut tme).is_ok();
        }
        let mut ht = LVHITTESTINFO { pt: POINT { x: lparam.0 as i16 as i32, y: (lparam.0 >> 16) as i16 as i32 }, ..Default::default() };
        let i = SendMessageW(hwnd, LVM_HITTEST, None, Some(LPARAM(&mut ht as *mut _ as isize))).0;
        crate::panes::set_hover(app, usize::try_from(i).ok());
    } else if msg == WM_MOUSELEAVE {
        (*app).panes.hover_tracking = false;
        crate::panes::stop_hover(app);
    } else if msg == WM_MOUSEWHEEL {
        if wparam.0 & windows::Win32::System::SystemServices::MK_CONTROL.0 as usize != 0 {
            views::on_ctrl_wheel(app, (wparam.0 >> 16) as i16);
            return LRESULT(0);
        }
        crate::panes::stop_hover(app);
    }
    let r = DefSubclassProc(hwnd, msg, wparam, lparam);
    if msg == WM_SIZE && (*app).views.view == views::View::Content {
        views::set_tile_info(app); // the tile is as wide as the list
    }
    theme::card_message(hwnd, msg, (*(data as *mut App)).theme.background);
    r
}

/// A flat header in both themes (the DarkMode header themes are ignored on this Windows build): surface fill,
/// secondary-text labels, no dividers or bottom line; a hovered column gets a rounded hover fill.
unsafe fn draw_header(t: &Theme, nm: &NMCUSTOMDRAW) -> u32 {
    let fill = |r: &RECT, c: u32| {
        let b = CreateSolidBrush(COLORREF(c));
        FillRect(nm.hdc, r, b);
        let _ = DeleteObject(b.into());
    };
    let header = nm.hdr.hwndFrom;
    let dpi = GetDpiForWindow(header) as i32;
    match nm.dwDrawStage {
        CDDS_PREPAINT => {
            fill(&nm.rc, t.surface);
            CDRF_NOTIFYITEMDRAW | CDRF_NOTIFYPOSTPAINT
        }
        CDDS_ITEMPREPAINT => {
            let r = nm.rc;
            fill(&r, t.surface);
            let hot = windows::Win32::UI::WindowsAndMessaging::GetPropW(header, HEADER_HOT).0 as usize == nm.dwItemSpec + 1;
            if hot || nm.uItemState.0 & CDIS_HOT.0 != 0 {
                let m = 4 * dpi / 96;
                let hr = RECT { left: r.left + m, top: r.top + m, right: r.right - m, bottom: r.bottom - m };
                theme::fill_round(nm.hdc, &hr, theme::hover(t), 6 * dpi / 96);
                theme::hot_outline(nm.hdc, &hr, t, 6 * dpi / 96, dpi as u32);
            }
            let mut buf = [0u16; 260];
            let mut hi = HDITEMW { mask: HDI_TEXT | HDI_FORMAT, pszText: PWSTR(buf.as_mut_ptr()), cchTextMax: buf.len() as i32, ..Default::default() };
            SendMessageW(header, HDM_GETITEMW, Some(WPARAM(nm.dwItemSpec)), Some(LPARAM(&mut hi as *mut _ as isize)));
            let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
            SetBkMode(nm.hdc, TRANSPARENT);
            SetTextColor(nm.hdc, COLORREF(t.secondary_text));
            let p = 8 * dpi / 96;
            let mut tr = RECT { left: r.left + p, right: r.right - p, ..r };
            let align = if hi.fmt.0 & HDF_RIGHT.0 != 0 { DT_RIGHT } else { Default::default() };
            DrawTextW(nm.hdc, &mut buf[..len], &mut tr, crate::rtl::text(DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX | align));
            if hi.fmt.0 & (HDF_SORTUP.0 | HDF_SORTDOWN.0) != 0 {
                let mut glyph = [if hi.fmt.0 & HDF_SORTUP.0 != 0 { 0xE70E } else { 0xE70D }];
                let font = theme::icon_font(dpi as u32, 8);
                let old = SelectObject(nm.hdc, font.into());
                let mut gr = RECT { left: r.left, right: r.right, top: r.top, bottom: r.top + 12 * dpi / 96 };
                DrawTextW(nm.hdc, &mut glyph, &mut gr, DT_SINGLELINE | DT_CENTER | DT_NOPREFIX);
                SelectObject(nm.hdc, old);
                let _ = DeleteObject(font.into());
            }
            CDRF_SKIPDEFAULT
        }
        CDDS_POSTPAINT => {
            // The area right of the last column.
            let n = SendMessageW(header, HDM_GETITEMCOUNT, None, None).0.max(0) as usize;
            let mut right = 0;
            for i in 0..n {
                let mut r = RECT::default();
                SendMessageW(header, HDM_GETITEMRECT, Some(WPARAM(i)), Some(LPARAM(&mut r as *mut _ as isize)));
                right = right.max(r.right);
            }
            fill(&RECT { left: right, ..nm.rc }, t.surface);
            // The header covers the card's top corners.
            if let Ok(list) = GetParent(header) {
                theme::paint_card_corners(list, header, nm.hdc, t.background);
            }
            CDRF_DODEFAULT
        }
        _ => CDRF_DODEFAULT,
    }
}

pub(crate) unsafe fn selected_indices(list: HWND) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i: isize = -1;
    loop {
        i = SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(i as usize)), Some(LPARAM(LVNI_SELECTED as isize))).0;
        if i < 0 {
            break;
        }
        out.push(i as usize);
    }
    out
}

/// `index == usize::MAX` applies to every item.
pub(crate) unsafe fn set_item_state(list: HWND, index: usize, state: LIST_VIEW_ITEM_STATE_FLAGS, mask: LIST_VIEW_ITEM_STATE_FLAGS) {
    let lvi = LVITEMW { state, stateMask: mask, ..Default::default() };
    SendMessageW(list, LVM_SETITEMSTATE, Some(WPARAM(index)), Some(LPARAM(&lvi as *const _ as isize)));
}

/// The row Explorer selects after a delete: the one that took the first deleted row's place, or the last row.
pub(crate) fn row_after_delete(first_gone: usize, len: usize) -> Option<usize> {
    (len > 0).then(|| first_gone.min(len - 1))
}

/// Selects and focuses `row_after_delete`'s row (the list already holds the `len` rows left).
pub(crate) unsafe fn select_after_delete(list: HWND, first_gone: usize, len: usize) {
    if let Some(i) = row_after_delete(first_gone, len) {
        let both = LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0);
        set_item_state(list, i, both, both);
        SendMessageW(list, LVM_SETSELECTIONMARK, None, Some(LPARAM(i as isize)));
        SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(i)), Some(LPARAM(0)));
    }
}

unsafe fn copy_text(text: &str, dst: PWSTR, max: i32) {
    if dst.is_null() || max <= 0 {
        return;
    }
    let buf = std::slice::from_raw_parts_mut(dst.0, max as usize);
    let mut n = 0;
    for (d, c) in buf[..max as usize - 1].iter_mut().zip(text.encode_utf16()) {
        *d = c;
        n += 1;
    }
    buf[n] = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rename_commit_follows_the_item_not_the_row() {
        let rows = |names: &'static [&'static str]| move |i: usize| names.get(i).map(|s| s.to_string());
        assert_eq!(edit_row(r"C:\b\x.txt", 1, 3, rows(&[r"C:\b\a", r"C:\b\x.txt", r"C:\b\z"])), Some(1));
        assert_eq!(edit_row(r"C:\b\x.txt", 1, 3, rows(&[r"C:\b\new", r"C:\b\a", r"C:\b\x.txt"])), Some(2), "moved");
        // The window went Back to another folder: its row 1 is someone else's file.
        assert_eq!(edit_row(r"C:\b\x.txt", 1, 3, rows(&[r"C:\a\1", r"C:\a\2", r"C:\a\3"])), None);
        assert_eq!(edit_row(r"C:\b\x.txt", 5, 0, rows(&[])), None);
        assert_eq!(edit_row("", 1, 3, rows(&[r"C:\b\a", "", r"C:\b\z"])), None, "an empty name matches nothing");
    }

    #[test]
    fn after_a_delete_the_next_row_takes_the_selection() {
        assert_eq!(row_after_delete(2, 5), Some(2), "the row that moved up into the deleted one's place");
        assert_eq!(row_after_delete(4, 4), Some(3), "the last row was deleted: the new last row");
        assert_eq!(row_after_delete(9, 3), Some(2));
        assert_eq!(row_after_delete(0, 0), None, "nothing left");
    }

    #[test]
    fn new_shortcuts_lists_only_lnk_files_made_since() {
        let dir = std::env::temp_dir().join(format!("rexplorer-newlnk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Old.lnk"), b"x").unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let d = dir.to_string_lossy().into_owned();
        let before = shortcut_names(&d);
        assert_eq!(before.len(), 1);
        std::fs::write(dir.join("a.txt - Shortcut.lnk"), b"x").unwrap();
        std::fs::write(dir.join("b.txt"), b"x").unwrap();
        assert_eq!(new_shortcuts(&d, &before), [dir.join("a.txt - Shortcut.lnk").to_string_lossy().into_owned()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_drop_that_did_nothing_records_nothing() {
        use windows::Win32::System::Ole::{DROPEFFECT_COPY, DROPEFFECT_MOVE, DROPEFFECT_NONE};
        let dir = std::env::temp_dir().join(format!("rexplorer-dropentry-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("dst")).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let plan = || undo::plan_transfer(&[dir.join("a.txt").to_string_lossy().into_owned()], &dir.join("dst").to_string_lossy());
        assert_eq!(plan().len(), 1);
        assert_eq!(drop_entry(DROPEFFECT_NONE, plan(), Vec::new), Op::Transfer(Vec::new()));
        for e in [DROPEFFECT_COPY, DROPEFFECT_MOVE] {
            assert_eq!(drop_entry(e, plan(), Vec::new), Op::Transfer(plan()), "{e:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_archive_extractions_in_temp_skip_undo() {
        let t = r"c:\users\u\appdata\local\temp\";
        assert!(extracted_for_transfer(r"c:\users\u\appdata\local\temp\temp1_photos.zip\a.jpg", t));
        assert!(extracted_for_transfer(r"c:\users\u\appdata\local\temp\temp12_x.zip\d\b.txt", t));
        assert!(extracted_for_transfer(r"c:\users\u\appdata\local\temp\rexplorer-archive\a.txt", t));
        assert!(extracted_for_transfer(r"c:\users\u\appdata\local\temp\rexplorer-show-42\a.txt", t));
        assert!(!extracted_for_transfer(r"c:\users\u\appdata\local\temp\work\a.txt", t), "an ordinary folder in %TEMP%");
        assert!(!extracted_for_transfer(r"c:\users\u\appdata\local\temp\template_x\a.txt", t));
        assert!(!extracted_for_transfer(r"c:\users\u\appdata\local\temp\temp_\a.txt", t));
        assert!(!extracted_for_transfer(r"d:\data\temp1_a.zip\a.txt", t), "outside %TEMP%");
        assert!(!extracted_for_transfer(r"c:\users\u\appdata\local\tempx\temp1_a.zip\a.txt", r"c:\users\u\appdata\local\temp"));
    }

    #[test]
    fn header_hot_slot() {
        use windows::Win32::UI::Controls::{HHT_NOWHERE, HHT_ONDIVIDER, HHT_ONHEADER};
        assert_eq!(hot_slot(HHT_ONHEADER.0, 0), 1);
        assert_eq!(hot_slot(HHT_ONDIVIDER.0, 2), 3);
        assert_eq!(hot_slot(HHT_NOWHERE.0, -1), 0, "right of the last column");
        assert_eq!(hot_slot(HHT_ONHEADER.0, -1), 0);
    }

    unsafe extern "system" fn rename_parent(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_NOTIFY && (*(lparam.0 as *const NMHDR)).code == LVN_BEGINLABELEDITW {
            select_stem_later((*(lparam.0 as *const NMHDR)).hwndFrom, 6);
            return LRESULT(0);
        }
        // A virtual list, as the real one: the row's text comes from here.
        if msg == WM_NOTIFY && (*(lparam.0 as *const NMHDR)).code == LVN_GETDISPINFOW {
            let di = &mut *(lparam.0 as *mut NMLVDISPINFOW);
            if di.item.mask.contains(LVIF_TEXT) {
                copy_text("report.txt", di.item.pszText, di.item.cchTextMax);
            }
            return LRESULT(0);
        }
        // What on_notify would take for a right click on the list itself.
        if msg == WM_NOTIFY && (*(lparam.0 as *const NMHDR)).code == NM_RCLICK && GetParent((*(lparam.0 as *const NMHDR)).hwndFrom) == Ok(hwnd) {
            LIST_RCLICKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    /// A hidden parent with a virtual report-view ListView (one column, one row) in it.
    unsafe fn test_list(class: PCWSTR) -> (HWND, HWND) {
        let hinst = GetModuleHandleW(None).unwrap().into();
        let wc = WNDCLASSW { lpfnWndProc: Some(rename_parent), hInstance: hinst, lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        let parent = CreateWindowExW(WINDOW_EX_STYLE(0), class, PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 400, 300, None, None, Some(hinst), None).unwrap();
        let style = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(LVS_REPORT | LVS_EDITLABELS | LVS_OWNERDATA);
        let list = CreateWindowExW(WINDOW_EX_STYLE(0), WC_LISTVIEWW, PCWSTR::null(), style, 0, 0, 400, 300, Some(parent), None, Some(hinst), None).unwrap();
        let col = LVCOLUMNW { mask: LVCF_WIDTH, cx: 200, ..Default::default() };
        SendMessageW(list, LVM_INSERTCOLUMNW, Some(WPARAM(0)), Some(LPARAM(&col as *const _ as isize)));
        SendMessageW(list, LVM_SETITEMCOUNT, Some(WPARAM(1)), None);
        (parent, list)
    }

    static HEADER_MENUS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static LIST_RCLICKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    unsafe extern "system" fn count_header_menus(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _: usize, _: usize) -> LRESULT {
        if msg == WM_CONTEXTMENU && wparam.0 == SendMessageW(hwnd, LVM_GETHEADER, None, None).0 as usize {
            HEADER_MENUS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return LRESULT(0);
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }

    /// The column chooser hangs on this: a right click on the header reaches the list once, as WM_CONTEXTMENU.
    #[test]
    fn header_right_click_reaches_the_list_as_a_context_menu() {
        unsafe {
            let (parent, list) = test_list(w!("RexHeaderMenuTestParent"));
            let _ = SetWindowSubclass(list, Some(count_header_menus), 1, 0);
            let header = HWND(SendMessageW(list, LVM_GETHEADER, None, None).0 as *mut c_void);
            let at = LPARAM((10 << 16) | 20);
            SendMessageW(header, WM_RBUTTONDOWN, Some(WPARAM(0x2)), Some(at));
            SendMessageW(header, WM_RBUTTONUP, Some(WPARAM(0)), Some(at));
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            assert_eq!(HEADER_MENUS.load(std::sync::atomic::Ordering::Relaxed), 1);
            assert_eq!(LIST_RCLICKS.load(std::sync::atomic::Ordering::Relaxed), 0, "no item menu as well");
            let _ = DestroyWindow(parent);
        }
    }

    /// The posted selection lands after the ListView's own select-all, so "report.txt" edits as "report", in every
    /// view (the icon views edit in a multi-line Edit of their own).
    #[test]
    fn rename_selects_the_stem_not_the_extension() {
        unsafe {
            let (parent, list) = test_list(w!("RexRenameTestParent"));
            for view in [LV_VIEW_DETAILS, LV_VIEW_ICON, LV_VIEW_SMALLICON, LV_VIEW_LIST, LV_VIEW_TILE] {
                SendMessageW(list, LVM_SETVIEW, Some(WPARAM(view as usize)), None);
                // A foreground change anywhere on the desktop (another test process, the user) takes the focus, and
                // the list then closes its edit: that run says nothing about the selection, so it is run again.
                let sel = (0..5).find_map(|_| {
                    let _ = SetFocus(Some(list));
                    let edit = HWND(SendMessageW(list, LVM_EDITLABELW, Some(WPARAM(0)), None).0 as *mut c_void);
                    assert!(!edit.is_invalid(), "view {view}: the list opened its label edit");
                    let mut msg = MSG::default();
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    if !IsWindow(Some(edit)).as_bool() {
                        return None;
                    }
                    let (mut a, mut b) = (0u32, 0u32);
                    SendMessageW(edit, EM_GETSEL, Some(WPARAM(&mut a as *mut u32 as usize)), Some(LPARAM(&mut b as *mut u32 as isize)));
                    SendMessageW(list, LVM_CANCELEDITLABEL, None, None);
                    Some((a, b))
                });
                assert_eq!(sel, Some((0, 6)), "view {view}");
            }
            let _ = DestroyWindow(parent);
        }
    }

    #[test]
    fn tile_columns_written_into_the_item() {
        let mut cols = [99u32; 20];
        let mut fmts = [99i32; 20];
        let mut item = LVITEMW { mask: LVIF_COLUMNS | LVIF_COLFMT, puColumns: cols.as_mut_ptr(), piColFmt: fmts.as_mut_ptr() as *mut _, ..Default::default() };
        put_tile_columns(&mut item, &[1, 2, 3]);
        assert_eq!((item.cColumns, cols[..4].to_vec(), fmts[..4].to_vec()), (3, vec![1, 2, 3, 99], vec![0, 0, 0, 99]));
        fmts = [99; 20];
        let mut only = LVITEMW { mask: LVIF_COLUMNS, puColumns: cols.as_mut_ptr(), piColFmt: fmts.as_mut_ptr() as *mut _, ..Default::default() };
        put_tile_columns(&mut only, &[1]);
        assert_eq!((only.cColumns, fmts[0]), (1, 99), "formats untouched without LVIF_COLFMT");
        let mut none = LVITEMW { mask: LVIF_COLUMNS | LVIF_COLFMT, cColumns: 5, ..Default::default() };
        put_tile_columns(&mut none, &[1, 2]);
        assert_eq!(none.cColumns, 0);
    }

    static TILE_COLUMN_ASKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// A virtual list's parent that answers like fill_dispinfo: the texts, and one extra tile line on request.
    unsafe extern "system" fn tile_parent(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_NOTIFY && (*(lparam.0 as *const NMHDR)).code == LVN_GETDISPINFOW {
            let item = &mut (*(lparam.0 as *mut NMLVDISPINFOW)).item;
            if item.mask.contains(LVIF_TEXT) {
                copy_text(if item.iSubItem == 0 { "report.txt" } else { "Text document" }, item.pszText, item.cchTextMax);
            }
            if item.mask.contains(LVIF_COLUMNS) && item.iSubItem == 0 {
                TILE_COLUMN_ASKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);                put_tile_columns(item, &[1]);
            }
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    /// Activates a Common Controls 6 context for this thread (the test exe has no manifest: comctl32 5.82). libtest
    /// runs each test on its own thread, so a context left active by a failed assert ends with that thread.
    unsafe fn activate_v6() -> (windows::Win32::Foundation::HANDLE, usize) {
        use windows::Win32::System::ApplicationInstallationAndServicing::{ACTCTXW, ActivateActCtx, CreateActCtxW};
        let path = std::env::temp_dir().join(format!("rexplorer-cc6-test-{}.manifest", std::process::id()));
        std::fs::write(
            &path,
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"><dependency><dependentAssembly><assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/></dependentAssembly></dependency></assembly>"#,
        )
        .unwrap();
        let wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain([0]).collect();
        let ctx = ACTCTXW { cbSize: size_of::<ACTCTXW>() as u32, lpSource: PCWSTR(wide.as_ptr()), ..Default::default() };
        let h = CreateActCtxW(&ctx);
        let _ = std::fs::remove_file(&path); // read once, at creation
        let h = h.expect("activation context");
        let mut cookie = 0usize;
        ActivateActCtx(Some(h), &mut cookie).unwrap();
        let _ = windows::Win32::System::LibraryLoader::LoadLibraryW(w!("comctl32.dll"));
        (h, cookie)
    }

    /// Proves a virtual (owner-data) tile is asked for its extra lines, which Tiles and Content rely on.
    #[test]
    fn owner_data_tile_view_asks_for_columns() {
        use windows::Win32::Graphics::Gdi::{CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, GetDC, ReleaseDC, SelectObject};
        use windows::Win32::System::ApplicationInstallationAndServicing::{DeactivateActCtx, ReleaseActCtx};
        unsafe {
            let (ctx, cookie) = activate_v6();
            let hinst = GetModuleHandleW(None).unwrap().into();
            let class = w!("RexTileTestParent");
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(tile_parent), hInstance: hinst, lpszClassName: class, ..Default::default() });
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), class, PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 400, 300, None, None, Some(hinst), None).unwrap();
            let style = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(LVS_REPORT | LVS_OWNERDATA);
            let list = CreateWindowExW(WINDOW_EX_STYLE(0), WC_LISTVIEWW, PCWSTR::null(), style, 0, 0, 400, 300, Some(parent), None, Some(hinst), None).unwrap();
            for i in 0..2 {
                let col = LVCOLUMNW { mask: LVCF_WIDTH, cx: 200, iSubItem: i, ..Default::default() };
                SendMessageW(list, LVM_INSERTCOLUMNW, Some(WPARAM(i as usize)), Some(LPARAM(&col as *const _ as isize)));
            }
            SendMessageW(list, LVM_SETITEMCOUNT, Some(WPARAM(3)), None);
            assert_eq!(SendMessageW(list, 0x2008, None, None).0, 6, "CCM_GETVERSION: comctl32 6");
            let info = LVTILEVIEWINFO {
                cbSize: size_of::<LVTILEVIEWINFO>() as u32,
                dwMask: LVTVIM_COLUMNS,
                dwFlags: LVTVIF_AUTOSIZE,
                cLines: 1,
                ..Default::default()
            };
            SendMessageW(list, LVM_SETTILEVIEWINFO, None, Some(LPARAM(&info as *const _ as isize)));
            SendMessageW(list, LVM_SETVIEW, Some(WPARAM(LV_VIEW_TILE as usize)), None);
            assert_eq!(SendMessageW(list, LVM_GETVIEW, None, None).0 as u32, LV_VIEW_TILE);
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            let bmp = CreateCompatibleBitmap(screen, 400, 300);
            let old = SelectObject(dc, bmp.into());
            SendMessageW(list, WM_PRINTCLIENT, Some(WPARAM(dc.0 as usize)), Some(LPARAM((PRF_CLIENT | PRF_ERASEBKGND) as isize)));
            SelectObject(dc, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(dc);
            ReleaseDC(None, screen);
            assert!(TILE_COLUMN_ASKS.load(std::sync::atomic::Ordering::Relaxed) > 0, "the tile asked for its extra lines");
            let _ = DestroyWindow(parent);
            let _ = DeactivateActCtx(0, cookie);
            ReleaseActCtx(ctx);
        }
    }

    #[test]
    fn stem_subclass_rewrites_the_first_select_all() {
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap().into();
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 200, 100, None, None, Some(hinst), None).unwrap();
            let edit = CreateWindowExW(WINDOW_EX_STYLE(0), WC_EDITW, w!("report.txt"), WS_CHILD, 0, 0, 100, 20, Some(parent), None, Some(hinst), None).unwrap();
            let _ = SetWindowSubclass(edit, Some(stem_subclass), STEM_ID, 6);
            let sel = || {
                let (mut a, mut b) = (0u32, 0u32);
                SendMessageW(edit, EM_GETSEL, Some(WPARAM(&mut a as *mut u32 as usize)), Some(LPARAM(&mut b as *mut u32 as isize)));
                (a, b)
            };
            SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
            assert_eq!(sel(), (0, 6));
            SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
            assert_eq!(sel(), (0, 10), "the hook is gone after the first");
            let _ = DestroyWindow(parent);
        }
    }

    #[test]
    fn sort_carry_keeps_selection_by_key() {
        unsafe {
            let (parent, list) = test_list(w!("RexCarryTestParent"));
            SendMessageW(list, LVM_SETITEMCOUNT, Some(WPARAM(3)), None);
            set_item_state(list, 0, LVIS_SELECTED, LVIS_SELECTED);
            set_item_state(list, 2, LVIS_FOCUSED, LVIS_FOCUSED);
            let carry = snapshot(list, 3, |i| Some(10 + i));
            restore(list, &carry, [12, 11, 10].into_iter());
            let state = |i: usize, m: LIST_VIEW_ITEM_STATE_FLAGS| SendMessageW(list, LVM_GETITEMSTATE, Some(WPARAM(i)), Some(LPARAM(m.0 as isize))).0 != 0;
            assert!(state(2, LVIS_SELECTED) && !state(0, LVIS_SELECTED), "row 0 moved to row 2 and stays selected");
            assert!(state(0, LVIS_FOCUSED), "row 2 moved to row 0 and keeps the focus");
            let _ = DestroyWindow(parent);
        }
    }

    #[test]
    fn type_to_select_searches_forward_then_wraps() {
        let names = ["sub", "Alpha", "beta", "c"];
        let name = |i: usize| names[i].to_string();
        assert_eq!(find_prefix(4, 2, true, "a", name), Some(1));
        assert_eq!(find_prefix(4, 2, false, "a", name), None);
        assert_eq!(find_prefix(4, 0, false, "BE", name), Some(2));
        assert_eq!(find_prefix(4, 4, true, "s", name), Some(0));
        assert_eq!(find_prefix(0, 0, true, "s", name), None);
    }

    #[test]
    fn type_ahead_rules() {
        let s = |t: (String, String)| (t.0, t.1);
        assert_eq!(s(type_ahead("", 0, 5000, 'a')), ("a".into(), "a".into()));
        assert_eq!(s(type_ahead("a", 5000, 5300, 'b')), ("ab".into(), "ab".into()));
        assert_eq!(s(type_ahead("a", 5000, 5300, 'a')), ("aa".into(), "a".into()));
        assert_eq!(s(type_ahead("ab", 5000, 6200, 'c')), ("c".into(), "c".into()));
        assert_eq!(s(type_ahead("a", u32::MAX - 100, 200, 'b')), ("ab".into(), "ab".into()));
        assert_eq!(s(type_ahead("ab", 5000, 5100, ' ')), ("ab".into(), String::new()));
        assert_eq!(s(type_ahead("ab", 5000, 5100, '\u{8}')), ("ab".into(), String::new()));
    }

    #[test]
    fn run_as_is_offered_on_programs_only() {
        for yes in ["setup.exe", "A.EXE", "x.bat", "y.Cmd", "z.msi", "c.msc", "old.com"] {
            assert!(runs_elevated(yes), "{yes}");
        }
        for no in ["a.txt", "exe", "a.exe.txt", "a.lnk", "folder", "a.ps1", ""] {
            assert!(!runs_elevated(no), "{no}");
        }
    }

    #[test]
    fn quoted_paths_joins_lines() {
        assert_eq!(quoted_paths(&["C:\\a b.txt".to_string(), "\\\\srv\\s\\x".to_string()]), "\"C:\\a b.txt\"\r\n\"\\\\srv\\s\\x\"");
        assert_eq!(quoted_paths(&[]), "");
        assert_eq!(quoted_paths(&["C:\\one".to_string()]), "\"C:\\one\"");
    }

    #[test]
    fn copy_as_path_refuses_names_that_break_out_of_the_quotes() {
        // A folder inside a .rar can be named anything: pasted into a terminal, a quote or a line break runs a command.
        assert!(pastable("C:\\a b\\c.txt"));
        assert!(!pastable("C:\\x.rar\\y\" & calc & \""));
        assert!(!pastable("C:\\x.rar\\y\ncalc"));
        assert!(!pastable("C:\\x.rar\\y\rcalc"));
    }
}
