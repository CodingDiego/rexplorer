use std::path::Path;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::DeleteObject;
use windows::Win32::System::Com::IDataObject;
use windows::Win32::UI::Shell::{
    CMF_CANRENAME, CMF_DEFAULTONLY, CMF_EXPLORE, CMF_EXTENDEDVERBS, CMF_NORMAL,
    CMF_OPTIMIZEFORINVOKE,
    CMIC_MASK_PTINVOKE, CMIC_MASK_SHIFT_DOWN, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX,
    FOLDERID_Desktop, GCS_VERBW, IContextMenu, IContextMenu2, IContextMenu3, ILCombine, KF_FLAG_DEFAULT, SEE_MASK_NOASYNC, SEE_MASK_UNICODE, SHCreateShellItemArrayFromIDLists, BHID_DataObject,
    SHGetDesktopFolder, SHGetKnownFolderPath, SHMultiFileProperties,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DeleteMenu, DestroyMenu, GMDI_GOINTOPOPUPS, GetMenuDefaultItem, GetMenuItemCount, GetMenuItemID,
    GetMenuItemInfoW, GetMenuStringW, GetSubMenu, HMENU, InsertMenuItemW, MENUITEMINFOW, MF_BYPOSITION, MFT_SEPARATOR, MFT_STRING, MIIM_BITMAP, MIIM_FTYPE, MIIM_ID, MIIM_STRING, SW_SHOWNORMAL, TPM_LAYOUTRTL, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TrackPopupMenuEx, WM_INITMENUPOPUP,
};
use windows::core::{HSTRING, Interface, PCSTR, PCWSTR, PSTR, PWSTR, Result};

use crate::com::{Pidl, take_cotaskmem_string};
use crate::folder::ShellFolder;
use crate::item::ShellItem;
use crate::packaged::{self, Target};

const ID_FIRST: u32 = 1;
// Below the packaged commands' ids (PACKAGED_FIRST..OWN_FIRST); the shell never gets more than this many.
const ID_LAST: u32 = 0x6FFF;
const PACKAGED_FIRST: u32 = 0x7000;
/// The caller's own entries (`show_in`) take ids from here, above the shell's range.
pub const OWN_FIRST: u32 = 0x8000;

/// What the user picked from a shown menu.
pub enum Picked {
    /// The shell handled it (open, copy, delete, properties, third-party verbs...); carries the canonical verb, empty
    /// when the handler has none.
    Invoked(String),
    /// Rename is the one verb the view must perform itself.
    Rename,
    /// Delete, not run yet: the view runs it (to record it for undo).
    Delete,
    /// An entry of the New submenu, already run: the view renames and records what appears.
    New,
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
    built: Built,
    /// Absolute PIDLs of items from several folders (`for_scattered`), else empty: the desktop's menu over them
    /// does nothing for Properties, so that verb goes to SHMultiFileProperties.
    scattered: Vec<Pidl>,
    /// Canonical verbs of shell entries `show_in` leaves out (the caller shows its own entry for them).
    hidden: Vec<&'static str>,
}

/// What the menu was built for, so `show_in` can ask the packaged commands about the same selection.
enum Built {
    Items(Vec<ShellItem>),
    /// The folder's file system path; None when it has none (virtual folders).
    Background(Option<String>),
}

impl ContextMenu {
    pub fn for_items(folder: &ShellFolder, items: &[&ShellItem], hwnd: HWND) -> Result<Self> {
        let pidls: Vec<_> = items.iter().map(|i| i.pidl.as_ptr()).collect();
        let menu: IContextMenu = unsafe { folder.folder.GetUIObjectOf(hwnd, &pidls, None)? };
        Self::new(menu, Built::Items(items.iter().map(|i| (*i).clone()).collect()))
    }

