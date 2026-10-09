//! Window chrome: child controls, theme and fonts, layout, the address Edit, popup menus, palette and status.

use super::*;
use crate::nav::{Target, cur, go, listing};

const THIS_PC: &str = "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}";

/// Cloud-sync roots for the palette, probed once on a worker (volume probing can stall on dead drives).
static CLOUD: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();

pub(crate) fn probe_cloud_roots() {
    std::thread::spawn(|| {
        let _ = CLOUD.set(connect::cloud_roots());
    });
}

fn scale(px: i32, dpi: u32) -> i32 {
    px * dpi as i32 / 96
}

/// A drive path or UNC path: something a terminal can start in.
pub(crate) fn is_fs_path(p: &str) -> bool {
    let b = p.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':') || p.starts_with("\\\\")
}

pub(crate) unsafe fn create_children(app: *mut App) -> Result<()> {
    let hwnd = (*app).hwnd;
    let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
    (*app).strip = tabs::create(hwnd, ID_TABS)?;
    (*app).nav = toolbar::create(
        hwnd,
        ID_NAV,
        &[
            (0xE72B, CMD_BACK, "Back (Alt+Left)"),
            (0xE72A, CMD_FORWARD, "Forward (Alt+Right)"),
            (0xE74A, CMD_UP, "Up (Alt+Up)"),
            (0xE72C, CMD_REFRESH, "Refresh (F5)"),
        ],
    )?;
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
    (*app).tools = toolbar::create(
        hwnd,
        ID_TOOLS,
        &[
            (0xE756, CMD_TERMINAL, "Open terminal here (Ctrl+`)"),
            (0xE8A9, CMD_VIEW, "View"),
            (0xE968, CMD_CONNECT, "Connect to server"),
            (0xE712, CMD_MORE, "More"),
        ],
    )?;
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
    let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_HEADERDRAGDROP;
    SendMessageW((*app).list, LVM_SETEXTENDEDLISTVIEWSTYLE, Some(WPARAM(0)), Some(LPARAM(ex as isize)));
    let _ = SetWindowSubclass((*app).list, Some(list::subclass), 1, app as usize);
    list::register_drop(app);
    panes::create_preview(app)?;
    if (*app).session.show_preview {
        let _ = ShowWindow((*app).panes.preview, SW_SHOW);
    }
    (*app).status = statusbar::create(hwnd, ID_STATUS)?;
    apply_theme(app);
    apply_font(app);
    views::init(app);
    Ok(())
}

pub(crate) unsafe fn apply_theme(app: *mut App) {
    let t = &(*app).theme;
    theme::apply_window((*app).hwnd, t);
    theme::apply_list((*app).list, t);
    theme::apply_control((*app).addr, t);
    tabs::set_theme((*app).strip, t);
    breadcrumb::set_theme((*app).crumbs, t);
    toolbar::set_theme((*app).nav, t);
    toolbar::set_theme((*app).tools, t);
    statusbar::set_theme((*app).status, t);
    preview_pane::set_theme((*app).panes.preview, t);
    // The preview's text Edit (its scroll bar) follows the theme too.
    if let Ok(edit) = FindWindowExW(Some((*app).panes.preview), None, WC_EDITW, PCWSTR::null()) {
        theme::apply_control(edit, t);
    }
    panes::theme_tree(app);
}

