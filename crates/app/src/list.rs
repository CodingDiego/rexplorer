//! The ListView: virtual items, sorting, shell context menus, keys, rename and middle-click.

use super::*;
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, DT_END_ELLIPSIS, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DrawTextW, FillRect, SetBkMode,
    SetTextColor, TRANSPARENT,
};
use crate::nav::{cur, go_up, listing, open_in_background, sort_listing, source};
use crate::source::{Source, activate};
use rexplorer_core::dragdrop;
use std::cell::RefCell;
use windows::Win32::Foundation::POINTL;
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Ole::IDropTarget;
use windows::Win32::UI::Shell::{ILClone, ILFindLastID};

/// Our own entry above the shell's in the list's context menu.
const PIN_TO_HOME: &str = "Pin to Home\tCtrl+D";

pub(crate) unsafe fn on_notify(app: *mut App, lparam: LPARAM) -> LRESULT {
    let hdr = &*(lparam.0 as *const NMHDR);
    match hdr.code {
        LVN_GETDISPINFOW => {
            let di = &mut *(lparam.0 as *mut NMLVDISPINFOW);
            fill_dispinfo(app, &mut di.item);
        }
        LVN_COLUMNCLICK => {
            let nm = &*(lparam.0 as *const NMLISTVIEW);
            on_column_click(app, nm.iSubItem as usize);
        }
        LVN_ITEMACTIVATE => {
            let nm = &*(lparam.0 as *const NMITEMACTIVATE);
            if nm.iItem >= 0 {
                activate(app, nm.iItem as usize);
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
        LVN_BEGINDRAG => begin_drag(app),
        NM_CUSTOMDRAW => return LRESULT(draw_item(app, &mut *(lparam.0 as *mut NMLVCUSTOMDRAW)) as isize),
        NM_RCLICK => {
            let nm = &*(lparam.0 as *const NMITEMACTIVATE);
            show_menu(app, nm.iItem >= 0);
            return LRESULT(1);
        }
        LVN_KEYDOWN => {
            let nm = &*(lparam.0 as *const NMLVKEYDOWN);
            on_key(app, VIRTUAL_KEY(nm.wVKey));
        }
        // Only shell items can be renamed.
        LVN_BEGINLABELEDITW => return LRESULT(listing(app).is_none() as isize),
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
            let found = find_prefix(s.len(), nm.iStart.max(0) as usize, wrap, &needle, |i| s.text(i, 0));
            return LRESULT(found.map_or(-1, |i| i as isize));
        }
        _ => {}
    }
    LRESULT(0)
}

pub(crate) fn find_prefix(n: usize, start: usize, wrap: bool, needle: &str, name: impl Fn(usize) -> String) -> Option<usize> {
    let start = start.min(n);
    let needle = needle.to_lowercase();
    (start..n).chain(if wrap { 0..start } else { 0..0 }).find(|&i| name(i).to_lowercase().starts_with(&needle))
}

unsafe fn fill_dispinfo(app: *mut App, item: &mut LVITEMW) {
    let i = item.iItem as usize;
    let icon_view = (*app).views.view != views::View::Details;
    if item.mask.contains(LVIF_IMAGE) && item.iSubItem == 0 && icon_view {
        item.iImage = views::image(app, i);
    }
    let Some(s) = source(app) else { return };
    if i >= s.len() {
        return;
    }
    if item.mask.contains(LVIF_TEXT) {
        copy_text(&s.text(i, item.iSubItem as usize), item.pszText, item.cchTextMax);
    }
    if item.mask.contains(LVIF_IMAGE) && item.iSubItem == 0 && !icon_view {
        item.iImage = views::tall_icon(app, source::small_icon(app, i));
    }
}

unsafe fn on_column_click(app: *mut App, sub: usize) {
    let list = (*app).list;
    let Some(Source::Shell(l)) = &mut cur(app).source else { return };
    let Some(col) = l.columns.get(sub) else { return };
    if col.index == l.sort_col {
        l.sort_desc = !l.sort_desc;
    } else {
        l.sort_col = col.index;
        l.sort_desc = false;
    }
    sort_listing(l);
    let _ = InvalidateRect(Some(list), None, true);
}

/// Drags the selection with the shell's data object; the shell performs the operation and the change watch refreshes.
unsafe fn begin_drag(app: *mut App) {
    let data = {
        let Some(l) = listing(app) else { return };
        let sel = selected_indices((*app).list);
        let items: Vec<&ShellItem> = sel.iter().filter_map(|&i| l.items.get(i)).collect();
        if items.is_empty() {
            return;
        }
        dragdrop::data_object(&l.folder, &items)
    };
    if let Ok(data) = data {
        let _ = dragdrop::drag(&data);
    }
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
            let mut p = POINT { x: pt.x, y: pt.y };
            let _ = ScreenToClient(list, &mut p);
            let mut ht = LVHITTESTINFO { pt: p, ..Default::default() };
            let i = SendMessageW(list, LVM_HITTEST, None, Some(LPARAM(&mut ht as *mut _ as isize))).0;
            let folder_item = usize::try_from(i).ok().and_then(|i| l.items.get(i)).filter(|it| it.is_folder());
            // By parsing name: a PIDL's address can be reused by a later folder's allocation.
            let key = (l.folder.parsing_name.clone(), if folder_item.is_some() { i } else { -1 });
            if let Some((k, t)) = &*last.borrow() {
                if *k == key {
                    return Some(t.clone());
                }
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
    let _ = dragdrop::register((*app).list, Box::new(pick));
}

unsafe fn show_menu(app: *mut App, on_item: bool) {
    let hwnd = (*app).hwnd;
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    if let Some(Source::Archive(_)) = source(app) {
        return source::archive_menu(app);
    }
    let sel = selected_indices((*app).list);
    let menu = if on_item {
        selection_menu(app, &sel)
    } else {
        listing(app).and_then(|l| ContextMenu::for_background(&l.folder, hwnd).ok())
    };
    let Some(menu) = menu else { return };
    // Pinning applies to the current folder (background) or to selected folders of a shell listing.
    let pin = listing(app).is_some_and(|l| !on_item || sel.iter().filter_map(|&i| l.items.get(i)).any(|i| i.is_folder()));
    let own: &[&str] = if pin { &[PIN_TO_HOME] } else { &[] };
    let extended = GetKeyState(VK_SHIFT.0 as i32) < 0;
    (*app).active_menu = Some(menu);
    let picked = (*app).active_menu.as_ref().unwrap().show_with(hwnd, pt.x, pt.y, extended, own);
    (*app).active_menu = None;
    match picked {
        Ok(Picked::Rename) => begin_rename(app),
        Ok(Picked::Own(_)) => pin_to_home(app),
        _ => {}
    }
}

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
    let color = theme::blend(t.selection, t.surface, if (*app).pulse { 0.75 } else { 0.5 });
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

unsafe fn on_key(app: *mut App, key: VIRTUAL_KEY) {
    let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
    let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
    match key {
        VK_BACK => go_up(app),
        VK_F2 if listing(app).is_some() => begin_rename(app),
        VK_DELETE => verb_on_selection(app, "delete", shift),
        VK_C if ctrl => verb_on_selection(app, "copy", false),
        VK_X if ctrl => verb_on_selection(app, "cut", false),
        VK_V if ctrl => verb_on_background(app, "paste"),
        VK_A if ctrl => set_item_state((*app).list, usize::MAX, LVIS_SELECTED, LVIS_SELECTED),
        _ => {}
    }
}

/// The shell context menu for rows of the active tab: shell items, or search results from one folder.
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

pub(crate) unsafe fn verb_on_selection(app: *mut App, verb: &str, shift: bool) {
    let hwnd = (*app).hwnd;
    if let Some(menu) = selection_menu(app, &selected_indices((*app).list)) {
        let _ = menu.invoke_verb(hwnd, verb, shift);
    }
}

pub(crate) unsafe fn verb_on_background(app: *mut App, verb: &str) {
    let hwnd = (*app).hwnd;
    let menu = {
        let Some(l) = listing(app) else { return };
        ContextMenu::for_background(&l.folder, hwnd)
    };
    if let Ok(menu) = menu {
        let _ = menu.invoke_verb(hwnd, verb, false);
    }
}

unsafe fn begin_rename(app: *mut App) {
    let list = (*app).list;
    let i = SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_FOCUSED as isize))).0;
    if i >= 0 {
        let _ = SetFocus(Some(list));
        SendMessageW(list, LVM_EDITLABELW, Some(WPARAM(i as usize)), None);
    }
}

