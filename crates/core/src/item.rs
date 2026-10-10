use windows::Win32::Foundation::HWND;
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Registry::{RRF_RT_ANY, RegCloseKey, RegGetValueW};
use windows::Win32::System::SystemServices::{SFGAO_FOLDER, SFGAO_HIDDEN};
use windows::Win32::System::Variant::{VariantToBoolean, VariantToStringAlloc, VariantToUInt64};
use windows::Win32::UI::Shell::Common::{SHELLDETAILS, STRRET};
use windows::Win32::UI::Shell::{
    ASSOCF_NONE, ASSOCKEY_CLASS, AssocQueryKeyW, ILCombine, IShellFolder, IShellFolder2, SHFILEINFOW, SHGDN_FORPARSING, SHGDN_INFOLDER,
    SHGDN_NORMAL, SHGDNF, SHGFI_PIDL, SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGetFileInfoW,
};
use windows::core::{GUID, HSTRING, Interface, PCWSTR, Result, w};

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

    /// Name inside its folder with the extension the display name may hide (`a.txt`).
    pub fn file_name(&self) -> String {
        self.name(SHGDNF(SHGDN_INFOLDER.0 | SHGDN_FORPARSING.0))
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

    /// Size in bytes from PKEY_Size (0 for folders); None when the folder has no such property.
    pub fn size(&self) -> Option<u64> {
        const PKEY_SIZE: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0xB725F130_47EF_101A_A5F1_02608C9EEBAC), pid: 12 };
        let f2 = self.parent.cast::<IShellFolder2>().ok()?;
        unsafe {
            let v = f2.GetDetailsEx(self.pidl.as_ptr(), &PKEY_SIZE).ok()?;
            VariantToUInt64(&v).ok()
        }
    }

    /// A property as the shell formats it for display (`GetDetailsEx` + `PropVariantToString`); None when the
    /// item has no value for it.
    pub fn property_text(&self, key: &PROPERTYKEY) -> Option<String> {
        let f2 = self.parent.cast::<IShellFolder2>().ok()?;
        unsafe {
            let v = f2.GetDetailsEx(self.pidl.as_ptr(), key).ok()?;
            let p = VariantToStringAlloc(&v).ok()?;
            let s = crate::com::take_cotaskmem_string(p);
            (!s.is_empty()).then_some(s)
        }
    }

    /// A boolean property (`GetDetailsEx` + `VariantToBoolean`); None when the item has no value for it.
    pub fn property_bool(&self, key: &PROPERTYKEY) -> Option<bool> {
        let f2 = self.parent.cast::<IShellFolder2>().ok()?;
        unsafe {
            let v = f2.GetDetailsEx(self.pidl.as_ptr(), key).ok()?;
            VariantToBoolean(&v).ok().map(|b| b.as_bool())
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

    /// Index into the system small image list, the opening app's icon in place of the shell's blank page.
    /// Returns the image list handle too (same for every call).
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
            // The parsing name keeps the extension that "hide extensions" strips from the display name.
            (himl, crate::icons::app_fallback(info.iIcon, || self.name(SHGDNF(SHGDN_INFOLDER.0 | SHGDN_FORPARSING.0))))
        }
    }

    /// Renames the item in place; the shell shows its own error UI on failure. `literal` takes `new_name` as the
    /// whole file name; otherwise it is a display name the shell puts a hidden extension back on.
    pub fn rename(&mut self, hwnd: HWND, new_name: &str, literal: bool) -> Result<()> {
        let wide = HSTRING::from(new_name);
        let mut out = std::ptr::null_mut();
        unsafe {
            self.parent.SetNameOf(
                Some(hwnd),
                self.pidl.as_ptr(),
                PCWSTR(wide.as_ptr()),
                if literal { SHGDNF(SHGDN_INFOLDER.0 | SHGDN_FORPARSING.0) } else { SHGDN_INFOLDER },
                Some(&mut out),
            )?;
            if !out.is_null() {
                self.pidl = Pidl::from_raw(out);
            }
        }
        Ok(())
    }
}

/// The file type of `ext` (".lnk") is marked NeverShowExt: Explorer hides that extension even with extensions shown.
pub fn never_shows_ext(ext: &str) -> bool {
    unsafe {
        let Ok(key) = AssocQueryKeyW(ASSOCF_NONE, ASSOCKEY_CLASS, &HSTRING::from(ext), PCWSTR::null()) else { return false };
        let found = RegGetValueW(key, PCWSTR::null(), w!("NeverShowExt"), RRF_RT_ANY, None, None, None).is_ok();
        let _ = RegCloseKey(key);
        found
    }
}
