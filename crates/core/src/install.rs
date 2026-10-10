//! `--install` / `--uninstall`: rexplorer as the per-user file manager, in place of Explorer's folder windows
//! (never the desktop shell).
//!
//! `install` copies the running exe to `<dir>\rexplorer.exe`, registers the copy (register.rs: Folder, Directory,
//! Drive and File Explorer's CLSID verbs), points `App Paths\explorer.exe` at it (ShellExecute callers of
//! `explorer.exe` by bare name; CreateProcess callers still get Explorer), adds a Start menu shortcut and a Startup
//! shortcut to `rexplorer --desktop` (the desktop's context menu; both on the real base only), and records every
//! key, value and file it created in `<dir>\install.txt`. `uninstall` reverts what
//! install.txt lists and leaves other programs' values alone. Everything lives under HKCU: no elevation.
//!
//! `install` journals each change before making it (a value's prior data, the keys and folders it creates, a
//! replaced file moved aside to `<path>.bak`), so a failed install hands back a `Rollback` that restores the
//! previous setup exactly.
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{E_FAIL, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyValueW};
use windows::Win32::UI::Shell::{
    FOLDERID_Programs, FOLDERID_Startup, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink,
};
use windows::core::{Error, HSTRING, Interface, PCWSTR, Result, w};

use crate::com::{ComInit, take_cotaskmem_string};
use crate::register::{self, delete_empty, is_foreign, key_exists, read_sz, remove_if_ours, set_sz};

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
        s.push_str("\nAdd rexplorer to the Start menu, and show its context menu on the desktop from now on (a small helper \
                    that starts at sign-in).\n");
    }
    s.push_str(
        "\nPer-user only: nothing under HKLM changes and no elevation is needed. The desktop itself, the taskbar and \
         other programs' Open / Save dialogs stay Explorer's. Undo with rexplorer --uninstall or More > Uninstall.",
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

/// One change `install` made, and how to revert it.
#[derive(Debug)]
enum Undo {
    /// `prior` None: the value did not exist. `name` "" is the default value.
    Value { key: String, name: &'static str, prior: Option<String> },
    /// A key install created: deleted again once empty.
    Key(String),
    /// A folder install created: removed again once empty.
    Dir(PathBuf),
    /// A file install wrote; `backup` is the file it replaced, moved aside.
    File { path: PathBuf, backup: Option<PathBuf> },
}

/// The changes of a failed install, oldest first: `run` reverts them.
#[derive(Debug)]
pub struct Rollback(Vec<Undo>);

/// A failed install: what went wrong and how to revert what it had changed by then.
#[derive(Debug)]
pub struct Failed {
    pub error: Error,
    pub rollback: Rollback,
}

#[cfg(test)]
thread_local!(static FAIL_AT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) });

/// Test seam: fails the step named by `FAIL_AT` once its undo is journaled, before it writes.
fn step(name: &str) -> Result<()> {
    #[cfg(test)]
    if FAIL_AT.get() == Some(name) {
        return Err(fail(format!("test failure at {name:?}")));
    }
    let _ = name;
    Ok(())
}

fn backup_of(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".bak");
    s.into()
}

impl Rollback {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Sets the `name` value of `HKCU\key`, journaling its prior value and every key the write creates.
    fn set(&mut self, key: &str, name: &'static str, value: &str) -> Result<()> {
        let (k, n) = (HSTRING::from(key), HSTRING::from(name));
        let mut created = vec![];
        let mut p = key;
        while !key_exists(p) {
            created.push(Undo::Key(p.to_string()));
            let Some((parent, _)) = p.rsplit_once('\\') else { break };
            p = parent;
        }
        // Top-most first, so the reverse replay deletes the deepest first.
        self.0.extend(created.into_iter().rev());
        self.0.push(Undo::Value { key: key.to_string(), name, prior: read_sz(&k, PCWSTR(n.as_ptr())) });
        step(name)?;
        set_sz(&k, PCWSTR(n.as_ptr()), value)
    }

