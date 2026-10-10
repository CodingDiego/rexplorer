//! Recursive name search through the shell namespace, delivered in batches.
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::System::SystemServices::{SFGAO_FOLDER, SFGAO_HIDDEN, SFGAO_LINK, SFGAO_STREAM};
use windows::core::Result;

use crate::com::Pidl;
use crate::folder::ShellFolder;
use crate::item::ShellItem;

/// One matching item and the editing name of the folder that contains it.
pub struct Hit {
    pub item: ShellItem,
    pub folder: String,
}

/// Case-insensitive substring match; an empty needle matches nothing.
pub fn matches(name: &str, needle: &str) -> bool {
    !needle.is_empty() && name.to_lowercase().contains(&needle.to_lowercase())
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

/// Breadth-first name search below `root`. Call it on a worker thread initialized with `ComInit::mta()`.
///
/// `sink` gets each folder's hits as soon as that folder is done. `cancel` is checked before every
/// folder; setting it makes the walk return `Ok(())`. Links are never entered, archives (`SFGAO_STREAM`) and hidden
/// `C:\$...` system folders only as `scope` says, and subfolders that fail to enumerate are skipped.
pub fn walk(
    root: &Pidl,
    needle: &str,
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
            if matches(&item.display_name(), needle) {
                hits.push(Hit { item, folder: folder.editing_name.clone() });
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

    #[test]
    fn matches_is_case_insensitive_substring() {
        assert!(matches("Report.TXT", "report"));
        assert!(matches("Report.TXT", ".txt"));
        assert!(!matches("Report.TXT", "pdf"));
        assert!(!matches("anything", ""));
        assert!(!matches("", "a"));
        assert!(matches("Tamaño", "maÑ"));
    }

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
        let mut hits = Vec::new();
        let res = root.and_then(|r| {
            walk(&r, "needle", Scope::default(), &AtomicBool::new(false), &mut |h| hits.extend(h))
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
    }

    #[test]
    fn walk_stops_when_cancelled() {
        let _c = ComInit::mta();
        let dir = temp_root("cancel");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("needle.txt"), b"x").unwrap();
        let root = ShellFolder::parse(dir.to_str().unwrap()).unwrap().pidl;
        let mut calls = 0;
        let res = walk(&root, "needle", Scope::default(), &AtomicBool::new(true), &mut |_| calls += 1);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(res.is_ok());
        assert_eq!(calls, 0);
    }
}