/// (Re)creates the UI font for the window's current DPI.
pub(crate) unsafe fn apply_font(app: *mut App) {
    let font = theme::ui_font(&(*app).theme, GetDpiForWindow((*app).hwnd));
    for h in [(*app).list, (*app).addr, (*app).strip, (*app).crumbs, (*app).status] {
        SendMessageW(h, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
    }
    if !(*app).font.is_invalid() {
        let _ = DeleteObject((*app).font.into());
    }
    (*app).font = font;
    panes::set_preview_fonts(app);
}

pub(crate) unsafe fn layout(app: *mut App) {
    let mut rc = RECT::default();
    let _ = GetClientRect((*app).hwnd, &mut rc);
    let dpi = GetDpiForWindow((*app).hwnd);
    let (w, pad) = (rc.right, scale(4, dpi));
    let th = tabs::height(dpi);
    let _ = MoveWindow((*app).strip, 0, 0, w, th, true);

    let (y, nh) = (th + pad, scale(32, dpi));
    let nw = toolbar::width((*app).nav);
    let tw = toolbar::width((*app).tools);
    let _ = MoveWindow((*app).nav, pad, y, nw, nh, true);
    let _ = MoveWindow((*app).tools, (w - pad - tw).max(0), y, tw, nh, true);
    let cx = 2 * pad + nw;
    let cw = (w - tw - 2 * pad - cx).max(0);
    let _ = MoveWindow((*app).crumbs, cx, y, cw, nh, true);
    // A single-line Edit draws its text at the top, so size it to one line and center it in the field,
    // right of the breadcrumb's icon (8 + 16 + 4 px).
    let eh = (*app).theme.font_size * dpi as i32 / 72 * 4 / 3 + scale(4, dpi);
    let ex = scale(28, dpi);
    let _ = MoveWindow((*app).addr, cx + ex, y + (nh - eh) / 2, (cw - ex - 2 * pad).max(0), eh, true);

    let sh = statusbar::height(dpi);
    let _ = MoveWindow((*app).status, 0, rc.bottom - sh, w, sh, true);
    panes::layout(app, y + nh + pad, rc.bottom - sh, w);
}

// ---- address Edit over the breadcrumb ----------------------------------------------------------

/// Crumb names and folder icon of the active tab.
pub(crate) unsafe fn set_crumbs(app: *mut App) {
    let tab = cur(app);
    let names: Vec<String> = tab.crumbs.iter().map(|c| c.name.clone()).collect();
    let editing = IsWindowVisible((*app).addr).as_bool();
    breadcrumb::set_crumbs((*app).crumbs, if editing { &[] } else { &names });
    let old = std::mem::take(&mut (*app).crumb_icon);
    let (himl, index) = tab.icon;
    if himl != 0 && tab.listing.is_some() {
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
    if let Some(l) = listing(app) {
        set_text((*app).addr, &l.folder.editing_name);
    }
    let _ = ShowWindow((*app).addr, SW_HIDE);
    set_crumbs(app);
}

// ---- popup menus -------------------------------------------------------------------------------

/// Shows `entries` (id, text; id 0 = separator) and returns the picked id, 0 when dismissed.
unsafe fn popup(app: *mut App, entries: &[(usize, String)], x: i32, y: i32) -> usize {
    let Ok(menu) = CreatePopupMenu() else { return 0 };
    for (id, text) in entries {
        if *id == 0 {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        } else {
            let h = HSTRING::from(text.as_str());
            let _ = AppendMenuW(menu, MF_STRING, *id, PCWSTR(h.as_ptr()));
        }
    }
    let picked = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, x, y, None, (*app).hwnd, None).0 as usize;
    let _ = DestroyMenu(menu);
    picked
}

/// Breadcrumb chevron: the subfolders of crumb `index`, listed on the UI thread only where that is cheap.
pub(crate) unsafe fn crumb_dropdown(app: *mut App, index: usize) {
    let Some(c) = cur(app).crumbs.get(index) else { return };
    if !(is_fs_path(&c.parsing) || c.parsing.eq_ignore_ascii_case(THIS_PC)) {
        return;
    }
    let Ok(folder) = ShellFolder::from_pidl(&c.pidl) else { return };
    let Ok(mut items) = folder.enumerate() else { return };
    items.retain(|i| i.is_folder() && !i.is_hidden());
    items.sort_by(|a, b| folder.compare(0, a, b));
    // shortcut: a menu past a few hundred entries is unusable anyway; a scrolling flyout would replace it
    items.truncate(500);
    let entries: Vec<(usize, String)> = items.iter().enumerate().map(|(i, it)| (i + 1, it.display_name())).collect();
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let picked = popup(app, &entries, pt.x, pt.y);
    if let Some(item) = picked.checked_sub(1).and_then(|i| items.get(i)) {
        go(app, Target::Pidl(folder.child_pidl(item)), nav::History::Push);
    }
}

pub(crate) unsafe fn more_menu(app: *mut App) {
    let exe = std::env::current_exe().unwrap_or_default();
    let reg = if register::is_registered(register::REAL_BASE, &exe) {
        (CMD_UNREGISTER as usize, "Unregister as folder handler...".to_string())
    } else {
        (CMD_REGISTER as usize, "Register as folder handler...".to_string())
    };
    let entries = [
        (CMD_EXPLORER as usize, "Open in Explorer\tCtrl+Shift+E".to_string()),
        reg,
        (CMD_SETTINGS as usize, "Settings folder".to_string()),
        (0, String::new()),
        (CMD_ABOUT as usize, "About rexplorer".to_string()),
    ];
    let r = toolbar::button_rect((*app).tools, CMD_MORE).unwrap_or_default();
    let picked = popup(app, &entries, r.left, r.bottom);
    if picked != 0 {
        on_command(app, picked as u16);
    }
}

/// --register / --unregister and the More menu: confirm, change HKCU, report.
pub(crate) unsafe fn register_flow(owner: Option<HWND>, unregister: bool) {
    let exe = std::env::current_exe().unwrap_or_default();
    let keys = r"HKEY_CURRENT_USER\Software\Classes\{Folder, Directory, Drive}\shell\open";
    let question = if unregister {
        format!("Stop opening folders with rexplorer?\n\nThis deletes {keys} (per-user). Windows Explorer opens folders again.")
    } else {
        format!(
            "Open folders with rexplorer?\n\nThis writes {keys}\\command = \"{}\" \"%1\" (per-user only; nothing under HKLM changes). \
             Win+E and the taskbar icon still open Explorer.\n\nUndo with rexplorer --unregister.",
            exe.display()
        )
    };
    let q = HSTRING::from(question);
    if MessageBoxW(owner, PCWSTR(q.as_ptr()), w!("rexplorer"), MB_YESNO | MB_ICONQUESTION) != IDYES {
        return;
    }
    let r = if unregister { register::unregister(register::REAL_BASE) } else { register::register(register::REAL_BASE, &exe) };
    let (text, icon) = match r {
        Ok(()) if unregister => ("rexplorer no longer opens folders.".to_string(), MB_ICONINFORMATION),
        Ok(()) => ("rexplorer now opens folders.".to_string(), MB_ICONINFORMATION),
        Err(e) => (format!("The registry change failed:\n\n{e}"), MB_ICONERROR),
    };
    let t = HSTRING::from(text);
    MessageBoxW(owner, PCWSTR(t.as_ptr()), w!("rexplorer"), MB_OK | icon);
}

pub(crate) unsafe fn about(app: *mut App) {
    let text = format!(
        "rexplorer {}\nA file manager that hosts the Windows shell namespace.\n\nProfile: {}\nTheme: {}\nSettings: {}",
        env!("CARGO_PKG_VERSION"),
        (*app).profile.name,
        (*app).theme.name,
        settings::dir().display()
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
    if hdr.code == palette::PN_PICK {
        if let Some(target) = palette::picked(hdr.hwndFrom) {
            go(app, Target::Name(target), nav::History::Push);
        }
    }
    let _ = SetFocus(Some((*app).list));
}

pub(crate) unsafe fn open_terminal(app: *mut App) {
    match listing(app).map(|l| l.folder.parsing_name.clone()).filter(|p| is_fs_path(p)) {
        Some(p) if instance::open_terminal(std::path::Path::new(&p)).is_ok() => {}
        _ => {
            let _ = MessageBeep(MB_OK);
        }
    }
}

pub(crate) unsafe fn update_status(app: *mut App) {
    let count = listing(app).map_or(0, |l| l.items.len());
    let selected = SendMessageW((*app).list, LVM_GETSELECTEDCOUNT, None, None).0 as usize;
    // Selected bytes are omitted: the size column is shell-formatted text, not a number.
    statusbar::set_text((*app).status, &statusbar::summary(count, selected, None), &(*app).profile.name);
}

#[cfg(test)]
mod tests {
    use super::is_fs_path;

    #[test]
    fn fs_paths() {
        assert!(is_fs_path("C:\\Users"));
        assert!(is_fs_path("d:"));
        assert!(is_fs_path("\\\\server\\share"));
        assert!(!is_fs_path("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"));
        assert!(!is_fs_path("shell:Desktop"));
        assert!(!is_fs_path("C"));
    }
}