    /// Items of several folders, each with its folder's absolute PIDL (search results): the desktop folder builds
    /// one menu over their absolute PIDLs, as no single parent folder holds them all.
    pub fn for_scattered(items: &[(&Pidl, &ShellItem)], hwnd: HWND) -> Result<Self> {
        let abs: Vec<Pidl> = items.iter().map(|(f, i)| unsafe { Pidl::from_raw(ILCombine(Some(f.as_ptr()), Some(i.pidl.as_ptr()))) }).collect();
        let pidls: Vec<_> = abs.iter().map(Pidl::as_ptr).collect();
        let menu: IContextMenu = unsafe { SHGetDesktopFolder()?.GetUIObjectOf(hwnd, &pidls, None)? };
        let mut m = Self::new(menu, Built::Items(items.iter().map(|(_, i)| (*i).clone()).collect()))?;
        m.scattered = abs;
        Ok(m)
    }

    pub fn for_background(folder: &ShellFolder, hwnd: HWND) -> Result<Self> {
        let menu: IContextMenu = unsafe { folder.folder.CreateViewObject(hwnd)? };
        // The desktop root has no path of its own; its background is the user's Desktop folder.
        let path = if folder.pidl.is_empty() {
            unsafe { SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None) }.ok().map(|p| unsafe { take_cotaskmem_string(p) })
        } else {
            Some(folder.parsing_name.clone()).filter(|p| !p.starts_with("::"))
        };
        Self::new(menu, Built::Background(path))
    }

    /// The default menu of `items`, or of the folder's background when empty, without extension handlers: only the
    /// shell's built-in verbs (copy, cut, paste). Running those through `for_items` / `for_background` loads every
    /// registered handler, and one of them (Work Folders, Windows 11) leaves a thread behind for each menu.
    pub fn builtin(folder: &ShellFolder, items: &[&ShellItem], hwnd: HWND) -> Result<Self> {
        use std::mem::ManuallyDrop;
        use windows::Win32::UI::Shell::{DEFCONTEXTMENU, SHCreateDefaultContextMenu};
        // The API takes mutable pointers but only reads them.
        let mut pidls: Vec<_> = items.iter().map(|i| i.pidl.as_ptr().cast_mut()).collect();
        let mut dcm = DEFCONTEXTMENU {
            hwnd,
            pidlFolder: folder.pidl.as_ptr().cast_mut(),
            psf: ManuallyDrop::new(Some(folder.folder.clone())),
            cidl: pidls.len() as u32,
            apidl: if pidls.is_empty() { std::ptr::null_mut() } else { pidls.as_mut_ptr() },
            ..Default::default()
        };
        let menu: Result<IContextMenu> = unsafe { SHCreateDefaultContextMenu(&dcm) };
        unsafe { ManuallyDrop::drop(&mut dcm.psf) };
        let built = if items.is_empty() {
            Built::Background(Some(folder.parsing_name.clone()).filter(|p| !p.starts_with("::")))
        } else {
            Built::Items(items.iter().map(|i| (*i).clone()).collect())
        };
        Self::new(menu?, built)
    }

    fn new(menu: IContextMenu, built: Built) -> Result<Self> {
        let hmenu = unsafe { CreatePopupMenu()? };
        Ok(ContextMenu { menu2: menu.cast().ok(), menu3: menu.cast().ok(), menu, hmenu, built, scattered: Vec::new(), hidden: Vec::new() })
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
            self.drop_hidden(menu, at);
            let (selection, cmds) = self.add_packaged(menu, at);
            // TPM_LAYOUTRTL mirrors the alignment too: left alignment then opens the menu leftwards from the point.
            let rtl = if crate::i18n::is_rtl() { TPM_LAYOUTRTL.0 } else { 0 };
            let picked = TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0 | rtl, x, y, hwnd, None).0 as u32;
            for c in &cmds {
                if let Some(b) = c.icon {
                    let _ = DeleteObject(b.into());
                }
            }
            if picked >= OWN_FIRST {
                return Ok(Picked::Own((picked - OWN_FIRST) as usize));
            }
            if (PACKAGED_FIRST..OWN_FIRST).contains(&picked) {
                let Some(cmd) = cmds.get((picked - PACKAGED_FIRST) as usize) else { return Ok(Picked::Cancelled) };
                packaged::invoke(&cmd.clsid, &selection.1)?;
                return Ok(Picked::Invoked(String::new()));
            }
            if picked < ID_FIRST {
                return Ok(Picked::Cancelled);
            }
            let id = picked - ID_FIRST;
            let verb = self.verb_of(id);
            if verb.eq_ignore_ascii_case("rename") {
                return Ok(Picked::Rename);
            }
            if verb.eq_ignore_ascii_case("delete") {
                return Ok(Picked::Delete);
            }
            let new = self.in_new_menu(menu, picked);
            if verb.eq_ignore_ascii_case("properties") && self.scattered_properties()? {
                return Ok(Picked::Invoked(verb));
            }
            self.invoke_id(hwnd, id, Some(POINT { x, y }), false)?;
            Ok(if new { Picked::New } else { Picked::Invoked(verb) })
        }
    }

    /// Leaves the shell's entry for `verb` out of `show_in`'s menu (the caller has its own, as Pin to Home).
    pub fn hide_verb(&mut self, verb: &'static str) {
        self.hidden.push(verb);
    }

    /// Deletes the top-level shell entries at or past `at` whose verb is hidden.
    unsafe fn drop_hidden(&self, menu: HMENU, at: u32) {
        if self.hidden.is_empty() {
            return;
        }
        let n = unsafe { GetMenuItemCount(Some(menu)) }.max(0) as u32;
        for pos in (at..n).rev() {
            let id = unsafe { GetMenuItemID(menu, pos as i32) };
            if (ID_FIRST..=ID_LAST).contains(&id) && self.hidden.iter().any(|v| self.verb_of(id - ID_FIRST).eq_ignore_ascii_case(v)) {
                let _ = unsafe { DeleteMenu(menu, pos, MF_BYPOSITION) };
            }
        }
    }

    /// Whether command `id` (as the menu holds it) sits in the New submenu of `menu`.
    unsafe fn in_new_menu(&self, menu: HMENU, id: u32) -> bool {
        unsafe {
            (0..GetMenuItemCount(Some(menu))).map(|p| GetSubMenu(menu, p)).filter(|s| !s.is_invalid()).any(|sub| {
                let ids: Vec<u32> = (0..GetMenuItemCount(Some(sub))).map(|i| GetMenuItemID(sub, i)).collect();
                ids.contains(&id) && ids.iter().any(|&i| self.is_new_folder(i))
            })
        }
    }

    /// The New submenu is recognized by its folder entry, canonical verb "NewFolder": its title is localized.
    fn is_new_folder(&self, id: u32) -> bool {
        (ID_FIRST..=ID_LAST).contains(&id) && self.verb_of(id - ID_FIRST).eq_ignore_ascii_case("NewFolder")
    }

    /// What the packaged commands are asked about: a target per item and the file system paths. None for virtual
    /// items, a mix of files and folders, or a selection that is gone.
    fn selection(&self) -> Option<(Vec<Target>, Vec<String>)> {
        match &self.built {
            Built::Background(p) => Some((vec![Target::Background], vec![p.clone()?])),
            // shortcut: a big selection gets no packaged commands (a name per item on the UI thread); raise if wanted.
            Built::Items(items) if items.is_empty() || items.len() > 16 => None,
            Built::Items(items) => {
                let paths: Vec<String> = items.iter().map(|i| i.parsing_name()).collect();
                let mut targets = Vec::new();
                for (item, p) in items.iter().zip(&paths) {
                    if p.starts_with("::") || in_recycle_bin(p) {
                        return None;
                    }
                    targets.push(if item.is_folder() {
                        Target::Folder
                    } else {
                        Target::File(Path::new(p).extension().map_or(String::new(), |e| format!(".{}", e.to_string_lossy().to_lowercase())))
                    });
                }
                let folders = targets.iter().filter(|t| **t == Target::Folder).count();
                (folders == 0 || folders == targets.len()).then_some((targets, paths))
            }
        }
    }

    /// Inserts the packaged commands that fit the selection right after the first separator at or past `at` (else at
    /// the end), with ids from PACKAGED_FIRST. Skipped when the registration scan has not finished: it never waits.
    unsafe fn add_packaged(&self, menu: HMENU, at: u32) -> ((Vec<Target>, Vec<String>), Vec<packaged::Command>) {
        let none = ((Vec::new(), Vec::new()), Vec::new());
        let Some(regs) = packaged::ready() else {
            packaged::warm();
            return none;
        };
        let Some(selection) = self.selection() else { return none };
        let count = unsafe { GetMenuItemCount(Some(menu)) }.max(0) as u32;
        // The verbs of the shell's own top-level items: a handler it already lists is not added twice.
        let verbs: Vec<String> = (at..count)
            .map(|i| unsafe { GetMenuItemID(menu, i as i32) })
            .filter(|id| (ID_FIRST..=ID_LAST).contains(id))
            .map(|id| self.verb_of(id - ID_FIRST))
            .collect();
        let mut cmds = packaged::query(&packaged::to_query(regs, &verbs), &selection.0, &selection.1);
        cmds.truncate((OWN_FIRST - PACKAGED_FIRST) as usize);
        let is_sep = |i: u32| {
            let mut mii = MENUITEMINFOW { cbSize: size_of::<MENUITEMINFOW>() as u32, fMask: MIIM_FTYPE, ..Default::default() };
            unsafe { GetMenuItemInfoW(menu, i, true, &mut mii) }.is_ok() && mii.fType.0 & MFT_SEPARATOR.0 != 0
        };
        let mut pos = (at..count).find(|&i| is_sep(i)).map_or(count, |i| i + 1);
        for (n, c) in cmds.iter().enumerate() {
            let text = HSTRING::from(c.title.as_str());
            let mii = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_ID | MIIM_STRING | MIIM_FTYPE | MIIM_BITMAP,
                fType: MFT_STRING,
                wID: PACKAGED_FIRST + n as u32,
                dwTypeData: PWSTR(text.as_ptr() as *mut u16),
                hbmpItem: c.icon.unwrap_or_default(),
                ..Default::default()
            };
            if unsafe { InsertMenuItemW(menu, pos, true, &mii) }.is_ok() {
                pos += 1;
            }
        }
        (selection, cmds)
    }

    /// One Properties sheet for items of several folders; false when this menu is not over such items.
    fn scattered_properties(&self) -> Result<bool> {
        if self.scattered.is_empty() {
            return Ok(false);
        }
        let pidls: Vec<_> = self.scattered.iter().map(Pidl::as_ptr).collect();
        unsafe {
            // A data object straight over absolute PIDLs gets "properties not available"; the item array's own works.
            let data: IDataObject = SHCreateShellItemArrayFromIDLists(&pidls)?.BindToHandler(None, &BHID_DataObject)?;
            SHMultiFileProperties(&data, 0)?;
        }
        Ok(true)
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
        if verb.eq_ignore_ascii_case("properties") && self.scattered_properties()? {
            return Ok(());
        }
        let narrow = std::ffi::CString::new(verb).map_err(|_| windows::core::Error::from(windows::Win32::Foundation::E_INVALIDARG))?;
        let wide = HSTRING::from(verb);
        unsafe {
            // The built-in verbs need no extension handlers; pins and "New" items do.
            let flags = if matches!(verb, "copy" | "cut" | "paste" | "delete" | "properties") { CMF_OPTIMIZEFORINVOKE } else { CMF_NORMAL };
            self.menu.QueryContextMenu(self.hmenu, 0, ID_FIRST, ID_LAST, flags).ok()?;
            // The Recycle Bin restores on a thread of its own otherwise, and one cut short (our process closing) leaves
            // the item's $I record behind in the bin.
            let sync = verb.eq_ignore_ascii_case("undelete");
            self.invoke(hwnd, PCSTR(narrow.as_ptr() as *const u8), PCWSTR(wide.as_ptr()), None, shift, sync)
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
                    if self.is_new_folder(id) {
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
        unsafe { self.invoke(hwnd, PCSTR(id as usize as *const u8), PCWSTR(id as usize as *const u16), at, shift, false) }
    }

    /// `sync` asks the handler to finish before returning (CMIC_MASK_NOASYNC).
    unsafe fn invoke(&self, hwnd: HWND, verb: PCSTR, verb_w: PCWSTR, at: Option<POINT>, shift: bool, sync: bool) -> Result<()> {
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
        // CMIC_MASK_NOASYNC is defined as SEE_MASK_NOASYNC in the SDK.
        if sync {
            info.fMask |= SEE_MASK_NOASYNC;
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

/// A Recycle Bin item: its parsing name is the deleted file's store path (`C:\$Recycle.Bin\<sid>\$R1234.txt`), a file
/// system path no app should open as the user's file. Any segment: `\\?\` paths, a share's bin and a volume mounted
/// in a folder put it deeper than a drive's root.
fn in_recycle_bin(path: &str) -> bool {
    path.split(['\\', '/']).any(|dir| dir.eq_ignore_ascii_case("$Recycle.Bin"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::ComInit;
    use crate::folder::list;

    #[test]
    fn recycle_bin_items_are_not_files_to_open() {
        assert!(in_recycle_bin(r"C:\$RECYCLE.BIN\S-1-5-21-1-2-3-1001\$RCEDAGN.txt"));
        assert!(in_recycle_bin(r"D:\$Recycle.Bin\S-1-5-21-1-2-3-1001\$R1.zip"));
        assert!(in_recycle_bin(r"\\?\C:\$Recycle.Bin\S-1-5-21-1-2-3-1001\$R1.txt"));
        assert!(in_recycle_bin(r"\\?\Volume{0b1c2d3e-0000-0000-0000-100000000000}\$Recycle.Bin\S-1-5-21-1-2-3-1001\$R1.txt"));
        assert!(in_recycle_bin(r"\\server\share\$RECYCLE.BIN\S-1-5-21-1-2-3-1001\$R1.txt"), "a share's bin");
        assert!(in_recycle_bin(r"C:\mnt\data\$Recycle.Bin\S-1-5-21-1-2-3-1001\$R1.txt"), "a volume mounted in a folder");
        assert!(!in_recycle_bin(r"C:\Users\u\$Recycle.Bin.txt"));
        assert!(!in_recycle_bin(r"C:\Users\u\a.txt"));
        assert!(!in_recycle_bin(r"\\server\share\a.txt"));
    }

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

    #[test]
    fn new_entries_are_told_from_the_rest() {
        let _c = ComInit::sta();
        let f = ShellFolder::parse(std::env::temp_dir().to_str().unwrap()).unwrap();
        let m = ContextMenu::for_background(&f, HWND::default()).unwrap();
        let items = m.new_items().unwrap();
        unsafe {
            assert!(!items.is_empty() && items.iter().all(|(id, _)| m.in_new_menu(m.hmenu, *id)), "{items:?}");
            // A top-level command (Paste, Properties...) is not a New entry.
            let top = (0..GetMenuItemCount(Some(m.hmenu))).map(|i| GetMenuItemID(m.hmenu, i)).find(|id| (ID_FIRST..=ID_LAST).contains(id));
            assert!(top.is_some_and(|id| !m.in_new_menu(m.hmenu, id)));
        }
    }

    #[test]
    fn several_items_get_one_menu_with_properties() {
        let _c = ComInit::sta();
        // One fixed folder, reused by every run: it cannot be removed here (see the end), so it must not pile up.
        let dir = std::env::temp_dir().join("rexplorer-multimenu");
        std::fs::create_dir_all(&dir).unwrap();
        for n in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(dir.join(n), n).unwrap();
        }
        let (f, items) = list(dir.to_str().unwrap()).unwrap();
        let all: Vec<&ShellItem> = items.iter().collect();
        let m = ContextMenu::for_items(&f, &all, HWND::default()).unwrap();
        let verbs: Vec<String> = unsafe {
            m.menu.QueryContextMenu(m.hmenu, 0, ID_FIRST, ID_LAST, CMF_NORMAL).ok().unwrap();
            (0..GetMenuItemCount(Some(m.hmenu))).map(|i| GetMenuItemID(m.hmenu, i)).filter(|id| (ID_FIRST..=ID_LAST).contains(id)).map(|id| m.verb_of(id - ID_FIRST)).collect()
        };
        // As Alt+Enter runs it: a fresh menu over the whole selection (the shell opens one sheet for all of them).
        let ran = ContextMenu::for_items(&f, &all, HWND::default()).unwrap().invoke_verb(HWND::default(), "properties", false);
        assert_eq!(all.len(), 3);
        assert!(verbs.iter().any(|v| v.eq_ignore_ascii_case("properties")), "{verbs:?}");
        assert!(ran.is_ok(), "{ran:?}");
        // Not removed: the sheet reads the files on its own thread after the verb returns.
    }

    #[test]
    fn hidden_verbs_leave_the_menu() {
        let _c = ComInit::sta();
        let dir = std::env::temp_dir().join(format!("rexplorer-hidden-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let (f, items) = list(dir.to_str().unwrap()).unwrap();
        let mut m = ContextMenu::for_items(&f, &[&items[0]], HWND::default()).unwrap();
        m.hide_verb("copy");
        let verbs: Vec<String> = unsafe {
            m.menu.QueryContextMenu(m.hmenu, 0, ID_FIRST, ID_LAST, CMF_NORMAL).ok().unwrap();
            m.drop_hidden(m.hmenu, 0);
            (0..GetMenuItemCount(Some(m.hmenu))).map(|i| GetMenuItemID(m.hmenu, i)).filter(|id| (ID_FIRST..=ID_LAST).contains(id)).map(|id| m.verb_of(id - ID_FIRST)).collect()
        };
        drop(m);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(!verbs.iter().any(|v| v.eq_ignore_ascii_case("copy")), "{verbs:?}");
        assert!(verbs.iter().any(|v| v.eq_ignore_ascii_case("cut")), "the rest stays: {verbs:?}");
    }

    #[test]
    fn items_of_two_folders_get_one_menu() {
        let _c = ComInit::sta();
        let dir = std::env::temp_dir().join(format!("rexplorer-scattered-{}", std::process::id()));
        for sub in ["x", "y"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
            std::fs::write(dir.join(sub).join("a.txt"), sub).unwrap();
        }
        let (fx, ix) = list(dir.join("x").to_str().unwrap()).unwrap();
        let (fy, iy) = list(dir.join("y").to_str().unwrap()).unwrap();
        let m = ContextMenu::for_scattered(&[(&fx.pidl, &ix[0]), (&fy.pidl, &iy[0])], HWND::default()).unwrap();
        let verbs: Vec<String> = unsafe {
            m.menu.QueryContextMenu(m.hmenu, 0, ID_FIRST, ID_LAST, CMF_NORMAL).ok().unwrap();
            (0..GetMenuItemCount(Some(m.hmenu))).map(|i| GetMenuItemID(m.hmenu, i)).filter(|id| (ID_FIRST..=ID_LAST).contains(id)).map(|id| m.verb_of(id - ID_FIRST)).collect()
        };
        // Properties goes to SHMultiFileProperties over both absolute PIDLs (the desktop menu's own does nothing).
        assert_eq!(m.scattered.len(), 2);
        assert!(!ContextMenu::for_items(&fx, &[&ix[0]], HWND::default()).unwrap().scattered_properties().unwrap());
        drop(m);
        std::fs::remove_dir_all(&dir).unwrap();
        for v in ["copy", "cut", "delete", "properties"] {
            assert!(verbs.iter().any(|x| x.eq_ignore_ascii_case(v)), "{v} missing: {verbs:?}");
        }
    }
}
