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
//! `rename_text`), HN_DRAG (the selection should be dragged; sent once the pointer moves past the system drag size
//! with the left button down on a selected entry).

use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::size_of;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Once};

use rexplorer_core::access;
use rexplorer_core::i18n::tr;
use rexplorer_core::com::{ComInit, Pidl, strret_to_string};
use rexplorer_core::settings::Theme;
use rexplorer_core::{ShellFolder, ShellItem, images};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, PROPERTYKEY, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{ROLE_SYSTEM_LIST, ROLE_SYSTEM_LISTITEM, ROLE_SYSTEM_PAGETAB};
use windows::Win32::UI::Controls::{NMHDR, SetScrollInfo, WC_EDITW, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, GetFocus, GetKeyState, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VIRTUAL_KEY, VK_A, VK_CONTROL, VK_DOWN, VK_END,
    VK_ESCAPE, VK_HOME, VK_LEFT, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_UP,
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
/// A pill was clicked; read the new filter with `filter`.
pub const HN_FILTER: u32 = 0x9056;
/// Shift+F10 or the Menu key: as HN_CONTEXT, shown at `menu_point` instead of the cursor.
pub const HN_CONTEXT_KEY: u32 = 0x9057;
pub const HN_DRAG: u32 = 0x9058;
/// Test hook: returns tiles << 16 | rows shown.
pub const HM_COUNTS: u32 = WM_APP + 0x52;
/// Test hook (the smoke test opens a recent file): selects and focuses the entry whose name or parsing name is
/// the page's window text (set it with WM_SETTEXT first), as a click would; returns its position or -1.
pub const HM_SELECT_NAMED: u32 = WM_APP + 0x53;
/// Test hook (the README screenshot hides personal names): wparam i < tiles: tile i's subtitle rectangle, 0 when
/// it is empty or "Stored locally"; wparam 0xFFFF: the table below its header; -1 past the last tile. Client
/// coordinates packed as four i16: left | top << 16 | right << 32 | bottom << 48.
pub const HM_RECT: u32 = WM_APP + 0x54;
/// wparam = generation << 32 | slot, lparam = HBITMAP (ours once received).
const HM_ICON: u32 = WM_APP + 0x50;
/// Coalesces the repaints HM_ICON asks for.
const ICON_TIMER: usize = 1;
/// Click-again rename: fires one double-click time after the release.
const RENAME_TIMER: usize = 2;
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
    /// Tiles: pinned (else frequent). Rows: a favorite (pinned file).
    pub pinned: bool,
    /// Rows: listed among the recent files.
    pub recent: bool,
    /// Rows: shared with you.
    pub shared: bool,
}

/// Which rows the table shows: the pills over it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Filter {
    #[default]
    Recent,
    Favorites,
    Shared,
}

/// The pills, left to right: filter, label key, the key of the table's text when nothing matches.
const PILLS: [(Filter, &str, &str); 3] = [
    (Filter::Recent, "home.pill_recent", "home.empty_recent"),
    (Filter::Favorites, "home.pill_favorites", "home.empty_favorites"),
    (Filter::Shared, "home.pill_shared", "home.empty_shared"),
];

impl Filter {
    fn shows(self, e: &Entry) -> bool {
        match self {
            Filter::Recent => e.recent,
            Filter::Favorites => e.pinned,
            Filter::Shared => e.shared,
        }
    }
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
const HOME_KEYS: u128 = 0x30C8EEF4_A832_41E2_AB32_E3C3CA28FD29;
/// System.Home.IsPinned: true for a pinned folder or a favorite file, false for a frequent folder or a recent file.
pub const IS_PINNED: PROPERTYKEY = pkey(HOME_KEYS, 4);
/// System.Home.Grouping: which part of Home lists the item; 5 is "shared with you" (OneDrive files of others).
const GROUPING: PROPERTYKEY = pkey(HOME_KEYS, 2);
const SHARED_GROUP: &str = "5";
/// System.ActivityInfo, Home's Activity column ("Ana edited this"): text only for cloud files.
const ACTIVITY: PROPERTYKEY = pkey(HOME_KEYS, 17);
/// The table: Name, then these. Home's own first columns (Size, Type, Date modified, Account, Activity) hold
/// text only for cloud files, so local files take theirs from their own folder.
const CELLS: [PROPERTYKEY; 4] = [MODIFIED, TYPE_KEY, SIZE_KEY, ACTIVITY];
const FALLBACK: [&str; 5] = ["col.name", "col.date_modified", "col.type", "col.size", "home.col_activity"];
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

/// The profile's Privacy keys: whether `build` lists frequent folders and recent files.
pub(crate) static SHOW_FREQUENT: AtomicBool = AtomicBool::new(true);
pub(crate) static SHOW_RECENT: AtomicBool = AtomicBool::new(true);

/// A tile's subtitle for the profile's own folders, as Explorer labels them.
const STORED_LOCALLY: &str = "home.stored_locally";

fn is_fs_path(p: &str) -> bool {
    let b = p.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':') || p.starts_with("\\\\")
}