unsafe fn on_end_edit(app: *mut App, item: &LVITEMW) {
    if item.pszText.is_null() {
        return;
    }
    let name = item.pszText.to_string().unwrap_or_default();
    // Work on a copy: the shell may pump messages while renaming, and a refresh could replace the listing.
    let target = listing(app).and_then(|l| l.items.get(item.iItem as usize).cloned());
    if let Some(mut target) = target {
        if !name.is_empty() && name != target.display_name() {
            let _ = target.rename((*app).hwnd, &name);
            SetTimer(Some((*app).hwnd), TIMER_REFRESH, 150, None);
        }
    }
}

/// Selected and hot rows (and icon tiles) as rounded pills: the pill is painted first, then the control draws the
/// item as unselected over it with a transparent text background.
unsafe fn draw_item(app: *mut App, cd: &mut NMLVCUSTOMDRAW) -> u32 {
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
            CDRF_NOTIFYITEMDRAW
        }
        CDDS_ITEMPREPAINT => {
            let (list, t) = ((*app).list, &(*app).theme);
            let i = cd.nmcd.dwItemSpec;
            let selected = SendMessageW(list, LVM_GETITEMSTATE, Some(WPARAM(i)), Some(LPARAM(LVIS_SELECTED.0 as isize))).0 != 0;
            let hot = cd.nmcd.uItemState.0 & CDIS_HOT.0 != 0;
            if selected || hot {
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
                theme::fill_round(cd.nmcd.hdc, &pill, if selected { t.selection } else { theme::hover(t) }, 6 * dpi / 96);
            }
            cd.nmcd.uItemState.0 &= !(CDIS_SELECTED.0 | CDIS_HOT.0 | CDIS_FOCUS.0);
            cd.clrText = COLORREF(t.text);
            cd.clrTextBk = COLORREF(CLR_NONE as u32);
            CDRF_NEWFONT
        }
        _ => CDRF_DODEFAULT,
    }
}

