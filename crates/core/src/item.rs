use std::os::windows::ffi::OsStrExt;

use windows::Win32::Foundation::{ERROR_INVALID_NAME, HWND};
use windows::Win32::Storage::FileSystem::{FILE_FLAGS_AND_ATTRIBUTES, MOVE_FILE_FLAGS, MoveFileExW};
use windows::Win32::Foundation::{FILETIME, PROPERTYKEY};
use windows::Win32::System::Registry::{RRF_RT_ANY, RegCloseKey, RegGetValueW};
use windows::Win32::System::SystemServices::{SFGAO_FOLDER, SFGAO_HIDDEN};
use windows::Win32::System::Variant::{PSTF_UTC, VariantToBoolean, VariantToFileTime, VariantToStringAlloc, VariantToUInt32, VariantToUInt64};
use windows::Win32::UI::Shell::Common::{SHELLDETAILS, STRRET};
use windows::Win32::UI::Shell::{
    ASSOCF_NONE, ASSOCKEY_CLASS, AssocQueryKeyW, ILCombine, IShellFolder, IShellFolder2, IShellIconOverlay, SHFILEINFOW, SHGDN_FORPARSING, SHGDN_INFOLDER,
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

/// System image list index and the shell's overlay (0 none, 1-15) from one packed value; bit 31 stays clear, so ICON_UNKNOWN (i32::MIN) never collides.
pub fn split_icon(packed: i32) -> (i32, u32) {
    (packed & 0x00FF_FFFF, (packed as u32 >> 24) & 0xF)
}

/// A cloud file's sync state from the attributes the cloud files API sets (OneDrive Files On-Demand): 0 none or
/// unknown, `CLOUD_ONLINE` online-only (a placeholder that downloads when opened), `CLOUD_PINNED` "Always keep on
/// this device". A downloaded but unpinned file has neither bit, like a plain local file: no mark.
pub fn cloud_state(attrs: u32) -> u32 {
    const OFFLINE: u32 = 0x1000;
    const RECALL_ON_OPEN: u32 = 0x0004_0000;
    const PINNED: u32 = 0x0008_0000;
    const RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
    if attrs & PINNED != 0 {
        CLOUD_PINNED
    } else if attrs & (RECALL_ON_DATA_ACCESS | RECALL_ON_OPEN | OFFLINE) != 0 {
        CLOUD_ONLINE
    } else {
        0
    }
}

pub const CLOUD_ONLINE: u32 = 1;
pub const CLOUD_PINNED: u32 = 2;

/// The cloud state `icon_index` packs in bits 28-29 when the shell gave no overlay of its own.
pub fn cloud_of(packed: i32) -> u32 {
    (packed as u32 >> 28) & 3
}

/// The shell's overlay slot (1-15; 0 none) for `child` of `parent`: shortcut arrow, sync state, shared.
// shortcut: asks the overlay handlers synchronously on the calling thread; OI_ASYNC (cache only, E_PENDING on a miss) if a big OneDrive folder stutters.
#[allow(clippy::not_unsafe_ptr_arg_deref)] // child comes from a live PIDL the caller owns; an unsafe fn would only move the unsafe block to every caller
pub fn overlay_slot(parent: &IShellFolder, child: *const windows::Win32::UI::Shell::Common::ITEMIDLIST) -> u32 {
    let Ok(o) = parent.cast::<IShellIconOverlay>() else { return 0 };
    let mut i = 0i32;
    match unsafe { o.GetOverlayIndex(child, &mut i) } {
        Ok(()) if (1..=15).contains(&i) => i as u32,
        _ => 0,
    }
}

/// A shortcut to `target` at `lnk` (tests only).
#[cfg(test)]
pub(crate) fn make_shortcut(target: &std::path::Path, lnk: &std::path::Path) {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
        link.SetPath(&HSTRING::from(target.as_os_str())).unwrap();
        link.cast::<IPersistFile>().unwrap().Save(&HSTRING::from(lnk.as_os_str()), true).unwrap();
    }
}

