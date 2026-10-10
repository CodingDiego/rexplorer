//! `--install` / `--uninstall`: rexplorer as the per-user file manager, in place of Explorer's folder windows
//! (never the desktop shell).
//!
//! `install` copies the running exe to `<dir>\rexplorer.exe`, registers the copy (register.rs: Folder, Directory,
//! Drive and File Explorer's CLSID verbs), points `App Paths\explorer.exe` at it (ShellExecute callers of
//! `explorer.exe` by bare name; CreateProcess callers still get Explorer), adds a Start menu shortcut (on the real
//! base only), and records every key, value and file it created in `<dir>\install.txt`. `uninstall` reverts what
//! install.txt lists and leaves other programs' values alone. Everything lives under HKCU: no elevation.
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::E_FAIL;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
use windows::Win32::UI::Shell::{FOLDERID_Programs, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink};
use windows::core::{Error, HSTRING, Interface, PCWSTR, Result, w};

use crate::com::{ComInit, take_cotaskmem_string};
use crate::register::{self, delete_empty, is_foreign, key_exists, remove_if_ours, set_sz};

/// The real registry base, relative to HKCU.
pub const REAL_REG_BASE: &str = "Software";
const RECORD: &str = "install.txt";
const EXE: &str = "rexplorer.exe";

/// `%LOCALAPPDATA%\Programs\rexplorer`; the undocumented `REXPLORER_INSTALL_DIR` overrides it for tests.
pub fn default_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("REXPLORER_INSTALL_DIR") {
        return d.into();
    }
    PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("Programs").join("rexplorer")
}

/// `Software` (under HKCU); the undocumented `REXPLORER_REG_BASE` overrides it for tests.
pub fn reg_base() -> String {
    std::env::var("REXPLORER_REG_BASE").unwrap_or_else(|_| REAL_REG_BASE.to_string())
}

fn classes(reg_base: &str) -> String {
    format!(r"{reg_base}\Classes")
}

fn app_paths(reg_base: &str) -> String {
    format!(r"{reg_base}\Microsoft\Windows\CurrentVersion\App Paths\explorer.exe")
}

pub fn is_installed(dir: &Path) -> bool {
    dir.join(RECORD).is_file()
}

/// What `install` changes, for the confirmation dialog.
pub fn plan(dir: &Path, reg_base: &str) -> String {
    let c = classes(reg_base);
    let mut s = format!(
        "Copy rexplorer to {}\n\n\
         Open folders, drives, Win+E, the taskbar's and Start's File Explorer with it:\n\
         HKCU\\{c}\\{{Folder, Directory, Drive}}\\shell\\open\\command\n\
         HKCU\\{c}\\CLSID\\{}\\shell\\{{open, opennewwindow}}\\command\n\n\
         Answer programs that start explorer.exe by name:\nHKCU\\{}\n",
        dir.join(EXE).display(),
        register::EXPLORER_CLSID,
        app_paths(reg_base)
    );
    if reg_base == REAL_REG_BASE {
        s.push_str("\nAdd rexplorer to the Start menu.\n");
    }
    s.push_str(
        "\nPer-user only: nothing under HKLM changes and no elevation is needed. The desktop, the taskbar and other \
         programs' Open / Save dialogs stay Explorer's. Undo with rexplorer --uninstall or More > Uninstall.",
    );
    s
}

fn read_record(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join(RECORD)).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default()
}

fn push(rec: &mut Vec<String>, line: String) {
    if !rec.contains(&line) {
        rec.push(line);
    }
}

fn fail(text: String) -> Error {
    Error::new(E_FAIL, text)
}

/// Copies `src` over `target`. A running copy cannot be overwritten but can be renamed: it moves aside to
/// `rexplorer.old`, which the next start deletes (`cleanup_old`).
fn replace(src: &Path, target: &Path) -> std::io::Result<()> {
    match std::fs::copy(src, target) {
        Ok(_) => Ok(()),
        Err(_) if target.exists() => {
            let old = target.with_extension("old");
            let _ = std::fs::remove_file(&old);
            std::fs::rename(target, &old)?;
            std::fs::copy(src, target).map(|_| ())
        }
        Err(e) => Err(e),
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

fn start_menu_programs() -> Result<PathBuf> {
    let p = unsafe { SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, None)? };
    Ok(PathBuf::from(unsafe { take_cotaskmem_string(p) }))
}

fn shortcut(target: &Path, lnk: &Path) -> Result<()> {
    let _com = ComInit::sta();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(target.as_os_str()))?;
        link.SetDescription(w!("rexplorer file manager"))?;
        link.cast::<IPersistFile>()?.Save(&HSTRING::from(lnk.as_os_str()), true)
    }
}

