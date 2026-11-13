//! Explorer's desktop icon view, driven from outside its process: which icon is at a point, the selection, and
//! selecting, and the View / Sort by settings of its background menu. The view lives in explorer.exe;
//! `IShellWindows` hands out proxies to it.

use windows::Win32::Foundation::{E_FAIL, HWND, POINT};
use windows::Win32::System::Com::{CLSCTX_LOCAL_SERVER, CoCreateInstance, IServiceProvider};
use windows::Win32::System::Variant::{VARIANT, VT_I4};
use std::ffi::c_void;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::Accessibility::{AccessibleObjectFromWindow, IAccessible};
use windows::Win32::UI::Shell::{
    CSIDL_DESKTOP, FolderItems2, IShellBrowser, IShellFolderViewDual, IShellView, IShellWindows, SID_STopLevelBrowser, SVGIO_BACKGROUND,
    SVSI_DESELECTOTHERS, SVSI_FOCUSED, SVSI_SELECT, SWC_DESKTOP, SWFO_NEEDDISPATCH, ShellWindows,
};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Controls::{LVM_GETNEXTITEM, LVNI_FOCUSED, LVNI_SELECTED};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetParent, GetShellWindow, GetWindowThreadProcessId, OBJID_CLIENT, SMTO_ABORTIFHUNG, SendMessageTimeoutW,
};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::UI::Shell::{FOLDERFLAGS, FVM_ICON, FWF_AUTOARRANGE, FWF_SNAPTOGRID, IFolderView2, SORT_ASCENDING, SORTCOLUMN, SVSI_EDIT};
use windows::core::{GUID, Interface, Result};

use crate::com::Pidl;

/// The View menu's icon sizes, as Explorer offers them on the desktop (label keys; the app translates).
pub const ICON_SIZES: [(&str, i32); 3] = [("desk.large_icons", 96), ("desk.medium_icons", 48), ("desk.small_icons", 32)];
/// The View menu's arrangement switches.
pub const ARRANGE: [(&str, FOLDERFLAGS); 2] = [("desk.auto_arrange", FWF_AUTOARRANGE), ("desk.align_grid", FWF_SNAPTOGRID)];

const fn pkey(g: u128, pid: u32) -> PROPERTYKEY {
    PROPERTYKEY { fmtid: GUID::from_u128(g), pid }
}
const STORAGE: u128 = 0xB725F130_47EF_101A_A5F1_02608C9EEBAC;
/// The Sort by menu: System.ItemNameDisplay, System.Size, System.ItemTypeText, System.DateModified.
pub const SORT_KEYS: [(&str, PROPERTYKEY); 4] = [
    ("col.name", pkey(STORAGE, 10)),
    ("col.size", pkey(STORAGE, 12)),
    ("desk.item_type", pkey(0x28636AA6_953D_11D2_B5D6_00C04FD918D0, 11)),
    ("col.date_modified", pkey(STORAGE, 14)),
];

pub struct DesktopView {
    shell_view: IShellView,
    view: IFolderView2,
}

