use std::collections::HashSet;
use std::ptr;

use windows::Win32::Foundation::{HWND, LPARAM, PROPERTYKEY};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW,
};
use windows::Win32::UI::Shell::Common::{ITEMIDLIST, SHCOLSTATE, SHCOLSTATE_HIDDEN, SHCOLSTATE_ONBYDEFAULT, SHCOLSTATE_SECONDARYUI, SHCOLSTATE_TYPEMASK, SHCOLSTATE_TYPE_DATE, SHCOLSTATE_TYPE_INT, SHELLDETAILS};
use windows::Win32::UI::Shell::{
    BHID_SFObject, CSIDL_DESKTOP, IEnumIDList, SHGetFolderLocation, ILClone, ILCombine, ILFindLastID, ILRemoveLastID, IShellFolder,
    IShellFolder2, IShellItem, SHCONTF_FOLDERS, SHCONTF_INCLUDEHIDDEN, SHCONTF_NONFOLDERS,
    SHCreateItemFromIDList, SHCreateItemFromParsingName, SHGetIDListFromObject,
    SHParseDisplayName, SIGDN_DESKTOPABSOLUTEEDITING, SIGDN_DESKTOPABSOLUTEPARSING,
    SIGDN_NORMALDISPLAY,
};
use windows::core::{HSTRING, Interface, PCWSTR, PWSTR, Result, w};

const THIS_PC: &str = "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}";

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
#[derive(Clone)]
pub struct Column {
    /// The shell's column index; gaps are normal because off-by-default columns are skipped.
    pub index: u32,
    pub title: String,
    pub width_chars: i32,
    pub right_align: bool,
    /// The shell's value type (`SHCOLSTATE_TYPE_*`), masked.
    pub kind: SHCOLSTATE,
}

impl Column {
    /// Sizes and dates start with the biggest/newest on the first sort, as Explorer does.
    pub fn descending_first(&self) -> bool {
        self.kind == SHCOLSTATE_TYPE_INT || self.kind == SHCOLSTATE_TYPE_DATE
    }
}