/// The folder holding `path`, short: relative to the profile folder ("Escritorio\project"), the profile
/// "Stored locally" for its direct children (as Explorer labels the known folders), empty for non-filesystem items.
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
        return tr(STORED_LOCALLY).to_string();
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
    let columns = [NAME].iter().chain(&CELLS).zip(FALLBACK).map(|(k, f)| find(&cols, k).map(|c| c.1).filter(|t| !t.is_empty()).unwrap_or_else(|| tr(f).to_string()));
    let profile = std::env::var("USERPROFILE").unwrap_or_default();
    let mut tiles = Vec::new();
    let mut rows: Vec<(String, Entry)> = Vec::new();
    // The columns of each local file's folder, mapped once per folder rather than once per row.
    let mut by_dir: HashMap<String, Vec<(PROPERTYKEY, u32, String)>> = HashMap::new();
    for item in items {
        let key = item.parsing_name();
        let attrs = item.attributes(SFGAO_FOLDER | SFGAO_STREAM);
        let subtitle = item.property_text(&NARROW).unwrap_or_else(|| parent_label(&key, &profile));
        // Without the property (an older Home) every folder counts as pinned, as before.
        let pinned = item.property_bool(&IS_PINNED);
        let mut e = Entry {
            index: usize::MAX,
            name: item.display_name(),
            subtitle,
            cells: Vec::new(),
            abs: folder.child_pidl(item),
            key,
            pinned: pinned.unwrap_or(true),
            recent: false,
            shared: false,
        };
        if attrs & SFGAO_FOLDER != 0 && attrs & SFGAO_STREAM == 0 {
            if e.pinned || SHOW_FREQUENT.load(Ordering::Relaxed) {
                tiles.push(e);
            }
            continue;
        }
        e.pinned = pinned == Some(true);
        e.shared = item.property_text(&GROUPING).as_deref() == Some(SHARED_GROUP);
        e.recent = !e.pinned && !e.shared;
        if e.recent && !SHOW_RECENT.load(Ordering::Relaxed) {
            continue;
        }
        // Home lists a file once per group it is in (a favorite that is also recent, a shared file opened lately).
        if let Some((_, r)) = rows.iter_mut().find(|(_, r)| r.key.eq_ignore_ascii_case(&e.key)) {
            r.pinned |= e.pinned;
            r.recent |= e.recent;
            r.shared |= e.shared;
            continue;
        }
        // Home fills its columns for cloud files; a local file's own folder knows its details.
        let mut own: Option<Option<(String, ShellItem)>> = None;
        e.cells = CELLS
            .iter()
            .map(|k| {
                let text = find(&cols, k).map(|(c, _)| item.detail(c)).unwrap_or_default();
                // A network file's cells stay empty, as Home's own do: binding its folder can block on the share.
                if !text.is_empty() || !is_fs_path(&e.key) || e.key.starts_with("\\\\") {
                    return text;
                }
                let o = own.get_or_insert_with(|| {
                    let h = crate::source::hit_for_path(Path::new(&e.key))?;
                    let dir = h.folder.to_lowercase();
                    by_dir.entry(dir.clone()).or_insert_with(|| columns_by_key(&h.item.parent));
                    Some((dir, h.item))
                });
                o.as_ref().and_then(|(d, it)| find(&by_dir[d], k).map(|(i, _)| it.detail(i))).unwrap_or_default()
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
const TILE_W: i32 = 280;
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
    /// Tiles per row.
    pub per_row: usize,
    /// The filter pills, in `PILLS` order.
    pub pills: Vec<RECT>,
    pub header: RECT,
    pub rows: Vec<RECT>,
    /// The "No recent files" line when there are no rows.
    pub empty: RECT,
    pub height: i32,
}

pub fn layout(width: i32, tiles: usize, rows: usize, pill_ws: &[i32], dpi: u32) -> Layout {
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
    let mut x = left;
    let pills: Vec<RECT> = pill_ws
        .iter()
        .map(|&w| {
            let r = RECT { left: x, top: y, right: x + w, bottom: y + s(PILL_H) };
            x = r.right + s(GAP);
            r
        })
        .collect();
    y += s(PILL_H) + s(8);
    let header = RECT { left, top: y, right, bottom: y + s(HEADER_H) };
    y = header.bottom;
    let row_rects: Vec<RECT> = (0..rows as i32).map(|i| RECT { left, top: y + i * s(ROW_H), right, bottom: y + (i + 1) * s(ROW_H) }).collect();
    let empty = RECT { left, top: y, right, bottom: y + if rows == 0 { s(ROW_H) } else { 0 } };
    y = row_rects.last().map_or(empty.bottom, |r| r.bottom);
    Layout { title, tiles: tile_rects, per_row, pills, header, rows: row_rects, empty, height: y + s(PAD) }
}

/// Left edge of each table column in `left..right`: the name takes 45%, the others share the rest.
fn column_lefts(left: i32, right: i32, n: usize) -> Vec<i32> {
    let w = right - left;
    let name = w * 45 / 100;
    let rest = if n > 1 { (w - name) / (n as i32 - 1) } else { 0 };
    (0..n).map(|i| if i == 0 { left } else { left + name + (i as i32 - 1) * rest }).collect()
}

/// Keyboard move over `n` entries (`tiles` tiles in rows of `per_row`, then the table's rows): Left/Right and
/// Up/Down step one entry, except Up/Down over tiles, which move a whole row of them; Home, End.
fn step(n: usize, tiles: usize, per_row: usize, focus: Option<usize>, key: VIRTUAL_KEY) -> Option<usize> {
    if n == 0 {
        return None;
    }
    let f = focus.unwrap_or(0);
    Some(match key {
        VK_LEFT if focus.is_some() => f.saturating_sub(1),
        VK_RIGHT if focus.is_some() => (f + 1).min(n - 1),
        VK_UP if focus.is_some() => match f {
            // The first table row goes back to the first tile of the last row of tiles.
            f if f == tiles && tiles > 0 => tiles - 1 - (tiles - 1) % per_row,
            f if f < tiles => if f >= per_row { f - per_row } else { f },
            f => f.saturating_sub(1),
        },
        VK_DOWN if focus.is_some() => match f {
            f if f + per_row < tiles => f + per_row,
            // A shorter last row below: its last tile.
            f if f < tiles && f / per_row < (tiles - 1) / per_row => tiles - 1,
            // The last row of tiles: the first table row, if any.
            f if f < tiles => if tiles < n { tiles } else { f },
            f => (f + 1).min(n - 1),
        },
        VK_HOME => 0,
        VK_END => n - 1,
        _ => f,
    })
}

/// A keyboard move to entry `to`: Shift extends from the anchor (Ctrl keeps the rest, else only the range stays),
/// Ctrl alone only moves the outline, plain selects `to` alone and anchors there.
fn key_select(selected: &mut [bool], anchor: &mut Option<usize>, to: usize, ctrl: bool, shift: bool) {
    if shift {
        let a = anchor.filter(|&a| a < selected.len()).unwrap_or(to);
        if !ctrl {
            selected.iter_mut().for_each(|x| *x = false);
        }
        selected[a.min(to)..=a.max(to)].iter_mut().for_each(|x| *x = true);
    } else if !ctrl {
        selected.iter_mut().for_each(|x| *x = false);
        selected[to] = true;
        *anchor = Some(to);
    }
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
    filter: Filter,
    /// The rows the filter shows, as indices into `content.rows`. Entries are the tiles, then these rows.
    shown: Vec<usize>,
    /// Per slot (the tiles, then every row, shown or not); owned.
    icons: Vec<Option<HBITMAP>>,
    /// ICON_TIMER is armed.
    icon_timer: bool,
    /// Per entry.
    selected: Vec<bool>,
    focus: Option<usize>,
    /// Where Shift+arrows extend from.
    anchor: Option<usize>,
    typed: String,
    typed_at: u32,
    hover: Option<usize>,
    /// Client point of a left press on an entry (no Ctrl); a drag starts when the pointer leaves it.
    press: Option<(i32, i32)>,
    /// The already-selected entry whose name the current press landed on: a rename candidate.
    again: Option<usize>,
    /// The entry RENAME_TIMER will rename.
    rename_at: Option<usize>,
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
    acc: Option<access::Server>,
}

impl State {
    fn count(&self) -> usize {
        self.content.tiles.len() + self.shown.len()
    }

    fn entry(&self, e: usize) -> Option<&Entry> {
        self.slot_entry(self.slot(e)?)
    }

    /// Icon slot of entry `e`.
    fn slot(&self, e: usize) -> Option<usize> {
        let n = self.content.tiles.len();
        if e < n { Some(e) } else { self.shown.get(e - n).map(|r| n + r) }
    }

    fn slot_entry(&self, slot: usize) -> Option<&Entry> {
        let n = self.content.tiles.len();
        if slot < n { self.content.tiles.get(slot) } else { self.content.rows.get(slot - n) }
    }

    fn icon(&self, e: usize) -> Option<HBITMAP> {
        self.slot(e).and_then(|i| self.icons.get(i).copied().flatten())
    }

    /// Applies the filter: the rows it shows, nothing selected.
    fn refilter(&mut self) {
        let f = self.filter;
        self.shown = (0..self.content.rows.len()).filter(|&r| f.shows(&self.content.rows[r])).collect();
        self.selected = vec![false; self.count()];
        self.focus = None;
        self.anchor = None;
        self.hover = None;
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
            filter: Filter::Recent,
            shown: Vec::new(),
            icons: Vec::new(),
            icon_timer: false,
            selected: Vec::new(),
            focus: None,
            anchor: None,
            typed: String::new(),
            typed_at: 0,
            hover: None,
            press: None,
            again: None,
            rename_at: None,
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
            acc: None,
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
        // A press or a pending rename names an entry position, which may now hold another item.
        s.press = None;
        cancel_rename(hwnd, s);
        let mut old: HashMap<(bool, String), HBITMAP> = HashMap::new();
        let n_old = s.content.tiles.len();
        for (slot, icon) in std::mem::take(&mut s.icons).into_iter().enumerate() {
            if let (Some(bmp), Some(entry)) = (icon, s.slot_entry(slot)) {
                if let Some(prev) = old.insert((slot < n_old, entry.key.clone()), bmp) {
                    let _ = DeleteObject(prev.into());
                }
            }
        }
        s.content = content;
        let n_tiles = s.content.tiles.len();
        let slots = n_tiles + s.content.rows.len();
        s.icons = (0..slots).map(|slot| s.slot_entry(slot).and_then(|x| old.remove(&(slot < n_tiles, x.key.clone())))).collect();
        for bmp in old.into_values() {
            let _ = DeleteObject(bmp.into());
        }
        s.refilter();
        if !IsWindowVisible(hwnd).as_bool() {
            s.scroll = 0;
        }
        let generation = s.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let dpi = GetDpiForWindow(hwnd);
        let wanted: Vec<(usize, Pidl, i32)> = (0..slots)
            .filter(|&slot| s.icons[slot].is_none())
            .filter_map(|slot| s.slot_entry(slot).map(|x| (slot, x.abs.clone(), scale(if slot < n_tiles { 40 } else { 20 }, dpi))))
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
                    images::premultiply(bmp);
                    let wp = WPARAM(((generation as usize) << 32) | slot);
                    if PostMessageW(Some(HWND(target as *mut c_void)), HM_ICON, wp, LPARAM(bmp.0 as isize)).is_err() {
                        let _ = DeleteObject(bmp.into());
                    }
                }
            });
        }
        sync_scroll(hwnd, s);
        let _ = InvalidateRect(Some(hwnd), None, false);
        access::event(EVENT_OBJECT_REORDER, hwnd, None);
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
        announce(hwnd, s);
    }
}

