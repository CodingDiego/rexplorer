//! Explorer's Home page, window class "RexHome": a "Quick access" grid of folder tiles, then a "Recent" table of
//! files, on a rounded surface card. It paints itself (double-buffered) and scrolls; icons render on a worker
//! thread and land as they finish.
//!
//! The parent owns the items: `build` (worker side) turns the Home folder's items into a `Content`, the parent
//! puts the index of each entry's item in its own listing into `Entry::index`, and every notification names
//! entries by that index. Notifications are WM_NOTIFY with an `NmHome`: HN_SELCHANGE (read the selection back
//! from the listing's point of view with `selection`), HN_ACTIVATE (double-click or Enter on `index`),
//! HN_CONTEXT (right-click; `index` usize::MAX on the background), HN_KEYDOWN (a key the page does not handle
//! itself; `index` is the virtual key), HN_RENAME (an in-place rename of `index` was committed; the new name is
//! `rename_text`).

use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::size_of;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Once};

use rexplorer_core::com::{ComInit, Pidl, strret_to_string};
use rexplorer_core::settings::Theme;
use rexplorer_core::{ShellFolder, ShellItem, images};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, PROPERTYKEY, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{NMHDR, SetScrollInfo, WC_EDITW, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VIRTUAL_KEY, VK_A, VK_CONTROL, VK_DOWN, VK_END,
    VK_ESCAPE, VK_HOME, VK_LEFT, VK_RETURN, VK_RIGHT, VK_UP,
};
use windows::Win32::UI::Shell::Common::SHELLDETAILS;
use windows::Win32::UI::Shell::{DefSubclassProc, IShellFolder, IShellFolder2, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{GUID, HSTRING, Interface, PCWSTR, Result, w};

use crate::theme;

pub const HN_SELCHANGE: u32 = 0x9051;
pub const HN_ACTIVATE: u32 = 0x9052;
pub const HN_CONTEXT: u32 = 0x9053;
pub const HN_KEYDOWN: u32 = 0x9054;
pub const HN_RENAME: u32 = 0x9055;
/// Test hook: returns tiles << 16 | rows.
pub const HM_COUNTS: u32 = WM_APP + 0x52;
/// wparam = generation << 32 | slot, lparam = HBITMAP (ours once received).
const HM_ICON: u32 = WM_APP + 0x50;
/// wparam: 0 cancels the rename edit, 1 commits it (a key), 2 commits it (the edit lost the focus).
const HM_END_EDIT: u32 = WM_APP + 0x51;

#[repr(C)]
pub struct NmHome {
    pub hdr: NMHDR,
    pub index: usize,
}

// ---- content (built on a worker) ------------------------------------------------------------------

#[derive(Clone)]
pub struct Entry {
    /// Index of the item in the parent's listing; usize::MAX until the parent sets it.
    pub index: usize,
    /// Parsing name: identifies the item across rebuilds.
    pub key: String,
    pub name: String,
    /// Tiles: the folder that holds it. Rows: the file's folder (the name's second line).
    pub subtitle: String,
    /// Rows: the text of `Content::columns[1..]`.
    pub cells: Vec<String>,
    pub abs: Pidl,
}

#[derive(Clone, Default)]
pub struct Content {
    /// Table headers: name first.
    pub columns: Vec<String>,
    pub tiles: Vec<Entry>,
    pub rows: Vec<Entry>,
}

const fn pkey(g: u128, pid: u32) -> PROPERTYKEY {
    PROPERTYKEY { fmtid: GUID::from_u128(g), pid }
}
const STORAGE: u128 = 0xB725F130_47EF_101A_A5F1_02608C9EEBAC;
const NAME: PROPERTYKEY = pkey(STORAGE, 10);
const SIZE_KEY: PROPERTYKEY = pkey(STORAGE, 12);
const MODIFIED: PROPERTYKEY = pkey(STORAGE, 14);
const ACCESSED: PROPERTYKEY = pkey(STORAGE, 16);
const TYPE_KEY: PROPERTYKEY = pkey(0x28636AA6_953D_11D2_B5D6_00C04FD918D0, 11);
const NARROW: PROPERTYKEY = pkey(0xDABD30ED_0043_4789_A7F8_D013A4736622, 100);
/// The table: Name, then these. Home's own first columns (Size, Type, Date modified, Account, Activity) hold
/// text only for cloud files, so local files take theirs from their own folder.
const CELLS: [PROPERTYKEY; 3] = [MODIFIED, TYPE_KEY, SIZE_KEY];
const FALLBACK: [&str; 4] = ["Name", "Date modified", "Type", "Size"];
/// SFGAO_FOLDER and SFGAO_STREAM (a .zip is both: a file, not a tile).
const SFGAO_FOLDER: u32 = 0x2000_0000;
const SFGAO_STREAM: u32 = 0x0040_0000;

fn same(a: &PROPERTYKEY, b: &PROPERTYKEY) -> bool {
    a.fmtid == b.fmtid && a.pid == b.pid
}

/// Property key, shell column index and (localized) title of each column `folder` offers.
fn columns_by_key(folder: &IShellFolder) -> Vec<(PROPERTYKEY, u32, String)> {
    let Ok(f2) = folder.cast::<IShellFolder2>() else { return Vec::new() };
    let mut out = Vec::new();
    for c in 0..64 {
        let mut k = PROPERTYKEY::default();
        if unsafe { f2.MapColumnToSCID(c, &mut k) }.is_err() {
            break;
        }
        let mut sd = SHELLDETAILS::default();
        let title = match unsafe { f2.GetDetailsOf(std::ptr::null(), c, &mut sd) } {
            // SHELLDETAILS is packed: copy the STRRET out before taking a reference.
            Ok(()) => unsafe { strret_to_string(&mut { sd.str }, std::ptr::null()) },
            Err(_) => String::new(),
        };
        out.push((k, c, title));
    }
    out
}

fn is_fs_path(p: &str) -> bool {
    let b = p.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':') || p.starts_with("\\\\")
}

