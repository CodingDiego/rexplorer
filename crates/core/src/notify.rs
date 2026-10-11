use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::UI::Shell::{
    ILClone, ILIsEqual, ILIsParent, SHCNE_ALLEVENTS, SHCNE_ASSOCCHANGED, SHCNE_EXTENDED_EVENT, SHCNE_FREESPACE,
    SHCNE_UPDATEIMAGE, SHCNRF_InterruptLevel, SHCNRF_NewDelivery, SHCNRF_SOURCE, SHCNRF_ShellLevel,
    SHChangeNotification_Lock, SHChangeNotification_Unlock, SHChangeNotifyDeregister,
    SHChangeNotifyEntry, SHChangeNotifyRegister,
};

use crate::com::Pidl;
use crate::folder::ShellFolder;

/// Delivers shell change notifications for one folder to a window as message `msg`.
/// Works for virtual folders too, unlike `ReadDirectoryChangesW`.
pub struct ChangeWatch {
    id: u32,
    folder: Pidl,
}

impl ChangeWatch {
    pub fn register(hwnd: HWND, folder: &ShellFolder, msg: u32) -> Option<Self> {
        let entry = SHChangeNotifyEntry { pidl: folder.pidl.as_ptr() as *mut _, fRecursive: false.into() };
        let sources = SHCNRF_SOURCE(SHCNRF_ShellLevel.0 | SHCNRF_InterruptLevel.0 | SHCNRF_NewDelivery.0);
        let id = unsafe { SHChangeNotifyRegister(hwnd, sources, SHCNE_ALLEVENTS.0 as i32, msg, 1, &entry) };
        (id != 0).then(|| ChangeWatch { id, folder: folder.pidl.clone() })
    }

    /// Whether this watch is on `folder`.
    pub fn is_on(&self, folder: &Pidl) -> bool {
        unsafe { ILIsEqual(self.folder.as_ptr(), folder.as_ptr()) }.as_bool()
    }

    /// Whether a notification about `changed` is about this folder: the folder itself or an item directly in it.
    pub fn concerns(&self, changed: &Pidl) -> bool {
        self.is_on(changed) || unsafe { ILIsParent(self.folder.as_ptr(), changed.as_ptr(), true) }.as_bool()
    }

    /// Call from the message handler: releases the notification payload and returns copies of the items it names
    /// (empty for events that name none).
    pub fn acknowledge(wparam: WPARAM, lparam: LPARAM) -> Vec<Pidl> {
        let mut items = Vec::new();
        unsafe {
            let (mut pidls, mut event) = (std::ptr::null_mut(), 0);
            let lock = SHChangeNotification_Lock(HANDLE(wparam.0 as *mut _), lparam.0 as u32, Some(&mut pidls), Some(&mut event));
            if !lock.is_invalid() {
                // These carry numbers, not PIDLs.
                let numeric = SHCNE_ASSOCCHANGED.0 | SHCNE_EXTENDED_EVENT.0 | SHCNE_FREESPACE.0 | SHCNE_UPDATEIMAGE.0;
                if !pidls.is_null() && event as u32 & numeric == 0 {
                    for i in 0..2 {
                        let p = *pidls.add(i);
                        if !p.is_null() {
                            let copy = ILClone(p);
                            if !copy.is_null() {
                                items.push(Pidl::from_raw(copy));
                            }
                        }
                    }
                }
                let _ = SHChangeNotification_Unlock(lock);
            }
        }
        items
    }
}

impl Drop for ChangeWatch {
    fn drop(&mut self) {
        let _ = unsafe { SHChangeNotifyDeregister(self.id) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::ComInit;
    use windows::Win32::UI::Shell::SHParseDisplayName;
    use windows::core::HSTRING;

    fn pidl(path: &std::path::Path) -> Pidl {
        let mut p = std::ptr::null_mut();
        unsafe { SHParseDisplayName(&HSTRING::from(path.as_os_str()), None, &mut p, 0, None) }.unwrap();
        unsafe { Pidl::from_raw(p) }
    }

    #[test]
    fn a_watch_concerns_its_folder_and_the_items_directly_in_it() {
        let _com = ComInit::sta();
        let dir = std::env::temp_dir().join(format!("rexplorer-notify-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        std::fs::write(dir.join("sub").join("b.txt"), b"x").unwrap();
        let w = ChangeWatch { id: 0, folder: pidl(&dir) };
        assert!(w.is_on(&pidl(&dir)) && !w.is_on(&pidl(&dir.join("sub"))));
        assert!(w.concerns(&pidl(&dir)) && w.concerns(&pidl(&dir.join("a.txt"))) && w.concerns(&pidl(&dir.join("sub"))));
        assert!(!w.concerns(&pidl(&dir.join("sub").join("b.txt"))), "grandchildren are another folder's");
        assert!(!w.concerns(&pidl(dir.parent().unwrap())));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