/// Installs the running exe into `dir` and makes it the file manager under `HKCU\<reg_base>`. Returns a summary.
/// Refuses, changing nothing, when another program already overrides one of the entry points.
pub fn install(dir: &Path, reg_base: &str) -> Result<String> {
    let classes = classes(reg_base);
    let ap = app_paths(reg_base);
    if let Some(k) = register::foreign_override(&classes) {
        return Err(fail(format!("Another program already overrides HKCU\\{k}; nothing was changed.")));
    }
    if is_foreign(&HSTRING::from(&ap)) {
        return Err(fail(format!("Another program already answers HKCU\\{ap}; nothing was changed.")));
    }
    let target = dir.join(EXE);
    let commands: Vec<String> = register::command_keys(&classes).into_iter().map(|(k, _)| k.to_string()).collect();
    let mut rec = read_record(dir);
    for leaf in commands.iter().chain(Some(&ap)) {
        let mut k = leaf.as_str();
        while !key_exists(k) {
            push(&mut rec, format!("key {k}"));
            let Some((parent, _)) = k.rsplit_once('\\') else { break };
            k = parent;
        }
    }
    if !dir.exists() {
        std::fs::create_dir_all(dir).map_err(|e| fail(format!("Cannot create {}: {e}", dir.display())))?;
        push(&mut rec, format!("dir {}", dir.display()));
    }
    let src = std::env::current_exe().map_err(|e| fail(format!("Cannot find the running exe: {e}")))?;
    if !same_file(&src, &target) {
        replace(&src, &target).map_err(|e| fail(format!("Cannot copy to {}: {e}", target.display())))?;
    }
    push(&mut rec, format!("file {}", target.display()));
    let mut summary = vec![format!("Copied to {}", target.display())];
    let result = (|| -> Result<()> {
        for k in &commands {
            push(&mut rec, format!("value {k}"));
        }
        register::register(&classes, &target)?;
        summary.push("Folders, drives and Win+E open rexplorer".to_string());
        push(&mut rec, format!("value {ap}"));
        let apk = HSTRING::from(&ap);
        set_sz(&apk, PCWSTR::null(), &target.display().to_string())?;
        set_sz(&apk, w!("Path"), &dir.display().to_string())?;
        summary.push("Programs that start explorer.exe by name get rexplorer".to_string());
        // Under a test base the real Start menu is left alone.
        if reg_base == REAL_REG_BASE {
            let lnk = start_menu_programs()?.join("rexplorer.lnk");
            push(&mut rec, format!("file {}", lnk.display()));
            shortcut(&target, &lnk)?;
            summary.push("Start menu shortcut added".to_string());
        }
        Ok(())
    })();
    // Written even after a failure, so that --uninstall can revert the part that was done.
    std::fs::write(dir.join(RECORD), rec.join("\n") + "\n")
        .map_err(|e| fail(format!("Cannot write {}: {e}", dir.join(RECORD).display())))?;
    result?;
    Ok(summary.join("\n"))
}