/// The header is 32 px high (96 DPI) whatever the font asks for.
pub(crate) unsafe extern "system" fn header_subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
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
    if msg == WM_NOTIFY && (*(lparam.0 as *const NMHDR)).code == NM_CUSTOMDRAW {
        return LRESULT(draw_header(&(*(data as *mut App)).theme, &*(lparam.0 as *const NMCUSTOMDRAW)) as isize);
    }
    if msg == WM_SIZE {
        let r = DefSubclassProc(hwnd, msg, wparam, lparam);
        theme::round_corners(hwnd, 10 * GetDpiForWindow(hwnd) as i32 / 96);
        return r;
    }
    if msg == WM_MBUTTONUP {
        let mut ht = LVHITTESTINFO { pt: POINT { x: lparam.0 as i16 as i32, y: (lparam.0 >> 16) as i16 as i32 }, ..Default::default() };
        let i = SendMessageW(hwnd, LVM_HITTEST, None, Some(LPARAM(&mut ht as *mut _ as isize))).0;
        if i >= 0 {
            open_in_background(data as *mut App, i as usize);
        }
        return LRESULT(0);
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
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
            if nm.uItemState.0 & CDIS_HOT.0 != 0 {
                let m = 4 * dpi / 96;
                theme::fill_round(nm.hdc, &RECT { left: r.left + m, top: r.top + m, right: r.right - m, bottom: r.bottom - m }, theme::hover(t), 6 * dpi / 96);
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
            DrawTextW(nm.hdc, &mut buf[..len], &mut tr, DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX | align);
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
    use super::find_prefix;

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
}