impl ShellFolder {
    /// Opens a folder by parsing name: a path, a `shell:` name, or a `::{CLSID}` string.
    pub fn parse(name: &str) -> Result<ShellFolder> {
        let wide = HSTRING::from(name);
        let item: IShellItem = unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None)? };
        Self::from_item(item)
    }

    /// The namespace root: the desktop as Explorer shows it (the user's and the public desktop folders merged).
    pub fn desktop() -> Result<ShellFolder> {
        Self::from_pidl(&unsafe { Pidl::from_raw(SHGetFolderLocation(None, CSIDL_DESKTOP as i32, None, 0)?) })
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
        self.enumerate_with(SHCONTF_FOLDERS.0 | SHCONTF_NONFOLDERS.0, usize::MAX)
    }

    /// Subfolders only, and no more than `max` of them (UI-thread callers bound their work).
    pub fn subfolders_up_to(&self, max: usize) -> Result<Vec<ShellItem>> {
        self.enumerate_with(SHCONTF_FOLDERS.0, max)
    }

    fn enumerate_with(&self, what: i32, max: usize) -> Result<Vec<ShellItem>> {
        let mut e: Option<IEnumIDList> = None;
        let flags = (what | SHCONTF_INCLUDEHIDDEN.0) as u32;
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
            if out.len() >= max {
                break;
            }
        }
        if self.parsing_name == THIS_PC {
            self.append_known_folders(&mut out);
        }
        Ok(out)
    }

    /// shortcut: EnumObjects on This PC returns drives only; Explorer adds the folders registered under
    /// MyComputer\NameSpace itself. Drop this if a Windows build starts enumerating them.
    fn append_known_folders(&self, out: &mut Vec<ShellItem>) {
        let mut key = HKEY::default();
        let path = w!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\MyComputer\NameSpace");
        if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path, None, KEY_READ, &mut key) }.is_err() {
            return;
        }
        let mut seen: HashSet<String> = out.iter().map(|i| i.parsing_name()).collect();
        for index in 0.. {
            let mut name = [0u16; 128];
            let mut len = name.len() as u32;
            let hr = unsafe { RegEnumKeyExW(key, index, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) };
            if hr.is_err() {
                break;
            }
            let guid = String::from_utf16_lossy(&name[..len as usize]);
            if !guid.starts_with('{') {
                continue;
            }
            let full = HSTRING::from(format!("{THIS_PC}\\::{guid}"));
            let mut abs = ptr::null_mut();
            if unsafe { SHParseDisplayName(PCWSTR(full.as_ptr()), None, &mut abs, 0, None) }.is_err() {
                continue;
            }
            let abs = unsafe { Pidl::from_raw(abs) };
            let child = unsafe { Pidl::from_raw(ILClone(ILFindLastID(abs.as_ptr()))) };
            let item = ShellItem { parent: self.folder.clone(), pidl: child };
            if !item.display_name().is_empty() && seen.insert(item.parsing_name()) {
                out.push(item);
            }
        }
        let _ = unsafe { RegCloseKey(key) };
    }

    /// The columns this folder shows by default (Name, Date modified, Type, Size for filesystem folders).
    pub fn columns(&self) -> Vec<Column> {
        self.columns_where(|state, index| index == 0 || state.is_some_and(|s| s.0 & SHCOLSTATE_ONBYDEFAULT.0 != 0))
    }

    /// The columns a header right-click offers: the defaults plus the other ones Explorer lists (not those it keeps
    /// for its "More..." dialog).
    pub fn menu_columns(&self) -> Vec<Column> {
        self.columns_where(in_menu)
    }

    fn columns_where(&self, keep: impl Fn(Option<SHCOLSTATE>, u32) -> bool) -> Vec<Column> {
        let mut cols = Vec::new();
        let Ok(f2) = self.folder.cast::<IShellFolder2>() else { return cols };
        for index in 0..64 {
            let mut sd = SHELLDETAILS::default();
            if unsafe { f2.GetDetailsOf(ptr::null(), index, &mut sd) }.is_err() {
                break;
            }
            let state = unsafe { f2.GetDefaultColumnState(index) }.ok();
            let kind = SHCOLSTATE(state.map_or(0, |s| s.0 & SHCOLSTATE_TYPEMASK.0));
            if !keep(state, index) {
                continue;
            }
            // SHELLDETAILS is packed: copy the STRRET out before taking a reference.
            let mut ret = { sd.str };
            let title = unsafe { strret_to_string(&mut ret, ptr::null()) };
            // fmt 1 is LVCFMT_RIGHT (sizes, counts).
            cols.push(Column { index, title, width_chars: sd.cxChar, right_align: sd.fmt == 1, kind });
        }
        cols
    }

    /// The property behind a shell column (what `ShellItem::property_time` reads for a date column).
    pub fn column_key(&self, column: u32) -> Option<PROPERTYKEY> {
        let mut key = PROPERTYKEY::default();
        unsafe { self.folder.cast::<IShellFolder2>().ok()?.MapColumnToSCID(column, &mut key) }.ok().map(|_| key)
    }

    /// Shell-correct ordering by a column (folders first, locale-aware names, real dates and sizes).
    pub fn compare(&self, column: u32, a: &ShellItem, b: &ShellItem) -> std::cmp::Ordering {
        let hr = unsafe { self.folder.CompareIDs(LPARAM(column as isize), a.pidl.as_ptr(), b.pidl.as_ptr()) };
        // A failure HRESULT is no ordering; decoding it would make sort_by's comparator inconsistent.
        if hr.is_err() {
            return std::cmp::Ordering::Equal;
        }
        // The low 16 bits of the HRESULT hold the signed comparison result.
        ((hr.0 & 0xFFFF) as i16).cmp(&0)
    }
}