/// Shows the rows `f` picks (the tab's pill); clears the selection, no notification.
pub fn set_filter(hwnd: HWND, f: Filter) {
    unsafe {
        let Some(s) = state(hwnd) else { return };
        if s.filter != f {
            s.filter = f;
            s.refilter();
            sync_scroll(hwnd, s);
            let _ = InvalidateRect(Some(hwnd), None, false);
            access::event(EVENT_OBJECT_REORDER, hwnd, None);
        }
    }
}

pub fn filter(hwnd: HWND) -> Filter {
    unsafe { state(hwnd).map(|s| s.filter).unwrap_or_default() }
}

/// The selected entries as listing indices, and the focused one.
pub fn selection(hwnd: HWND) -> (Vec<usize>, Option<usize>) {
    unsafe {
        let Some(s) = state(hwnd) else { return (Vec::new(), None) };
        let sel = (0..s.count()).filter(|&e| s.selected[e]).filter_map(|e| s.entry(e).map(|x| x.index)).collect();
        (sel, s.focus.and_then(|e| s.entry(e)).map(|x| x.index))
    }
}

/// Parsing name of the tile (a folder) under screen point (x, y); rows and the background give None.
pub fn tile_key_at_screen(hwnd: HWND, x: i32, y: i32) -> Option<String> {
    unsafe {
        let s = state(hwnd)?;
        let mut p = POINT { x, y };
        let _ = ScreenToClient(hwnd, &mut p);
        let e = entry_at(hwnd, s, LPARAM(((p.y as isize) << 16) | (p.x as isize & 0xFFFF)))?;
        s.content.tiles.get(e).map(|t| t.key.clone())
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
        let (name, key) = s.entry(e).map(|x| (x.name.clone(), x.key.clone())).unwrap_or_default();
        let text = HSTRING::from(name.as_str());
        let style = WS_CHILD | WS_VISIBLE | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32);
        let Ok(edit) = CreateWindowExW(WINDOW_EX_STYLE(0), WC_EDITW, PCWSTR(text.as_ptr()), style, r.left, r.top, r.right - r.left, r.bottom - r.top, Some(hwnd), None, None, None) else {
            return false;
        };
        s.edit = edit;
        s.edit_entry = e;
        SendMessageW(edit, WM_SETFONT, Some(WPARAM(s.font.0 as usize)), Some(LPARAM(1)));
        // A file's name is selected without the extension it shows (none when the shell hides it), as Explorer does.
        let file = key.rsplit('\\').next().unwrap_or_default();
        let stem = (e >= s.content.tiles.len()).then(|| crate::source::stem_len(&name, file)).flatten();
        let end = stem.map_or(-1, |n| n as isize);
        SendMessageW(edit, windows::Win32::UI::Controls::EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(end)));
        let _ = SetWindowSubclass(edit, Some(edit_subclass), 1, hwnd.0 as usize);
        crate::edit::attach(edit);
        let _ = SetFocus(Some(edit));
        true
    }
}

/// The name the last HN_RENAME committed.
/// Screen point for a keyboard-opened menu: under the focused, selected entry's name when it is in view, else the
/// page's top-left corner (as Explorer does).
pub fn menu_point(hwnd: HWND) -> windows::Win32::Foundation::POINT {
    use windows::Win32::Foundation::POINT;
    unsafe {
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let named = state(hwnd).and_then(|s| s.focus.filter(|&e| s.selected[e]).map(|e| name_rect(hwnd, s, e)));
        let mut pt = match named {
            Some(r) if r.bottom > client.top && r.top < client.bottom => POINT { x: r.left, y: r.bottom.min(client.bottom) },
            _ => POINT { x: client.left, y: client.top },
        };
        let _ = ClientToScreen(hwnd, &mut pt);
        pt
    }
}

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