impl DesktopView {
    /// The desktop's view in the running shell; fails when Explorer is not the shell or is restarting.
    pub fn find() -> Result<DesktopView> {
        unsafe {
            let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)?;
            let loc = VARIANT::from(CSIDL_DESKTOP as i32);
            let mut hwnd = 0;
            let disp = windows.FindWindowSW(&loc, &VARIANT::default(), SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)?;
            let sp: IServiceProvider = disp.cast()?;
            let browser: IShellBrowser = sp.QueryService(&SID_STopLevelBrowser)?;
            let shell_view = browser.QueryActiveShellView()?;
            let view = shell_view.cast()?;
            Ok(DesktopView { shell_view, view })
        }
    }

    /// The index of the item of desktop list `list` under screen point `pt`, or None over the background.
    pub fn index_at(list: HWND, pt: POINT) -> Option<usize> {
        unsafe {
            let mut acc: Option<IAccessible> = None;
            AccessibleObjectFromWindow(list, OBJID_CLIENT.0 as u32, &IAccessible::IID, &mut acc as *mut _ as *mut *mut c_void).ok()?;
            let child = acc?.accHitTest(pt.x, pt.y).ok()?;
            // Child ids of a list view are its item indices plus one; 0 is the list itself.
            let v = &child.Anonymous.Anonymous;
            (v.vt == VT_I4 && v.Anonymous.lVal > 0).then(|| v.Anonymous.lVal as usize - 1)
        }
    }

    /// Screen rectangle's top-left corner of item `index` in desktop list `list` (for tests and menus).
    pub fn item_point(&self, list: HWND, index: usize) -> Result<POINT> {
        unsafe {
            let mut pt = self.view.GetItemPosition(self.item(index)?.as_ptr())?;
            let _ = ClientToScreen(list, &mut pt);
            Ok(pt)
        }
    }

    /// Child PIDL (relative to the desktop root) of item `index`.
    pub fn item(&self, index: usize) -> Result<Pidl> {
        unsafe { Ok(Pidl::from_raw(self.view.Item(index as i32)?)) }
    }

    /// Indices of the selected items of desktop list `list`, None when the list does not answer within a second.
    /// Read from the list itself: the view's own selection calls (ItemCount, GetSelectedItem) come back empty across
    /// the process boundary.
    pub fn selection(list: HWND) -> Option<Vec<usize>> {
        let mut out = Vec::new();
        let mut i = -1isize;
        loop {
            i = next_item(list, i, LVNI_SELECTED)?;
            if i < 0 {
                return Some(out);
            }
            out.push(i as usize);
        }
    }

    /// Index of the focused item of desktop list `list`, if any (None also when the list does not answer).
    pub fn focused(list: HWND) -> Option<usize> {
        usize::try_from(next_item(list, -1, LVNI_FOCUSED)?).ok()
    }

    /// What a right-button press does in Explorer: on an unselected icon, select just it; on a selected one, keep
    /// the selection; on the background, clear it.
    /// `selection` is the list's selection now.
    pub fn select_for_menu(&self, index: Option<usize>, selection: &[usize]) -> Result<()> {
        unsafe {
            match index {
                Some(i) if selection.contains(&i) => Ok(()),
                Some(i) => self.shell_view.SelectItem(self.item(i)?.as_ptr(), (SVSI_SELECT.0 | SVSI_DESELECTOTHERS.0 | SVSI_FOCUSED.0) as u32),
                None => self.shell_view.SelectItem(std::ptr::null(), SVSI_DESELECTOTHERS.0 as u32),
            }
        }
    }

    /// Starts Explorer's in-place rename of item `index`.
    pub fn rename(&self, index: usize) -> Result<()> {
        let flags = SVSI_EDIT.0 | SVSI_SELECT.0 | SVSI_DESELECTOTHERS.0 | SVSI_FOCUSED.0;
        unsafe { self.shell_view.SelectItem(self.item(index)?.as_ptr(), flags as u32) }
    }

    pub fn icon_size(&self) -> Result<i32> {
        let (mut mode, mut size) = (Default::default(), 0);
        unsafe { self.view.GetViewModeAndIconSize(&mut mode, &mut size)? };
        Ok(size)
    }

    pub fn set_icon_size(&self, size: i32) -> Result<()> {
        unsafe { self.view.SetViewModeAndIconSize(FVM_ICON, size) }
    }

    pub fn has_flag(&self, flag: FOLDERFLAGS) -> Result<bool> {
        Ok(unsafe { self.view.GetCurrentFolderFlags()? } & flag.0 as u32 != 0)
    }

    pub fn toggle_flag(&self, flag: FOLDERFLAGS) -> Result<()> {
        let on = self.has_flag(flag)?;
        unsafe { self.view.SetCurrentFolderFlags(flag.0 as u32, if on { 0 } else { flag.0 as u32 }) }
    }

    /// Arranges the icons by `key`, ascending, as the Sort by menu does.
    pub fn sort_by(&self, key: PROPERTYKEY) -> Result<()> {
        unsafe { self.view.SetSortColumns(&[SORTCOLUMN { propkey: key, direction: SORT_ASCENDING }]) }
    }

    pub fn refresh(&self) -> Result<()> {
        unsafe { self.shell_view.Refresh() }
    }

    /// Deletes the selected icons inside explorer.exe (Shell automation), so Explorer's own Ctrl+Z on the desktop
    /// undoes it: a delete run in this process lands in no undo history the desktop has. `paths` are the parsing
    /// names of the items the menu was built for. Fails, doing nothing, when Explorer cannot be asked or its
    /// selection is no longer exactly those items; a cancel in Explorer's own confirmation is no failure (the caller
    /// would ask again).
    pub fn delete_selection(&self, paths: &[String]) -> Result<()> {
        unsafe {
            let dual: IShellFolderViewDual = self.shell_view.GetItemObject(SVGIO_BACKGROUND)?;
            let items: FolderItems2 = dual.SelectedItems()?.cast()?;
            let mut selected = Vec::new();
            for i in 0..items.Count()? {
                selected.push(items.Item(&VARIANT::from(i))?.Path()?.to_string());
            }
            if !same_items(&selected, paths) {
                return Err(E_FAIL.into());
            }
            let _ = items.InvokeVerbEx(&VARIANT::from("delete"), &VARIANT::default());
            Ok(())
        }
    }
}

