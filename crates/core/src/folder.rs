use std::ptr;

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::Shell::Common::{ITEMIDLIST, SHCOLSTATE_ONBYDEFAULT, SHELLDETAILS};
use windows::Win32::UI::Shell::{
    BHID_SFObject, IEnumIDList, ILCombine, ILRemoveLastID, IShellFolder, IShellFolder2,
    IShellItem, SHCONTF_FOLDERS, SHCONTF_INCLUDEHIDDEN, SHCONTF_NONFOLDERS,
    SHCreateItemFromIDList, SHCreateItemFromParsingName, SHGetIDListFromObject,
    SIGDN_DESKTOPABSOLUTEEDITING, SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_NORMALDISPLAY,
};
use windows::core::{HSTRING, Interface, PCWSTR, Result};

use crate::com::{Pidl, strret_to_string, take_cotaskmem_string};
use crate::item::ShellItem;

/// An opened shell folder (filesystem or virtual).
pub struct ShellFolder {
    pub folder: IShellFolder,
    /// Absolute PIDL from the desktop root.
    pub pidl: Pidl,
    pub name: String,
    /// What `explorer.exe` and `SHCreateItemFromParsingName` accept.
    pub parsing_name: String,
    /// What the address bar shows ("This PC", "C:\Users").
    pub editing_name: String,
}

/// One detail column the folder shows by default.
pub struct Column {
    /// The shell's column index; gaps are normal because off-by-default columns are skipped.
    pub index: u32,
    pub title: String,
    pub width_chars: i32,
    pub right_align: bool,
}

impl ShellFolder {
    /// Opens a folder by parsing name: a path, a `shell:` name, or a `::{CLSID}` string.
    pub fn parse(name: &str) -> Result<ShellFolder> {
        let wide = HSTRING::from(name);
        let item: IShellItem = unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None)? };
        Self::from_item(item)
    }

    pub fn from_pidl(pidl: &Pidl) -> Result<ShellFolder> {
        let item: IShellItem = unsafe { SHCreateItemFromIDList(pidl.as_ptr())? };
        Self::from_item(item)
    }

    fn from_item(item: IShellItem) -> Result<ShellFolder> {
        unsafe {
            let folder: IShellFolder = item.BindToHandler(None, &BHID_SFObject)?;
            let pidl = Pidl::from_raw(SHGetIDListFromObject(&item)?);
            let name = take_cotaskmem_string(item.GetDisplayName(SIGDN_NORMALDISPLAY)?);
            let parsing_name = take_cotaskmem_string(item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING)?);
            let editing_name = take_cotaskmem_string(item.GetDisplayName(SIGDN_DESKTOPABSOLUTEEDITING)?);
            Ok(ShellFolder { folder, pidl, name, parsing_name, editing_name })
        }
    }

    /// Absolute PIDL of a child; cheap, no binding happens.
    pub fn child_pidl(&self, item: &ShellItem) -> Pidl {
        unsafe { Pidl::from_raw(ILCombine(Some(self.pidl.as_ptr()), Some(item.pidl.as_ptr()))) }
    }

    /// Absolute PIDL of the containing folder, or `None` at the desktop root.
    pub fn parent_pidl(&self) -> Option<Pidl> {
        if self.pidl.is_empty() {
            return None;
        }
        let mut p = self.pidl.clone();
        unsafe { ILRemoveLastID(Some(p.as_mut_ptr())) }.as_bool().then_some(p)
    }

    /// Opens a child of this folder.
    pub fn child(&self, item: &ShellItem) -> Result<ShellFolder> {
        Self::from_pidl(&self.child_pidl(item))
    }

    /// The containing folder, or `None` at the desktop root.
    pub fn parent(&self) -> Option<ShellFolder> {
        Self::from_pidl(&self.parent_pidl()?).ok()
    }

    /// Lists folders and files, hidden ones included (the UI decides what to show).
    pub fn enumerate(&self) -> Result<Vec<ShellItem>> {
        let mut e: Option<IEnumIDList> = None;
        let flags = (SHCONTF_FOLDERS.0 | SHCONTF_NONFOLDERS.0 | SHCONTF_INCLUDEHIDDEN.0) as u32;
        unsafe { self.folder.EnumObjects(HWND::default(), flags, &mut e).ok()? };
        // S_FALSE with no enumerator means an empty folder.
        let Some(e) = e else { return Ok(Vec::new()) };

        let mut out = Vec::new();
        let mut batch: [*mut ITEMIDLIST; 256] = [ptr::null_mut(); 256];
        loop {
            let mut fetched = 0u32;
            let hr = unsafe { e.Next(&mut batch, Some(&mut fetched)) };
            if hr.is_err() || fetched == 0 {
                break;
            }
            for p in &batch[..fetched as usize] {
                out.push(ShellItem { parent: self.folder.clone(), pidl: unsafe { Pidl::from_raw(*p) } });
            }
        }
        Ok(out)
    }

    /// The columns this folder shows by default (Name, Date modified, Type, Size for filesystem folders).
    pub fn columns(&self) -> Vec<Column> {
        let mut cols = Vec::new();
        let Ok(f2) = self.folder.cast::<IShellFolder2>() else { return cols };
        for index in 0..64 {
            let mut sd = SHELLDETAILS::default();
            if unsafe { f2.GetDetailsOf(ptr::null(), index, &mut sd) }.is_err() {
                break;
            }
            let on = unsafe { f2.GetDefaultColumnState(index) }
                .map(|s| s.0 & SHCOLSTATE_ONBYDEFAULT.0 != 0)
                .unwrap_or(index == 0);
            if !on && index != 0 {
                continue;
            }
            // SHELLDETAILS is packed: copy the STRRET out before taking a reference.
            let mut ret = { sd.str };
            let title = unsafe { strret_to_string(&mut ret, ptr::null()) };
            // fmt 1 is LVCFMT_RIGHT (sizes, counts).
            cols.push(Column { index, title, width_chars: sd.cxChar, right_align: sd.fmt == 1 });
        }
        cols
    }

    /// Shell-correct ordering by a column (folders first, locale-aware names, real dates and sizes).
    pub fn compare(&self, column: u32, a: &ShellItem, b: &ShellItem) -> std::cmp::Ordering {
        let hr = unsafe { self.folder.CompareIDs(LPARAM(column as isize), a.pidl.as_ptr(), b.pidl.as_ptr()) };
        // The low 16 bits of the HRESULT hold the signed comparison result.
        ((hr.0 & 0xFFFF) as i16).cmp(&0)
    }
}

