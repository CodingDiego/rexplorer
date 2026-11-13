//! The taskbar identity: one AppUserModelID for the process, its windows and the Start menu shortcut, and the
//! taskbar jump list (Frequent and Recent folders, a "New window" task). Needs no install: the ID is explicit.
use std::path::Path;

use windows::Win32::Foundation::{HWND, PROPERTYKEY};
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ};
use windows::Win32::System::Variant::{VT_EMPTY, VT_LPWSTR};
use windows::Win32::UI::Shell::Common::{IObjectArray, IObjectCollection};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::Shell::{
    DestinationList, EnumerableObjectCollection, ICustomDestinationList, IShellLinkW, SHStrDupW,
    SetCurrentProcessExplicitAppUserModelID, ShellLink,
};
use windows::core::{GUID, HSTRING, Interface, Result};

/// The AppUserModelID of the process, its windows, the jump list and the installed Start menu shortcut.
pub const APP_ID: &str = "CodingDiego.rexplorer";

const PKEY_APP_ID: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3), pid: 5 };
const PKEY_TITLE: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0xF29F85E0_4FF9_1068_AB91_08002B27B3D9), pid: 2 };

/// Words the caller translates.
pub struct Labels {
    pub frequent: String,
    pub recent: String,
    pub new_window: String,
}

/// An owned VT_LPWSTR (or VT_EMPTY) PROPVARIANT, cleared on drop.
struct Pv(PROPVARIANT);

impl Pv {
    fn text(s: &str) -> Result<Pv> {
        let mut pv = PROPVARIANT::default();
        unsafe {
            let p = SHStrDupW(&HSTRING::from(s))?;
            (*pv.Anonymous.Anonymous).vt = VT_LPWSTR;
            (*pv.Anonymous.Anonymous).Anonymous.pwszVal = p;
        }
        Ok(Pv(pv))
    }

    fn empty() -> Pv {
        let mut pv = PROPVARIANT::default();
        unsafe { (*pv.Anonymous.Anonymous).vt = VT_EMPTY };
        Pv(pv)
    }
}

impl Drop for Pv {
    fn drop(&mut self) {
        let _ = unsafe { PropVariantClear(&mut self.0) };
    }
}

/// Before any window: the taskbar groups every window of this process under `APP_ID`.
pub fn set_process_app_id() -> Result<()> {
    unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(APP_ID)) }
}

/// The same ID on `hwnd` itself (readable from outside the process); `clear` before the window is destroyed.
pub fn set_window_app_id(hwnd: HWND, clear: bool) -> Result<()> {
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd)?;
        let pv = if clear { Pv::empty() } else { Pv::text(APP_ID)? };
        store.SetValue(&PKEY_APP_ID, &pv.0)?;
        store.Commit()
    }
}

/// Stamps `APP_ID` on a shortcut before it is saved, so a pinned Start menu entry shares the taskbar button.
pub fn stamp_link(link: &IShellLinkW) -> Result<()> {
    let store: IPropertyStore = link.cast()?;
    let pv = Pv::text(APP_ID)?;
    unsafe {
        store.SetValue(&PKEY_APP_ID, &pv.0)?;
        store.Commit()
    }
}

/// The AppUserModelID a saved shortcut carries, if any.
pub fn link_app_id(lnk: &Path) -> Option<String> {
    let _com = crate::com::ComInit::sta();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        link.cast::<IPersistFile>().ok()?.Load(&HSTRING::from(lnk.as_os_str()), STGM_READ).ok()?;
        let mut pv = Pv(link.cast::<IPropertyStore>().ok()?.GetValue(&PKEY_APP_ID).ok()?);
        let s = crate::com::take_cotaskmem_string(PropVariantToStringAlloc(&pv.0).ok()?);
        let _ = PropVariantClear(&mut pv.0);
        (!s.is_empty()).then_some(s)
    }
}