/// Whether two lists of parsing names hold the same items, in any order (paths compare case-insensitively).
fn same_items(a: &[String], b: &[String]) -> bool {
    let sorted = |v: &[String]| {
        let mut v: Vec<String> = v.iter().map(|s| s.to_lowercase()).collect();
        v.sort();
        v
    };
    sorted(a) == sorted(b)
}

/// LVM_GETNEXTITEM, waiting at most a second: a list that does not answer (a hung or hostile owner) must not
/// freeze the helper's main thread while the hooks keep taking the desktop's right-clicks. None on a timeout.
fn next_item(list: HWND, from: isize, flags: u32) -> Option<isize> {
    let mut out = 0usize;
    let sent = unsafe {
        SendMessageTimeoutW(list, LVM_GETNEXTITEM, WPARAM(from as usize), LPARAM(flags as isize), SMTO_ABORTIFHUNG, 1000, Some(&mut out))
    };
    (sent.0 != 0).then_some(out as isize)
}

/// Whether `hwnd` is the desktop's icon list: a SysListView32 in a SHELLDLL_DefView hosted by Progman or WorkerW,
/// all three owned by the shell's process. Any program can build that class chain; only the shell owns the real one.
/// Safe inside a low-level hook: GetParent, GetClassNameW, GetWindowThreadProcessId and GetShellWindow read window
/// data and send no message.
pub fn is_desktop_list(hwnd: HWND) -> bool {
    let Some((classes, owners)) = chain(hwnd) else { return false };
    let shell = unsafe { GetShellWindow() };
    let shell_pid = if shell.is_invalid() { 0 } else { owner(shell) };
    is_shell_desktop_list(classes.each_ref().map(String::as_str), owners, shell_pid)
}

fn owner(h: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    pid
}

