use std::ptr;

use windows::Win32::System::Com::{
    COINIT, COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};
use windows::Win32::System::Ole::{OleInitialize, OleUninitialize};
use windows::Win32::UI::Shell::Common::{ITEMIDLIST, STRRET};
use windows::Win32::UI::Shell::{ILClone, ILFree, StrRetToStrW};
use windows::core::PWSTR;

/// Initializes COM for the current thread and uninitializes it on drop.
pub struct ComInit {
    ole: bool,
}

impl ComInit {
    /// Single-threaded apartment: the UI thread.
    pub fn sta() -> Self {
        Self::init(COINIT_APARTMENTTHREADED)
    }

    /// Multi-threaded apartment: worker threads.
    pub fn mta() -> Self {
        Self::init(COINIT_MULTITHREADED)
    }

    /// STA plus OLE (drag and drop, clipboard); needed before RegisterDragDrop or DoDragDrop.
    pub fn ole() -> Self {
        unsafe { OleInitialize(None).expect("OleInitialize") };
        ComInit { ole: true }
    }

    fn init(kind: COINIT) -> Self {
        // S_FALSE (already initialized) is fine; a real failure here is unrecoverable.
        unsafe { CoInitializeEx(None, kind).ok().expect("CoInitializeEx") };
        ComInit { ole: false }
    }
}

impl Drop for ComInit {
    fn drop(&mut self) {
        unsafe { if self.ole { OleUninitialize() } else { CoUninitialize() } };
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

/// Converts a `STRRET` produced for `pidl` into a `String`.
pub unsafe fn strret_to_string(ret: &mut STRRET, pidl: *const ITEMIDLIST) -> String {
    let mut pw = PWSTR::null();
    if unsafe { StrRetToStrW(ret, Some(pidl), &mut pw) }.is_err() {
        return String::new();
    }
    unsafe { take_cotaskmem_string(pw) }
}

/// Owned item ID list; freed with ILFree on drop.
pub struct Pidl(*mut ITEMIDLIST);

impl Pidl {
    /// Takes ownership of a PIDL allocated by the shell.
    pub unsafe fn from_raw(p: *mut ITEMIDLIST) -> Self {
        assert!(!p.is_null(), "null PIDL");
        Pidl(p)
    }

    pub fn as_ptr(&self) -> *const ITEMIDLIST {
        self.0
    }

    pub fn as_mut_ptr(&mut self) -> *mut ITEMIDLIST {
        self.0
    }

    /// The empty PIDL that denotes the desktop root.
    pub fn is_empty(&self) -> bool {
        unsafe { (*self.0).mkid.cb == 0 }
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
