use std::ptr;

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_SFObject, IEnumIDList, IShellFolder, IShellItem, SHCONTF_FOLDERS, SHCONTF_INCLUDEHIDDEN,
    SHCONTF_NONFOLDERS, SHCreateItemFromParsingName, SHGetDesktopFolder,
    SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_NORMALDISPLAY,
};
use windows::core::{HSTRING, PCWSTR, Result};

use crate::com::{Pidl, take_cotaskmem_string};
use crate::item::ShellItem;

/// An opened shell folder (filesystem or virtual).
pub struct ShellFolder {
    pub folder: IShellFolder,
    pub name: String,
    pub parsing_name: String,
}

impl ShellFolder {
    /// Opens a folder by parsing name: a path, a `shell:` name, or a `::{CLSID}` string.
    pub fn parse(name: &str) -> Result<ShellFolder> {
        let wide = HSTRING::from(name);
        unsafe {
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None)?;
            let folder: IShellFolder = item.BindToHandler(None, &BHID_SFObject)?;
            let name = take_cotaskmem_string(item.GetDisplayName(SIGDN_NORMALDISPLAY)?);
            let parsing_name = take_cotaskmem_string(item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING)?);
            Ok(ShellFolder { folder, name, parsing_name })
        }
    }

    pub fn desktop() -> Result<ShellFolder> {
        let folder = unsafe { SHGetDesktopFolder()? };
        Ok(ShellFolder { folder, name: "Desktop".into(), parsing_name: String::new() })
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

    /// Shell-correct name ordering (folders first, locale-aware), via `CompareIDs`.
    pub fn compare(&self, a: &ShellItem, b: &ShellItem) -> std::cmp::Ordering {
        let hr = unsafe { self.folder.CompareIDs(LPARAM(0), a.pidl.as_ptr(), b.pidl.as_ptr()) };
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
    }

    #[test]
    fn enumerates_temp_dir_contents() {
        let _c = ComInit::sta();
        let dir = std::env::temp_dir().join(format!("rexplorer-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for n in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let (_f, items) = list(dir.to_str().unwrap()).unwrap();
        let mut names: Vec<(String, bool)> =
            items.iter().map(|i| (i.display_name(), i.is_folder())).collect();
        names.sort();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            names,
            vec![
                ("a.txt".into(), false),
                ("b.txt".into(), false),
                ("c.txt".into(), false),
                ("sub".into(), true)
            ]
        );
    }

    #[test]
    fn virtual_folders_open_and_enumerate() {
        let _c = ComInit::sta();
        for name in ["shell:MyComputerFolder", "shell:RecycleBinFolder"] {
            let (_f, items) = list(name).unwrap();
            let _ = items.len();
        }
    }

    #[test]
    fn child_folder_opens() {
        let _c = ComInit::sta();
        let (f, items) = list("shell:MyComputerFolder").unwrap();
        let c = items.iter().find(|i| i.parsing_name() == r"C:\").expect("C: drive listed");
        assert!(c.is_folder());
        let opened = c.open().unwrap();
        assert!(!opened.enumerate().unwrap().is_empty());
        assert_eq!(f.compare(&items[0], &items[0]), std::cmp::Ordering::Equal);
    }
}
