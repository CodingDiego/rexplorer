//! Explorer's desktop icon view, driven from outside its process: which icon is at a point, the selection, and
//! selecting, and the View / Sort by settings of its background menu. The view lives in explorer.exe;
//! `IShellWindows` hands out proxies to it.

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::Com::{CLSCTX_LOCAL_SERVER, CoCreateInstance, IServiceProvider};
use windows::Win32::System::Variant::{VARIANT, VT_I4};
use std::ffi::c_void;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::Accessibility::{AccessibleObjectFromWindow, IAccessible};
use windows::Win32::UI::Shell::{
    CSIDL_DESKTOP, IShellBrowser, IShellView, IShellWindows, SID_STopLevelBrowser,
    SVSI_DESELECTOTHERS, SVSI_FOCUSED, SVSI_SELECT, SWC_DESKTOP, SWFO_NEEDDISPATCH, ShellWindows,
};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Controls::{LVM_GETNEXTITEM, LVNI_SELECTED};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetParent, OBJID_CLIENT, SendMessageW};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::UI::Shell::{FOLDERFLAGS, FVM_ICON, FWF_AUTOARRANGE, FWF_SNAPTOGRID, IFolderView2, SORT_ASCENDING, SORTCOLUMN, SVSI_EDIT};
use windows::core::{GUID, Interface, Result};

use crate::com::Pidl;

/// The View menu's icon sizes, as Explorer offers them on the desktop.
pub const ICON_SIZES: [(&str, i32); 3] = [("Large icons", 96), ("Medium icons", 48), ("Small icons", 32)];
/// The View menu's arrangement switches.
pub const ARRANGE: [(&str, FOLDERFLAGS); 2] = [("Auto arrange icons", FWF_AUTOARRANGE), ("Align icons to grid", FWF_SNAPTOGRID)];

const fn pkey(g: u128, pid: u32) -> PROPERTYKEY {
    PROPERTYKEY { fmtid: GUID::from_u128(g), pid }
}
const STORAGE: u128 = 0xB725F130_47EF_101A_A5F1_02608C9EEBAC;
/// The Sort by menu: System.ItemNameDisplay, System.Size, System.ItemTypeText, System.DateModified.
pub const SORT_KEYS: [(&str, PROPERTYKEY); 4] = [
    ("Name", pkey(STORAGE, 10)),
    ("Size", pkey(STORAGE, 12)),
    ("Item type", pkey(0x28636AA6_953D_11D2_B5D6_00C04FD918D0, 11)),
    ("Date modified", pkey(STORAGE, 14)),
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

    /// Indices of the selected items of desktop list `list`. Read from the list itself: the view's own selection
    /// calls (ItemCount, GetSelectedItem) come back empty across the process boundary.
    pub fn selection(list: HWND) -> Vec<usize> {
        let mut out = Vec::new();
        let mut i = -1isize;
        loop {
            i = unsafe { SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(i as usize)), Some(LPARAM(LVNI_SELECTED as isize))) }.0;
            if i < 0 {
                return out;
            }
            out.push(i as usize);
        }
    }

    /// What a right-button press does in Explorer: on an unselected icon, select just it; on a selected one, keep
    /// the selection; on the background, clear it.
    pub fn select_for_menu(&self, list: HWND, index: Option<usize>) -> Result<()> {
        unsafe {
            match index {
                Some(i) if Self::selection(list).contains(&i) => Ok(()),
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
}

/// Whether `hwnd` is the desktop's icon list: a SysListView32 in a SHELLDLL_DefView hosted by Progman or WorkerW.
pub fn is_desktop_list(hwnd: HWND) -> bool {
    let class = |h: HWND| {
        let mut buf = [0u16; 32];
        let n = unsafe { GetClassNameW(h, &mut buf) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    };
    let Ok(view) = (unsafe { GetParent(hwnd) }) else { return false };
    let Ok(host) = (unsafe { GetParent(view) }) else { return false };
    class(hwnd) == "SysListView32" && class(view) == "SHELLDLL_DefView" && matches!(class(host).as_str(), "Progman" | "WorkerW")
}
