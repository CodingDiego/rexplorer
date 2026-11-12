//! Window chrome: child controls, theme and fonts, layout, the address Edit, popup menus, palette and status.
//!
//! Top to bottom: the tab strip; a 44 px header bar (Back / Forward segment, Up, Refresh, the path bar, the
//! search field, Terminal, Connect); the 44 px command bar (commands.rs); the sidebar (or tree) beside the list
//! (or Home page) and preview cards; the status bar. The frame, header, sidebar and status bar share `theme.background`; cards and fields are
//! `theme.surface`.

use super::*;
use rexplorer_core::i18n::{tr, trf};
use crate::nav::{Target, cur, go, listing, source};
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Gdi::{COLOR_MENUTEXT, CreateCompatibleDC, DT_NOCLIP, DeleteDC, GdiFlush, GetSysColor, HBITMAP};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DrawTextW, EndPaint, FillRect, PAINTSTRUCT, SelectObject,
    SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::Storage::FileSystem::GetDriveTypeW;
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN};
use windows::Win32::System::WindowsProgramming::DRIVE_REMOTE;

/// A tooltip: the translated name, then the untranslated key hint in parentheses (the accessible name splits there).
pub(crate) fn tip(key: &'static str, hint: &str) -> String {
    format!("{} ({hint})", tr(key))
}

/// A menu entry's text: the translated name, a tab, then the untranslated key hint.
pub(crate) fn hinted(key: &'static str, hint: &str) -> String {
    format!("{}\t{hint}", tr(key))
}

pub(crate) const THIS_PC: &str = "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}";
const HEADER_H: i32 = 44;
const CONTROL_H: i32 = 32;
const SEARCH_W: i32 = 220;
const PAD: i32 = 8;

/// Cloud-sync roots for the sidebar, the palette and the More menu, probed once by the sidebar worker (volume
/// probing can stall on dead drives, so the UI thread only reads what is there).
static CLOUD: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();

/// Probes on first use: call on a worker thread.
pub(crate) fn cloud_roots() -> &'static [(String, String)] {
    CLOUD.get_or_init(connect::cloud_roots)
}

fn scale(px: i32, dpi: u32) -> i32 {
    px * dpi as i32 / 96
}

/// Puts the calling worker thread in background mode (lower CPU, I/O and memory priority): speculative work yields to the user.
pub(crate) fn background_priority() {
    let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) };
}

/// A drive path or UNC path: something a terminal can start in.
pub(crate) fn is_fs_path(p: &str) -> bool {
    let b = p.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':') || p.starts_with("\\\\")
}

/// A network path: a UNC name, or a drive letter mapped to a share.
pub(crate) fn is_remote(p: &str) -> bool {
    let drive = p.get(1..3) == Some(":\\");
    p.starts_with("\\\\") || (drive && unsafe { GetDriveTypeW(&HSTRING::from(&p[..3])) } == DRIVE_REMOTE)
}

pub(crate) unsafe fn create_children(app: *mut App) -> Result<()> {
    let hwnd = (*app).hwnd;
    let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
    (*app).strip = tabs::create(hwnd, ID_TABS)?;
    let (back, forward, up, refresh) = (tip("tb.back", "Alt+Left"), tip("tb.forward", "Alt+Right"), tip("tb.up", "Alt+Up"), tip("tb.refresh", "F5"));
    (*app).nav = toolbar::create(hwnd, ID_NAV, &[(0xE72B, CMD_BACK, &back), (0xE72A, CMD_FORWARD, &forward), (0xE74A, CMD_UP, &up), (0xE72C, CMD_REFRESH, &refresh)])?;
    let _ = SetWindowTextW((*app).nav, &HSTRING::from(tr("tb.nav_name"))); // the accessible name
    toolbar::set_group((*app).nav, &[CMD_BACK, CMD_FORWARD]);
    (*app).crumbs = breadcrumb::create(hwnd, ID_CRUMBS)?;
    // The address Edit sits on top of the breadcrumb while editing; keep the breadcrumb from painting over it.
    let style = GetWindowLongPtrW((*app).crumbs, GWL_STYLE);
    SetWindowLongPtrW((*app).crumbs, GWL_STYLE, style | WS_CLIPSIBLINGS.0 as isize);
    (*app).addr = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        WC_EDITW,
        PCWSTR::null(),
        WS_CHILD | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        0,
        0,
        0,
        0,
        Some(hwnd),
        Some(HMENU(ID_ADDR as *mut c_void)),
        Some(hinst),
        None,
    )?;
    (*app).search = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        WC_EDITW,
        PCWSTR::null(),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        0,
        0,
        0,
        0,
        Some(hwnd),
        Some(HMENU(ID_SEARCH as *mut c_void)),
        Some(hinst),
        None,
    )?;
    // wparam 1: the cue stays visible while the box has the focus and is empty.
    let cue = HSTRING::from(tr("tb.search_cue"));
    SendMessageW((*app).search, EM_SETCUEBANNER, Some(WPARAM(1)), Some(LPARAM(cue.as_ptr() as isize)));
    // Neither box has a label next to it, so screen readers would announce a nameless edit (#25).
    let _ = rexplorer_core::access::set_name((*app).search, tr("tb.search_cue"));
    let _ = rexplorer_core::access::set_name((*app).addr, tr("tb.address_name"));
    let _ = SetWindowSubclass((*app).search, Some(source::search_subclass), 2, app as usize);
    edit::attach((*app).search);
    let _ = edit::autocomplete((*app).addr); // before attach: the word-delete subclass must stay outermost
    edit::attach((*app).addr);
    let terminal = tip("tb.terminal", "Ctrl+`");
    (*app).tools = toolbar::create(hwnd, ID_TOOLS, &[(0xE756, CMD_TERMINAL, &terminal), (0xE968, CMD_CONNECT, tr("tb.connect"))])?;
    let _ = SetWindowTextW((*app).tools, &HSTRING::from(tr("tb.tools_name")));
    (*app).cmdbar = commands::create(hwnd)?;
    let _ = windows::Win32::System::DataExchange::AddClipboardFormatListener(hwnd);
    (*app).sidebar = sidebar::create(hwnd, ID_SIDEBAR)?;
    if !(*app).session.show_tree && (*app).session.show_sidebar {
        let _ = ShowWindow((*app).sidebar, SW_SHOW);
    }
    let list_style = LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS | LVS_EDITLABELS | LVS_SHAREIMAGELISTS;
    (*app).list = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        WC_LISTVIEWW,
        PCWSTR::null(),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(list_style),
        0,
        0,
        0,
        0,
        Some(hwnd),
        Some(HMENU(ID_LIST as *mut c_void)),
        Some(hinst),
        None,
    )?;
    // The profile's styles come from sync_list_style (after apply_theme: WM_THEMECHANGED replaces the state list).
    let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_HEADERDRAGDROP;
    SendMessageW((*app).list, LVM_SETEXTENDEDLISTVIEWSTYLE, Some(WPARAM(0)), Some(LPARAM(ex as isize)));
    let _ = SetWindowSubclass((*app).list, Some(list::subclass), 1, app as usize);
    let _ = rexplorer_core::access::set_name((*app).list, tr("tb.items_name"));
    let header = HWND(SendMessageW((*app).list, LVM_GETHEADER, None, None).0 as *mut c_void);
    // The header only reports a hot column (draw_header's hover fill) with HDS_HOTTRACK, which the ListView leaves off.
    SetWindowLongPtrW(header, GWL_STYLE, GetWindowLongPtrW(header, GWL_STYLE) | HDS_HOTTRACK as isize);
    let _ = SetWindowSubclass(header, Some(list::header_subclass), 1, 0);
    list::register_drop(app);
    (*app).home = home::create(hwnd, ID_HOME)?;
    list::register_folder_drops(app);
    panes::create_preview(app)?;
    if (*app).session.show_preview || (*app).session.show_details {
        let _ = ShowWindow((*app).panes.preview, SW_SHOW);
    }
    (*app).status = statusbar::create(hwnd, ID_STATUS)?;
    apply_theme(app);
    sync_list_style(app);
    apply_font(app);
    views::init(app);
    panes::refresh_sidebar(app);
    // A cut made before launch fades too.
    list::refresh_cut(app);
    Ok(())
}


