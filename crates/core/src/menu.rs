use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Shell::{
    CMF_CANRENAME, CMF_DEFAULTONLY, CMF_EXPLORE, CMF_EXTENDEDVERBS, CMF_NORMAL,
    CMIC_MASK_PTINVOKE, CMIC_MASK_SHIFT_DOWN, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX,
    GCS_VERBW, IContextMenu, IContextMenu2, IContextMenu3, SEE_MASK_UNICODE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, GMDI_GOINTOPOPUPS, GetMenuDefaultItem, GetMenuItemCount, GetMenuItemID,
    GetMenuStringW, GetSubMenu, HMENU, MF_BYPOSITION, SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenuEx, WM_INITMENUPOPUP,
};
use windows::core::{HSTRING, Interface, PCSTR, PCWSTR, PSTR, Result};

use crate::folder::ShellFolder;
use crate::item::ShellItem;

const ID_FIRST: u32 = 1;
const ID_LAST: u32 = 0x7FFF;
/// The caller's own entries (`show_in`) take ids from here, above the shell's range.
pub const OWN_FIRST: u32 = 0x8000;

/// What the user picked from a shown menu.
pub enum Picked {
    /// The shell handled it (open, copy, delete, properties, third-party verbs...).
    Invoked,
    /// Rename is the one verb the view must perform itself.
    Rename,
    Cancelled,
    /// One of the caller's own entries of a `show_in` menu: its id minus `OWN_FIRST`.
    Own(usize),
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

    /// Shows the caller's `menu` at screen point (x, y) with the shell's entries inserted at position `at`, and
    /// invokes a chosen shell command. The caller's entries carry ids `OWN_FIRST + n` and come back as
    /// `Picked::Own(n)`. The owning window must forward menu messages to `handle_message` while this runs.
    pub fn show_in(&self, hwnd: HWND, menu: HMENU, at: u32, x: i32, y: i32, extended: bool) -> Result<Picked> {
        let mut flags = CMF_EXPLORE | CMF_CANRENAME;
        if extended {
            flags |= CMF_EXTENDEDVERBS;
        }
        unsafe {
            self.menu.QueryContextMenu(menu, at, ID_FIRST, ID_LAST, flags).ok()?;
            let picked = TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, x, y, hwnd, None).0 as u32;
            if picked >= OWN_FIRST {
                return Ok(Picked::Own((picked - OWN_FIRST) as usize));
            }
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

    /// The entries of a background menu's "New" submenu: (command id, text) in menu order, without separators and
    /// without the folder entry (canonical verb "NewFolder", which is how the submenu is recognized: its title is
    /// localized). Empty when the menu has no such submenu. Run an entry with `invoke_command` on this same menu.
    pub fn new_items(&self) -> Result<Vec<(u32, String)>> {
        unsafe {
            self.menu.QueryContextMenu(self.hmenu, 0, ID_FIRST, ID_LAST, CMF_NORMAL).ok()?;
            for pos in 0..GetMenuItemCount(Some(self.hmenu)) {
                let sub = GetSubMenu(self.hmenu, pos);
                if sub.is_invalid() {
                    continue;
                }
                // The shell fills some submenus, New among them, only when they open.
                self.handle_message(WM_INITMENUPOPUP, WPARAM(sub.0 as usize), LPARAM(pos as isize));
                let mut entries = Vec::new();
                let mut found = false;
                for i in 0..GetMenuItemCount(Some(sub)) {
                    let id = GetMenuItemID(sub, i);
                    if !(ID_FIRST..=ID_LAST).contains(&id) {
                        continue; // separators and nested submenus
                    }
                    if self.verb_of(id - ID_FIRST).eq_ignore_ascii_case("NewFolder") {
                        found = true;
                        continue;
                    }
                    let mut buf = [0u16; 128];
                    let n = GetMenuStringW(sub, i as u32, Some(&mut buf), MF_BYPOSITION).max(0) as usize;
                    let text = String::from_utf16_lossy(&buf[..n]);
                    if !text.is_empty() {
                        entries.push((id, text));
                    }
                }
                if found {
                    return Ok(entries);
                }
            }
            Ok(Vec::new())
        }
    }

    /// Runs an entry by the id `new_items` gave it.
    pub fn invoke_command(&self, hwnd: HWND, id: u32) -> Result<()> {
        unsafe { self.invoke_id(hwnd, id - ID_FIRST, None, false) }
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

    #[test]
    fn background_new_submenu_and_new_folder() {
        let _c = ComInit::sta();
        let dir = std::env::temp_dir().join(format!("rexplorer-newmenu-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = ShellFolder::parse(dir.to_str().unwrap()).unwrap();
        let items = ContextMenu::for_background(&f, HWND::default()).unwrap().new_items().unwrap();
        // NewFolder, run without a view, still creates the folder.
        let made = ContextMenu::for_background(&f, HWND::default()).unwrap().invoke_verb(HWND::default(), "NewFolder", false);
        let subdirs = std::fs::read_dir(&dir).unwrap().filter(|e| e.as_ref().is_ok_and(|e| e.path().is_dir())).count();
        std::fs::remove_dir_all(&dir).unwrap();
        println!("new items: {items:?}");
        assert!(!items.is_empty(), "the New submenu lists file types");
        assert!(made.is_ok() && subdirs == 1, "NewFolder made {subdirs} folders: {made:?}");
    }
}