/// The command-line argument a folder item launches with: the path in quotes. None for what is not an absolute file
/// system path or would break out of the quotes.
pub fn folder_arg(path: &str) -> Option<String> {
    let ok = Path::new(path).is_absolute() && !path.chars().any(|c| c == '"' || c.is_control());
    // Backslashes before the closing quote are doubled, or `"C:\"` would read as `C:"` (the argv rule for `\"`).
    let trailing = &path[path.trim_end_matches('\\').len()..];
    ok.then(|| format!("\"{path}{trailing}\""))
}

/// (frequent, recent) items for `slots` places: frequent first, recent without them; nothing the user removed from
/// the list (`removed` holds launch arguments), no duplicates, only what `folder_arg` accepts.
pub fn pick(frequent: &[String], recent: &[String], removed: &[String], slots: usize) -> (Vec<String>, Vec<String>) {
    let mut seen: Vec<String> = Vec::new();
    let mut take = |from: &[String], n: usize| -> Vec<String> {
        let mut out = Vec::new();
        for p in from {
            if out.len() >= n {
                break;
            }
            let Some(arg) = folder_arg(p) else { continue };
            let key = p.to_lowercase();
            if seen.contains(&key) || removed.iter().any(|r| r.eq_ignore_ascii_case(&arg)) {
                continue;
            }
            seen.push(key);
            out.push(p.clone());
        }
        out
    };
    // Frequent gets half the slots when there are recents to show too.
    let f = take(frequent, if recent.is_empty() { slots } else { slots.div_ceil(2) });
    let r = take(recent, slots - f.len());
    (f, r)
}

fn link(exe: &Path, args: &str, title: &str, tip: &str, icon: Option<(&str, i32)>) -> Result<IShellLinkW> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(exe.as_os_str()))?;
        link.SetArguments(&HSTRING::from(args))?;
        link.SetDescription(&HSTRING::from(tip))?;
        match icon {
            Some((file, index)) => link.SetIconLocation(&HSTRING::from(file), index)?,
            None => link.SetIconLocation(&HSTRING::from(exe.as_os_str()), 0)?,
        }
        let store: IPropertyStore = link.cast()?;
        let pv = Pv::text(title)?;
        store.SetValue(&PKEY_TITLE, &pv.0)?;
        store.Commit()?;
        Ok(link)
    }
}

fn args_of(items: &IObjectArray) -> Vec<String> {
    let n = unsafe { items.GetCount() }.unwrap_or(0);
    (0..n)
        .filter_map(|i| unsafe { items.GetAt::<IShellLinkW>(i) }.ok())
        .map(|l| {
            let mut buf = [0u16; 1024];
            let _ = unsafe { l.GetArguments(&mut buf) };
            String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(buf.len())])
        })
        .collect()
}

/// A folder's name for its jump list entry: the last component, or the whole path for a drive or share root.
pub fn title_of(path: &str) -> String {
    Path::new(path).file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

/// Replaces the jump list of `APP_ID`: Frequent and Recent folders (opened by `exe "<path>"`) and a New window task.
/// Call on a thread with COM.
pub fn update(exe: &Path, frequent: &[String], recent: &[String], labels: &Labels) -> Result<()> {
    unsafe {
        let list: ICustomDestinationList = CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER)?;
        list.SetAppID(&HSTRING::from(APP_ID))?;
        let mut slots = 0u32;
        let removed: IObjectArray = list.BeginList(&mut slots)?;
        let built = (|| -> Result<()> {
            let (f, r) = pick(frequent, recent, &args_of(&removed), slots as usize);
            let folder_icon = std::env::var("SystemRoot").map(|s| format!(r"{s}\System32\imageres.dll")).unwrap_or_default();
            for (label, paths) in [(&labels.frequent, &f), (&labels.recent, &r)] {
                if paths.is_empty() {
                    continue;
                }
                let coll: IObjectCollection = CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)?;
                for p in paths.iter() {
                    let arg = folder_arg(p).unwrap_or_default();
                    // imageres.dll resource 3 is the folder icon.
                    coll.AddObject(&link(exe, &arg, &title_of(p), p, (!folder_icon.is_empty()).then_some((folder_icon.as_str(), -3)))?)?;
                }
                list.AppendCategory(&HSTRING::from(label.as_str()), &coll.cast::<IObjectArray>()?)?;
            }
            let tasks: IObjectCollection = CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)?;
            tasks.AddObject(&link(exe, "--new-window", &labels.new_window, &labels.new_window, None)?)?;
            list.AddUserTasks(&tasks.cast::<IObjectArray>()?)?;
            list.CommitList()
        })();
        if built.is_err() {
            let _ = list.AbortList();
        }
        built
    }
}

