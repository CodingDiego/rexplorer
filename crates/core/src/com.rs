use std::ptr;

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{ILClone, ILFree};
use windows::core::PWSTR;

/// Initializes COM (STA) for the current thread and uninitializes it on drop.
pub struct ComInit;

impl ComInit {
    pub fn sta() -> Self {
        // S_FALSE (already initialized) is fine; a real failure here is unrecoverable.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().expect("CoInitializeEx") };
        ComInit
    }
}

impl Drop for ComInit {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

/// Copies a shell-allocated wide string into a `String` and frees the original.
pub unsafe fn take_cotaskmem_string(pw: PWSTR) -> String {
    if pw.is_null() {
        return String::new();
    }
    let s = unsafe { pw.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(pw.as_ptr() as *const _)) };
    s
}

/// Owned item ID list; freed with ILFree on drop.
pub struct Pidl(*mut ITEMIDLIST);

impl Pidl {
    /// Takes ownership of a PIDL allocated by the shell.
    pub unsafe fn from_raw(p: *mut ITEMIDLIST) -> Self {
        debug_assert!(!p.is_null());
        Pidl(p)
    }

    pub fn as_ptr(&self) -> *const ITEMIDLIST {
        self.0
    }
}

impl Clone for Pidl {
    fn clone(&self) -> Self {
        let p = unsafe { ILClone(self.0) };
        assert!(!p.is_null(), "ILClone out of memory");
        Pidl(p)
    }
}

impl Drop for Pidl {
    fn drop(&mut self) {
        unsafe { ILFree(Some(self.0)) };
        self.0 = ptr::null_mut();
    }
}

// PIDLs are plain memory; the shell folder objects we pair them with are free-threaded in practice.
unsafe impl Send for Pidl {}
unsafe impl Sync for Pidl {}