impl ShellItem {
    pub fn display_name(&self) -> String {
        self.name(SHGDN_NORMAL)
    }

    /// The name its folder lists it by, as Explorer's Name column shows it (a Recycle Bin item's display name is its
    /// original path; this is its file name).
    pub fn in_folder_name(&self) -> String {
        self.name(SHGDN_INFOLDER)
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

    /// A date property in UTC (`GetDetailsEx` + `VariantToFileTime`); None when the item has no value for it.
    pub fn property_time(&self, key: &PROPERTYKEY) -> Option<FILETIME> {
        let f2 = self.parent.cast::<IShellFolder2>().ok()?;
        unsafe {
            let v = f2.GetDetailsEx(self.pidl.as_ptr(), key).ok()?;
            VariantToFileTime(&v, PSTF_UTC).ok()
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

    /// Index into the system small image list, the opening app's icon in place of the shell's blank page, with the
    /// shell's overlay (shortcut arrow, sync state) packed in bits 24-27 (see `split_icon`).
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
            let icon = crate::icons::app_fallback(info.iIcon, || self.name(SHGDNF(SHGDN_INFOLDER.0 | SHGDN_FORPARSING.0)));
            let overlay = overlay_slot(&self.parent, self.pidl.as_ptr());
            // No sync engine overlay (OneDrive not running, or its handlers out of the 15 slots): our own mark.
            let cloud = if overlay == 0 { self.file_attributes().map_or(0, cloud_state) } else { 0 };
            (himl, icon | (overlay << 24) as i32 | (cloud << 28) as i32)
        }
    }

    /// FILE_ATTRIBUTE_* from PKEY_FileAttributes (the folder's cached data, no disk read for file system items).
    pub fn file_attributes(&self) -> Option<u32> {
        const PKEY_FILE_ATTRIBUTES: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0xB725F130_47EF_101A_A5F1_02608C9EEBAC), pid: 13 };
        let f2 = self.parent.cast::<IShellFolder2>().ok()?;
        unsafe {
            let v = f2.GetDetailsEx(self.pidl.as_ptr(), &PKEY_FILE_ATTRIBUTES).ok()?;
            VariantToUInt32(&v).ok()
        }
    }

    /// Renames the item in place; the shell shows its own error UI on failure. `literal` takes `new_name` as the
    /// whole file name; otherwise it is a display name the shell puts a hidden extension back on.
    pub fn rename(&mut self, hwnd: HWND, new_name: &str, literal: bool) -> Result<()> {
        let path = self.parsing_name();
        // shell32's SetNameOf stays MAX_PATH-bound (ERROR_FILENAME_EXCED_RANGE) even in a longPathAware process.
        let long = |p: &std::path::Path| p.as_os_str().encode_wide().count() >= 260;
        let old = std::path::Path::new(&path);
        if old.is_absolute() && (long(old) || long(&old.with_file_name(new_name))) {
            return self.rename_long(&path, new_name, literal);
        }
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

    /// Renames a file system item whose path is over MAX_PATH, never replacing an existing name.
    // shortcut: no conflict or error dialog here (an existing name just fails); add one if long paths get common.
    fn rename_long(&mut self, path: &str, new_name: &str, literal: bool) -> Result<()> {
        // A \\?\ path keeps trailing dots and spaces literally; Explorer drops them (and so "." and ".." go empty).
        let new_name = new_name.trim_end_matches(['.', ' ']);
        if new_name.is_empty()
            || crate::archive::is_device_name(new_name)
            || new_name.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|'])
        {
            return Err(ERROR_INVALID_NAME.to_hresult().into());
        }
        let old = std::path::Path::new(path);
        let mut name = new_name.to_string();
        // A display name: the shell keeps the extension it hides.
        if !literal
            && self.display_name() != self.file_name()
            && let Some(ext) = old.extension()
        {
            name = format!("{name}.{}", ext.to_string_lossy());
        }
        let to = old.with_file_name(&name);
        unsafe {
            MoveFileExW(&HSTRING::from(verbatim(old)), &HSTRING::from(verbatim(&to)), MOVE_FILE_FLAGS(0))?;
            let mut child = std::ptr::null_mut();
            self.parent.ParseDisplayName(HWND::default(), None, &HSTRING::from(name), None, &mut child, std::ptr::null_mut())?;
            self.pidl = Pidl::from_raw(child);
        }
        Ok(())
    }
}