/// Reverts what `<dir>\install.txt` lists: values that still name rexplorer, then the keys install created
/// (only if empty), the files, the exe last. Returns a summary.
pub fn uninstall(dir: &Path) -> Result<String> {
    let rec = read_record(dir);
    if rec.is_empty() {
        return Err(fail(format!("rexplorer is not installed in {} (no {RECORD}).", dir.display())));
    }
    let lines = |kind: &str| -> Vec<String> {
        rec.iter().filter_map(|l| l.strip_prefix(kind).and_then(|r| r.strip_prefix(' ')).map(str::to_string)).collect()
    };
    let mut summary = vec![];
    for k in lines("value") {
        if !remove_if_ours(&HSTRING::from(&k))? {
            summary.push(format!("Left HKCU\\{k}: another program changed it"));
        }
    }
    let mut keys = lines("key");
    keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
    for k in &keys {
        delete_empty(k);
    }
    register::notify();
    summary.insert(0, "Folders, drives, Win+E and explorer.exe open Windows Explorer again".to_string());
    let exe = dir.join(EXE);
    for f in lines("file").iter().map(PathBuf::from).filter(|f| *f != exe) {
        let _ = std::fs::remove_file(f);
    }
    let _ = std::fs::remove_file(dir.join(RECORD));
    if exe.exists() && std::fs::remove_file(&exe).is_err() {
        // Running: it can still be moved out of the way; the next rexplorer start deletes it.
        let aside = std::env::temp_dir().join(format!("rexplorer-{}.old", std::process::id()));
        if std::fs::rename(&exe, &aside).is_err() {
            summary.push(format!("Close rexplorer, then delete {}", exe.display()));
        }
    }
    for d in lines("dir") {
        let _ = std::fs::remove_dir(d);
    }
    summary.push(format!("Removed {}", exe.display()));
    Ok(summary.join("\n"))
}

/// Deletes copies moved aside by `install` (next to the exe) and `uninstall` (in %TEMP%). Touches the disk:
/// call on a worker thread.
pub fn cleanup_old() {
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        let _ = std::fs::remove_file(dir.join("rexplorer.old"));
    }
    // Same name as archive_view::extract_dir(): one subfolder per archive version. A day old is past any app that
    // still has one of its files open (another instance, or a scratch profile, may be using the recent ones).
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(24 * 3600);
    for e in std::fs::read_dir(std::env::temp_dir().join("rexplorer-archive")).into_iter().flatten().flatten() {
        if e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t < old) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if name.starts_with("rexplorer-") && name.ends_with(".old") {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::register::read_default;
    use std::os::windows::fs::OpenOptionsExt;

    #[test]
    fn replace_moves_a_locked_target_aside() {
        let dir = std::env::temp_dir().join(format!("rexplorer-install-replace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (src, target) = (dir.join("src.exe"), dir.join(EXE));
        std::fs::write(&src, "new").unwrap();
        std::fs::write(&target, "old").unwrap();
        {
            // Like a running image: readable and renamable, not writable (FILE_SHARE_READ | FILE_SHARE_DELETE).
            let _lock = std::fs::OpenOptions::new().read(true).share_mode(0x1 | 0x4).open(&target).unwrap();
            replace(&src, &target).unwrap();
        }
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(dir.join("rexplorer.old")).unwrap(), "old");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_then_uninstall_under_a_test_base() {
        let base = format!(r"Software\rexplorer-test\install-{}", std::process::id());
        let dir = std::env::temp_dir().join(format!("rexplorer-install-{}", std::process::id())).join("app");
        let _ = std::fs::remove_dir_all(&dir);
        install(&dir, &base).unwrap();
        assert!(dir.join(EXE).is_file() && is_installed(&dir));
        for (k, _) in register::command_keys(&classes(&base)) {
            assert!(read_default(&k).is_some_and(|v| v.contains(&dir.display().to_string())), "{k}");
        }
        assert!(read_default(&HSTRING::from(app_paths(&base))).is_some());
        // A second install (an update) keeps the record of what the first one created.
        install(&dir, &base).unwrap();
        // Another program takes the Drive verb meanwhile: uninstall leaves it.
        let drive = HSTRING::from(format!(r"{}\Drive\shell\open\command", classes(&base)));
        set_sz(&drive, PCWSTR::null(), r"C:\other.exe %1").unwrap();
        let s = uninstall(&dir).unwrap();
        assert!(s.contains("another program"), "{s}");
        assert_eq!(read_default(&drive).as_deref(), Some(r"C:\other.exe %1"));
        assert!(read_default(&HSTRING::from(app_paths(&base))).is_none());
        assert!(!key_exists(&format!(r"{}\CLSID", classes(&base))));
        assert!(!dir.exists());
        let _ = unsafe {
            windows::Win32::System::Registry::RegDeleteTreeW(
                windows::Win32::System::Registry::HKEY_CURRENT_USER,
                &HSTRING::from(base.as_str()),
            )
        };
        delete_empty(r"Software\rexplorer-test");
        assert!(uninstall(&dir).is_err(), "nothing left to uninstall");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }
}