/// The folder holding `path`, short: relative to the profile folder ("Escritorio\project"), the profile
/// folder's own name for its direct children, empty for items that are not filesystem paths.
fn parent_label(path: &str, profile: &str) -> String {
    if !is_fs_path(path) {
        return String::new();
    }
    let Some(k) = path.trim_end_matches('\\').rfind('\\') else { return String::new() };
    let parent = &path[..k];
    let p = profile.trim_end_matches('\\');
    if p.is_empty() {
        return parent.to_string();
    }
    if parent.eq_ignore_ascii_case(p) {
        return p.rsplit('\\').next().unwrap_or(p).to_string();
    }
    match (parent.get(..p.len()), parent.get(p.len()..)) {
        (Some(head), Some(rest)) if head.eq_ignore_ascii_case(p) && rest.starts_with('\\') => rest[1..].to_string(),
        _ => parent.to_string(),
    }
}

/// The Home page of `folder` (Home or Quick Access) from its items in enumeration order: folders become tiles in
/// that (pinned) order, everything else rows, most recently used first. Run it on a worker: local recent files
/// are read through their own folder.
pub fn build(folder: &ShellFolder, items: &[ShellItem]) -> Content {
    let cols = columns_by_key(&folder.folder);
    let find = |cols: &[(PROPERTYKEY, u32, String)], k: &PROPERTYKEY| cols.iter().find(|c| same(&c.0, k)).map(|c| (c.1, c.2.clone()));
    let columns = [NAME].iter().chain(&CELLS).zip(FALLBACK).map(|(k, f)| find(&cols, k).map(|c| c.1).filter(|t| !t.is_empty()).unwrap_or_else(|| f.to_string()));
    let profile = std::env::var("USERPROFILE").unwrap_or_default();
    let mut tiles = Vec::new();
    let mut rows: Vec<(String, Entry)> = Vec::new();
    for item in items {
        let key = item.parsing_name();
        let attrs = item.attributes(SFGAO_FOLDER | SFGAO_STREAM);
        let subtitle = item.property_text(&NARROW).unwrap_or_else(|| parent_label(&key, &profile));
        let mut e = Entry { index: usize::MAX, name: item.display_name(), subtitle, cells: Vec::new(), abs: folder.child_pidl(item), key };
        if attrs & SFGAO_FOLDER != 0 && attrs & SFGAO_STREAM == 0 {
            tiles.push(e);
            continue;
        }
        if rows.iter().any(|(_, r)| r.key.eq_ignore_ascii_case(&e.key)) {
            continue; // Home lists some cloud files twice
        }
        // Home fills its columns for cloud files; a local file's own folder knows its details.
        let mut own: Option<Option<(Vec<(PROPERTYKEY, u32, String)>, ShellItem)>> = None;
        e.cells = CELLS
            .iter()
            .map(|k| {
                let text = find(&cols, k).map(|(c, _)| item.detail(c)).unwrap_or_default();
                if !text.is_empty() || !is_fs_path(&e.key) {
                    return text;
                }
                let o = own.get_or_insert_with(|| crate::source::hit_for_path(Path::new(&e.key)).map(|h| (columns_by_key(&h.item.parent), h.item)));
                o.as_ref().and_then(|(c, it)| find(c, k).map(|(i, _)| it.detail(i))).unwrap_or_default()
            })
            .collect();
        // The shell writes dates as yyyy/mm/dd:hh:mm:ss.fff, which sorts as text.
        rows.push((item.property_text(&ACCESSED).unwrap_or_default(), e));
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    Content { columns: columns.collect(), tiles, rows: rows.into_iter().map(|(_, e)| e).collect() }
}

// ---- geometry (96-DPI units, scaled) ---------------------------------------------------------------

const PAD: i32 = 20;
const TITLE_H: i32 = 36;
const TILE_W: i32 = 300;
const TILE_H: i32 = 72;
const GAP: i32 = 8;
const SECTION_GAP: i32 = 20;
const PILL_H: i32 = 32;
const HEADER_H: i32 = 32;
const ROW_H: i32 = 48;

fn scale(px: i32, dpi: u32) -> i32 {
    px * dpi as i32 / 96
}

/// Where everything goes in content coordinates (before scrolling) for a client `width` wide.
#[derive(Debug, PartialEq)]
pub struct Layout {
    pub title: RECT,
    pub tiles: Vec<RECT>,
    pub pill: RECT,
    pub header: RECT,
    pub rows: Vec<RECT>,
    /// The "No recent files" line when there are no rows.
    pub empty: RECT,
    pub height: i32,
}

pub fn layout(width: i32, tiles: usize, rows: usize, pill_w: i32, dpi: u32) -> Layout {
    let s = |v: i32| scale(v, dpi);
    let (left, right) = (s(PAD), (width - s(PAD)).max(s(PAD) + 1));
    let mut y = s(PAD);
    let title = RECT { left, top: y, right, bottom: y + s(TITLE_H) };
    y = title.bottom + s(4);
    let per_row = ((right - left + s(GAP)) / (s(TILE_W) + s(GAP))).max(1) as usize;
    let tile_rects: Vec<RECT> = (0..tiles)
        .map(|i| {
            let (c, r) = ((i % per_row) as i32, (i / per_row) as i32);
            let (x, top) = (left + c * (s(TILE_W) + s(GAP)), y + r * (s(TILE_H) + s(GAP)));
            RECT { left: x, top, right: x + s(TILE_W), bottom: top + s(TILE_H) }
        })
        .collect();
    y = tile_rects.last().map_or(y, |r| r.bottom) + s(SECTION_GAP);
    let pill = RECT { left, top: y, right: left + pill_w, bottom: y + s(PILL_H) };
    y = pill.bottom + s(8);
    let header = RECT { left, top: y, right, bottom: y + s(HEADER_H) };
    y = header.bottom;
    let row_rects: Vec<RECT> = (0..rows as i32).map(|i| RECT { left, top: y + i * s(ROW_H), right, bottom: y + (i + 1) * s(ROW_H) }).collect();
    let empty = RECT { left, top: y, right, bottom: y + if rows == 0 { s(ROW_H) } else { 0 } };
    y = row_rects.last().map_or(empty.bottom, |r| r.bottom);
    Layout { title, tiles: tile_rects, pill, header, rows: row_rects, empty, height: y + s(PAD) }
}

/// Left edge of each table column in `left..right`: the name takes 45%, the others share the rest.
fn column_lefts(left: i32, right: i32, n: usize) -> Vec<i32> {
    let w = right - left;
    let name = w * 45 / 100;
    let rest = if n > 1 { (w - name) / (n as i32 - 1) } else { 0 };
    (0..n).map(|i| if i == 0 { left } else { left + name + (i as i32 - 1) * rest }).collect()
}

/// Keyboard move over `n` entries (tiles, then rows): Up/Left back, Down/Right forward, Home, End.
fn step(n: usize, focus: Option<usize>, key: VIRTUAL_KEY) -> Option<usize> {
    if n == 0 {
        return None;
    }
    let f = focus.unwrap_or(0);
    Some(match key {
        VK_UP | VK_LEFT if focus.is_some() => f.saturating_sub(1),
        VK_DOWN | VK_RIGHT if focus.is_some() => (f + 1).min(n - 1),
        VK_HOME => 0,
        VK_END => n - 1,
        _ => f,
    })
}

// ---- window -------------------------------------------------------------------------------------------

struct Fonts {
    dpi: u32,
    title: HFONT,
    bold: HFONT,
    glyph: HFONT,
    small_glyph: HFONT,
}

impl Fonts {
    fn delete(&mut self) {
        for f in [self.title, self.bold, self.glyph, self.small_glyph] {
            if !f.is_invalid() {
                let _ = unsafe { DeleteObject(f.into()) };
            }
        }
        self.dpi = 0;
    }
}

struct State {
    content: Content,
    /// Per entry (tiles, then rows); owned.
    icons: Vec<Option<HBITMAP>>,
    selected: Vec<bool>,
    focus: Option<usize>,
    hover: Option<usize>,
    tracking: bool,
    scroll: i32,
    theme: Theme,
    /// The UI font from WM_SETFONT; not owned.
    font: HFONT,
    fonts: Fonts,
    /// Bumped by each `set_content`; icon deliveries for older content are dropped.
    generation: Arc<AtomicU64>,
    edit: HWND,
    edit_entry: usize,
    rename_text: String,
    edit_brush: HBRUSH,
}

impl State {
    fn count(&self) -> usize {
        self.content.tiles.len() + self.content.rows.len()
    }

    fn entry(&self, e: usize) -> Option<&Entry> {
        let n = self.content.tiles.len();
        if e < n { self.content.tiles.get(e) } else { self.content.rows.get(e - n) }
    }

    fn by_index(&self, index: usize) -> Option<usize> {
        (0..self.count()).find(|&e| self.entry(e).is_some_and(|x| x.index == index))
    }
}

unsafe fn state<'a>(hwnd: HWND) -> Option<&'a mut State> {
    (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State).as_mut()
}

pub fn create(parent: HWND, id: usize) -> Result<HWND> {
    static REGISTER: Once = Once::new();
    unsafe {
        let hinst = GetModuleHandleW(None)?.into();
        REGISTER.call_once(|| {
            let wc = WNDCLASSW {
                style: CS_DBLCLKS,
                lpfnWndProc: Some(wndproc),
                hInstance: hinst,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: w!("RexHome"),
                ..Default::default()
            };
            RegisterClassW(&wc);
        });
        // Freed by WM_NCDESTROY.
        let st = Box::into_raw(Box::new(State {
            content: Content::default(),
            icons: Vec::new(),
            selected: Vec::new(),
            focus: None,
            hover: None,
            tracking: false,
            scroll: 0,
            theme: Theme::light(),
            font: HFONT::default(),
            fonts: Fonts { dpi: 0, title: HFONT::default(), bold: HFONT::default(), glyph: HFONT::default(), small_glyph: HFONT::default() },
            generation: Arc::new(AtomicU64::new(0)),
            edit: HWND::default(),
            edit_entry: 0,
            rename_text: String::new(),
            edit_brush: HBRUSH::default(),
        }));
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("RexHome"),
            PCWSTR::null(),
            WS_CHILD | WS_CLIPSIBLINGS | WS_CLIPCHILDREN | WS_TABSTOP | WS_VSCROLL,
            0,
            0,
            0,
            0,
            Some(parent),
            Some(HMENU(id as *mut c_void)),
            Some(hinst),
            Some(st as *const c_void),
        )
    }
}