/// The list's extended styles that follow the profile: single click with its underlining, check boxes.
pub(crate) const LIST_EX_PROFILE: u32 = LVS_EX_ONECLICKACTIVATE | LVS_EX_UNDERLINEHOT | LVS_EX_UNDERLINECOLD | LVS_EX_CHECKBOXES;

pub(crate) fn list_ex(p: &Profile) -> u32 {
    let underline = if p.underline_titles == "always" { LVS_EX_UNDERLINEHOT | LVS_EX_UNDERLINECOLD } else { LVS_EX_UNDERLINEHOT };
    (if p.single_click { LVS_EX_ONECLICKACTIVATE | underline } else { 0 }) | if p.item_checkboxes { LVS_EX_CHECKBOXES } else { 0 }
}

/// Sets the profile's extended list styles; the check boxes' state images come from our own dispinfo answers.
pub(crate) unsafe fn sync_list_style(app: *mut App) {
    let p = &(*app).profile;
    let list = (*app).list;
    // The control owns the state image list it holds even with LVS_SHAREIMAGELISTS: it destroys it when the check
    // box style is cleared or the theme changes. So each list we hand it is a fresh one, never kept on our side.
    SendMessageW(list, LVM_SETEXTENDEDLISTVIEWSTYLE, Some(WPARAM(LIST_EX_PROFILE as usize)), Some(LPARAM(list_ex(p) as isize)));
    // LVIS_CUT: cut files show a faded icon (fill_dispinfo).
    SendMessageW(list, LVM_SETCALLBACKMASK, Some(WPARAM((LVIS_CUT.0 | if p.item_checkboxes { LVIS_STATEIMAGEMASK.0 } else { 0 }) as usize)), None);
    if p.item_checkboxes {
        let checks = check_images(&(*app).theme, GetDpiForWindow((*app).hwnd) as i32);
        let auto = HIMAGELIST(SendMessageW(list, LVM_SETIMAGELIST, Some(WPARAM(LVSIL_STATE as usize)), Some(LPARAM(checks.0))).0);
        if auto.0 != 0 && auto != checks {
            let _ = ImageList_Destroy(Some(auto));
        }
    }
}

/// The list's check box state images (1 unchecked, 2 checked) in the theme's colors: the system's stay light
/// on the dark list.
unsafe fn check_images(t: &Theme, dpi: i32) -> HIMAGELIST {
    const KEY: u32 = 0x00FF_00FF;
    let g = 16 * dpi / 96;
    let il = ImageList_Create(g, g, ILC_COLOR32, 2, 0);
    let glyphs = theme::icon_font(dpi as u32, 12);
    let dc = CreateCompatibleDC(None);
    let radius = 4 * dpi / 96;
    let r = RECT { left: 0, top: 0, right: g, bottom: g };
    for on in [false, true] {
        let Some((bmp, bits)) = views::dib(g, g) else { continue };
        let old = SelectObject(dc, bmp.into());
        let key = windows::Win32::Graphics::Gdi::CreateSolidBrush(COLORREF(KEY));
        FillRect(dc, &r, key);
        let _ = DeleteObject(key.into());
        if on {
            theme::fill_round(dc, &r, t.accent, radius);
            let old_font = SelectObject(dc, glyphs.into());
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(theme::on_selection(t, 0x00FF_FFFF)));
            let mut gr = r;
            DrawTextW(dc, &mut "\u{E73E}".encode_utf16().collect::<Vec<u16>>(), &mut gr, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
            SelectObject(dc, old_font);
        } else {
            theme::fill_round(dc, &r, t.secondary_text, radius);
            theme::fill_round(dc, &RECT { left: 1, top: 1, right: g - 1, bottom: g - 1 }, KEY, (radius - 1).max(0));
        }
        let _ = GdiFlush();
        // The key color becomes transparent, the rest opaque.
        let px = std::slice::from_raw_parts_mut(bits as *mut u32, (g * g) as usize);
        for p in px.iter_mut() {
            *p = if *p & 0x00FF_FFFF == KEY { 0 } else { *p | 0xFF00_0000 };
        }
        SelectObject(dc, old);
        ImageList_Add(il, bmp, None);
        let _ = DeleteObject(bmp.into());
    }
    let _ = DeleteDC(dc);
    let _ = DeleteObject(glyphs.into());
    il
}

pub(crate) unsafe fn apply_theme(app: *mut App) {
    let t = &(*app).theme;
    theme::set_menu_theme(t.dark);
    theme::apply_window((*app).hwnd, t);
    theme::apply_list((*app).list, t);
    theme::apply_control((*app).addr, t);
    theme::apply_control((*app).search, t);
    tabs::set_theme((*app).strip, t);
    sidebar::set_theme((*app).sidebar, t);
    breadcrumb::set_theme((*app).crumbs, t);
    toolbar::set_theme((*app).nav, t);
    toolbar::set_theme((*app).tools, t);
    toolbar::set_theme((*app).cmdbar, t);
    home::set_theme((*app).home, t);
    statusbar::set_theme((*app).status, t);
    preview_pane::set_theme((*app).panes.preview, t);
    // The preview's text Edit (its scroll bar) follows the theme too.
    if let Ok(edit) = FindWindowExW(Some((*app).panes.preview), None, WC_EDITW, PCWSTR::null()) {
        theme::apply_control(edit, t);
    }
    panes::theme_tree(app);
}

