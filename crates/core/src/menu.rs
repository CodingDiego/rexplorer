use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Shell::{
    CMF_CANRENAME, CMF_DEFAULTONLY, CMF_EXPLORE, CMF_EXTENDEDVERBS, CMF_NORMAL,
    CMIC_MASK_PTINVOKE, CMIC_MASK_SHIFT_DOWN, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX,
    GCS_VERBW, IContextMenu, IContextMenu2, IContextMenu3, SEE_MASK_UNICODE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, GMDI_GOINTOPOPUPS, GetMenuDefaultItem, HMENU, SW_SHOWNORMAL,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx,
};
use windows::core::{HSTRING, Interface, PCSTR, PCWSTR, PSTR, Result};

use crate::folder::ShellFolder;
use crate::item::ShellItem;

const ID_FIRST: u32 = 1;
const ID_LAST: u32 = 0x7FFF;

/// What the user picked from a shown menu.
pub enum Picked {
    /// The shell handled it (open, copy, delete, properties, third-party verbs...).
    Invoked,
    /// Rename is the one verb the view must perform itself.
    Rename,
    Cancelled,
}

/// The shell context menu for a selection or for the folder background.
pub struct ContextMenu {
    menu: IContextMenu,
    hmenu: HMENU,
    menu2: Option<IContextMenu2>,
    menu3: Option<IContextMenu3>,
}

impl ContextMenu {
    pub fn for_items(folder: &ShellFolder, items: &[&ShellItem], hwnd: HWND) -> Result<Self> {
        let pidls: Vec<_> = items.iter().map(|i| i.pidl.as_ptr()).collect();
        let menu: IContextMenu = unsafe { folder.folder.GetUIObjectOf(hwnd, &pidls, None)? };
        Self::new(menu)
    }

    pub fn for_background(folder: &ShellFolder, hwnd: HWND) -> Result<Self> {
        let menu: IContextMenu = unsafe { folder.folder.CreateViewObject(hwnd)? };
        Self::new(menu)
    }

    fn new(menu: IContextMenu) -> Result<Self> {
        let hmenu = unsafe { CreatePopupMenu()? };
        Ok(ContextMenu { menu2: menu.cast().ok(), menu3: menu.cast().ok(), menu, hmenu })
    }

    /// Shows the menu at screen point (x, y) and invokes the chosen command.
    /// The owning window must forward menu messages to `handle_message` while this runs.
    pub fn show(&self, hwnd: HWND, x: i32, y: i32, extended: bool) -> Result<Picked> {
        let mut flags = CMF_EXPLORE | CMF_CANRENAME;
        if extended {
            flags |= CMF_EXTENDEDVERBS;
        }
        unsafe {
            self.menu.QueryContextMenu(self.hmenu, 0, ID_FIRST, ID_LAST, flags).ok()?;
            let picked = TrackPopupMenuEx(self.hmenu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, x, y, hwnd, None).0 as u32;
            if picked < ID_FIRST {
                return Ok(Picked::Cancelled);
            }
            let id = picked - ID_FIRST;
            if self.verb_of(id).eq_ignore_ascii_case("rename") {
                return Ok(Picked::Rename);
            }
            self.invoke_id(hwnd, id, Some(POINT { x, y }), false)?;
            Ok(Picked::Invoked)
        }
    }

    /// Runs the default verb (what a double-click does).
    pub fn invoke_default(&self, hwnd: HWND) -> Result<()> {
        unsafe {
            self.menu.QueryContextMenu(self.hmenu, 0, ID_FIRST, ID_LAST, CMF_DEFAULTONLY).ok()?;
            let id = GetMenuDefaultItem(self.hmenu, 0, GMDI_GOINTOPOPUPS);
            if id == u32::MAX {
                return Err(windows::core::Error::empty());
            }
            self.invoke_id(hwnd, id - ID_FIRST, None, false)
        }
    }

    /// Runs a canonical verb: "copy", "cut", "paste", "delete", "properties"...
    pub fn invoke_verb(&self, hwnd: HWND, verb: &str, shift: bool) -> Result<()> {
        let narrow = std::ffi::CString::new(verb).expect("ascii verb");
        let wide = HSTRING::from(verb);
        unsafe {
            self.menu.QueryContextMenu(self.hmenu, 0, ID_FIRST, ID_LAST, CMF_NORMAL).ok()?;
            self.invoke(hwnd, PCSTR(narrow.as_ptr() as *const u8), PCWSTR(wide.as_ptr()), None, shift)
        }
    }

    unsafe fn invoke_id(&self, hwnd: HWND, id: u32, at: Option<POINT>, shift: bool) -> Result<()> {
        // MAKEINTRESOURCE: the verb pointer carries the command id in its low word.
        unsafe { self.invoke(hwnd, PCSTR(id as usize as *const u8), PCWSTR(id as usize as *const u16), at, shift) }
    }

    unsafe fn invoke(&self, hwnd: HWND, verb: PCSTR, verb_w: PCWSTR, at: Option<POINT>, shift: bool) -> Result<()> {
        let mut info = CMINVOKECOMMANDINFOEX {
            cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
            // CMIC_MASK_UNICODE is defined as SEE_MASK_UNICODE in the SDK.
            fMask: SEE_MASK_UNICODE,
            hwnd,
            lpVerb: verb,
            lpVerbW: verb_w,
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        if let Some(pt) = at {
            info.fMask |= CMIC_MASK_PTINVOKE;
            info.ptInvoke = pt;
        }
        if shift {
            info.fMask |= CMIC_MASK_SHIFT_DOWN;
        }
        unsafe { self.menu.InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO) }
    }

    fn verb_of(&self, id: u32) -> String {
        let mut buf = [0u16; 64];
        let ok = unsafe {
            self.menu.GetCommandString(id as usize, GCS_VERBW, None, PSTR(buf.as_mut_ptr() as *mut u8), buf.len() as u32)
        };
        if ok.is_err() {
            return String::new();
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    }

    /// Lets the shell fill submenus (Send to, Open with) and draw owner-drawn items.
    pub fn handle_message(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        unsafe {
            if let Some(m3) = &self.menu3 {
                let mut result = LRESULT(0);
                m3.HandleMenuMsg2(msg, wparam, lparam, Some(&mut result)).ok()?;
                return Some(result);
            }
            if let Some(m2) = &self.menu2 {
                m2.HandleMenuMsg(msg, wparam, lparam).ok()?;
                return Some(LRESULT(0));
            }
        }
        None
    }
}

impl Drop for ContextMenu {
    fn drop(&mut self) {
        unsafe { DestroyMenu(self.hmenu).ok() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::ComInit;
    use crate::folder::list;

    #[test]
    fn menus_build_for_items_and_background() {
        let _c = ComInit::sta();
        let (f, items) = list(r"C:\Windows").unwrap();
        let first: Vec<&ShellItem> = items.iter().take(1).collect();
        let m = ContextMenu::for_items(&f, &first, HWND::default()).unwrap();
        unsafe { m.menu.QueryContextMenu(m.hmenu, 0, ID_FIRST, ID_LAST, CMF_NORMAL).ok().unwrap() };
        assert!(ContextMenu::for_background(&f, HWND::default()).is_ok());
    }
}