/// The classes and owning processes of `hwnd`, its parent and its grandparent.
fn chain(hwnd: HWND) -> Option<([String; 3], [u32; 3])> {
    let class = |h: HWND| {
        let mut buf = [0u16; 32];
        let n = unsafe { GetClassNameW(h, &mut buf) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    };
    let view = unsafe { GetParent(hwnd) }.ok()?;
    let host = unsafe { GetParent(view) }.ok()?;
    Some(([hwnd, view, host].map(class), [hwnd, view, host].map(owner)))
}

/// The decision of `is_desktop_list`: the classes of (list, its parent, its grandparent), the processes owning them,
/// and the shell's process (0 when there is no shell window).
fn is_shell_desktop_list(classes: [&str; 3], owners: [u32; 3], shell_pid: u32) -> bool {
    classes[0] == "SysListView32"
        && classes[1] == "SHELLDLL_DefView"
        && matches!(classes[2], "Progman" | "WorkerW")
        && shell_pid != 0
        && owners.iter().all(|&p| p == shell_pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_shells_own_list_is_the_desktop() {
        let real = ["SysListView32", "SHELLDLL_DefView", "Progman"];
        assert!(is_shell_desktop_list(real, [7, 7, 7], 7));
        assert!(is_shell_desktop_list(["SysListView32", "SHELLDLL_DefView", "WorkerW"], [7, 7, 7], 7));
        // The same class chain built by another program, or a list of its own planted inside the shell's view.
        assert!(!is_shell_desktop_list(real, [9, 9, 9], 7));
        assert!(!is_shell_desktop_list(real, [9, 7, 7], 7));
        assert!(!is_shell_desktop_list(real, [7, 7, 9], 7));
        // No shell window: nothing is the desktop.
        assert!(!is_shell_desktop_list(real, [0, 0, 0], 0));
        assert!(!is_shell_desktop_list(["SysListView32", "SHELLDLL_DefView", "CabinetWClass"], [7, 7, 7], 7));
        assert!(!is_shell_desktop_list(["SysTreeView32", "SHELLDLL_DefView", "Progman"], [7, 7, 7], 7));
    }

    #[test]
    fn a_forged_class_chain_is_refused_live() {
        // Windows of this process with the desktop's class names: the chain matches, the owner does not. The host is
        // message-only and nothing is shown, so no other program (FindWindow, EnumWindows, a click) can come upon them.
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, HWND_MESSAGE, RegisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
            WS_CHILD,
        };
        use windows::core::{PCWSTR, w};
        unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> windows::Win32::Foundation::LRESULT {
            unsafe { DefWindowProcW(h, m, w, l) }
        }
        let make = |class: PCWSTR, parent: HWND, style: WINDOW_STYLE| unsafe {
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(proc), lpszClassName: class, ..Default::default() });
            CreateWindowExW(WINDOW_EX_STYLE(0), class, None, style, 0, 0, 1, 1, Some(parent), None, None, None).unwrap()
        };
        let host = make(w!("Progman"), HWND_MESSAGE, WINDOW_STYLE(0));
        let view = make(w!("SHELLDLL_DefView"), host, WS_CHILD);
        let list = make(w!("SysListView32"), view, WS_CHILD);
        let (classes, owners) = chain(list).unwrap();
        let me = std::process::id();
        assert!(is_shell_desktop_list(classes.each_ref().map(String::as_str), owners, me), "{classes:?} {owners:?}");
        assert!(!is_desktop_list(list));
        unsafe { DestroyWindow(host).unwrap() };
    }

    #[test]
    fn selections_compare_as_sets_of_paths() {
        let v = |s: &[&str]| s.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(same_items(&v(&[r"C:\Users\me\Desktop\a.txt", r"C:\Users\me\Desktop\B"]), &v(&[r"c:\users\me\desktop\b", r"C:\Users\me\Desktop\a.txt"])));
        assert!(same_items(&[], &[]));
        assert!(!same_items(&v(&[r"C:\d\a.txt"]), &v(&[r"C:\d\b.txt"])));
        assert!(!same_items(&v(&[r"C:\d\a.txt", r"C:\d\b.txt"]), &v(&[r"C:\d\a.txt"])));
        assert!(!same_items(&v(&[r"C:\d\a.txt", r"C:\d\a.txt"]), &v(&[r"C:\d\a.txt", r"C:\d\b.txt"])));
    }
}
