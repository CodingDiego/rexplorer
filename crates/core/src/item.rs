use windows::Win32::Foundation::HWND;
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::System::SystemServices::{SFGAO_FOLDER, SFGAO_HIDDEN};
use windows::Win32::UI::Shell::Common::{SHELLDETAILS, STRRET};
use windows::Win32::UI::Shell::{
    ILCombine, IShellFolder, IShellFolder2, SHFILEINFOW, SHGDN_FORPARSING, SHGDN_INFOLDER,
    SHGDN_NORMAL, SHGDNF, SHGFI_PIDL, SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGetFileInfoW,
};
use windows::core::{HSTRING, Interface, PCWSTR, Result};

use crate::com::{Pidl, strret_to_string};

/// One entry of a folder: its parent folder plus the child-relative PIDL.
#[derive(Clone)]
pub struct ShellItem {
    pub parent: IShellFolder,
    pub pidl: Pidl,
}

impl ShellItem {
    pub fn display_name(&self) -> String {
        self.name(SHGDN_NORMAL)
    }

    /// Full parsing name, e.g. `C:\Users` or `::{20D04FE0-...}`.
    pub fn parsing_name(&self) -> String {
        self.name(SHGDN_FORPARSING)
    }

    fn name(&self, flags: SHGDNF) -> String {
        unsafe {
            let mut ret = STRRET::default();
            if self.parent.GetDisplayNameOf(self.pidl.as_ptr(), flags, &mut ret).is_err() {
                return String::new();
            }
            strret_to_string(&mut ret, self.pidl.as_ptr())
        }
    }

    /// Text of one of the folder's detail columns, formatted by the shell ("12 KB", "Text Document").
    pub fn detail(&self, column: u32) -> String {
        let Ok(f2) = self.parent.cast::<IShellFolder2>() else { return String::new() };
        unsafe {
            let mut sd = SHELLDETAILS::default();
            if f2.GetDetailsOf(self.pidl.as_ptr(), column, &mut sd).is_err() {
                return String::new();
            }
            // SHELLDETAILS is packed: copy the STRRET out before taking a reference.
            let mut ret = { sd.str };
            strret_to_string(&mut ret, self.pidl.as_ptr())
        }
    }

    /// Attributes masked by `mask` (SFGAO_* flags).
    pub fn attributes(&self, mask: u32) -> u32 {
        let mut flags = mask;
        unsafe {
            if self.parent.GetAttributesOf(&[self.pidl.as_ptr()], &mut flags).is_err() {
                return 0;
            }
        }
        flags
    }

    pub fn is_folder(&self) -> bool {
        self.attributes(SFGAO_FOLDER.0) & SFGAO_FOLDER.0 != 0
    }

    pub fn is_hidden(&self) -> bool {
        self.attributes(SFGAO_HIDDEN.0) & SFGAO_HIDDEN.0 != 0
    }

    /// Index into the system small image list. Returns the image list handle too (same for every call).
    pub fn icon_index(&self, folder_pidl: &Pidl) -> (usize, i32) {
        unsafe {
            let abs = Pidl::from_raw(ILCombine(Some(folder_pidl.as_ptr()), Some(self.pidl.as_ptr())));
            let mut info = SHFILEINFOW::default();
            let himl = SHGetFileInfoW(
                PCWSTR(abs.as_ptr() as *const u16),
                FILE_FLAGS_AND_ATTRIBUTES(0),
                Some(&mut info),
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_PIDL | SHGFI_SYSICONINDEX | SHGFI_SMALLICON,
            );
            (himl, info.iIcon)
        }
    }

    /// Renames the item in place; the shell shows its own error UI on failure.
    pub fn rename(&mut self, hwnd: HWND, new_name: &str) -> Result<()> {
        let wide = HSTRING::from(new_name);
        let mut out = std::ptr::null_mut();
        unsafe {
            self.parent.SetNameOf(
                Some(hwnd),
                self.pidl.as_ptr(),
                PCWSTR(wide.as_ptr()),
                SHGDN_INFOLDER,
                Some(&mut out),
            )?;
            if !out.is_null() {
                self.pidl = Pidl::from_raw(out);
            }
        }
        Ok(())
    }
}
