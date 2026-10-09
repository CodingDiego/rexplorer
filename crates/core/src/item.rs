use windows::Win32::System::SystemServices::SFGAO_FOLDER;
use windows::Win32::UI::Shell::Common::STRRET;
use windows::Win32::UI::Shell::{IShellFolder, SHGDN_FORPARSING, SHGDN_NORMAL, SHGDNF, StrRetToStrW};
use windows::core::{PWSTR, Result};

use crate::com::{Pidl, take_cotaskmem_string};
use crate::folder::ShellFolder;

/// One entry of a folder: its parent folder plus the child-relative PIDL.
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
            let mut pw = PWSTR::null();
            if StrRetToStrW(&mut ret, Some(self.pidl.as_ptr()), &mut pw).is_err() {
                return String::new();
            }
            take_cotaskmem_string(pw)
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

    /// Binds this item as a folder so it can be enumerated.
    pub fn open(&self) -> Result<ShellFolder> {
        let folder: IShellFolder = unsafe { self.parent.BindToObject(self.pidl.as_ptr(), None)? };
        Ok(ShellFolder { folder, name: self.display_name(), parsing_name: self.parsing_name() })
    }
}
