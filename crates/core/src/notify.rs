use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::UI::Shell::{
    SHCNE_ALLEVENTS, SHCNRF_InterruptLevel, SHCNRF_NewDelivery, SHCNRF_SOURCE, SHCNRF_ShellLevel,
    SHChangeNotification_Lock, SHChangeNotification_Unlock, SHChangeNotifyDeregister,
    SHChangeNotifyEntry, SHChangeNotifyRegister,
};

use crate::folder::ShellFolder;

/// Delivers shell change notifications for one folder to a window as message `msg`.
/// Works for virtual folders too, unlike `ReadDirectoryChangesW`.
pub struct ChangeWatch(u32);

impl ChangeWatch {
    pub fn register(hwnd: HWND, folder: &ShellFolder, msg: u32) -> Option<Self> {
        let entry = SHChangeNotifyEntry { pidl: folder.pidl.as_ptr() as *mut _, fRecursive: false.into() };
        let sources = SHCNRF_SOURCE(SHCNRF_ShellLevel.0 | SHCNRF_InterruptLevel.0 | SHCNRF_NewDelivery.0);
        let id = unsafe { SHChangeNotifyRegister(hwnd, sources, SHCNE_ALLEVENTS.0 as i32, msg, 1, &entry) };
        (id != 0).then_some(ChangeWatch(id))
    }

    /// Call from the message handler: releases the notification payload we do not inspect.
    pub fn acknowledge(wparam: WPARAM, lparam: LPARAM) {
        unsafe {
            let lock = SHChangeNotification_Lock(HANDLE(wparam.0 as *mut _), lparam.0 as u32, None, None);
            if !lock.is_invalid() {
                let _ = SHChangeNotification_Unlock(lock);
            }
        }
    }
}

impl Drop for ChangeWatch {
    fn drop(&mut self) {
        let _ = unsafe { SHChangeNotifyDeregister(self.0) };
    }
}