pub fn set_theme(hwnd: HWND, t: &Theme) {
    unsafe {
        if let Some(s) = state(hwnd) {
            s.theme = t.clone();
            s.fonts.delete(); // the title fonts follow the theme's face
            if !s.edit_brush.is_invalid() {
                let _ = DeleteObject(s.edit_brush.into());
                s.edit_brush = HBRUSH::default();
            }
            theme::apply_control(hwnd, t); // the scroll bar
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

/// Shows `content` (entries whose `index` is usize::MAX are left out). Icons already shown for the same items
/// are kept; the others load on a worker. The selection is cleared; a hidden page also scrolls back to the top.
pub fn set_content(hwnd: HWND, mut content: Content) {
    unsafe {
        let Some(s) = state(hwnd) else { return };
        content.tiles.retain(|e| e.index != usize::MAX);
        content.rows.retain(|e| e.index != usize::MAX);
        end_edit(hwnd, s, 0);
        let mut old: HashMap<(bool, String), HBITMAP> = HashMap::new();
        let n_old = s.content.tiles.len();
        for (e, icon) in std::mem::take(&mut s.icons).into_iter().enumerate() {
            if let (Some(bmp), Some(entry)) = (icon, s.entry(e)) {
                if let Some(prev) = old.insert((e < n_old, entry.key.clone()), bmp) {
                    let _ = DeleteObject(prev.into());
                }
            }
        }
        s.content = content;
        let n_tiles = s.content.tiles.len();
        s.icons = (0..s.count()).map(|e| s.entry(e).and_then(|x| old.remove(&(e < n_tiles, x.key.clone())))).collect();
        for bmp in old.into_values() {
            let _ = DeleteObject(bmp.into());
        }
        s.selected = vec![false; s.count()];
        s.focus = None;
        s.hover = None;
        if !IsWindowVisible(hwnd).as_bool() {
            s.scroll = 0;
        }
        let generation = s.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let dpi = GetDpiForWindow(hwnd);
        let wanted: Vec<(usize, Pidl, i32)> = (0..s.count())
            .filter(|&e| s.icons[e].is_none())
            .filter_map(|e| s.entry(e).map(|x| (e, x.abs.clone(), scale(if e < n_tiles { 40 } else { 20 }, dpi))))
            .collect();
        if !wanted.is_empty() {
            let (latest, target) = (s.generation.clone(), hwnd.0 as isize);
            std::thread::spawn(move || {
                let _com = ComInit::mta();
                for (slot, abs, size) in wanted {
                    if latest.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    let Ok(bmp) = images::icon(&abs, size) else { continue };
                    let wp = WPARAM(((generation as usize) << 32) | slot);
                    if PostMessageW(Some(HWND(target as *mut c_void)), HM_ICON, wp, LPARAM(bmp.0 as isize)).is_err() {
                        let _ = DeleteObject(bmp.into());
                    }
                }
            });
        }
        sync_scroll(hwnd, s);
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

/// Selects the entries of the listing items `indices`, focusing `focus`'s; no notification.
pub fn set_selection(hwnd: HWND, indices: &[usize], focus: Option<usize>) {
    unsafe {
        let Some(s) = state(hwnd) else { return };
        for e in 0..s.count() {
            s.selected[e] = s.entry(e).is_some_and(|x| indices.contains(&x.index));
        }
        s.focus = focus.and_then(|i| s.by_index(i));
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

/// The selected entries as listing indices, and the focused one.
pub fn selection(hwnd: HWND) -> (Vec<usize>, Option<usize>) {
    unsafe {
        let Some(s) = state(hwnd) else { return (Vec::new(), None) };
        let sel = (0..s.count()).filter(|&e| s.selected[e]).filter_map(|e| s.entry(e).map(|x| x.index)).collect();
        (sel, s.focus.and_then(|e| s.entry(e)).map(|x| x.index))
    }
}

/// Starts an in-place rename of the listing item `index`'s name; false when the page does not show it.
pub fn begin_rename(hwnd: HWND, index: usize) -> bool {
    unsafe {
        let Some(s) = state(hwnd) else { return false };
        let Some(e) = s.by_index(index) else { return false };
        end_edit(hwnd, s, 0);
        ensure_visible(hwnd, s, e);
        let r = name_rect(hwnd, s, e);
        let name = s.entry(e).map(|x| x.name.clone()).unwrap_or_default();
        let text = HSTRING::from(name.as_str());
        let style = WS_CHILD | WS_VISIBLE | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32);
        let Ok(edit) = CreateWindowExW(WINDOW_EX_STYLE(0), WC_EDITW, PCWSTR(text.as_ptr()), style, r.left, r.top, r.right - r.left, r.bottom - r.top, Some(hwnd), None, None, None) else {
            return false;
        };
        s.edit = edit;
        s.edit_entry = e;
        SendMessageW(edit, WM_SETFONT, Some(WPARAM(s.font.0 as usize)), Some(LPARAM(1)));
        // A file's name is selected without its extension, as Explorer does.
        let end = if e >= s.content.tiles.len() { name.rfind('.').filter(|&k| k > 0).unwrap_or(name.len()) } else { name.len() };
        let end = name[..end].encode_utf16().count();
        SendMessageW(edit, windows::Win32::UI::Controls::EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(end as isize)));
        let _ = SetWindowSubclass(edit, Some(edit_subclass), 1, hwnd.0 as usize);
        let _ = SetFocus(Some(edit));
        true
    }
}

/// The name the last HN_RENAME committed.
pub fn rename_text(hwnd: HWND) -> String {
    unsafe { state(hwnd).map(|s| s.rename_text.clone()).unwrap_or_default() }
}

unsafe extern "system" fn edit_subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, data: usize) -> LRESULT {
    let home = HWND(data as *mut c_void);
    match msg {
        WM_KEYDOWN if wparam.0 == VK_RETURN.0 as usize || wparam.0 == VK_ESCAPE.0 as usize => {
            let _ = PostMessageW(Some(home), HM_END_EDIT, WPARAM((wparam.0 == VK_RETURN.0 as usize) as usize), LPARAM(0));
            LRESULT(0)
        }
        // No beep for Enter and Escape in a single-line Edit.
        WM_CHAR if wparam.0 == 0x0D || wparam.0 == 0x1B => LRESULT(0),
        WM_KILLFOCUS => {
            let _ = PostMessageW(Some(home), HM_END_EDIT, WPARAM(2), LPARAM(0));
            DefSubclassProc(hwnd, msg, wparam, lparam)
        }
        _ => DefSubclassProc(hwnd, msg, wparam, lparam),
    }
}

/// Closes the rename edit, if any: `how` 0 cancels, 1 commits and refocuses the page, 2 commits.
unsafe fn end_edit(hwnd: HWND, s: &mut State, how: usize) {
    if s.edit.is_invalid() {
        return;
    }
    let edit = std::mem::take(&mut s.edit);
    let len = GetWindowTextLengthW(edit);
    let mut buf = vec![0u16; len as usize + 1];
    let n = GetWindowTextW(edit, &mut buf) as usize;
    let text = String::from_utf16_lossy(&buf[..n]);
    let _ = DestroyWindow(edit);
    if how == 1 {
        let _ = SetFocus(Some(hwnd));
    }
    let Some((index, name)) = s.entry(s.edit_entry).map(|x| (x.index, x.name.clone())) else { return };
    if how == 0 || text.trim().is_empty() || text == name {
        return;
    }
    s.rename_text = text;
    notify(hwnd, HN_RENAME, index);
}

unsafe fn notify(hwnd: HWND, code: u32, index: usize) {
    // No state reference is used after notifying: the parent may do anything, including destroying us.
    if let Ok(parent) = GetParent(hwnd) {
        let id = GetDlgCtrlID(hwnd) as usize;
        let nm = NmHome { hdr: NMHDR { hwndFrom: hwnd, idFrom: id, code }, index };
        SendMessageW(parent, WM_NOTIFY, Some(WPARAM(id)), Some(LPARAM(&nm as *const _ as isize)));
    }
}

unsafe fn client(hwnd: HWND) -> RECT {
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    rc
}

unsafe fn ui_font(s: &State) -> HFONT {
    if s.font.is_invalid() { HFONT(GetStockObject(DEFAULT_GUI_FONT).0) } else { s.font }
}

unsafe fn ensure_fonts(s: &mut State, dpi: u32) {
    if s.fonts.dpi == dpi {
        return;
    }
    s.fonts.delete();
    s.fonts = Fonts {
        dpi,
        title: theme::header_font(&s.theme, dpi, s.theme.font_size + 3),
        bold: theme::header_font(&s.theme, dpi, s.theme.font_size),
        glyph: theme::icon_font(dpi, 12),
        small_glyph: theme::icon_font(dpi, 10),
    };
}

/// Width of the "Recent" pill for the current font.
unsafe fn pill_width(hwnd: HWND, s: &State) -> i32 {
    let dc = GetDC(Some(hwnd));
    let old = SelectObject(dc, ui_font(s).into());
    let mut size = windows::Win32::Foundation::SIZE::default();
    let text: Vec<u16> = "Recent".encode_utf16().collect();
    let _ = GetTextExtentPoint32W(dc, &text, &mut size);
    SelectObject(dc, old);
    ReleaseDC(Some(hwnd), dc);
    size.cx + scale(32, GetDpiForWindow(hwnd))
}

unsafe fn current_layout(hwnd: HWND, s: &State) -> Layout {
    let dpi = GetDpiForWindow(hwnd);
    // Laid out as if the scroll bar always showed: a layout that depends on it can flip it on and off forever
    // (the bar narrows the page, the tiles wrap, the page grows, the bar is needed...).
    let mut wr = RECT::default();
    let _ = GetWindowRect(hwnd, &mut wr);
    let width = wr.right - wr.left - GetSystemMetricsForDpi(SM_CXVSCROLL, dpi);
    layout(width, s.content.tiles.len(), s.content.rows.len(), pill_width(hwnd, s), dpi)
}

/// Content rectangle of entry `e`.
fn entry_rect(l: &Layout, n_tiles: usize, e: usize) -> RECT {
    if e < n_tiles { l.tiles[e] } else { l.rows.get(e - n_tiles).copied().unwrap_or_default() }
}

/// Client rectangle of entry `e`'s name (where the rename edit goes).
unsafe fn name_rect(hwnd: HWND, s: &State, e: usize) -> RECT {
    let dpi = GetDpiForWindow(hwnd);
    let l = current_layout(hwnd, s);
    let n = s.content.tiles.len();
    let r = entry_rect(&l, n, e);
    let sc = |v| scale(v, dpi);
    let (left, top, right, bottom) = if e < n {
        (r.left + sc(62), r.top + sc(8), r.right - sc(8), r.top + sc(32))
    } else {
        let cols = column_lefts(r.left, r.right, s.content.columns.len().max(1));
        (r.left + sc(42), r.top + sc(4), cols.get(1).copied().unwrap_or(r.right) - sc(8), r.top + sc(26))
    };
    RECT { left, top: top - s.scroll, right, bottom: bottom - s.scroll }
}

unsafe fn sync_scroll(hwnd: HWND, s: &mut State) {
    let h = current_layout(hwnd, s).height;
    let page = client(hwnd).bottom;
    s.scroll = s.scroll.clamp(0, (h - page).max(0));
    let si = SCROLLINFO {
        cbSize: size_of::<SCROLLINFO>() as u32,
        fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
        nMin: 0,
        nMax: h - 1,
        nPage: page.max(0) as u32,
        nPos: s.scroll,
        ..Default::default()
    };
    SetScrollInfo(hwnd, SB_VERT, &si, true);
}

unsafe fn scroll_to(hwnd: HWND, s: &mut State, y: i32) {
    s.scroll = y;
    sync_scroll(hwnd, s);
    if !s.edit.is_invalid() {
        end_edit(hwnd, s, 2);
    }
    let _ = InvalidateRect(Some(hwnd), None, false);
}

unsafe fn ensure_visible(hwnd: HWND, s: &mut State, e: usize) {
    let l = current_layout(hwnd, s);
    let r = entry_rect(&l, s.content.tiles.len(), e);
    let page = client(hwnd).bottom;
    // The rows sit under the table header: keep it in view above the first row.
    let top = if e == s.content.tiles.len() { l.pill.top } else { r.top };
    if top < s.scroll {
        scroll_to(hwnd, s, top - scale(8, GetDpiForWindow(hwnd)));
    } else if r.bottom > s.scroll + page {
        scroll_to(hwnd, s, r.bottom - page + scale(8, GetDpiForWindow(hwnd)));
    }
}

/// Entry under client point (x, y).
unsafe fn entry_at(hwnd: HWND, s: &State, lparam: LPARAM) -> Option<usize> {
    let (x, y) = (lparam.0 as i16 as i32, (lparam.0 >> 16) as i16 as i32 + s.scroll);
    let l = current_layout(hwnd, s);
    let inside = |r: &RECT| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
    (0..s.count()).find(|&e| inside(&entry_rect(&l, s.content.tiles.len(), e)))
}

unsafe fn select_only(s: &mut State, e: Option<usize>) {
    s.selected.iter_mut().for_each(|x| *x = false);
    if let Some(e) = e {
        s.selected[e] = true;
    }
    s.focus = e.or(s.focus);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lparam.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    if msg == WM_NCDESTROY {
        let p = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut State;
        if !p.is_null() {
            let mut s = Box::from_raw(p);
            s.generation.fetch_add(1, Ordering::SeqCst);
            for bmp in s.icons.iter().flatten() {
                let _ = DeleteObject((*bmp).into());
            }
            s.fonts.delete();
            if !s.edit_brush.is_invalid() {
                let _ = DeleteObject(s.edit_brush.into());
            }
        }
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let Some(s) = state(hwnd) else { return DefWindowProcW(hwnd, msg, wparam, lparam) };
    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        WM_SETFONT => {
            s.font = HFONT(wparam.0 as *mut c_void);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        WM_PAINT => paint(hwnd, s),
        WM_SIZE => {
            theme::round_corners(hwnd, scale(10, GetDpiForWindow(hwnd)));
            sync_scroll(hwnd, s);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        HM_COUNTS => return LRESULT(((s.content.tiles.len() << 16) | s.content.rows.len()) as isize),
        HM_ICON => {
            let bmp = HBITMAP(lparam.0 as *mut c_void);
            let (generation, slot) = (wparam.0 >> 32, wparam.0 & 0xFFFF_FFFF);
            if generation as u64 == s.generation.load(Ordering::SeqCst) && slot < s.icons.len() {
                if let Some(old) = s.icons[slot].replace(bmp) {
                    let _ = DeleteObject(old.into());
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
            } else {
                let _ = DeleteObject(bmp.into());
            }
        }
        HM_END_EDIT => end_edit(hwnd, s, wparam.0),
        WM_CTLCOLOREDIT => {
            let dc = HDC(wparam.0 as *mut c_void);
            SetTextColor(dc, COLORREF(s.theme.text));
            SetBkColor(dc, COLORREF(s.theme.surface));
            if s.edit_brush.is_invalid() {
                s.edit_brush = CreateSolidBrush(COLORREF(s.theme.surface));
            }
            return LRESULT(s.edit_brush.0 as isize);
        }
        WM_VSCROLL => {
            let dpi = GetDpiForWindow(hwnd);
            let page = client(hwnd).bottom;
            let mut si = SCROLLINFO { cbSize: size_of::<SCROLLINFO>() as u32, fMask: SIF_ALL, ..Default::default() };
            let _ = GetScrollInfo(hwnd, SB_VERT, &mut si);
            let y = match SCROLLBAR_COMMAND((wparam.0 & 0xFFFF) as i32) {
                SB_LINEUP => s.scroll - scale(ROW_H, dpi),
                SB_LINEDOWN => s.scroll + scale(ROW_H, dpi),
                SB_PAGEUP => s.scroll - page,
                SB_PAGEDOWN => s.scroll + page,
                SB_THUMBTRACK | SB_THUMBPOSITION => si.nTrackPos,
                SB_TOP => 0,
                SB_BOTTOM => i32::MAX / 2,
                _ => s.scroll,
            };
            scroll_to(hwnd, s, y);
        }
        WM_MOUSEWHEEL => {
            let delta = (wparam.0 >> 16) as i16 as i32;
            let y = s.scroll - delta * 3 * scale(ROW_H, GetDpiForWindow(hwnd)) / 120;
            scroll_to(hwnd, s, y);
        }
        WM_MOUSEMOVE => {
            if !s.tracking {
                let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                s.tracking = TrackMouseEvent(&mut tme).is_ok();
            }
            let h = entry_at(hwnd, s, lparam);
            if s.hover != h {
                s.hover = h;
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
        WM_MOUSELEAVE => {
            s.tracking = false;
            s.hover = None;
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            let _ = SetFocus(Some(hwnd));
            let hit = entry_at(hwnd, s, lparam);
            let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
            match hit {
                Some(e) if msg == WM_LBUTTONDOWN && ctrl => {
                    s.selected[e] = !s.selected[e];
                    s.focus = Some(e);
                }
                // A right-click keeps a selection it lands in, as Explorer does.
                Some(e) if msg == WM_RBUTTONDOWN && s.selected[e] => s.focus = Some(e),
                _ if ctrl => {}
                _ => select_only(s, hit),
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
            notify(hwnd, HN_SELCHANGE, 0);
        }
        WM_LBUTTONDBLCLK => {
            if let Some(index) = entry_at(hwnd, s, lparam).and_then(|e| s.entry(e)).map(|x| x.index) {
                notify(hwnd, HN_ACTIVATE, index);
            }
        }
        WM_RBUTTONUP => {
            let index = entry_at(hwnd, s, lparam).and_then(|e| s.entry(e)).map_or(usize::MAX, |x| x.index);
            notify(hwnd, HN_CONTEXT, index);
        }
        WM_KEYDOWN => {
            let key = VIRTUAL_KEY(wparam.0 as u16);
            let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
            match key {
                VK_UP | VK_DOWN | VK_LEFT | VK_RIGHT | VK_HOME | VK_END => {
                    let to = step(s.count(), s.focus, key);
                    select_only(s, to);
                    if let Some(e) = to {
                        ensure_visible(hwnd, s, e);
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    notify(hwnd, HN_SELCHANGE, 0);
                }
                VK_A if ctrl => {
                    let n = s.content.tiles.len();
                    for e in 0..s.count() {
                        s.selected[e] = e >= n;
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    notify(hwnd, HN_SELCHANGE, 0);
                }
                VK_RETURN => {
                    if let Some(index) = s.focus.and_then(|e| s.entry(e)).map(|x| x.index) {
                        notify(hwnd, HN_ACTIVATE, index);
                    }
                }
                _ => notify(hwnd, HN_KEYDOWN, key.0 as usize),
            }
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
    }
    LRESULT(0)
}

unsafe fn text(dc: HDC, s: &str, r: RECT, flags: DRAW_TEXT_FORMAT) {
    // DrawTextW with DT_END_ELLIPSIS faults on an empty string (an empty Vec's pointer is dangling).
    if s.is_empty() {
        return;
    }
    let mut w: Vec<u16> = s.encode_utf16().collect();
    let mut r = r;
    DrawTextW(dc, &mut w, &mut r, flags | DT_NOPREFIX);
}

unsafe fn draw_bitmap(dc: HDC, bmp: HBITMAP, x: i32, y: i32, size: i32) {
    let mut bm = BITMAP::default();
    if GetObjectW(bmp.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void)) == 0 {
        return;
    }
    let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
    // Keep the aspect: fit the bitmap in the square, centered.
    let k = size as f64 / w.max(h).max(1) as f64;
    let (dw, dh) = ((w as f64 * k) as i32, (h as f64 * k) as i32);
    let src = CreateCompatibleDC(Some(dc));
    let old = SelectObject(src, bmp.into());
    let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
    let _ = AlphaBlend(dc, x + (size - dw) / 2, y + (size - dh) / 2, dw, dh, src, 0, 0, w, h, bf);
    SelectObject(src, old);
    let _ = DeleteDC(src);
}

unsafe fn paint(hwnd: HWND, s: &mut State) {
    let mut ps = PAINTSTRUCT::default();
    let wdc = BeginPaint(hwnd, &mut ps);
    let rc = client(hwnd);
    let (cw, ch) = (rc.right.max(1), rc.bottom.max(1));
    let dpi = GetDpiForWindow(hwnd);
    ensure_fonts(s, dpi);
    let l = current_layout(hwnd, s);
    let dc = CreateCompatibleDC(Some(wdc));
    let bmp = CreateCompatibleBitmap(wdc, cw, ch);
    let old_bmp = SelectObject(dc, bmp.into());
    let t = &s.theme;
    let bg = CreateSolidBrush(COLORREF(t.surface));
    FillRect(dc, &rc, bg);
    let _ = DeleteObject(bg.into());
    SetBkMode(dc, TRANSPARENT);
    let sc = |v: i32| scale(v, dpi);
    let dy = |r: RECT| RECT { top: r.top - s.scroll, bottom: r.bottom - s.scroll, ..r };
    let one = DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS;
    let ui = ui_font(s);
    let focused = GetFocus() == hwnd;
    let fill_for = |e: usize| -> Option<u32> {
        if s.selected.get(e) == Some(&true) {
            Some(if focused { t.selection } else { theme::blend(t.selection, t.surface, 0.4) })
        } else if s.hover == Some(e) {
            Some(theme::hover(t))
        } else {
            None
        }
    };

    // Quick access: chevron and title.
    let title = dy(l.title);
    SetTextColor(dc, COLORREF(t.text));
    SelectObject(dc, s.fonts.glyph.into());
    text(dc, "\u{E70D}", RECT { right: title.left + sc(20), ..title }, one);
    SelectObject(dc, s.fonts.title.into());
    text(dc, "Quick access", RECT { left: title.left + sc(24), ..title }, one);

    let n = s.content.tiles.len();
    for (i, e) in s.content.tiles.iter().enumerate() {
        let r = dy(l.tiles[i]);
        if r.bottom < 0 || r.top > ch {
            continue;
        }
        if let Some(c) = fill_for(i) {
            theme::fill_round(dc, &r, c, sc(8));
        }
        let (icon, x) = (sc(40), r.left + sc(12));
        match s.icons.get(i).copied().flatten() {
            Some(b) => draw_bitmap(dc, b, x, r.top + (r.bottom - r.top - icon) / 2, icon),
            None => theme::fill_round(dc, &RECT { left: x, top: r.top + sc(16), right: x + icon, bottom: r.top + sc(56) }, theme::hover(t), sc(6)),
        }
        let tx = r.left + sc(62);
        SelectObject(dc, s.fonts.bold.into());
        SetTextColor(dc, COLORREF(t.text));
        text(dc, &e.name, RECT { left: tx, top: r.top + sc(8), right: r.right - sc(8), bottom: r.top + sc(32) }, one);
        SelectObject(dc, ui.into());
        SetTextColor(dc, COLORREF(t.secondary_text));
        text(dc, &e.subtitle, RECT { left: tx, top: r.top + sc(30), right: r.right - sc(8), bottom: r.top + sc(50) }, one);
        SelectObject(dc, s.fonts.small_glyph.into());
        text(dc, "\u{E718}", RECT { left: tx, top: r.top + sc(50), right: tx + sc(14), bottom: r.top + sc(64) }, one);
    }

    // Recent: the pill, the header, the rows.
    let pill = dy(l.pill);
    theme::fill_round(dc, &pill, t.selection, (pill.bottom - pill.top) / 2);
    SelectObject(dc, ui.into());
    SetTextColor(dc, COLORREF(t.text));
    text(dc, "Recent", pill, one | DT_CENTER);
    let header = dy(l.header);
    let cols = column_lefts(header.left, header.right, s.content.columns.len().max(1));
    let col_right = |c: usize| cols.get(c + 1).copied().unwrap_or(header.right) - sc(8);
    SetTextColor(dc, COLORREF(t.secondary_text));
    for (c, title) in s.content.columns.iter().enumerate() {
        let left = cols[c] + if c == 0 { sc(12) } else { 0 };
        text(dc, title, RECT { left, right: col_right(c), ..header }, one);
    }
    if s.content.rows.is_empty() {
        text(dc, "No recent files", RECT { left: header.left + sc(12), ..dy(l.empty) }, one);
    }
    for (i, e) in s.content.rows.iter().enumerate() {
        let r = dy(l.rows[i]);
        if r.bottom < 0 || r.top > ch {
            continue;
        }
        if let Some(c) = fill_for(n + i) {
            theme::fill_round(dc, &RECT { top: r.top + sc(1), bottom: r.bottom - sc(1), ..r }, c, sc(6));
        }
        let (icon, x) = (sc(20), r.left + sc(12));
        if let Some(b) = s.icons.get(n + i).copied().flatten() {
            draw_bitmap(dc, b, x, r.top + (r.bottom - r.top - icon) / 2, icon);
        }
        let tx = r.left + sc(42);
        SetTextColor(dc, COLORREF(t.text));
        text(dc, &e.name, RECT { left: tx, top: r.top + sc(4), right: col_right(0), bottom: r.top + sc(26) }, one);
        SetTextColor(dc, COLORREF(t.secondary_text));
        text(dc, &e.subtitle, RECT { left: tx, top: r.top + sc(24), right: col_right(0), bottom: r.top + sc(44) }, one);
        SetTextColor(dc, COLORREF(t.text));
        for (c, cell) in e.cells.iter().enumerate().take(cols.len().saturating_sub(1)) {
            text(dc, cell, RECT { left: cols[c + 1], right: col_right(c + 1), ..r }, one);
        }
    }

    let _ = BitBlt(wdc, 0, 0, cw, ch, Some(dc), 0, 0, SRCCOPY);
    SelectObject(dc, old_bmp);
    let _ = DeleteObject(bmp.into());
    let _ = DeleteDC(dc);
    let _ = EndPaint(hwnd, &ps);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtitles_relative_to_the_profile() {
        let p = r"C:\Users\v";
        assert_eq!(parent_label(r"C:\Users\v\Escritorio\project\dist", p), r"Escritorio\project");
        assert_eq!(parent_label(r"C:\Users\v\Downloads", p), "v");
        assert_eq!(parent_label(r"C:\Users\v\Downloads\a.txt", r"C:\Users\v\"), "Downloads");
        assert_eq!(parent_label(r"C:\Users\vx\a.txt", p), r"C:\Users\vx", "a name prefix is no parent");
        assert_eq!(parent_label(r"D:\x\y.txt", p), r"D:\x");
        assert_eq!(parent_label("::{645FF040-5081-101B-9F08-00AA002F954E}", p), "");
        assert_eq!(parent_label("a.mp4", p), "");
        assert_eq!(parent_label(r"C:\Users\vé\ñ\a.txt", r"C:\Users\v"), r"C:\Users\vé\ñ");
    }

    #[test]
    fn layout_wraps_tiles_and_stacks_sections() {
        // 2 * 20 padding + 3 tiles of 300 with gaps of 8: 956 px hold three per row.
        let l = layout(956, 4, 2, 80, 96);
        assert_eq!((l.tiles[0].left, l.tiles[2].left, l.tiles[3].left), (20, 636, 20));
        assert_eq!(l.tiles[3].top, l.tiles[0].bottom + 8);
        assert_eq!(l.pill.top, l.tiles[3].bottom + 20);
        assert_eq!(l.pill.right - l.pill.left, 80);
        assert_eq!(l.rows[0].top, l.header.bottom);
        assert_eq!(l.rows[1].bottom - l.rows[1].top, 48);
        assert_eq!(l.height, l.rows[1].bottom + 20);
        // Too narrow for one: still one per row.
        let l = layout(200, 2, 0, 80, 96);
        assert_eq!((l.tiles[1].left, l.tiles[1].top), (20, l.tiles[0].bottom + 8));
        assert_eq!(l.empty.bottom - l.empty.top, 48, "the empty line takes a row");
        assert_eq!(layout(956, 4, 2, 80, 192).tiles[0].right - layout(956, 4, 2, 80, 192).tiles[0].left, 600);
    }

    #[test]
    fn keys_step_across_tiles_then_rows() {
        assert_eq!(step(5, None, VK_DOWN), Some(0));
        assert_eq!(step(5, Some(0), VK_DOWN), Some(1));
        assert_eq!(step(5, Some(4), VK_DOWN), Some(4));
        assert_eq!(step(5, Some(0), VK_UP), Some(0));
        assert_eq!(step(5, Some(3), VK_LEFT), Some(2));
        assert_eq!(step(5, Some(2), VK_END), Some(4));
        assert_eq!(step(5, Some(2), VK_HOME), Some(0));
        assert_eq!(step(0, None, VK_DOWN), None);
        assert_eq!(column_lefts(0, 1000, 4), [0, 450, 633, 816]);
    }

    static LAST: std::sync::Mutex<Vec<(u32, usize)>> = std::sync::Mutex::new(Vec::new());

    unsafe extern "system" fn parent_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_NOTIFY {
            let nm = &*(lparam.0 as *const NmHome);
            LAST.lock().unwrap().push((nm.hdr.code, nm.index));
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    #[test]
    fn real_home_content_and_the_window() {
        let _com = ComInit::sta();
        // The real Home folder: its pinned folders become tiles (on any machine: Quick Access pins some).
        let home = ShellFolder::parse("shell:::{F874310E-B6B7-47DC-BC84-B9E6B38F5903}").unwrap();
        let items = home.enumerate().unwrap();
        let mut c = build(&home, &items);
        assert!(!c.tiles.is_empty(), "Home has pinned folders");
        assert_eq!(c.columns.len(), 4);
        assert!(c.rows.iter().all(|r| r.cells.len() == 3));
        for (i, e) in c.tiles.iter_mut().chain(c.rows.iter_mut()).enumerate() {
            e.index = 100 + i;
        }
        let (nt, nr) = (c.tiles.len(), c.rows.len());
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap().into();
            let wc = WNDCLASSW { lpfnWndProc: Some(parent_proc), hInstance: hinst, lpszClassName: w!("RexHomeTestParent"), ..Default::default() };
            RegisterClassW(&wc);
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("RexHomeTestParent"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 1000, 700, None, None, Some(hinst), None).unwrap();
            let h = create(parent, 9).unwrap();
            let _ = MoveWindow(h, 0, 0, 956, 600, false);
            set_theme(h, &Theme::dark());
            set_content(h, c);
            assert_eq!(SendMessageW(h, HM_COUNTS, None, None).0 as usize, (nt << 16) | nr);
            set_selection(h, &[100], Some(100));
            assert_eq!(selection(h), (vec![100], Some(100)));
            // A click on the second tile selects it and tells the parent; a double-click activates it.
            let l = current_layout(h, state(h).unwrap());
            let r = l.tiles.get(1).copied().unwrap_or(l.tiles[0]);
            let at = LPARAM((((r.top + 10) as isize) << 16) | (r.left + 10) as isize);
            LAST.lock().unwrap().clear();
            SendMessageW(h, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(at));
            SendMessageW(h, WM_LBUTTONDBLCLK, Some(WPARAM(1)), Some(at));
            let want = if nt > 1 { 101 } else { 100 };
            assert_eq!(selection(h).0, vec![want]);
            assert_eq!(*LAST.lock().unwrap(), vec![(HN_SELCHANGE, 0), (HN_ACTIVATE, want)]);
            // Keys: End goes to the last entry, Ctrl+A is not pressed here so other keys go to the parent.
            SendMessageW(h, WM_KEYDOWN, Some(WPARAM(VK_END.0 as usize)), None);
            assert_eq!(selection(h).1, Some(100 + nt + nr - 1));
            LAST.lock().unwrap().clear();
            SendMessageW(h, WM_KEYDOWN, Some(WPARAM(0x2E)), None);
            assert_eq!(*LAST.lock().unwrap(), vec![(HN_KEYDOWN, 0x2E)]);
            // Rename: an edit over the name; Escape cancels without a notification.
            assert!(begin_rename(h, 100));
            assert!(!state(h).unwrap().edit.is_invalid());
            LAST.lock().unwrap().clear();
            SendMessageW(h, HM_END_EDIT, Some(WPARAM(0)), None);
            assert!(state(h).unwrap().edit.is_invalid());
            assert!(LAST.lock().unwrap().is_empty());
            assert!(!begin_rename(h, 7), "no such item");
            DestroyWindow(parent).unwrap();
        }
    }
}