/// Convenience for the UI: open then list.
pub fn list(name: &str) -> Result<(ShellFolder, Vec<ShellItem>)> {
    let f = ShellFolder::parse(name)?;
    let items = f.enumerate()?;
    Ok((f, items))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::ComInit;

    #[test]
    fn parses_windows_dir() {
        let _c = ComInit::sta();
        let f = ShellFolder::parse(r"C:\Windows").unwrap();
        assert_eq!(f.name, "Windows");
        assert_eq!(f.parsing_name, r"C:\Windows");
        assert_eq!(f.editing_name, r"C:\Windows");
        assert_eq!(f.parent().unwrap().parsing_name, r"C:\");
    }

    #[test]
    fn enumerates_temp_dir_contents() {
        let _c = ComInit::sta();
        let dir = std::env::temp_dir().join(format!("rexplorer-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for n in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let (f, mut items) = list(dir.to_str().unwrap()).unwrap();
        items.sort_by(|a, b| f.compare(0, a, b));
        let names: Vec<(String, bool)> =
            items.iter().map(|i| (i.display_name(), i.is_folder())).collect();
        // Column titles are localized; filesystem folders always expose Name, Size, Type, Date modified.
        let cols = f.columns();
        assert!(cols.len() >= 4, "{} columns", cols.len());
        let size_of_a = items.iter().find(|i| i.display_name() == "a.txt").unwrap().detail(cols[1].index);
        std::fs::remove_dir_all(&dir).unwrap();
        // Folders sort first.
        assert_eq!(
            names,
            vec![
                ("sub".into(), true),
                ("a.txt".into(), false),
                ("b.txt".into(), false),
                ("c.txt".into(), false)
            ]
        );
        assert!(size_of_a.starts_with("1 "), "size text: {size_of_a:?}");
    }

    #[test]
    fn virtual_folders_open_and_enumerate() {
        let _c = ComInit::sta();
        for name in ["shell:MyComputerFolder", "shell:RecycleBinFolder"] {
            let (f, items) = list(name).unwrap();
            assert!(!f.columns().is_empty(), "{name} has columns");
            let _ = items.len();
        }
    }

    #[test]
    fn child_folder_opens_and_parent_returns() {
        let _c = ComInit::sta();
        let (pc, items) = list("shell:MyComputerFolder").unwrap();
        let c = items.iter().find(|i| i.parsing_name() == r"C:\").expect("C: drive listed");
        assert!(c.is_folder());
        let drive = pc.child(c).unwrap();
        assert_eq!(drive.parsing_name, r"C:\");
        assert!(!drive.enumerate().unwrap().is_empty());
        assert_eq!(drive.parent().unwrap().parsing_name, pc.parsing_name);
        // Root of the namespace: This PC's parent is the desktop, whose parent is None.
        assert!(pc.parent().unwrap().parent().is_none());
    }
}