    /// Creates `dir` and its missing parents.
    fn create_dir(&mut self, dir: &Path) -> Result<()> {
        let created: Vec<&Path> = dir.ancestors().take_while(|d| !d.as_os_str().is_empty() && !d.exists()).collect();
        self.0.extend(created.into_iter().rev().map(|d| Undo::Dir(d.to_path_buf())));
        step("dir")?;
        std::fs::create_dir_all(dir).map_err(|e| fail(format!("Cannot create {}: {e}", dir.display())))
    }

    /// Writes `path` with `write`, first moving an existing file aside to `<path>.bak` (renaming works even for a
    /// running exe).
    fn file(&mut self, path: &Path, write: impl FnOnce() -> Result<()>) -> Result<()> {
        let backup = if path.exists() {
            let bak = backup_of(path);
            let _ = std::fs::remove_file(&bak);
            // The rename is atomic, so it is journaled once done: an undo for a failed rename would delete `path`.
            std::fs::rename(path, &bak).map_err(|e| fail(format!("Cannot move {} aside: {e}", path.display())))?;
            Some(bak)
        } else {
            None
        };
        self.0.push(Undo::File { path: path.to_path_buf(), backup });
        step(&path.file_name().unwrap_or_default().to_string_lossy())?;
        write()
    }

    /// After a successful install, or a failed one whose changes are kept: deletes the backups. A running old exe
    /// cannot be deleted; it becomes `<dir>\rexplorer.old`, which `cleanup_old` deletes at the next start.
    pub fn commit(self, dir: &Path) {
        for u in self.0 {
            if let Undo::File { backup: Some(bak), .. } = u
                && std::fs::remove_file(&bak).is_err()
            {
                let old = dir.join("rexplorer.old");
                let _ = std::fs::remove_file(&old);
                let _ = std::fs::rename(&bak, &old);
            }
        }
    }

    /// Reverts every change, newest first, and keeps going past failures. Ok(summary), or Err listing what could
    /// not be restored.
    pub fn run(self) -> Result<String> {
        let mut failed = vec![];
        for u in self.0.into_iter().rev() {
            let (ok, what) = match u {
                Undo::Value { key, name, prior } => {
                    let (k, n) = (HSTRING::from(&key), HSTRING::from(name));
                    let name = if name.is_empty() { "(default)" } else { name };
                    match prior {
                        Some(v) => (
                            set_sz(&k, PCWSTR(n.as_ptr()), &v).is_ok(),
                            format!("HKCU\\{key} {name}: set it back to \"{v}\""),
                        ),
                        None => {
                            let err = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &k, PCWSTR(n.as_ptr())) };
                            let ok = err.is_ok() || err == ERROR_FILE_NOT_FOUND || err == ERROR_PATH_NOT_FOUND;
                            (ok, format!("HKCU\\{key} {name}: delete it"))
                        }
                    }
                }
                Undo::Key(k) => (!key_exists(&k) || delete_empty(&k), format!("HKCU\\{k}: delete it once empty")),
                Undo::Dir(d) => {
                    (!d.exists() || std::fs::remove_dir(&d).is_ok(), format!("{}: delete it once empty", d.display()))
                }
                Undo::File { path, backup } => {
                    let gone = !path.exists() || std::fs::remove_file(&path).is_ok();
                    match backup {
                        Some(b) => (
                            gone && std::fs::rename(&b, &path).is_ok(),
                            format!("{}: replace it with {}", path.display(), b.display()),
                        ),
                        None => (gone, format!("{}: delete it", path.display())),
                    }
                }
            };
            if !ok {
                failed.push(what);
            }
        }
        register::notify();
        if failed.is_empty() {
            Ok("Rolled back: everything is as it was before the install.".to_string())
        } else {
            Err(fail(format!("Roll back could not restore these; fix them by hand:\n\n{}", failed.join("\n"))))
        }
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

fn known_folder(id: &windows::core::GUID) -> Result<PathBuf> {
    let p = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None)? };
    Ok(PathBuf::from(unsafe { take_cotaskmem_string(p) }))
}

fn shortcut(target: &Path, args: &str, lnk: &Path) -> Result<()> {
    let _com = ComInit::sta();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(target.as_os_str()))?;
        link.SetArguments(&HSTRING::from(args))?;
        link.SetDescription(w!("rexplorer file manager"))?;
        link.cast::<IPersistFile>()?.Save(&HSTRING::from(lnk.as_os_str()), true)
    }
}

