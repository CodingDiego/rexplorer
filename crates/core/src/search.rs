//! Recursive name search through the shell namespace, delivered in batches.
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::SystemServices::{SFGAO_FOLDER, SFGAO_HIDDEN, SFGAO_LINK, SFGAO_STREAM};
use windows::core::{GUID, Result};

use crate::com::Pidl;
use crate::folder::ShellFolder;
use crate::item::ShellItem;
use crate::query::{Query, local_day};

/// One matching item and the editing name of the folder that contains it.
pub struct Hit {
    pub item: ShellItem,
    pub folder: String,
    /// The containing folder's absolute PIDL.
    pub pidl: Pidl,
}

/// What the walk goes into besides plain folders.
#[derive(Clone, Copy)]
pub struct Scope {
    /// Hidden `C:\$...` system folders.
    pub system: bool,
    /// Archives the shell opens as folders (ZIP, CAB...).
    pub archives: bool,
}

impl Default for Scope {
    fn default() -> Scope {
        Scope { system: true, archives: false }
    }
}

const PKEY_DATE_MODIFIED: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0xB725F130_47EF_101A_A5F1_02608C9EEBAC), pid: 14 };

/// Whether `item` (with `attrs` holding at least SFGAO_FOLDER and SFGAO_STREAM) answers `query`: the words on its
/// display name, the filters on its file name, size and date, read only when a filter needs them.
fn hit(item: &ShellItem, attrs: u32, query: &Query) -> bool {
    query.name_matches(&item.display_name())
        && (!query.needs_meta() || {
            // A zip is a folder to the shell but a file to the user.
            let folder = attrs & SFGAO_FOLDER.0 != 0 && attrs & SFGAO_STREAM.0 == 0;
            query.meta_matches(&item.file_name(), folder, || item.size(), || item.property_time(&PKEY_DATE_MODIFIED).and_then(local_day))
        })
}

/// Breadth-first search below `root` for `query`. Call it on a worker thread initialized with `ComInit::mta()`.
///
/// `sink` gets each folder's hits as soon as that folder is done. `cancel` is checked before every
/// folder; setting it makes the walk return `Ok(())`. Links are never entered, archives (`SFGAO_STREAM`) and hidden
/// `C:\$...` system folders only as `scope` says, and subfolders that fail to enumerate are skipped.
pub fn walk(
    root: &Pidl,
    query: &Query,
    scope: Scope,
    cancel: &AtomicBool,
    sink: &mut dyn FnMut(Vec<Hit>),
) -> Result<()> {
    let mut queue = VecDeque::from([root.clone()]);
    let mut first = true;
    let mut seen: HashSet<String> = HashSet::new();
    while let Some(pidl) = queue.pop_front() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let folder = match ShellFolder::from_pidl(&pidl) {
            Ok(f) => f,
            Err(e) if first => return Err(e),
            Err(_) => continue,
        };
        let items = match folder.enumerate() {
            Ok(i) => i,
            Err(e) if first => return Err(e),
            Err(_) => continue,
        };
        first = false;
        let mut hits = Vec::new();
        for item in items {
            let mask = SFGAO_FOLDER.0 | SFGAO_STREAM.0 | SFGAO_LINK.0 | SFGAO_HIDDEN.0;
            let attrs = item.attributes(mask);
            if !scope.system && attrs & SFGAO_HIDDEN.0 != 0 && item.parsing_name().starts_with(r"C:\$") {
                continue;
            }
            let skip = SFGAO_LINK.0 | if scope.archives { 0 } else { SFGAO_STREAM.0 };
            // Junction loops reach the same folder again under a new path; the visited set ends them.
            if attrs & SFGAO_FOLDER.0 != 0 && attrs & skip == 0 && seen.insert(item.parsing_name().to_lowercase()) {
                queue.push_back(folder.child_pidl(&item));
            }
            if hit(&item, attrs, query) {
                hits.push(Hit { item, folder: folder.editing_name.clone(), pidl: folder.pidl.clone() });
            }
        }
        if !hits.is_empty() {
            sink(hits);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::ComInit;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("rexplorer-search-{tag}-{}", std::process::id()))
    }

    #[test]
    fn walk_finds_nested_file() {
        let _c = ComInit::mta();
        let dir = temp_root("nested");
        std::fs::create_dir_all(dir.join("a").join("b")).unwrap();
        for p in ["a/b/needle-x.txt", "needle-y.txt", "other.txt"] {
            std::fs::write(dir.join(p), b"x").unwrap();
        }
        let root = ShellFolder::parse(dir.to_str().unwrap()).map(|f| f.pidl);
        let want = ShellFolder::parse(dir.join("a").join("b").to_str().unwrap()).unwrap().pidl;
        let mut hits = Vec::new();
        let res = root.and_then(|r| {
            walk(&r, &Query::parse("needle"), Scope::default(), &AtomicBool::new(false), &mut |h| hits.extend(h))
        });
        std::fs::remove_dir_all(&dir).unwrap();
        res.unwrap();
        // Display names follow the user's "hide extensions for known file types" setting.
        let bare = |s: String| s.trim_end_matches(".txt").to_string();
        let mut names: Vec<String> = hits.iter().map(|h| bare(h.item.display_name())).collect();
        names.sort();
        assert_eq!(names, ["needle-x", "needle-y"]);
        let x = hits.iter().find(|h| bare(h.item.display_name()) == "needle-x").unwrap();
        assert!(x.folder.ends_with('b'), "{}", x.folder);
        assert!(unsafe { windows::Win32::UI::Shell::ILIsEqual(want.as_ptr(), x.pidl.as_ptr()) }.as_bool(), "the hit carries its folder's PIDL");
    }

    #[test]
    fn walk_stops_when_cancelled() {
        let _c = ComInit::mta();
        let dir = temp_root("cancel");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("needle.txt"), b"x").unwrap();
        let root = ShellFolder::parse(dir.to_str().unwrap()).unwrap().pidl;
        let mut calls = 0;
        let res = walk(&root, &Query::parse("needle"), Scope::default(), &AtomicBool::new(true), &mut |_| calls += 1);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(res.is_ok());
        assert_eq!(calls, 0);
    }

    #[test]
    fn walk_applies_filters() {
        let _c = ComInit::mta();
        let dir = temp_root("filters");
        std::fs::create_dir_all(dir.join("photos")).unwrap();
        std::fs::write(dir.join("photos").join("trip.jpg"), vec![0u8; 3000]).unwrap();
        std::fs::write(dir.join("trip.txt"), b"x").unwrap();
        std::fs::write(dir.join("Tamaño.txt"), b"").unwrap();
        let root = ShellFolder::parse(dir.to_str().unwrap()).unwrap().pidl;
        let names = |q: &str| {
            let mut out = Vec::new();
            walk(&root, &Query::parse(q), Scope::default(), &AtomicBool::new(false), &mut |h| out.extend(h.iter().map(|h| h.item.file_name()))).unwrap();
            out.sort();
            out
        };
        let got = [names("trip"), names("kind:picture"), names("kind:folder"), names("trip size:>2kb"), names("maÑ size:empty"), names("ext:txt date:today")];
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(got[0], ["trip.jpg", "trip.txt"]);
        assert_eq!(got[1], ["trip.jpg"]);
        assert_eq!(got[2], ["photos"]);
        assert_eq!(got[3], ["trip.jpg"]);
        assert_eq!(got[4], ["Tamaño.txt"]);
        assert_eq!(got[5], ["Tamaño.txt", "trip.txt"]);
    }
}