/// A system color or contrast change (and ImmersiveColorSet): rebuilds the theme from the profile's theme name
/// (a `system` theme follows Windows' app mode) and the contrast theme, and swaps it in when it differs.
// shortcut: the icon theme stays what startup loaded and open modal dialogs keep their colours; a restart picks both up.
pub(crate) unsafe fn refresh_theme(app: *mut App) {
    use windows::Win32::Graphics::Gdi::{RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE, RedrawWindow};
    let name = settings::resolve_theme_name(&(*app).profile.theme, rexplorer_core::import_explorer::apps_use_dark());
    let base = Settings::load().theme(name).clone();
    let theme = theme::effective_theme(&base, theme::high_contrast_on(), theme::sys_color);
    if theme == (*app).theme {
        return;
    }
    let brushes = theme::Brushes::new(&theme);
    // The class brush first: the old brushes are deleted when they are replaced below.
    SetClassLongPtrW((*app).hwnd, GCLP_HBRBACKGROUND, brushes.background.0 as isize);
    (*app).theme = theme;
    (*app).brushes = brushes;
    apply_theme(app);
    sync_list_style(app);
    apply_font(app);
    layout(app);
    let _ = RedrawWindow(Some((*app).hwnd), None, None, RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_FRAME);
}

/// (Re)creates the UI font for the window's current DPI.
pub(crate) unsafe fn apply_font(app: *mut App) {
    let font = theme::ui_font(&(*app).theme, GetDpiForWindow((*app).hwnd));
    let a = &*app;
    for h in [a.list, a.addr, a.search, a.strip, a.crumbs, a.status, a.sidebar, a.cmdbar, a.home] {
        SendMessageW(h, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
    }
    if !(*app).font.is_invalid() {
        let _ = DeleteObject((*app).font.into());
    }
    (*app).font = font;
    panes::set_preview_fonts(app);
}

/// After a DPI change (WM_DPICHANGED) or a Windows text size change: fonts, image lists, columns, view spacing
/// and layout again, then a full repaint for the children whose size did not change.
pub(crate) unsafe fn rescale(app: *mut App) {
    use windows::Win32::Graphics::Gdi::{RDW_ALLCHILDREN, RDW_ERASE, RDW_INVALIDATE, RedrawWindow};
    apply_font(app);
    sync_list_style(app);
    views::rebuild_tall(app);
    if let Some(s) = nav::source(app) {
        nav::set_columns(app, &s.columns());
        nav::apply_layout(app);
    }
    views::apply(app);
    layout(app);
    let _ = RedrawWindow(Some((*app).hwnd), None, None, RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN);
}

pub(crate) unsafe fn layout(app: *mut App) {
    let mut rc = RECT::default();
    let _ = GetClientRect((*app).hwnd, &mut rc);
    let dpi = GetDpiForWindow((*app).hwnd);
    // Bar heights hold text, so they follow the Windows text size too; widths and glyphs stay at `dpi`.
    let tdpi = theme::text_dpi(dpi);
    let (w, pad) = (rc.right, scale(PAD, dpi));
    let th = tabs::height(tdpi);
    let _ = MoveWindow((*app).strip, 0, 0, w, th, true);

    // The header bar: 32 px controls centered in 44 px.
    let (hh, nh) = (scale(HEADER_H, tdpi), scale(CONTROL_H, tdpi));
    let y = th + (hh - nh) / 2;
    let nw = toolbar::width((*app).nav);
    let tw = toolbar::width((*app).tools);
    let _ = MoveWindow((*app).nav, pad, y, nw, nh, true);
    let _ = MoveWindow((*app).tools, (w - pad - tw).max(0), y, tw, nh, true);
    let cx = 2 * pad + nw;
    // The search field keeps 220 px right of the path bar; the path bar gives way first on a narrow window.
    let sw = scale(SEARCH_W, dpi).min((w - tw - 3 * pad - cx).max(0));
    let cw = (w - tw - 3 * pad - sw - cx).max(0);
    let _ = MoveWindow((*app).crumbs, cx, y, cw, nh, true);
    // A single-line Edit draws its text at the top, so size it to one line and center it in the field,
    // right of the breadcrumb's icon (8 + 16 + 4 px).
    let eh = (*app).theme.font_size * tdpi as i32 / 72 * 4 / 3 + scale(4, dpi);
    let ex = scale(28, dpi);
    let _ = MoveWindow((*app).addr, cx + ex, y + (nh - eh) / 2, (cw - ex - 2 * pad).max(0), eh, true);
    // The search Edit sits in a rounded field the window paints, right of a magnifier glyph.
    let field = RECT { left: cx + cw + pad, top: y, right: cx + cw + pad + sw, bottom: y + nh };
    if field != (*app).search_field {
        let _ = InvalidateRect(Some((*app).hwnd), Some(&(*app).search_field as *const RECT), true);
        let _ = InvalidateRect(Some((*app).hwnd), Some(&field as *const RECT), true);
        (*app).search_field = field;
    }
    let gx = scale(32, dpi);
    let _ = MoveWindow((*app).search, field.left + gx, y + (nh - eh) / 2, (sw - gx - pad).max(0), eh, true);

    // The command bar: a second 44 px row, its spacer pushing Preview to the right edge.
    let _ = MoveWindow((*app).cmdbar, pad, th + hh + (hh - nh) / 2, (w - 2 * pad).max(0), nh, true);

    let show_status = (*app).profile.show_status;
    let sh = if show_status { statusbar::height(tdpi) } else { 0 };
    let _ = ShowWindow((*app).status, if show_status { SW_SHOW } else { SW_HIDE });
    let _ = MoveWindow((*app).status, 0, rc.bottom - sh, w, sh, true);
    panes::layout(app, th + 2 * hh, rc.bottom - sh, w);
}

/// The window keyboard focus goes to for the active tab: the Home page when it shows, else the list.
pub(crate) unsafe fn view_hwnd(app: *mut App) -> HWND {
    if IsWindowVisible((*app).home).as_bool() { (*app).home } else { (*app).list }
}

/// F6 / Shift+F6: the pane after (before) `at` among `n`; the first when focus is in none of them.
fn next_pane(at: Option<usize>, n: usize, back: bool) -> usize {
    match at {
        Some(i) if back => (i + n - 1) % n,
        Some(i) => (i + 1) % n,
        None => 0,
    }
}

/// The panes in F6 order; the crumbs, the left pane, the strip and the toolbars (navigation, tools, command bar)
/// are left out when hidden.
fn panes_in_order<T>(view: T, crumbs: Option<T>, addr: T, search: T, left: Option<T>, strip: Option<T>, bars: [Option<T>; 3]) -> Vec<T> {
    let [nav, tools, cmds] = bars;
    [Some(view), crumbs, Some(addr), Some(search), left, strip, nav, tools, cmds].into_iter().flatten().collect()
}

/// Moves the keyboard focus (F6, Tab) around the view, the breadcrumb, the address bar, the search box, the left
/// pane (the folder tree, else the sidebar, when shown), the tab strip and the toolbars.
pub(crate) unsafe fn cycle_focus(app: *mut App, back: bool) {
    let view = source::browser_hwnd(app).unwrap_or_else(|| view_hwnd(app));
    let tree = (*app).panes.tree.as_ref().filter(|_| (*app).session.show_tree).map(|t| t.hwnd());
    let shown = |h: HWND| IsWindowVisible(h).as_bool().then_some(h);
    let left = tree.or_else(|| shown((*app).sidebar));
    // The address Edit covers the breadcrumb while it is shown.
    let crumbs = shown((*app).crumbs).filter(|_| !IsWindowVisible((*app).addr).as_bool());
    let bars = [(*app).nav, (*app).tools, (*app).cmdbar].map(shown);
    let panes = panes_in_order(view, crumbs, (*app).addr, (*app).search, left, shown((*app).strip), bars);
    let focus = GetFocus();
    let at = panes.iter().position(|&h| h == focus || IsChild(h, focus).as_bool());
    let h = panes[next_pane(at, panes.len(), back)];
    if h == (*app).addr {
        show_edit(app);
    } else {
        let _ = SetFocus(Some(h));
        if h == (*app).search {
            SendMessageW(h, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        }
    }
}

/// WM_PAINT of the main window: the frame color everywhere the children leave, and the search field.
pub(crate) unsafe fn paint(app: *mut App) {
    let hwnd = (*app).hwnd;
    let mut ps = PAINTSTRUCT::default();
    let dc = BeginPaint(hwnd, &mut ps);
    let t = &(*app).theme;
    FillRect(dc, &ps.rcPaint, (*app).brushes.background);
    let f = (*app).search_field;
    if f.right > f.left {
        let dpi = GetDpiForWindow(hwnd);
        theme::fill_round(dc, &f, t.surface, scale(8, dpi));
        if t.contrast.is_some() {
            theme::outline_round(dc, &f, t.text, scale(8, dpi), 1);
        }
        let glyphs = theme::icon_font(dpi, 16);
        let old = SelectObject(dc, glyphs.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(t.secondary_text));
        let mut r = RECT { left: f.left + scale(8, dpi), right: f.left + scale(28, dpi), ..f };
        let mut g: Vec<u16> = "\u{E721}".encode_utf16().collect();
        DrawTextW(dc, &mut g, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(dc, old);
        let _ = DeleteObject(glyphs.into());
    }
    let _ = EndPaint(hwnd, &ps);
}

// ---- address Edit over the breadcrumb ----------------------------------------------------------

/// Crumb names and folder icon of the active tab.
pub(crate) unsafe fn set_crumbs(app: *mut App) {
    let tab = cur(app);
    let names: Vec<String> = match &tab.loading {
        Some(l) => vec![l.name.clone()],
        None => tab.crumbs.iter().map(|c| crate::connect::redact(&c.name)).collect(),
    };
    let editing = IsWindowVisible((*app).addr).as_bool();
    breadcrumb::set_crumbs((*app).crumbs, if editing { &[] } else { &names });
    let old = std::mem::take(&mut (*app).crumb_icon);
    // While loading, a plain folder icon: the destination's own may need disk or network access.
    let (himl, index) = if tab.loading.is_some() { source::sys_icon("folder", true) } else { tab.icon };
    if himl != 0 && (tab.source.is_some() || tab.loading.is_some()) {
        (*app).crumb_icon = ImageList_GetIcon(HIMAGELIST(himl as isize), index, ILD_NORMAL);
    }
    let icon = (*app).crumb_icon;
    breadcrumb::set_icon((*app).crumbs, (!icon.is_invalid()).then_some(icon));
    if !old.is_invalid() {
        let _ = DestroyIcon(old);
    }
}

pub(crate) unsafe fn show_edit(app: *mut App) {
    let addr = (*app).addr;
    breadcrumb::set_crumbs((*app).crumbs, &[]);
    let _ = SetWindowPos(addr, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
    let _ = SetFocus(Some(addr));
    SendMessageW(addr, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
}

/// On EN_KILLFOCUS: whatever was typed and not entered is dropped.
pub(crate) unsafe fn hide_edit(app: *mut App) {
    if let Some(l) = &cur(app).loading {
        set_text((*app).addr, &l.parsing);
    } else if let Some(s) = source(app) {
        set_text((*app).addr, &s.editing_name());
    }
    let _ = ShowWindow((*app).addr, SW_HIDE);
    set_crumbs(app);
}

// ---- popup menus -------------------------------------------------------------------------------

/// Menu entry id of a greyed heading.
const HEADING: usize = usize::MAX;
/// More-menu ids of the Quick access entries start here.
const QUICK: usize = 0x1000;

/// Shows `entries` (id, text; id 0 = separator, HEADING = greyed text), the ids in `checked` with a check mark,
/// and returns the picked id, 0 when dismissed.
pub(crate) unsafe fn popup(app: *mut App, entries: &[(usize, String)], checked: &[usize], x: i32, y: i32) -> usize {
    let Ok(menu) = CreatePopupMenu() else { return 0 };
    for (id, text) in entries {
        if *id == 0 {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        } else if *id == HEADING {
            let h = HSTRING::from(text.as_str());
            let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(h.as_ptr()));
        } else {
            let h = HSTRING::from(text.as_str());
            let check = if checked.contains(id) { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(menu, MF_STRING | check, *id, PCWSTR(h.as_ptr()));
        }
    }
    let picked = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, x, y, None, (*app).hwnd, None).0 as usize;
    let _ = DestroyMenu(menu);
    picked
}

/// An entry of a context menu built by `build_menu`: id 0 with no children is a separator; children make a
/// submenu; `glyph` is a Segoe Fluent Icons code point drawn as the item's icon, 0 for none.
pub(crate) struct Entry {
    pub id: usize,
    pub text: String,
    pub glyph: u16,
    pub enabled: bool,
    pub checked: bool,
    pub sub: Vec<Entry>,
}

impl Entry {
    pub fn new(id: usize, text: &str, glyph: u16) -> Entry {
        Entry { id, text: text.to_string(), glyph, enabled: true, checked: false, sub: Vec::new() }
    }

    pub fn sep() -> Entry {
        Entry::new(0, "", 0)
    }

    pub fn enabled(mut self, on: bool) -> Entry {
        self.enabled = on;
        self
    }

    /// A submenu from `popup`-style entries (id, text) with the ids in `checked` ticked, each id offset by `base`.
    pub fn submenu(text: &str, glyph: u16, entries: &[(usize, String)], checked: &[usize], base: usize) -> Entry {
        let mut e = Entry::new(0, text, glyph);
        e.sub = entries
            .iter()
            .map(|(id, t)| match *id {
                0 => Entry::sep(),
                id => Entry { checked: checked.contains(&id), ..Entry::new(base + id, t, 0) },
            })
            .collect();
        e
    }
}

/// A popup menu of `entries`, every non-zero id offset by `menu::OWN_FIRST` so the shell's own ids (below it)
/// can share the menu. Glyph bitmaps are pushed to `bitmaps`: delete them after the menu closes.
pub(crate) unsafe fn build_menu(owner: HWND, entries: &[Entry], bitmaps: &mut Vec<HBITMAP>) -> HMENU {
    let Ok(menu) = CreatePopupMenu() else { return HMENU::default() };
    for (pos, e) in entries.iter().enumerate() {
        if e.id == 0 && e.sub.is_empty() {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            continue;
        }
        let text = HSTRING::from(e.text.as_str());
        let mut flags = MF_STRING | if e.enabled { MF_ENABLED } else { MF_GRAYED } | if e.checked { MF_CHECKED } else { MF_UNCHECKED };
        let id = if e.sub.is_empty() {
            rexplorer_core::menu::OWN_FIRST as usize + e.id
        } else {
            flags |= MF_POPUP;
            build_menu(owner, &e.sub, bitmaps).0 as usize
        };
        let _ = AppendMenuW(menu, flags, id, PCWSTR(text.as_ptr()));
        if e.glyph != 0
            && let Some(b) = glyph_bitmap(owner, e.glyph) {
                bitmaps.push(b);
                let mii = MENUITEMINFOW { cbSize: size_of::<MENUITEMINFOW>() as u32, fMask: MIIM_BITMAP, hbmpItem: b, ..Default::default() };
                let _ = SetMenuItemInfoW(menu, pos as u32, true, &mii);
            }
    }
    menu
}

/// Shows `entries` as a popup at screen point (x, y) and returns the picked entry's id (None when dismissed).
/// For pages whose background menu is all our own (Home, the future Desktop).
pub(crate) unsafe fn pick_entries(app: *mut App, entries: &[Entry], x: i32, y: i32) -> Option<usize> {
    let mut bitmaps = Vec::new();
    let menu = build_menu((*app).hwnd, entries, &mut bitmaps);
    let picked = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, x, y, None, (*app).hwnd, None).0 as usize;
    let _ = DestroyMenu(menu);
    for b in bitmaps {
        let _ = DeleteObject(b.into());
    }
    picked.checked_sub(rexplorer_core::menu::OWN_FIRST as usize)
}

/// A 16 px (at 96 DPI) premultiplied ARGB bitmap of `glyph` in the menu text color, for a menu item. The glyph
/// is drawn white on black and that gray becomes the alpha, since GDI text leaves a DIB's alpha untouched.
unsafe fn glyph_bitmap(owner: HWND, glyph: u16) -> Option<HBITMAP> {
    let dpi = GetDpiForWindow(owner);
    let size = (16 * dpi / 96) as i32;
    let (bmp, bits) = views::dib(size, size)?;
    let dc = CreateCompatibleDC(None);
    let old = SelectObject(dc, bmp.into());
    let font = theme::icon_font(dpi, 14);
    let old_font = SelectObject(dc, font.into());
    SetBkMode(dc, TRANSPARENT);
    SetTextColor(dc, COLORREF(0x00FF_FFFF));
    let mut text = [glyph];
    let mut r = RECT { left: 0, top: 0, right: size, bottom: size };
    DrawTextW(dc, &mut text, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOCLIP);
    let _ = GdiFlush();
    SelectObject(dc, old_font);
    let _ = DeleteObject(font.into());
    SelectObject(dc, old);
    let _ = DeleteDC(dc);
    // The glyph takes the menu's text color: white on dark menus, the system's menu text otherwise.
    let color = if theme::menus_dark() { 0x00FF_FFFF } else { GetSysColor(COLOR_MENUTEXT) };
    let (cr, cg, cb) = (color & 0xFF, (color >> 8) & 0xFF, (color >> 16) & 0xFF);
    let px = std::slice::from_raw_parts_mut(bits, (size * size * 4) as usize);
    for p in px.as_chunks_mut::<4>().0 {
        let a = p[0].max(p[1]).max(p[2]) as u32;
        p[0] = (cb * a / 255) as u8;
        p[1] = (cg * a / 255) as u8;
        p[2] = (cr * a / 255) as u8;
        p[3] = a as u8;
    }
    Some(bmp)
}

/// A crumb's subfolders, listed by a worker: (display name, absolute PIDL), or None when the folder failed.
pub(crate) struct CrumbItems {
    token: u64,
    generation: u64,
    at: POINT,
    keyboard: bool,
    items: Option<Vec<(String, Pidl)>>,
}

/// The newest dropdown request: an older listing that arrives late is dropped.
static CRUMB_TOKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Breadcrumb chevron: the subfolders of crumb `index` (`breadcrumb::ROOT`: the desktop's places, as Explorer's
/// leftmost chevron), listed off the UI thread: This PC, a network or a dead share can take seconds.
/// `at`: the screen point of a keyboard-opened menu (which beeps when the crumb has no dropdown); else the cursor.
pub(crate) unsafe fn crumb_dropdown(app: *mut App, index: usize, at: Option<POINT>) {
    let pidl = if index == breadcrumb::ROOT {
        ShellFolder::desktop().ok().map(|d| d.pidl)
    } else {
        cur(app).crumbs.get(index).and_then(|c| c.pidl.clone())
    };
    // Archive and search crumbs have no shell folder behind them.
    let Some(pidl) = pidl else {
        if at.is_some() {
            let _ = MessageBeep(MB_OK);
        }
        return;
    };
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let at_pt = at.unwrap_or(pt);
    let token = CRUMB_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let (hwnd, generation, keyboard) = ((*app).hwnd.0 as isize, (*app).generation, at.is_some());
    std::thread::spawn(move || {
        background_priority();
        let _com = ComInit::mta();
        let items = (|| {
            let folder = ShellFolder::from_pidl(&pidl).ok()?;
            let mut items = folder.subfolders_up_to(2000).ok()?;
            items.retain(|i| i.is_folder() && !i.is_hidden());
            items.sort_by(|a, b| folder.compare(0, a, b));
            // shortcut: a menu past a few hundred entries is unusable anyway; a scrolling flyout would replace it
            items.truncate(500);
            Some(items.iter().map(|it| (it.display_name(), folder.child_pidl(it))).collect())
        })();
        let done = Box::into_raw(Box::new(CrumbItems { token, generation, at: at_pt, keyboard, items }));
        if unsafe { PostMessageW(Some(HWND(hwnd as *mut c_void)), WM_CRUMB_ITEMS, WPARAM(0), LPARAM(done as isize)) }.is_err() {
            drop(unsafe { Box::from_raw(done) });
        }
    });
}

/// A dropdown's listing is in: shown unless a newer one was asked for, the tab navigated, or the window lost the
/// foreground meanwhile.
pub(crate) unsafe fn on_crumb_items(app: *mut App, done: CrumbItems) {
    let current = done.token == CRUMB_TOKEN.load(std::sync::atomic::Ordering::Relaxed) && done.generation == (*app).generation;
    if !current || GetForegroundWindow() != (*app).hwnd {
        return;
    }
    let items = done.items.unwrap_or_default();
    if items.is_empty() {
        if done.keyboard {
            let _ = MessageBeep(MB_OK);
        }
        return;
    }
    let entries: Vec<(usize, String)> = items.iter().enumerate().map(|(i, (name, _))| (i + 1, name.clone())).collect();
    let picked = popup(app, &entries, &[], done.at.x, done.at.y);
    if let Some((_, pidl)) = picked.checked_sub(1).and_then(|i| items.get(i)) {
        go(app, Target::Pidl(pidl.clone()), nav::History::Push);
    }
}

pub(crate) unsafe fn more_menu(app: *mut App) {
    let exe = std::env::current_exe().unwrap_or_default();
    let reg = if register::is_registered(&rexplorer_core::install::classes(&rexplorer_core::install::reg_base()), &exe) {
        (CMD_UNREGISTER as usize, tr("menu.more.unregister").to_string())
    } else {
        (CMD_REGISTER as usize, tr("menu.more.register").to_string())
    };
    let tree = if (*app).session.show_tree { tr("menu.more.places_sidebar") } else { tr("menu.view.folder_tree") };
    let mut entries = vec![
        (CMD_FOLDER_TREE as usize, tree.to_string()),
        (0, String::new()),
        (CMD_PIN as usize, hinted("menu.more.pin_home", "Ctrl+D")),
        (CMD_EXPLORER as usize, hinted("menu.more.open_explorer", "Ctrl+Shift+E")),
        reg,
        (CMD_INSTALL as usize, tr("menu.more.install").to_string()),
        (CMD_UNINSTALL as usize, tr("menu.more.uninstall").to_string()),
        (CMD_FOLDER_OPTIONS as usize, tr("menu.more.folder_options").to_string()),
        (CMD_SETTINGS as usize, tr("menu.more.settings_folder").to_string()),
        (0, String::new()),
        (CMD_ABOUT as usize, tr("menu.more.about").to_string()),
    ];
    // The keyboard's way to the update notice's page.
    if let Some(v) = UPDATE.get() {
        entries.push((CMD_UPDATE as usize, trf("menu.more.update", &[("version", v)])));
    }
    // Quick access: profile bookmarks, then cloud-sync roots.
    let quick: Vec<(String, String)> = (*app).profile.bookmarks.iter().chain(CLOUD.get().into_iter().flatten()).cloned().collect();
    if !quick.is_empty() {
        entries.push((0, String::new()));
        entries.push((HEADING, tr("menu.more.quick_access").to_string()));
        entries.extend(quick.iter().enumerate().map(|(i, (label, _))| (QUICK + i, label.clone())));
    }
    let r = toolbar::button_rect((*app).cmdbar, CMD_MORE).unwrap_or_default();
    let picked = popup(app, &entries, &[], r.left, r.bottom);
    if let Some((_, target)) = picked.checked_sub(QUICK).and_then(|i| quick.get(i)) {
        go(app, Target::Name(target.clone()), nav::History::Push);
    } else if picked != 0 {
        on_command(app, picked as u16);
    }
}

/// --register / --unregister and the More menu: make rexplorer the default file manager (journaled in the install
/// folder's takeover.txt) or give folders back to Windows Explorer. Confirms unless `quiet`, reports, and like
/// install_flow offers (or with `quiet` runs) the rollback of a failed take-over.
pub(crate) unsafe fn register_flow(owner: Option<HWND>, unregister: bool, quiet: bool) -> InstallOutcome {
    use rexplorer_core::install;
    let exe = std::env::current_exe().unwrap_or_default();
    let (dir, base) = (install::default_dir(), install::reg_base());
    if !quiet {
        let question = if unregister {
            tr("dlg.register.stop").to_string()
        } else {
            trf("dlg.register.start", &[("plan", &install::takeover_plan(&base, &exe))])
        };
        let q = HSTRING::from(question);
        if MessageBoxW(owner, PCWSTR(q.as_ptr()), w!("rexplorer"), MB_YESNO | MB_ICONQUESTION) != IDYES {
            return InstallOutcome::Cancelled;
        }
    }
    // The desktop helper is part of taking over; a test base (REXPLORER_REG_BASE) leaves the real one alone.
    let real = base == install::REAL_REG_BASE;
    let (report, outcome) = if unregister {
        let _ = real && crate::desktop::stop();
        match install::give_back(&dir, &base) {
            Ok(s) => (Some((format!("{}\n\n{s}", tr("dlg.register.stopped")), MB_ICONINFORMATION)), InstallOutcome::Done),
            Err(e) => (Some((trf("err.registry", &[("error", &e.message())]), MB_ICONERROR)), InstallOutcome::Failed),
        }
    } else {
        match install::take_over(&dir, &base, &exe) {
            Ok(s) => {
                if real {
                    let _ = std::process::Command::new(&exe).arg("--desktop").spawn();
                }
                (Some((format!("{}\n\n{s}", tr("dlg.register.started")), MB_ICONINFORMATION)), InstallOutcome::Done)
            }
            Err(f) if !f.rollback.is_empty() => {
                if quiet {
                    let _ = f.rollback.run();
                } else {
                    offer_rollback(owner, &f.error, f.rollback, &dir);
                }
                (None, InstallOutcome::Failed)
            }
            Err(f) => (Some((trf("err.registry", &[("error", &f.error.message())]), MB_ICONERROR)), InstallOutcome::Failed),
        }
    };
    if let Some((text, icon)) = report.filter(|_| !quiet) {
        let t = HSTRING::from(text);
        MessageBoxW(owner, PCWSTR(t.as_ptr()), w!("rexplorer"), MB_OK | icon);
    }
    outcome
}

pub(crate) enum InstallOutcome {
    Done,
    Cancelled,
    Failed,
}

/// --install / --uninstall and the More menu: optionally confirm (listing what changes), run, report. A failed
/// install offers to roll back what it changed. `quiet` (a package manager's `--quiet`): no dialog at all, a failed
/// install rolls back, the outcome is the only report.
pub(crate) unsafe fn install_flow(owner: Option<HWND>, uninstall: bool, confirm: bool, quiet: bool) -> InstallOutcome {
    use rexplorer_core::install;
    let dir = install::default_dir();
    let base = install::reg_base();
    if confirm && !quiet {
        let question = if uninstall {
            let listed = install::recorded(&dir);
            let listed = if listed.is_empty() { tr("dlg.uninstall.nothing").to_string() } else { listed.join("\n") };
            trf("dlg.uninstall.confirm", &[("record", &dir.join("install.txt").display()), ("listed", &listed)])
        } else {
            trf("dlg.install.confirm", &[("plan", &install::plan(&dir, &base))])
        };
        let q = HSTRING::from(question);
        // A standalone install changes no association any more: an ordinary question, Yes by default.
        if MessageBoxW(owner, PCWSTR(q.as_ptr()), w!("rexplorer"), MB_YESNO | MB_ICONQUESTION) != IDYES {
            return InstallOutcome::Cancelled;
        }
    }
    // A running desktop helper holds the installed exe: close it before replacing or removing it, and start it
    // again afterwards. A test base (REXPLORER_REG_BASE) leaves the real helper alone.
    let real = base == install::REAL_REG_BASE;
    let helper_was_running = real && crate::desktop::stop();
    // The icons view may hold the installed exe too; it is opt-in, so it is not started again afterwards.
    let _ = real && crate::desktop_icons::stop();
    let r = if uninstall {
        install::uninstall(&dir, &base).map_err(|e| (e, None))
    } else {
        install::install(&dir, &base).map_err(|f| (f.error, Some(f.rollback)))
    };
    let (report, outcome) = match r {
        Ok(s) => (Some((s, MB_ICONINFORMATION)), InstallOutcome::Done),
        Err((e, Some(rollback))) if !rollback.is_empty() => {
            if quiet {
                let _ = rollback.run();
            } else {
                offer_rollback(owner, &e, rollback, &dir);
            }
            (None, InstallOutcome::Failed)
        }
        Err((e, _)) => {
            let key = if uninstall { "err.uninstall" } else { "err.install" };
            (Some((trf(key, &[("error", &e.message())]), MB_ICONERROR)), InstallOutcome::Failed)
        }
    };
    // A standalone install starts no helper (that is --register's); one that was running comes back after an update or
    // a failure, on whichever copy is in place, but not after an uninstall.
    let restart = helper_was_running && !(uninstall && matches!(outcome, InstallOutcome::Done));
    let exe = dir.join("rexplorer.exe");
    if restart && exe.is_file() {
        let _ = std::process::Command::new(exe).arg("--desktop").spawn();
    }
    if let Some((text, icon)) = report.filter(|_| !quiet) {
        let t = HSTRING::from(text);
        MessageBoxW(owner, PCWSTR(t.as_ptr()), w!("rexplorer"), MB_OK | icon);
    }
    outcome
}

/// After a failed install: Roll back (the default) or Keep changes, then report the roll back.
unsafe fn offer_rollback(
    owner: Option<HWND>,
    error: &windows::core::Error,
    rollback: rexplorer_core::install::Rollback,
    dir: &std::path::Path,
) {
    const ROLL_BACK: i32 = 100;
    let content = HSTRING::from(trf("dlg.rollback.content", &[("error", &error.message())]));
    let (roll_text, keep_text, title) = (HSTRING::from(tr("dlg.rollback.roll_back")), HSTRING::from(tr("dlg.rollback.keep")), HSTRING::from(tr("dlg.rollback.title")));
    let buttons = [
        TASKDIALOG_BUTTON { nButtonID: ROLL_BACK, pszButtonText: PCWSTR(roll_text.as_ptr()) },
        TASKDIALOG_BUTTON { nButtonID: 101, pszButtonText: PCWSTR(keep_text.as_ptr()) },
    ];
    let config = TASKDIALOGCONFIG {
        cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: owner.unwrap_or_default(),
        dwFlags: TDF_USE_COMMAND_LINKS,
        pszWindowTitle: w!("rexplorer"),
        Anonymous1: TASKDIALOGCONFIG_0 { pszMainIcon: TD_WARNING_ICON },
        pszMainInstruction: PCWSTR(title.as_ptr()),
        pszContent: PCWSTR(content.as_ptr()),
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        nDefaultButton: ROLL_BACK,
        ..Default::default()
    };
    let mut picked = 0;
    let roll_back = if TaskDialogIndirect(&config, Some(&mut picked), None, None).is_ok() {
        picked == ROLL_BACK
    } else {
        let q = HSTRING::from(trf("dlg.rollback.question", &[("error", &error.message())]));
        MessageBoxW(owner, PCWSTR(q.as_ptr()), w!("rexplorer"), MB_YESNO | MB_ICONWARNING) == IDYES
    };
    if roll_back {
        let (text, icon) = match rollback.run() {
            Ok(s) => (s, MB_ICONINFORMATION),
            Err(e) => (e.message(), MB_ICONERROR),
        };
        let t = HSTRING::from(text);
        MessageBoxW(owner, PCWSTR(t.as_ptr()), w!("rexplorer"), MB_OK | icon);
    } else {
        rollback.commit(dir);
    }
}

/// --import-explorer: copies Windows Explorer's settings (read only) into the profile's keys in settings.ini,
/// after a dated backup, and reports what was imported and skipped. True unless writing failed.
pub(crate) unsafe fn import_explorer_flow(profile: Option<&str>, overwrite: bool) -> bool {
    use rexplorer_core::import_explorer as ie;
    let dir = settings::dir();
    Settings::ensure_files(&dir);
    let s = Settings::load_from(&dir);
    let p = profile.and_then(|n| s.profiles.iter().find(|p| p.name == n)).unwrap_or_else(|| s.active_profile());
    let mapped = ie::map(&ie::read());
    let found = mapped.len();
    let (write, skip) = ie::decide(p, mapped, overwrite);
    let ini = dir.join("settings.ini");
    let t = windows::Win32::System::SystemInformation::GetLocalTime();
    let day = format!("settings.ini.bak-{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay);
    // Never reuse an existing backup: it may predate values set by hand since.
    let bak = (1..).map(|n| dir.join(if n == 1 { day.clone() } else { format!("{day}-{n}") })).find(|b| !b.exists()).unwrap();
    let ok = write.is_empty() || (std::fs::copy(&ini, &bak).is_ok() && views::try_save_profile_keys(&p.name, &write).is_ok());
    let list = |v: Vec<String>| if v.is_empty() { format!("  {}", tr("dlg.import.none")) } else { v.join("\n") };
    let text = if !ok {
        trf("dlg.import.failed", &[("path", &ini.display())])
    } else {
        let backup = if write.is_empty() { tr("dlg.import.not_needed").to_string() } else { bak.display().to_string() };
        trf(
            "dlg.import.report",
            &[
                ("profile", &p.name),
                ("set", &write.len()),
                ("found", &found),
                ("written", &list(write.iter().map(|(k, v)| format!("  {k} = {v}")).collect())),
                ("skipped", &list(skip.iter().map(|(k, why)| format!("  {k}: {}", tr(why))).collect())),
                ("backup", &backup),
            ],
        )
    };
    let h = HSTRING::from(text);
    MessageBoxW(None, PCWSTR(h.as_ptr()), w!("rexplorer"), MB_OK | if ok { MB_ICONINFORMATION } else { MB_ICONERROR });
    ok
}

pub(crate) unsafe fn about(app: *mut App) {
    let text = trf(
        "dlg.about",
        &[("version", &env!("CARGO_PKG_VERSION")), ("profile", &(*app).profile.name), ("theme", &(*app).theme.name), ("dir", &settings::dir().display())],
    );
    let h = HSTRING::from(text);
    MessageBoxW(Some((*app).hwnd), PCWSTR(h.as_ptr()), w!("rexplorer"), MB_OK | MB_ICONINFORMATION);
}

// ---- palette, terminal, status -----------------------------------------------------------------

pub(crate) unsafe fn open_palette(app: *mut App) {
    let a = &mut *app;
    let mut c: Vec<Candidate> = a.recent.items().iter().map(|r| Candidate { text: r.clone(), target: r.clone() }).collect();
    let pairs = a.profile.bookmarks.iter().chain(CLOUD.get().into_iter().flatten());
    c.extend(pairs.map(|(label, target)| Candidate { text: label.clone(), target: target.clone() }));
    if let Some(l) = listing(app) {
        c.extend(l.items.iter().filter(|i| i.is_folder()).map(|i| Candidate { text: i.display_name(), target: i.parsing_name() }));
    }
    palette::close(a.palette);
    if let Ok(p) = palette::open(a.hwnd, ID_PALETTE, c, &a.theme, a.font) {
        a.palette = p;
    }
}

pub(crate) unsafe fn on_palette(app: *mut App, hdr: &NMHDR) {
    if hdr.code == palette::PN_PICK
        && let Some(target) = palette::picked(hdr.hwndFrom) {
            go(app, Target::Name(target), nav::History::Push);
        }
    let _ = SetFocus(Some(ui::view_hwnd(app)));
}

pub(crate) unsafe fn open_terminal(app: *mut App) {
    let Some(p) = listing(app).map(|l| l.folder.parsing_name.clone()).filter(|p| is_fs_path(p)) else {
        let _ = MessageBeep(MB_OK);
        return;
    };
    // The click returns at once; ShellExecute on an app alias (wt.exe) can take a while.
    std::thread::spawn(move || {
        let _com = ComInit::sta();
        if instance::open_terminal(std::path::Path::new(&p)).is_err() {
            let _ = MessageBeep(MB_OK);
        }
    });
}

/// F11: the window covers its monitor without borders, or returns to its placement.
pub(crate) unsafe fn toggle_fullscreen(app: *mut App) {
    let hwnd = (*app).hwnd;
    if let Some((placement, style)) = (*app).fullscreen.take() {
        SetWindowLongPtrW(hwnd, GWL_STYLE, style);
        let _ = SetWindowPlacement(hwnd, &placement);
        let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED);
        return;
    }
    let mut placement = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    if GetWindowPlacement(hwnd, &mut placement).is_err() || !GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi).as_bool() {
        return;
    }
    let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
    (*app).fullscreen = Some((placement, style));
    SetWindowLongPtrW(hwnd, GWL_STYLE, style & !(WS_OVERLAPPEDWINDOW.0 as isize));
    let r = mi.rcMonitor;
    let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_FRAMECHANGED | SWP_NOOWNERZORDER);
}

/// The summed size of the selected files; None when it is not worth asking (see below) or no file has a size.
pub(crate) unsafe fn selected_bytes(app: *mut App, selected: usize) -> Option<u64> {
    // Remote folders are skipped: sizing up to 500 rows would stall on the network.
    match source(app) {
        Some(Source::Shell(l)) if (1..=500).contains(&selected) && !is_remote(&l.folder.parsing_name) => {
            let sizes: Vec<u64> = list::selected_indices((*app).list).iter().filter_map(|&i| l.items.get(i)).filter(|it| !it.is_folder()).filter_map(|it| it.size()).collect();
            (!sizes.is_empty()).then(|| sizes.iter().sum())
        }
        // The walk sized every result already.
        Some(Source::Search(s)) => {
            let sizes: Vec<u64> = list::selected_indices((*app).list).iter().filter_map(|&i| s.rows.get(i)).filter(|r| !r.is_folder).filter_map(|r| r.bytes).collect();
            (!sizes.is_empty()).then(|| sizes.iter().sum())
        }
        _ => None,
    }
}

/// The status bar and the command bar's enabled states.
pub(crate) unsafe fn update_status(app: *mut App) {
    commands::update(app);
    let right = match UPDATE.get() {
        Some(v) => format!("{}  \u{b7}  {}", trf("status.update", &[("version", v)]), (*app).profile.name),
        None => (*app).profile.name.clone(),
    };
    if cur(app).loading.is_some() {
        return statusbar::set_text((*app).status, tr("status.loading"), &right);
    }
    let count = source(app).map_or(0, |s| s.len());
    let selected = SendMessageW((*app).list, LVM_GETSELECTEDCOUNT, None, None).0 as usize;
    let mut left = statusbar::summary(count, selected, selected_bytes(app, selected));
    if let Some(Source::Shell(l)) = source(app)
        && let Some(e) = &l.denied {
            left = trf("status.access_denied", &[("error", e)]);
        }
    if let Some(Source::Search(s)) = source(app) {
        if s.running && s.rows.is_empty() {
            left = tr("status.searching").to_string();
        } else {
            left.push_str("  \u{b7}  ");
            left.push_str(tr(if s.running { "status.search_running" } else { "status.search_done" }));
        }
    }
    statusbar::set_text((*app).status, &left, &right);
}

#[cfg(test)]
mod tests {
    use super::{is_fs_path, next_pane, panes_in_order};

    #[test]
    fn fs_paths() {
        assert!(is_fs_path("C:\\Users"));
        assert!(is_fs_path("d:"));
        assert!(is_fs_path("\\\\server\\share"));
        assert!(!is_fs_path("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"));
        assert!(!is_fs_path("shell:Desktop"));
        assert!(!is_fs_path("C"));
    }

    #[test]
    fn f6_cycles_panes() {
        assert_eq!(next_pane(Some(0), 4, false), 1);
        assert_eq!(next_pane(Some(3), 4, false), 0);
        assert_eq!(next_pane(Some(0), 4, true), 3);
        assert_eq!(next_pane(Some(2), 3, true), 1);
        assert_eq!((next_pane(None, 3, false), next_pane(None, 3, true)), (0, 0));
        let bars = [Some("nav"), Some("tools"), Some("cmds")];
        let order = panes_in_order("view", Some("crumbs"), "addr", "search", Some("left"), Some("strip"), bars);
        assert_eq!(order, ["view", "crumbs", "addr", "search", "left", "strip", "nav", "tools", "cmds"]);
        let walked: Vec<_> = std::iter::successors(Some(0), |&i| Some(next_pane(Some(i), order.len(), false))).take(10).map(|i| order[i]).collect();
        assert_eq!(walked, ["view", "crumbs", "addr", "search", "left", "strip", "nav", "tools", "cmds", "view"]);
        assert_eq!(panes_in_order("view", None, "addr", "search", None, None, [None, Some("tools"), None]), ["view", "addr", "search", "tools"], "hidden panes are skipped");
    }
}