/// Installs the running exe into `dir` and makes it the file manager under `HKCU\<reg_base>`. Returns a summary.
/// Refuses, changing nothing, when another program already overrides one of the entry points. On failure the
/// `Failed` carries the rollback of what had changed by then.
pub fn install(dir: &Path, reg_base: &str) -> std::result::Result<String, Failed> {
    let mut journal = Rollback(vec![]);
    match apply(dir, reg_base, &mut journal) {
        Ok(summary) => {
            journal.commit(dir);
            Ok(summary)
        }
        Err(error) => Err(Failed { error, rollback: journal }),
    }
}

fn apply(dir: &Path, reg_base: &str, journal: &mut Rollback) -> Result<String> {
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
    let mut summary = vec![];
    let result = (|| -> Result<()> {
        if !dir.exists() {
            push(&mut rec, format!("dir {}", dir.display()));
            journal.create_dir(dir)?;
        }
        let src = std::env::current_exe().map_err(|e| fail(format!("Cannot find the running exe: {e}")))?;
        push(&mut rec, format!("file {}", target.display()));
        if !same_file(&src, &target) {
            let copy = || std::fs::copy(&src, &target).map(|_| ());
            journal.file(&target, || copy().map_err(|e| fail(format!("Cannot copy to {}: {e}", target.display()))))?;
        }
        summary.push(format!("Copied to {}", target.display()));
        for k in &commands {
            push(&mut rec, format!("value {k}"));
        }
        for (k, name, value) in register::writes(&classes, &target) {
            journal.set(&k.to_string(), name, &value)?;
        }
        register::notify();
        summary.push("Folders, drives and Win+E open rexplorer".to_string());
        push(&mut rec, format!("value {ap}"));
        journal.set(&ap, "", &target.display().to_string())?;
        journal.set(&ap, "Path", &dir.display().to_string())?;
        summary.push("Programs that start explorer.exe by name get rexplorer".to_string());
        // Under a test base the real Start menu is left alone.
        if reg_base == REAL_REG_BASE {
            let lnk = known_folder(&FOLDERID_Programs)?.join("rexplorer.lnk");
            push(&mut rec, format!("file {}", lnk.display()));
            journal.file(&lnk, || shortcut(&target, "", &lnk))?;
            summary.push("Start menu shortcut added".to_string());
            let lnk = known_folder(&FOLDERID_Startup)?.join("rexplorer desktop menu.lnk");
            push(&mut rec, format!("file {}", lnk.display()));
            journal.file(&lnk, || shortcut(&target, "--desktop", &lnk))?;
            summary.push("The desktop shows rexplorer's context menu (started now and at every sign-in)".to_string());
        }
        Ok(())
    })();
    // Written even after a failure, so that keeping the changes leaves --uninstall able to revert them.
    let record = dir.join(RECORD);
    let wrote = journal.file(&record, || {
        std::fs::write(&record, rec.join("\n") + "\n")
            .map_err(|e| fail(format!("Cannot write {}: {e}", record.display())))
    });
    result?;
    wrote?;
    step("end")?;
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
    use windows::Win32::System::Registry::RegDeleteTreeW;

    /// A scratch registry root (its own, so parallel tests never share a key a rollback deletes) and install dir.
    struct Scratch {
        root: String,
        base: String,
        dir: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Self {
            let root = format!(r"Software\rexplorer-test-rollback-{tag}-{}", std::process::id());
            let dir = std::env::temp_dir().join(format!("rexplorer-rollback-{tag}-{}", std::process::id())).join("app");
            let s = Scratch { base: format!(r"{root}\Software"), root, dir };
            s.clean();
            s
        }

        fn clean(&self) {
            let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(&self.root)) };
            let _ = std::fs::remove_dir_all(self.dir.parent().unwrap());
        }

        /// Every value an install writes that exists now, as `key\name=data` lines.
        fn registry(&self) -> Vec<String> {
            let mut keys: Vec<String> =
                register::command_keys(&classes(&self.base)).iter().map(|(k, _)| k.to_string()).collect();
            keys.push(app_paths(&self.base));
            let mut out = vec![];
            for k in keys {
                for name in ["", "DelegateExecute", "Path"] {
                    let n = HSTRING::from(name);
                    if let Some(v) = read_sz(&HSTRING::from(&k), PCWSTR(n.as_ptr())) {
                        out.push(format!(r"{k}\{name}={v}"));
                    }
                }
            }
            out
        }

        fn failing_install(&self, at: &'static str) -> Failed {
            FAIL_AT.set(Some(at));
            let r = install(&self.dir, &self.base);
            FAIL_AT.set(None);
            r.expect_err("the install was made to fail")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            self.clean();
        }
    }

    #[test]
    fn rollback_after_a_fresh_install_removes_everything() {
        let s = Scratch::new("fresh");
        let failed = s.failing_install("end");
        assert!(s.dir.join(EXE).is_file() && is_installed(&s.dir), "the install got to its end");
        assert_eq!(s.registry().len(), 12, "{:?}", s.registry());
        failed.rollback.run().unwrap();
        assert!(!key_exists(&s.root), "every key the install created is gone");
        assert!(!s.dir.parent().unwrap().exists(), "the install dir and the parent it created are gone");
    }

    #[test]
    fn rollback_after_an_update_restores_the_previous_install() {
        let s = Scratch::new("update");
        install(&s.dir, &s.base).unwrap();
        // The previous install, made distinguishable from what the update writes.
        std::fs::write(s.dir.join(EXE), "previous exe").unwrap();
        std::fs::write(s.dir.join(RECORD), "previous record\n").unwrap();
        let ap = HSTRING::from(app_paths(&s.base));
        set_sz(&ap, w!("Path"), r"C:\previous\rexplorer").unwrap();
        let before = s.registry();
        // Fails at install.txt, the last step: the exe and every value are rewritten by then.
        let failed = s.failing_install(RECORD);
        assert_ne!(std::fs::read(s.dir.join(EXE)).unwrap(), b"previous exe", "the update replaced the exe");
        assert_ne!(s.registry(), before, "the update rewrote values");
        failed.rollback.run().unwrap();
        assert_eq!(std::fs::read(s.dir.join(EXE)).unwrap(), b"previous exe");
        assert_eq!(std::fs::read_to_string(s.dir.join(RECORD)).unwrap(), "previous record\n");
        assert_eq!(s.registry(), before);
        assert_eq!(files(&s.dir), ["install.txt", "rexplorer.exe"], "no backups left");
    }

    #[test]
    fn rollback_restores_a_previous_registration_instead_of_deleting_it() {
        let s = Scratch::new("prior");
        register::register(&classes(&s.base), Path::new(r"C:\elsewhere\rexplorer.exe")).unwrap();
        let before = s.registry();
        assert_eq!(before.len(), 10);
        let failed = s.failing_install("Path");
        assert!(s.registry().iter().any(|v| v.contains(&s.dir.display().to_string())), "the install rewrote values");
        failed.rollback.run().unwrap();
        assert_eq!(s.registry(), before);
        assert!(!key_exists(&app_paths(&s.base)) && !s.dir.exists());
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> =
            std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn a_successful_update_leaves_no_backups() {
        let s = Scratch::new("success");
        install(&s.dir, &s.base).unwrap();
        install(&s.dir, &s.base).unwrap();
        assert_eq!(files(&s.dir), ["install.txt", "rexplorer.exe"]);
    }

    #[test]
    fn a_successful_update_over_a_running_exe_moves_it_to_old() {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn LoadLibraryExW(name: PCWSTR, file: isize, flags: u32) -> isize;
            fn FreeLibrary(module: isize) -> i32;
        }
        let s = Scratch::new("running");
        install(&s.dir, &s.base).unwrap();
        // Mapped as an image, like a running exe: it can be renamed but not deleted.
        let image = unsafe { LoadLibraryExW(PCWSTR(HSTRING::from(s.dir.join(EXE).as_os_str()).as_ptr()), 0, 0x20) };
        assert_ne!(image, 0);
        install(&s.dir, &s.base).unwrap();
        unsafe { FreeLibrary(image) };
        assert_eq!(files(&s.dir), ["install.txt", "rexplorer.exe", "rexplorer.old"]);
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