/// Removes the jump list of `APP_ID` (tests and uninstall).
pub fn delete() -> Result<()> {
    unsafe {
        let list: ICustomDestinationList = CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER)?;
        list.DeleteList(&HSTRING::from(APP_ID))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    /// How Windows splits a command line (the rules Rust's own args follow).
    fn argv(line: &str) -> Vec<String> {
        use windows::Win32::Foundation::{HLOCAL, LocalFree};
        use windows::Win32::UI::Shell::CommandLineToArgvW;
        unsafe {
            let mut n = 0;
            let p = CommandLineToArgvW(&HSTRING::from(line), &mut n);
            let out = (0..n as usize).map(|i| (*p.add(i)).to_string().unwrap()).collect();
            LocalFree(Some(HLOCAL(p as _)));
            out
        }
    }

    #[test]
    fn folder_arg_quotes_paths_and_refuses_the_rest() {
        assert_eq!(folder_arg(r"C:\Users\a b").as_deref(), Some("\"C:\\Users\\a b\""));
        assert_eq!(folder_arg(r"\\server\share").as_deref(), Some("\"\\\\server\\share\""));
        // A root keeps its backslash through the command line's quoting rules.
        for root in [r"C:\", r"\\server\share\", r"C:\a\\"] {
            let arg = folder_arg(root).unwrap();
            assert_eq!(argv(&format!("rexplorer.exe {arg}")), [String::from("rexplorer.exe"), root.to_string()], "{arg}");
        }
        assert_eq!(argv(&format!("x {}", folder_arg(r"C:\Users\a b").unwrap())), ["x", r"C:\Users\a b"]);
        for bad in ["::{20D04FE0-3AEA-1069-A2D8-08002B30309D}", "shell:Downloads", "relative", "C:\\a\"b", "C:\\a\nb", ""] {
            assert_eq!(folder_arg(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn pick_splits_slots_skips_removed_duplicates_and_virtual_places() {
        let freq = v(&[r"C:\f1", r"C:\f2", r"C:\f3"]);
        let rec = v(&[r"c:\F1", r"C:\r1", "::{x}", r"C:\r2", r"C:\r3"]);
        let (f, r) = pick(&freq, &rec, &[], 4);
        assert_eq!((f, r), (v(&[r"C:\f1", r"C:\f2"]), v(&[r"C:\r1", r"C:\r2"])));
        // A removed entry stays out; the next one takes its place.
        let (f, r) = pick(&freq, &rec, &v(&["\"C:\\F2\""]), 4);
        assert_eq!((f, r), (v(&[r"C:\f1", r"C:\f3"]), v(&[r"C:\r1", r"C:\r2"])));
        // Fewer frequent folders leave their slots to recent ones; no recents give frequent all of them.
        assert_eq!(pick(&v(&[r"C:\f1"]), &rec, &[], 4).1.len(), 3);
        assert_eq!(pick(&freq, &[], &[], 2).0.len(), 2);
        assert_eq!(pick(&[], &[], &[], 10), (vec![], vec![]));
        assert_eq!(pick(&freq, &rec, &[], 0), (vec![], vec![]));
    }

    #[test]
    fn titles_are_the_last_component_or_the_root() {
        assert_eq!(title_of(r"C:\Users\me\Documents"), "Documents");
        assert_eq!(title_of(r"C:\"), r"C:\");
        assert_eq!(title_of(r"\\server\share"), r"\\server\share");
    }
}