/// `\\?\C:\x` or `\\?\UNC\server\share\x`: the form Win32 file calls take past MAX_PATH.
fn verbatim(p: &std::path::Path) -> String {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\") {
        Some(_) if s.starts_with(r"\\?\") => s.into_owned(),
        Some(unc) => format!(r"\\?\UNC\{unc}"),
        None => format!(r"\\?\{s}"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::ComInit;
    use crate::folder::ShellFolder;

    #[test]
    fn verbatim_paths_for_drives_and_shares() {
        use std::path::Path;
        assert_eq!(verbatim(Path::new(r"C:\a\b.txt")), r"\\?\C:\a\b.txt");
        assert_eq!(verbatim(Path::new(r"\\srv\share\b.txt")), r"\\?\UNC\srv\share\b.txt");
        assert_eq!(verbatim(Path::new(r"\\?\C:\a")), r"\\?\C:\a");
    }

    #[test]
    fn split_icon_round_trip() {
        assert_eq!(split_icon(5 | (2 << 24)), (5, 2));
        assert_eq!(split_icon(0x00FF_FFFF), (0xFF_FFFF, 0));
        assert!((15u32 << 24) as i32 > 0);
    }

    #[test]
    fn cloud_states_from_attributes_and_their_packing() {
        // FILE_ATTRIBUTE_ARCHIVE alone, a pinned file, online-only placeholders (data access, open, offline), and
        // pinned winning over a recall bit while it downloads.
        assert_eq!(cloud_state(0x20), 0);
        assert_eq!(cloud_state(0x20 | 0x0008_0000), CLOUD_PINNED);
        assert_eq!(cloud_state(0x20 | 0x0040_0000), CLOUD_ONLINE);
        assert_eq!(cloud_state(0x10 | 0x0004_0000), CLOUD_ONLINE);
        assert_eq!(cloud_state(0x1000), CLOUD_ONLINE);
        assert_eq!(cloud_state(0x0008_0000 | 0x0040_0000), CLOUD_PINNED);
        // An unpinned (being freed) file with its data still local: no mark.
        assert_eq!(cloud_state(0x0010_0000), 0);
        // Bits 28-29 carry it next to the index and overlay; bit 31 stays clear.
        let packed = 0x00AB_CDEF | (3 << 24) | (CLOUD_PINNED << 28) as i32;
        assert_eq!((split_icon(packed), cloud_of(packed)), ((0xAB_CDEF, 3), CLOUD_PINNED));
        assert!(packed > 0 && cloud_of(5) == 0);
    }

    #[test]
    fn shortcut_has_an_overlay_and_a_text_file_not() {
        let _c = ComInit::sta();
        let dir = std::env::temp_dir().join(format!("rexplorer-overlay-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("t.txt"), b"x").unwrap();
        make_shortcut(&dir.join("t.txt"), &dir.join("t.lnk"));
        let f = ShellFolder::parse(dir.to_str().unwrap()).unwrap();
        let items = f.enumerate().unwrap();
        let overlay = |name: &str| items.iter().find(|i| i.file_name() == name).map(|i| split_icon(i.icon_index(&f.pidl).1).1);
        let (lnk, txt) = (overlay("t.lnk"), overlay("t.txt"));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(lnk.unwrap() > 0, "the shortcut arrow overlay (absent if the Shell Icons 29 override removed it)");
        assert_eq!(txt, Some(0));
    }
}