/// Whether the header menu offers a column: Name, the defaults, and any not hidden or kept for the "More..." dialog.
fn in_menu(state: Option<SHCOLSTATE>, index: u32) -> bool {
    index == 0 || state.is_some_and(|s| s.0 & SHCOLSTATE_ONBYDEFAULT.0 != 0 || s.0 & (SHCOLSTATE_HIDDEN.0 | SHCOLSTATE_SECONDARYUI.0) == 0)
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
    use windows::Win32::UI::Shell::Common::SHCOLSTATE_TYPE_STR;

    #[test]
    fn descending_first_by_type() {
        let c = |kind| Column { index: 0, title: String::new(), width_chars: 0, right_align: false, kind };
        assert!(c(SHCOLSTATE_TYPE_INT).descending_first() && c(SHCOLSTATE_TYPE_DATE).descending_first());
        assert!(!c(SHCOLSTATE_TYPE_STR).descending_first());
    }

    #[test]
    fn in_menu_rule() {
        let st = |s: SHCOLSTATE| Some(s);
        assert!(in_menu(st(SHCOLSTATE_ONBYDEFAULT), 3));
        assert!(in_menu(st(SHCOLSTATE(0)), 3));
        assert!(!in_menu(st(SHCOLSTATE_SECONDARYUI), 3));
        assert!(!in_menu(st(SHCOLSTATE_HIDDEN), 3));
        assert!(in_menu(st(SHCOLSTATE(SHCOLSTATE_ONBYDEFAULT.0 | SHCOLSTATE_SECONDARYUI.0)), 3));
        assert!(in_menu(None, 0));
        assert!(!in_menu(None, 5));
    }

    #[test]
    fn menu_columns_extend_the_defaults() {
        let _c = ComInit::sta();
        let dir = std::env::temp_dir().join(format!("rexplorer-test-menucols-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = ShellFolder::parse(dir.to_str().unwrap()).unwrap();
        let (defaults, menu) = (f.columns(), f.menu_columns());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(defaults.iter().all(|d| menu.iter().any(|m| m.index == d.index)));
        assert!(menu.len() > defaults.len());
        assert_eq!(menu[0].index, 0);
    }

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
        // Display names follow the user's "hide extensions for known file types" setting.
        let bare = |s: String| s.trim_end_matches(".txt").to_string();
        items.sort_by(|a, b| f.compare(0, a, b));
        let names: Vec<(String, bool)> =
            items.iter().map(|i| (bare(i.display_name()), i.is_folder())).collect();
        // Column titles are localized; filesystem folders always expose Name, Size, Type, Date modified.
        let cols = f.columns();
        assert!(cols.len() >= 4, "{} columns", cols.len());
        let a = items.iter().find(|i| bare(i.display_name()) == "a").unwrap();
        let size_of_a = a.detail(cols[1].index);
        let sizes = (a.size(), items.iter().find(|i| i.is_folder()).unwrap().size());
        std::fs::remove_dir_all(&dir).unwrap();
        // Folders sort first.
        assert_eq!(
            names,
            vec![
                ("sub".into(), true),
                ("a".into(), false),
                ("b".into(), false),
                ("c".into(), false)
            ]
        );
        assert_eq!(cols[0].kind, SHCOLSTATE_TYPE_STR, "Name");
        assert!(cols.iter().any(|c| c.kind == SHCOLSTATE_TYPE_DATE), "a date column");
        assert!(size_of_a.starts_with("1 "), "size text: {size_of_a:?}");
        assert_eq!(sizes, (Some(1), Some(0)), "the shell reports 0 for folders");
    }

    #[test]
    fn file_time_matches_fs() {
        let _c = ComInit::sta();
        let dir = std::env::temp_dir().join(format!("rexplorer-time-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 86_400);
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(old).unwrap();
        let (f, items) = list(dir.to_str().unwrap()).unwrap();
        let date = f.columns().into_iter().find(|c| c.kind == SHCOLSTATE_TYPE_DATE).expect("a date column");
        let key = f.column_key(date.index).expect("its property");
        let ft = items[0].property_time(&key).expect("a date");
        let unix = ((ft.dwHighDateTime as u64) << 32 | ft.dwLowDateTime as u64) / 10_000_000 - 11_644_473_600;
        let fs = std::fs::metadata(&file).unwrap().modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(unix.abs_diff(fs) <= 2, "shell {unix} vs fs {fs}");
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
    fn this_pc_lists_drives_and_user_folders() {
        let _c = ComInit::sta();
        let (_, items) = list("shell:MyComputerFolder").unwrap();
        let names: Vec<String> = items.iter().map(|i| i.display_name()).collect();
        assert!(items.iter().any(|i| i.parsing_name() == r"C:\"), "{names:?}");
        assert!(items.iter().filter(|i| !i.is_hidden()).count() >= 2, "{names:?}");
        assert!(items.iter().all(|i| i.is_folder()), "{names:?}");
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