/// Drops a pending click-again rename.
unsafe fn cancel_rename(hwnd: HWND, s: &mut State) {
    let _ = KillTimer(Some(hwnd), RENAME_TIMER);
    s.rename_at = None;
    s.again = None;
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

/// Width of each pill for the current font.
unsafe fn pill_widths(hwnd: HWND, s: &State) -> Vec<i32> {
    let dc = GetDC(Some(hwnd));
    let old = SelectObject(dc, ui_font(s).into());
    let pad = scale(32, GetDpiForWindow(hwnd));
    let widths = PILLS
        .iter()
        .map(|(_, label, _)| {
            let mut size = windows::Win32::Foundation::SIZE::default();
            let text: Vec<u16> = tr(label).encode_utf16().collect();
            let _ = GetTextExtentPoint32W(dc, &text, &mut size);
            size.cx + pad
        })
        .collect();
    SelectObject(dc, old);
    ReleaseDC(Some(hwnd), dc);
    widths
}

unsafe fn current_layout(hwnd: HWND, s: &State) -> Layout {
    let dpi = GetDpiForWindow(hwnd);
    // Laid out as if the scroll bar always showed: a layout that depends on it can flip it on and off forever
    // (the bar narrows the page, the tiles wrap, the page grows, the bar is needed...).
    let mut wr = RECT::default();
    let _ = GetWindowRect(hwnd, &mut wr);
    let width = wr.right - wr.left - GetSystemMetricsForDpi(SM_CXVSCROLL, dpi);
    layout(width, s.content.tiles.len(), s.shown.len(), &pill_widths(hwnd, s), dpi)
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
    let top = if e == s.content.tiles.len() { l.pills.first().map_or(r.top, |p| p.top) } else { r.top };
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

/// Accessible child index of entry `e` when `nt` tiles come first: the three pills sit between the tiles and the rows.
fn acc_index(nt: usize, e: usize) -> usize {
    if e < nt { e } else { e + PILLS.len() }
}

#[derive(Debug, PartialEq)]
enum Part {
    Entry(usize),
    Pill(usize),
}

/// Inverse of `acc_index`; None when `i` is out of the `count` entries plus the pills.
fn part(nt: usize, count: usize, i: usize) -> Option<Part> {
    if i < nt {
        Some(Part::Entry(i))
    } else if i < nt + PILLS.len() {
        Some(Part::Pill(i - nt))
    } else {
        (i < count + PILLS.len()).then(|| Part::Entry(i - PILLS.len()))
    }
}

/// Tells screen readers the focused entry and whether it is selected. Call after the change, before notifying.
unsafe fn announce(hwnd: HWND, s: &State) {
    if let Some(e) = s.focus.filter(|&e| e < s.count()) {
        let i = acc_index(s.content.tiles.len(), e);
        if GetFocus() == hwnd {
            access::event(EVENT_OBJECT_FOCUS, hwnd, Some(i));
        }
        if s.selected[e] {
            access::event(EVENT_OBJECT_SELECTION, hwnd, Some(i));
        }
    }
}

/// A click on pill `i`: shows its rows and tells the parent.
unsafe fn pick_pill(hwnd: HWND, s: &mut State, i: usize) {
    s.filter = PILLS[i].0;
    s.refilter();
    sync_scroll(hwnd, s);
    let _ = InvalidateRect(Some(hwnd), None, false);
    access::event(EVENT_OBJECT_REORDER, hwnd, None);
    notify(hwnd, HN_FILTER, i);
    notify(hwnd, HN_SELCHANGE, 0);
}

/// Reads only (no layout caching, no scroll sync): a client may ask at any time.
unsafe fn acc_tree(hwnd: HWND, s: &State, t: &mut access::Tree) {
    let l = current_layout(hwnd, s);
    let nt = s.content.tiles.len();
    let up = |r: RECT| RECT { top: r.top - s.scroll, bottom: r.bottom - s.scroll, ..r };
    t.name = tr("home.window_name").into();
    t.role = ROLE_SYSTEM_LIST;
    t.focusable = true;
    t.focus = s.focus.map(|e| acc_index(nt, e));
    let item = |e: usize| {
        let x = s.entry(e)?;
        let description = if e < nt { x.subtitle.clone() } else { std::iter::once(&x.subtitle).chain(&x.cells).filter(|c| !c.is_empty()).cloned().collect::<Vec<_>>().join(", ") };
        Some(access::Item {
            name: x.name.clone(),
            description,
            role: ROLE_SYSTEM_LISTITEM,
            state: STATE_SYSTEM_SELECTABLE | if s.selected[e] { STATE_SYSTEM_SELECTED } else { 0 },
            rect: up(entry_rect(&l, nt, e)),
            action: tr("tb.action_open"),
            ..Default::default()
        })
    };
    t.items.extend((0..nt).filter_map(item));
    for (i, (f, label, _)) in PILLS.iter().enumerate() {
        t.items.push(access::Item {
            name: tr(label).into(),
            role: ROLE_SYSTEM_PAGETAB,
            state: STATE_SYSTEM_SELECTABLE | if s.filter == *f { STATE_SYSTEM_SELECTED } else { 0 },
            rect: l.pills.get(i).copied().map_or_else(RECT::default, up),
            action: tr("tb.action_select"),
            ..Default::default()
        });
    }
    t.items.extend((nt..s.count()).filter_map(item));
}

unsafe fn select_only(s: &mut State, e: Option<usize>) {
    s.selected.iter_mut().for_each(|x| *x = false);
    if let Some(e) = e {
        s.selected[e] = true;
    }
    s.focus = e.or(s.focus);
    s.anchor = e.or(s.anchor);
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
    // Read now: a notification below may destroy the page.
    let outer = s.theme.background;
    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        WM_SETFONT => {
            s.font = HFONT(wparam.0 as *mut c_void);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        WM_PAINT => paint(hwnd, s),
        WM_GETOBJECT if access::is_client(lparam) => return s.acc.get_or_insert_with(|| access::Server::new(hwnd)).answer(wparam),
        access::WM_ACC_ITEMS => {
            acc_tree(hwnd, s, &mut *(lparam.0 as *mut access::Tree));
            return LRESULT(1);
        }
        // What a click (or a double-click, for an entry) on the item does.
        access::WM_ACC_INVOKE => {
            match part(s.content.tiles.len(), s.count(), wparam.0) {
                Some(Part::Entry(e)) => {
                    cancel_rename(hwnd, s);
                    s.press = None;
                    if let Some(index) = s.entry(e).map(|x| x.index) {
                        notify(hwnd, HN_ACTIVATE, index);
                    }
                }
                Some(Part::Pill(i)) => pick_pill(hwnd, s, i),
                None => {}
            }
            return LRESULT(0);
        }
        WM_SIZE | WM_DPICHANGED_AFTERPARENT => {
            sync_scroll(hwnd, s);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        HM_COUNTS => return LRESULT(((s.content.tiles.len() << 16) | s.shown.len()) as isize),
        HM_SELECT_NAMED => {
            let mut buf = [0u16; 1024];
            let n = GetWindowTextW(hwnd, &mut buf) as usize;
            let want = String::from_utf16_lossy(&buf[..n]);
            let Some(e) = (0..s.count()).find(|&e| s.entry(e).is_some_and(|x| x.name == want || x.key.eq_ignore_ascii_case(&want))) else {
                return LRESULT(-1);
            };
            select_only(s, Some(e));
            s.focus = Some(e);
            ensure_visible(hwnd, s, e);
            let _ = InvalidateRect(Some(hwnd), None, false);
            announce(hwnd, s);
            notify(hwnd, HN_SELCHANGE, 0);
            return LRESULT(e as isize);
        }
        HM_RECT => {
            let l = current_layout(hwnd, s);
            let dpi = GetDpiForWindow(hwnd);
            let r = if wparam.0 == 0xFFFF {
                RECT { top: l.header.bottom, bottom: client(hwnd).bottom + s.scroll, ..l.header }
            } else if let Some(t) = l.tiles.get(wparam.0) {
                let sub = &s.content.tiles[wparam.0].subtitle;
                if sub.is_empty() || sub == tr(STORED_LOCALLY) {
                    return LRESULT(0);
                }
                RECT { left: t.left + scale(62, dpi), top: t.top + scale(30, dpi), right: t.right - scale(8, dpi), bottom: t.top + scale(50, dpi) }
            } else {
                return LRESULT(-1);
            };
            let p = |v: i32| (v as i16 as u16) as u64;
            return LRESULT((p(r.left) | p(r.top - s.scroll) << 16 | p(r.right) << 32 | p(r.bottom - s.scroll) << 48) as isize);
        }
        HM_ICON => {
            let bmp = HBITMAP(lparam.0 as *mut c_void);
            let (generation, slot) = (wparam.0 >> 32, wparam.0 & 0xFFFF_FFFF);
            if generation as u64 == s.generation.load(Ordering::SeqCst) && slot < s.icons.len() {
                if let Some(old) = s.icons[slot].replace(bmp) {
                    let _ = DeleteObject(old.into());
                }
                // One repaint per batch of icons, not per icon; re-arming would postpone it for as long as icons keep coming.
                if !s.icon_timer {
                    s.icon_timer = SetTimer(Some(hwnd), ICON_TIMER, 50, None) != 0;
                    if !s.icon_timer {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                }
            } else {
                let _ = DeleteObject(bmp.into());
            }
        }
        WM_TIMER if wparam.0 == ICON_TIMER => {
            let _ = KillTimer(Some(hwnd), ICON_TIMER);
            s.icon_timer = false;
            let _ = InvalidateRect(Some(hwnd), None, false);
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
            cancel_rename(hwnd, s);
            let delta = (wparam.0 >> 16) as i16 as i32;
            let y = s.scroll - delta * 3 * scale(ROW_H, GetDpiForWindow(hwnd)) / 120;
            scroll_to(hwnd, s, y);
        }
        WM_MOUSEMOVE => {
            if let Some((px, py)) = s.press {
                let (x, y) = (lparam.0 as i16 as i32, (lparam.0 >> 16) as i16 as i32);
                // MK_LBUTTON: the button is still down.
                if wparam.0 & 1 != 0 && ((x - px).abs() > GetSystemMetrics(SM_CXDRAG) || (y - py).abs() > GetSystemMetrics(SM_CYDRAG)) {
                    s.press = None;
                    cancel_rename(hwnd, s);
                    notify(hwnd, HN_DRAG, 0);
                    return LRESULT(0);
                }
            }
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
            s.press = None;
            s.hover = None;
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            cancel_rename(hwnd, s);
            s.press = None;
            let _ = SetFocus(Some(hwnd));
            if msg == WM_LBUTTONDOWN {
                let (x, y) = (lparam.0 as i16 as i32, (lparam.0 >> 16) as i16 as i32 + s.scroll);
                let l = current_layout(hwnd, s);
                if let Some(i) = l.pills.iter().position(|p| x >= p.left && x < p.right && y >= p.top && y < p.bottom) {
                    pick_pill(hwnd, s, i);
                    return LRESULT(0);
                }
            }
            let hit = entry_at(hwnd, s, lparam);
            let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
            let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
            // Clicking the name of the only selected entry again renames it (when the release is not a drag or a double-click).
            let (cx, cy) = (lparam.0 as i16 as i32, (lparam.0 >> 16) as i16 as i32);
            s.again = hit.filter(|&e| {
                msg == WM_LBUTTONDOWN && !ctrl && !shift && s.selected[e] && s.selected.iter().filter(|&&b| b).count() == 1 && s.edit.is_invalid() && {
                    let r = name_rect(hwnd, s, e);
                    cx >= r.left && cx < r.right && cy >= r.top && cy < r.bottom
                }
            });
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
            if msg == WM_LBUTTONDOWN && !ctrl && hit.is_some() {
                s.press = Some((cx, cy));
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
            announce(hwnd, s);
            notify(hwnd, HN_SELCHANGE, 0);
        }
        WM_LBUTTONUP => {
            let again = s.again.take();
            if s.press.take().is_some() && let Some(e) = again.filter(|&e| entry_at(hwnd, s, lparam) == Some(e)) {
                let _ = SetTimer(Some(hwnd), RENAME_TIMER, GetDoubleClickTime(), None);
                s.rename_at = Some(e);
            }
        }
        WM_TIMER if wparam.0 == RENAME_TIMER => {
            let _ = KillTimer(Some(hwnd), RENAME_TIMER);
            let index = s.rename_at.take().filter(|&e| s.selected.get(e) == Some(&true) && GetFocus() == hwnd).and_then(|e| s.entry(e)).map(|x| x.index);
            // begin_rename reads the state itself: `s` is not used after it.
            if let Some(index) = index {
                let _ = begin_rename(hwnd, index);
            }
        }
        WM_LBUTTONDBLCLK => {
            cancel_rename(hwnd, s);
            s.press = None;
            if let Some(index) = entry_at(hwnd, s, lparam).and_then(|e| s.entry(e)).map(|x| x.index) {
                notify(hwnd, HN_ACTIVATE, index);
            }
        }
        // Shift+F10 and the Menu key: the focused entry's menu, else the background's.
        WM_CONTEXTMENU if lparam.0 as i32 == -1 => {
            let index = s.focus.filter(|&e| s.selected[e]).and_then(|e| s.entry(e)).map_or(usize::MAX, |x| x.index);
            notify(hwnd, HN_CONTEXT_KEY, index);
        }
        WM_RBUTTONUP => {
            let index = entry_at(hwnd, s, lparam).and_then(|e| s.entry(e)).map_or(usize::MAX, |x| x.index);
            notify(hwnd, HN_CONTEXT, index);
        }
        WM_KEYDOWN => {
            cancel_rename(hwnd, s);
            let key = VIRTUAL_KEY(wparam.0 as u16);
            let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
            let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
            match key {
                VK_UP | VK_DOWN | VK_LEFT | VK_RIGHT | VK_HOME | VK_END | VK_PRIOR | VK_NEXT => {
                    let (n, nt) = (s.count(), s.content.tiles.len());
                    let l = current_layout(hwnd, s);
                    let to = match key {
                        // A page: Up/Down until the entry's top has moved by the page's height.
                        VK_PRIOR | VK_NEXT => {
                            let (arrow, page) = if key == VK_NEXT { (VK_DOWN, 1) } else { (VK_UP, -1) };
                            let (mut e, top0) = (s.focus, s.focus.map_or(0, |f| entry_rect(&l, nt, f).top));
                            while let Some(next) = step(n, nt, l.per_row, e, arrow).filter(|&x| Some(x) != e) {
                                e = Some(next);
                                if (entry_rect(&l, nt, next).top - top0) * page >= client(hwnd).bottom {
                                    break;
                                }
                            }
                            e
                        }
                        _ => step(n, nt, l.per_row, s.focus, key),
                    };
                    if let Some(e) = to {
                        key_select(&mut s.selected, &mut s.anchor, e, ctrl, shift);
                        s.focus = Some(e);
                        ensure_visible(hwnd, s, e);
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    announce(hwnd, s);
                    notify(hwnd, HN_SELCHANGE, 0);
                }
                VK_SPACE => {
                    if let Some(f) = s.focus {
                        if ctrl {
                            s.selected[f] = !s.selected[f];
                            s.anchor = Some(f);
                        } else {
                            select_only(s, Some(f));
                        }
                        let _ = InvalidateRect(Some(hwnd), None, false);
                        announce(hwnd, s);
                        notify(hwnd, HN_SELCHANGE, 0);
                    }
                }
                VK_A if ctrl => {
                    let n = s.content.tiles.len();
                    for e in 0..s.count() {
                        s.selected[e] = e >= n;
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    announce(hwnd, s);
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
        WM_CHAR => {
            let now = GetMessageTime() as u32;
            let (typed, needle) = crate::list::type_ahead(&s.typed, s.typed_at, now, char::from_u32(wparam.0 as u32).unwrap_or('\0'));
            if !needle.is_empty() {
                s.typed = typed;
                s.typed_at = now;
                let start = if needle.chars().count() == 1 { s.focus.map_or(0, |f| f + 1) } else { s.focus.unwrap_or(0) };
                let found = crate::list::find_prefix(s.count(), start, true, &needle, |e| s.entry(e).map(|x| x.name.clone()).unwrap_or_default());
                if let Some(e) = found {
                    select_only(s, Some(e));
                    ensure_visible(hwnd, s, e);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    announce(hwnd, s);
                    notify(hwnd, HN_SELCHANGE, 0);
                }
            }
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            if msg == WM_KILLFOCUS {
                cancel_rename(hwnd, s);
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
            if msg == WM_SETFOCUS {
                announce(hwnd, s);
            }
        }
        _ => {
            let r = DefWindowProcW(hwnd, msg, wparam, lparam);
            theme::card_message(hwnd, msg, outer);
            return r;
        }
    }
    theme::card_message(hwnd, msg, outer);
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
    let picked = |e: usize| s.selected.get(e) == Some(&true);
    // Contrast: the keyboard focus is a rectangle, hover an outline, selection always the full HIGHLIGHT.
    let hot = |dc: HDC, e: usize, r: &RECT, radius: i32| {
        if s.hover == Some(e) && !picked(e) {
            theme::hot_outline(dc, r, t, radius, dpi);
        }
        // A selected entry shows its fill; the outline marks one Ctrl+arrows moved to without selecting (always in a contrast theme).
        if focused && s.focus == Some(e) && (t.contrast.is_some() || !picked(e)) {
            SetTextColor(dc, COLORREF(t.text));
            SetBkColor(dc, COLORREF(t.surface));
            let _ = DrawFocusRect(dc, r);
        }
    };
    let fill_for = |e: usize| -> Option<u32> {
        if picked(e) {
            Some(if focused || t.contrast.is_some() { t.selection } else { theme::blend(t.selection, t.surface, 0.4) })
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
    text(dc, tr("home.quick_access"), RECT { left: title.left + sc(24), ..title }, one);

    let n = s.content.tiles.len();
    for (i, e) in s.content.tiles.iter().enumerate() {
        let r = dy(l.tiles[i]);
        if r.bottom < 0 || r.top > ch {
            continue;
        }
        if let Some(c) = fill_for(i) {
            theme::fill_round(dc, &r, c, sc(8));
        }
        hot(dc, i, &r, sc(8));
        let sel = |c: u32| if picked(i) { theme::on_selection(t, c) } else { c };
        let (icon, x) = (sc(40), r.left + sc(12));
        match s.icon(i) {
            Some(b) => draw_bitmap(dc, b, x, r.top + (r.bottom - r.top - icon) / 2, icon),
            None => {
                let ph = RECT { left: x, top: r.top + sc(16), right: x + icon, bottom: r.top + sc(56) };
                theme::fill_round(dc, &ph, theme::hover(t), sc(6));
                if let Some(c) = t.contrast {
                    theme::outline_round(dc, &ph, c.gray, sc(6), 1);
                }
            }
        }
        let tx = r.left + sc(62);
        SelectObject(dc, s.fonts.bold.into());
        SetTextColor(dc, COLORREF(sel(t.text)));
        text(dc, &e.name, RECT { left: tx, top: r.top + sc(8), right: r.right - sc(8), bottom: r.top + sc(32) }, one);
        SelectObject(dc, ui.into());
        SetTextColor(dc, COLORREF(sel(t.secondary_text)));
        text(dc, &e.subtitle, RECT { left: tx, top: r.top + sc(30), right: r.right - sc(8), bottom: r.top + sc(50) }, one);
        if e.pinned {
            SelectObject(dc, s.fonts.small_glyph.into());
            text(dc, "\u{E718}", RECT { left: tx, top: r.top + sc(50), right: tx + sc(14), bottom: r.top + sc(64) }, one);
        }
    }

    // The pills (the active one filled), the header, the rows.
    SelectObject(dc, ui.into());
    for (p, (f, label, _)) in l.pills.iter().zip(PILLS) {
        let p = dy(*p);
        if f == s.filter {
            theme::fill_round(dc, &p, t.selection, (p.bottom - p.top) / 2);
        }
        SetTextColor(dc, COLORREF(if f == s.filter { theme::on_selection(t, t.text) } else { t.secondary_text }));
        text(dc, tr(label), p, one | DT_CENTER);
    }
    let header = dy(l.header);
    let cols = column_lefts(header.left, header.right, s.content.columns.len().max(1));
    let col_right = |c: usize| cols.get(c + 1).copied().unwrap_or(header.right) - sc(8);
    SetTextColor(dc, COLORREF(t.secondary_text));
    for (c, title) in s.content.columns.iter().enumerate() {
        let left = cols[c] + if c == 0 { sc(12) } else { 0 };
        text(dc, title, RECT { left, right: col_right(c), ..header }, one);
    }
    if s.shown.is_empty() {
        let empty = PILLS.iter().find(|p| p.0 == s.filter).map_or("", |p| tr(p.2));
        text(dc, empty, RECT { left: header.left + sc(12), ..dy(l.empty) }, one);
    }
    for (i, e) in s.shown.iter().map(|&r| &s.content.rows[r]).enumerate() {
        let r = dy(l.rows[i]);
        if r.bottom < 0 || r.top > ch {
            continue;
        }
        let pill = RECT { top: r.top + sc(1), bottom: r.bottom - sc(1), ..r };
        if let Some(c) = fill_for(n + i) {
            theme::fill_round(dc, &pill, c, sc(6));
        }
        hot(dc, n + i, &pill, sc(6));
        let sel = |c: u32| if picked(n + i) { theme::on_selection(t, c) } else { c };
        let (icon, x) = (sc(20), r.left + sc(12));
        if let Some(b) = s.icon(n + i) {
            draw_bitmap(dc, b, x, r.top + (r.bottom - r.top - icon) / 2, icon);
        }
        let tx = r.left + sc(42);
        SetTextColor(dc, COLORREF(sel(t.text)));
        text(dc, &e.name, RECT { left: tx, top: r.top + sc(4), right: col_right(0), bottom: r.top + sc(26) }, one);
        SetTextColor(dc, COLORREF(sel(t.secondary_text)));
        text(dc, &e.subtitle, RECT { left: tx, top: r.top + sc(24), right: col_right(0), bottom: r.top + sc(44) }, one);
        SetTextColor(dc, COLORREF(sel(t.text)));
        for (c, cell) in e.cells.iter().enumerate().take(cols.len().saturating_sub(1)) {
            text(dc, cell, RECT { left: cols[c + 1], right: col_right(c + 1), ..r }, one);
        }
    }

    theme::paint_card_corners(hwnd, hwnd, dc, t.background);
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
    fn tile_key_at_screen_tiles_only() {
        let _c = ComInit::sta();
        let abs = ShellFolder::parse(std::env::temp_dir().to_str().unwrap()).unwrap().pidl;
        let entry = |index: usize, key: &str, recent: bool, pinned: bool| Entry {
            index,
            key: key.into(),
            name: String::new(),
            subtitle: String::new(),
            cells: Vec::new(),
            abs: abs.clone(),
            pinned,
            recent,
            shared: false,
        };
        unsafe {
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 1000, 700, None, None, None, None).unwrap();
            let h = create(parent, 9).unwrap();
            let _ = MoveWindow(h, 0, 0, 956, 600, false);
            set_content(h, Content { columns: vec!["Name".into()], tiles: vec![entry(0, "C:/t", false, true)], rows: vec![entry(1, "C:/r.txt", true, false)] });
            let l = current_layout(h, state(h).unwrap());
            let screen = |r: RECT| {
                let mut p = windows::Win32::Foundation::POINT { x: r.left + 10, y: r.top + 10 };
                let _ = ClientToScreen(h, &mut p);
                p
            };
            let p = screen(l.tiles[0]);
            assert_eq!(tile_key_at_screen(h, p.x, p.y), Some("C:/t".to_string()));
            let p = screen(l.rows[0]);
            assert_eq!(tile_key_at_screen(h, p.x, p.y), None);
            let _ = DestroyWindow(parent);
        }
    }

    #[test]
    fn key_select_rules() {
        let (mut sel, mut anchor) = ([false; 5], None);
        let on = |s: &[bool; 5]| (0..5).filter(|&i| s[i]).collect::<Vec<_>>();
        key_select(&mut sel, &mut anchor, 2, false, false);
        assert_eq!((on(&sel), anchor), (vec![2], Some(2)));
        key_select(&mut sel, &mut anchor, 4, false, true);
        assert_eq!((on(&sel), anchor), (vec![2, 3, 4], Some(2)));
        key_select(&mut sel, &mut anchor, 0, false, true);
        assert_eq!(on(&sel), vec![0, 1, 2]);
        key_select(&mut sel, &mut anchor, 3, true, false);
        assert_eq!(on(&sel), vec![0, 1, 2], "Ctrl alone only moves the outline");
        key_select(&mut sel, &mut anchor, 4, true, true);
        assert_eq!(on(&sel), vec![0, 1, 2, 3, 4]);
        // An anchor left from longer content (the recent list shrank) extends from the target instead.
        let mut anchor = Some(9);
        key_select(&mut sel, &mut anchor, 1, false, true);
        assert_eq!(on(&sel), vec![1]);
    }

    #[test]
    fn subtitles_relative_to_the_profile() {
        let p = r"C:\Users\v";
        assert_eq!(parent_label(r"C:\Users\v\Escritorio\project\dist", p), r"Escritorio\project");
        assert_eq!(parent_label(r"C:\Users\v\Downloads", p), "Stored locally");
        assert_eq!(parent_label(r"C:\Users\v\Downloads\a.txt", r"C:\Users\v\"), "Downloads");
        assert_eq!(parent_label(r"C:\Users\vx\a.txt", p), r"C:\Users\vx", "a name prefix is no parent");
        assert_eq!(parent_label(r"D:\x\y.txt", p), r"D:\x");
        assert_eq!(parent_label("::{645FF040-5081-101B-9F08-00AA002F954E}", p), "");
        assert_eq!(parent_label("a.mp4", p), "");
        assert_eq!(parent_label(r"C:\Users\vé\ñ\a.txt", r"C:\Users\v"), r"C:\Users\vé\ñ");
    }

    #[test]
    fn layout_wraps_tiles_and_stacks_sections() {
        // 2 * 20 padding + 3 tiles of 280 with gaps of 8: 896 px hold three per row.
        let l = layout(956, 4, 2, &[80, 90, 70], 96);
        assert_eq!((l.tiles[0].left, l.tiles[2].left, l.tiles[3].left), (20, 596, 20));
        assert_eq!(l.tiles[3].top, l.tiles[0].bottom + 8);
        assert_eq!(l.pills[0].top, l.tiles[3].bottom + 20);
        assert_eq!((l.pills[0].right - l.pills[0].left, l.pills[1].left, l.pills[2].left), (80, 108, 206), "side by side, 8 apart");
        assert_eq!(l.rows[0].top, l.header.bottom);
        assert_eq!(l.rows[1].bottom - l.rows[1].top, 48);
        assert_eq!(l.height, l.rows[1].bottom + 20);
        // Too narrow for one: still one per row.
        let l = layout(200, 2, 0, &[80], 96);
        assert_eq!((l.tiles[1].left, l.tiles[1].top), (20, l.tiles[0].bottom + 8));
        assert_eq!(l.empty.bottom - l.empty.top, 48, "the empty line takes a row");
        assert_eq!(layout(956, 4, 2, &[80], 192).tiles[0].right - layout(956, 4, 2, &[80], 192).tiles[0].left, 560);
    }

    #[test]
    fn tiles_flow_as_wide_as_fits() {
        let per = |w, dpi| layout(w, 9, 0, &[], dpi).per_row;
        assert_eq!([per(300, 96), per(700, 96), per(1400, 96)], [1, 2, 4]);
        assert_eq!([per(300, 144), per(700, 144), per(1400, 144)], [1, 1, 3]);
        // Every cell inside the content, none overlapping, a row's tiles on one line, and the height covers the last.
        for (w, dpi) in [(300, 96), (700, 96), (1400, 96), (700, 144), (1400, 144)] {
            let l = layout(w, 9, 3, &[80], dpi);
            for (i, r) in l.tiles.iter().enumerate() {
                assert!(r.left >= 20 * dpi as i32 / 96 && r.bottom <= l.height);
                assert_eq!((r.top == l.tiles[0].top), i < l.per_row);
                assert_eq!(r.left == l.tiles[0].left, i % l.per_row == 0);
                assert!(l.tiles[..i].iter().all(|o| o.right <= r.left || o.left >= r.right || o.bottom <= r.top || o.top >= r.bottom));
            }
            assert!(l.pills[0].top >= l.tiles[8].bottom);
        }
    }

    #[test]
    fn keys_step_across_tiles_then_rows() {
        assert_eq!(step(5, 5, 1, None, VK_DOWN), Some(0));
        assert_eq!(step(5, 5, 1, Some(0), VK_DOWN), Some(1));
        assert_eq!(step(5, 5, 1, Some(4), VK_DOWN), Some(4));
        assert_eq!(step(5, 5, 1, Some(0), VK_UP), Some(0));
        assert_eq!(step(5, 5, 1, Some(3), VK_LEFT), Some(2));
        assert_eq!(step(5, 5, 1, Some(2), VK_END), Some(4));
        assert_eq!(step(5, 5, 1, Some(2), VK_HOME), Some(0));
        assert_eq!(step(0, 0, 1, None, VK_DOWN), None);
        // No tiles at all (nothing pinned, frequent folders hidden): the first row stays put.
        assert_eq!((step(3, 0, 3, Some(0), VK_UP), step(3, 0, 3, Some(0), VK_PRIOR)), (Some(0), Some(0)));
        // 7 tiles in rows of 3 (0 1 2 / 3 4 5 / 6), then 2 table rows (7, 8).
        let k = |f, key| step(9, 7, 3, Some(f), key);
        assert_eq!((k(1, VK_DOWN), k(4, VK_DOWN), k(5, VK_DOWN)), (Some(4), Some(6), Some(6)), "a short last row: its last tile");
        assert_eq!((k(0, VK_DOWN), k(4, VK_UP), k(1, VK_UP), k(2, VK_RIGHT), k(3, VK_LEFT)), (Some(3), Some(1), Some(1), Some(3), Some(2)));
        assert_eq!((k(6, VK_DOWN), k(7, VK_UP), k(7, VK_DOWN), k(8, VK_UP), k(8, VK_DOWN)), (Some(7), Some(6), Some(8), Some(7), Some(8)));
        // The first table row goes up to the first tile of the last row of tiles.
        assert_eq!(step(9, 6, 3, Some(6), VK_UP), Some(3));
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
        // ShellItem::property_bool reads System.Home.IsPinned, a VT_BOOL, on every folder of Home.
        assert!(items.iter().filter(|i| i.is_folder()).all(|i| i.property_bool(&IS_PINNED).is_some()));
        let mut c = build(&home, &items);
        assert!(!c.tiles.is_empty(), "Home has pinned folders");
        assert_eq!(c.columns.len(), 5, "Name, Date modified, Type, Size, Activity");
        assert!(c.rows.iter().all(|r| r.cells.len() == 4));
        for (i, e) in c.tiles.iter_mut().chain(c.rows.iter_mut()).enumerate() {
            e.index = 100 + i;
        }
        let nt = c.tiles.len();
        // The Recent pill shows the recent rows.
        let recent: Vec<usize> = c.rows.iter().filter(|r| r.recent).map(|r| r.index).collect();
        let favorites = c.rows.iter().filter(|r| r.pinned).count();
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap().into();
            let wc = WNDCLASSW { lpfnWndProc: Some(parent_proc), hInstance: hinst, lpszClassName: w!("RexHomeTestParent"), ..Default::default() };
            RegisterClassW(&wc);
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("RexHomeTestParent"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 1000, 700, None, None, Some(hinst), None).unwrap();
            let h = create(parent, 9).unwrap();
            let _ = MoveWindow(h, 0, 0, 956, 600, false);
            set_theme(h, &Theme::dark());
            let reload = c.clone();
            set_content(h, c);
            assert_eq!(SendMessageW(h, HM_COUNTS, None, None).0 as usize, (nt << 16) | recent.len());
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
            assert_eq!(selection(h).1, Some(recent.last().copied().unwrap_or(100 + nt - 1)));
            // Space selects the focused entry alone.
            let focus = state(h).unwrap().focus.unwrap();
            SendMessageW(h, WM_KEYDOWN, Some(WPARAM(VK_SPACE.0 as usize)), None);
            assert_eq!(state(h).unwrap().selected.iter().enumerate().filter(|(_, on)| **on).map(|(e, _)| e).collect::<Vec<_>>(), vec![focus]);
            // A letter jumps to the next entry whose name starts with it, from the entry after the focus.
            let names: Vec<String> = (0..state(h).unwrap().count()).map(|e| state(h).unwrap().entry(e).unwrap().name.clone()).collect();
            if let Some(letter) = names[0].chars().next().filter(|c| c.is_alphanumeric()) {
                let want = crate::list::find_prefix(names.len(), focus + 1, true, &letter.to_lowercase().to_string(), |e| names[e].clone());
                SendMessageW(h, WM_CHAR, Some(WPARAM(letter.to_lowercase().next().unwrap() as usize)), None);
                assert_eq!(state(h).unwrap().focus, want);
            }
            SendMessageW(h, WM_KEYDOWN, Some(WPARAM(VK_END.0 as usize)), None);
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
            // A click on the Favorites pill shows the favorite files.
            let s = state(h).unwrap();
            let p = current_layout(h, s).pills[1];
            let at = LPARAM((((p.top - s.scroll + 5) as isize) << 16) | (p.left + 5) as isize);
            LAST.lock().unwrap().clear();
            SendMessageW(h, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(at));
            assert_eq!(filter(h), Filter::Favorites);
            assert_eq!(LAST.lock().unwrap().first(), Some(&(HN_FILTER, 1)));
            assert_eq!(SendMessageW(h, HM_COUNTS, None, None).0 as usize & 0xFFFF, favorites);
            // The test hook selects an entry by its name.
            let name = HSTRING::from(state(h).unwrap().content.tiles[0].name.as_str());
            let _ = SetWindowTextW(h, PCWSTR(name.as_ptr()));
            assert_eq!(SendMessageW(h, HM_SELECT_NAMED, None, None).0, 0);
            assert_eq!(selection(h).0, vec![100]);
            // A press on an entry, then the pointer moving past the drag size with the button down, starts a drag.
            let drag = |dx: i32, keys: usize| {
                let l = current_layout(h, state(h).unwrap());
                let r = l.tiles[0];
                let (x, y) = (r.left + 10, r.top + 10);
                SendMessageW(h, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(LPARAM(((y as isize) << 16) | x as isize)));
                LAST.lock().unwrap().clear();
                SendMessageW(h, WM_MOUSEMOVE, Some(WPARAM(keys)), Some(LPARAM(((y as isize) << 16) | (x + dx) as isize)));
                SendMessageW(h, WM_LBUTTONUP, None, None);
                LAST.lock().unwrap().contains(&(HN_DRAG, 0))
            };
            assert!(drag(40, 1), "far with the button down");
            assert!(!drag(1, 1), "within the drag size");
            assert!(!drag(40, 0), "button up");
            // A reload between the press and the move drops the press: the entry under it may be another item now.
            let r = current_layout(h, state(h).unwrap()).tiles[0];
            let (x, y) = (r.left + 10, r.top + 10);
            SendMessageW(h, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(LPARAM(((y as isize) << 16) | x as isize)));
            set_content(h, reload);
            LAST.lock().unwrap().clear();
            SendMessageW(h, WM_MOUSEMOVE, Some(WPARAM(1)), Some(LPARAM(((y as isize) << 16) | (x + 40) as isize)));
            assert!(!LAST.lock().unwrap().contains(&(HN_DRAG, 0)));
            assert!(state(h).unwrap().again.is_none() && state(h).unwrap().rename_at.is_none());
            DestroyWindow(parent).unwrap();
        }
    }

    thread_local! {
        static ACC_NOTES: std::cell::RefCell<Vec<(u32, usize)>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    unsafe extern "system" fn acc_parent(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_NOTIFY {
            let nm = &*(lparam.0 as *const NmHome);
            ACC_NOTES.with(|n| n.borrow_mut().push((nm.hdr.code, nm.index)));
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    #[test]
    fn acc_index_and_part_are_inverse() {
        for nt in [0, 1, 3] {
            for e in 0..6 {
                assert_eq!(part(nt, 6, acc_index(nt, e)), Some(Part::Entry(e)));
            }
            for p in 0..3 {
                assert_eq!(part(nt, 6, nt + p), Some(Part::Pill(p)));
            }
            assert_eq!(part(nt, 6, 9), None);
        }
    }

    #[test]
    fn accessible_tiles_pills_and_rows() {
        use crate::acc_test::*;
        use windows::Win32::UI::Accessibility::IAccessible;
        let _com = ComInit::sta();
        let abs = ShellFolder::parse(std::env::temp_dir().to_str().unwrap()).unwrap().pidl;
        let entry = |index: usize, name: &str, subtitle: &str, recent: bool| Entry {
            index,
            key: format!("C:/{name}"),
            name: name.into(),
            subtitle: subtitle.into(),
            cells: if recent { vec!["x".into()] } else { Vec::new() },
            abs: abs.clone(),
            pinned: false,
            recent,
            shared: false,
        };
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap().into();
            let wc = WNDCLASSW { lpfnWndProc: Some(acc_parent), hInstance: hinst, lpszClassName: w!("RexHomeAccParent"), ..Default::default() };
            RegisterClassW(&wc);
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("RexHomeAccParent"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 1000, 700, None, None, Some(hinst), None).unwrap();
            let h = create(parent, 9).unwrap();
            let _ = MoveWindow(h, 0, 0, 956, 600, false);
            set_content(
                h,
                Content {
                    columns: vec!["Name".into(), "Date modified".into()],
                    tiles: vec![entry(100, "T0", "", false), entry(101, "T1", "Docs", false)],
                    rows: vec![entry(102, "r0.txt", "", true), entry(103, "r1.txt", "", true)],
                },
            );
            let acc = object(h);
            let k = kids(&acc);
            assert_eq!(k.iter().map(|k| k.name.as_str()).collect::<Vec<_>>(), ["T0", "T1", "Recent", "Favorites", "Shared", "r0.txt", "r1.txt"]);
            assert_eq!(
                k.iter().map(|k| k.role).collect::<Vec<_>>(),
                [ROLE_SYSTEM_LISTITEM, ROLE_SYSTEM_LISTITEM, ROLE_SYSTEM_PAGETAB, ROLE_SYSTEM_PAGETAB, ROLE_SYSTEM_PAGETAB, ROLE_SYSTEM_LISTITEM, ROLE_SYSTEM_LISTITEM]
            );
            assert_ne!(k[2].state & STATE_SYSTEM_SELECTED, 0);
            assert_eq!(k[3].state & STATE_SYSTEM_SELECTED, 0);
            assert_eq!(k[1].description, "Docs");
            assert_eq!(k[5].description, "x");
            let want = screen(h, entry_rect(&current_layout(h, state(h).unwrap()), 2, 0));
            assert_eq!((k[0].rect.left, k[0].rect.top, k[0].rect.right, k[0].rect.bottom), (want.left, want.top, want.right, want.bottom));
            let r0 = root(&acc);
            assert_eq!((r0.name.as_str(), r0.role), ("Home", ROLE_SYSTEM_LIST));
            assert_ne!(r0.state & windows::Win32::UI::Controls::STATE_SYSTEM_FOCUSABLE.0, 0);
            let hk = hook();
            set_selection(h, &[101], Some(101));
            let sel = |acc: &IAccessible| kids(acc).iter().map(|k| k.state & STATE_SYSTEM_SELECTED != 0).collect::<Vec<_>>();
            assert_eq!(&sel(&acc)[..2], [false, true]);
            assert!(hk.take().contains(&(EVENT_OBJECT_SELECTION, h, 2)));
            SendMessageW(h, WM_KEYDOWN, Some(WPARAM(VK_LEFT.0 as usize)), None);
            assert!(hk.take().contains(&(EVENT_OBJECT_SELECTION, h, 1)));
            // A row's id counts the tiles and the three pills before it.
            set_selection(h, &[102], Some(102));
            assert!(hk.take().contains(&(EVENT_OBJECT_SELECTION, h, 6)));
            assert_eq!(&sel(&acc)[5..], [true, false]);
            drop(hk);
            ACC_NOTES.with(|n| n.borrow_mut().clear());
            invoke(&acc, 2).unwrap();
            assert_eq!(ACC_NOTES.with(|n| n.borrow().clone()), [(HN_ACTIVATE, 101)]);
            ACC_NOTES.with(|n| n.borrow_mut().clear());
            let hk = hook();
            invoke(&acc, 4).unwrap();
            assert_eq!(ACC_NOTES.with(|n| n.borrow().clone()), [(HN_FILTER, 1), (HN_SELCHANGE, 0)]);
            assert_eq!(filter(h), Filter::Favorites);
            assert_eq!(kids(&acc).len(), 5);
            assert!(hk.take().contains(&(EVENT_OBJECT_REORDER, h, 0)));
            drop(hk);
            DestroyWindow(parent).unwrap();
            assert_eq!(acc.accChildCount().unwrap_err().code(), windows::Win32::Foundation::CO_E_OBJNOTCONNECTED);
        }
    }
}
