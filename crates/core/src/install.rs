//! `--install` / `--uninstall`: rexplorer as the per-user file manager, in place of Explorer's folder windows
//! (never the desktop shell).
//!
//! `install` copies the running exe to `<dir>\rexplorer.exe`, registers the copy (register.rs: Folder, Directory,
//! Drive and File Explorer's CLSID verbs), points `App Paths\explorer.exe` at it (ShellExecute callers of
//! `explorer.exe` by bare name; CreateProcess callers still get Explorer), adds a Start menu shortcut and a Startup
//! shortcut to `rexplorer --desktop` (the desktop's context menu; on the real base, or under a test base when
//! `REXPLORER_PROGRAMS_DIR` and `REXPLORER_STARTUP_DIR` name scratch folders), and records every key, value, folder
//! and file it created in `<dir>\install.txt`. `uninstall` reverts what
//! install.txt lists and leaves other programs' values alone. Everything lives under HKCU: no elevation.
//!
//! `install` journals each change before making it (a value's prior data, the keys and folders it creates, a
//! replaced file moved aside to `<path>.bak`), so a failed install hands back a `Rollback` that restores the
//! previous setup exactly.
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{E_FAIL, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyValueW};
use windows::Win32::UI::Shell::{
    FOLDERID_Programs, FOLDERID_Startup, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink,
};
use windows::core::{Error, HSTRING, Interface, PCWSTR, Result};

use crate::com::{ComInit, take_cotaskmem_string};
use crate::i18n::{tr, trf};
use crate::register::{self, delete_empty, is_foreign, key_exists, read_sz, remove_if_ours, set_sz};

/// The real registry base, relative to HKCU.
pub const REAL_REG_BASE: &str = "Software";
const RECORD: &str = "install.txt";
const EXE: &str = "rexplorer.exe";
/// The previous exe, moved aside by an update while it ran.
const OLD: &str = "rexplorer.old";

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
    let ex = "explorer.exe";
    let mut s = format!(
        "{}\n\n{}\n\
         HKCU\\{c}\\{{Folder, Directory, Drive}}\\shell\\open\\command\n\
         HKCU\\{c}\\CLSID\\{}\\shell\\{{open, opennewwindow}}\\command\n\n\
         {}\nHKCU\\{}\n",
        trf("inst.plan.copy", &[("path", &dir.join(EXE).display())]),
        tr("inst.plan.open"),
        register::EXPLORER_CLSID,
        trf("inst.plan.app_paths", &[("explorer", &ex)]),
        app_paths(reg_base)
    );
    if reg_base == REAL_REG_BASE {
        s.push_str(&format!("\n{}\n", tr("inst.plan.start")));
    }
    s.push_str(&format!("\n{}", trf("inst.plan.footer", &[("explorer", &ex), ("undo", &"rexplorer --uninstall")])));
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
        std::fs::create_dir_all(dir).map_err(|e| fail(trf("inst.err.create_dir", &[("path", &dir.display()), ("error", &e)])))
    }

    /// Writes `path` with `write`, first moving an existing file aside to `<path>.bak` (renaming works even for a
    /// running exe).
    fn file(&mut self, path: &Path, write: impl FnOnce() -> Result<()>) -> Result<()> {
        let backup = if path.exists() {
            let bak = backup_of(path);
            let _ = std::fs::remove_file(&bak);
            // The rename is atomic, so it is journaled once done: an undo for a failed rename would delete `path`.
            std::fs::rename(path, &bak).map_err(|e| fail(trf("inst.err.move_aside", &[("path", &path.display()), ("error", &e)])))?;
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
                let old = dir.join(OLD);
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
                    let name = if name.is_empty() { tr("inst.undo.default_value") } else { name };
                    match prior {
                        Some(v) => (
                            set_sz(&k, PCWSTR(n.as_ptr()), &v).is_ok(),
                            trf("inst.undo.set_back", &[("key", &format!("HKCU\\{key}")), ("name", &name), ("value", &v)]),
                        ),
                        None => {
                            let err = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &k, PCWSTR(n.as_ptr())) };
                            let ok = err.is_ok() || err == ERROR_FILE_NOT_FOUND || err == ERROR_PATH_NOT_FOUND;
                            (ok, trf("inst.undo.delete", &[("key", &format!("HKCU\\{key}")), ("name", &name)]))
                        }
                    }
                }
                Undo::Key(k) => (!key_exists(&k) || delete_empty(&k), trf("inst.undo.delete_key", &[("key", &format!("HKCU\\{k}"))])),
                Undo::Dir(d) => {
                    (!d.exists() || std::fs::remove_dir(&d).is_ok(), trf("inst.undo.delete_dir", &[("path", &d.display())]))
                }
                Undo::File { path, backup } => {
                    let gone = !path.exists() || std::fs::remove_file(&path).is_ok();
                    match backup {
                        Some(b) => (
                            gone && std::fs::rename(&b, &path).is_ok(),
                            trf("inst.undo.restore_file", &[("path", &path.display()), ("backup", &b.display())]),
                        ),
                        None => (gone, trf("inst.undo.delete_file", &[("path", &path.display())])),
                    }
                }
            };
            if !ok {
                failed.push(what);
            }
        }
        register::notify();
        if failed.is_empty() {
            Ok(tr("inst.rolled_back").to_string())
        } else {
            Err(fail(trf("inst.rollback_failed", &[("list", &failed.join("\n"))])))
        }
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

pub fn known_folder(id: &windows::core::GUID) -> Result<PathBuf> {
    let p = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None)? };
    Ok(PathBuf::from(unsafe { take_cotaskmem_string(p) }))
}

fn shortcut(target: &Path, args: &str, lnk: &Path) -> Result<()> {
    let _com = ComInit::sta();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(target.as_os_str()))?;
        link.SetArguments(&HSTRING::from(args))?;
        link.SetDescription(&HSTRING::from(tr("inst.shortcut_desc")))?;
        link.cast::<IPersistFile>()?.Save(&HSTRING::from(lnk.as_os_str()), true)
    }
}

/// Where the Start menu and Startup shortcuts go: (Programs, Startup). `programs` / `startup` are the undocumented
/// `REXPLORER_PROGRAMS_DIR` / `REXPLORER_STARTUP_DIR` overrides for tests. None under a test base without both:
/// the real Start menu is left alone.
fn shortcut_dirs(reg_base: &str, programs: Option<OsString>, startup: Option<OsString>) -> Result<Option<(PathBuf, PathBuf)>> {
    match (programs, startup) {
        (Some(p), Some(s)) => Ok(Some((p.into(), s.into()))),
        _ if reg_base != REAL_REG_BASE => Ok(None),
        (p, s) => {
            let or_known = |o: Option<OsString>, id: &windows::core::GUID| o.map_or_else(|| known_folder(id), |d| Ok(d.into()));
            Ok(Some((or_known(p, &FOLDERID_Programs)?, or_known(s, &FOLDERID_Startup)?)))
        }
    }
}

/// Installs the running exe into `dir` and makes it the file manager under `HKCU\<reg_base>`. Returns a summary.
/// Refuses, changing nothing, when another program already overrides one of the entry points. On failure the
/// `Failed` carries the rollback of what had changed by then.
pub fn install(dir: &Path, reg_base: &str) -> std::result::Result<String, Failed> {
    let links = shortcut_dirs(reg_base, std::env::var_os("REXPLORER_PROGRAMS_DIR"), std::env::var_os("REXPLORER_STARTUP_DIR"))
        .map_err(|error| Failed { error, rollback: Rollback(vec![]) })?;
    install_to(dir, reg_base, links.as_ref())
}

fn install_to(dir: &Path, reg_base: &str, links: Option<&(PathBuf, PathBuf)>) -> std::result::Result<String, Failed> {
    let mut journal = Rollback(vec![]);
    match apply(dir, reg_base, links, &mut journal) {
        Ok(summary) => {
            journal.commit(dir);
            Ok(summary)
        }
        Err(error) => Err(Failed { error, rollback: journal }),
    }
}

fn apply(dir: &Path, reg_base: &str, links: Option<&(PathBuf, PathBuf)>, journal: &mut Rollback) -> Result<String> {
    let classes = classes(reg_base);
    let ap = app_paths(reg_base);
    if let Some(k) = register::foreign_override(&classes) {
        return Err(fail(trf("inst.err.overrides", &[("key", &format!("HKCU\\{k}"))])));
    }
    if is_foreign(&HSTRING::from(&ap)) {
        return Err(fail(trf("inst.err.answers", &[("key", &format!("HKCU\\{ap}"))])));
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
            // Deepest first, the order uninstall removes them in (%LOCALAPPDATA%\Programs too, on a fresh profile).
            for d in dir.ancestors().take_while(|d| !d.as_os_str().is_empty() && !d.exists()) {
                push(&mut rec, format!("dir {}", d.display()));
            }
            journal.create_dir(dir)?;
        }
        let src = std::env::current_exe().map_err(|e| fail(trf("inst.err.no_exe", &[("error", &e)])))?;
        push(&mut rec, format!("file {}", target.display()));
        if !same_file(&src, &target) {
            let copy = || std::fs::copy(&src, &target).map(|_| ());
            journal.file(&target, || copy().map_err(|e| fail(trf("inst.err.copy", &[("path", &target.display()), ("error", &e)]))))?;
        }
        summary.push(trf("inst.sum.copied", &[("path", &target.display())]));
        for k in &commands {
            push(&mut rec, format!("value {k}"));
        }
        for (k, name, value) in register::writes(&classes, &target) {
            journal.set(&k.to_string(), name, &value)?;
        }
        register::notify();
        summary.push(tr("inst.sum.open").to_string());
        push(&mut rec, format!("value {ap}"));
        journal.set(&ap, "", &target.display().to_string())?;
        journal.set(&ap, "Path", &dir.display().to_string())?;
        summary.push(trf("inst.sum.app_paths", &[("explorer", &"explorer.exe")]));
        if let Some((programs, startup)) = links {
            let lnk = programs.join("rexplorer.lnk");
            push(&mut rec, format!("file {}", lnk.display()));
            journal.file(&lnk, || shortcut(&target, "", &lnk))?;
            summary.push(tr("inst.sum.start_menu").to_string());
            let lnk = startup.join("rexplorer desktop menu.lnk");
            push(&mut rec, format!("file {}", lnk.display()));
            journal.file(&lnk, || shortcut(&target, "--desktop", &lnk))?;
            summary.push(tr("inst.sum.desktop").to_string());
        }
        Ok(())
    })();
    // Written even after a failure, so that keeping the changes leaves --uninstall able to revert them.
    let record = dir.join(RECORD);
    let wrote = journal.file(&record, || {
        std::fs::write(&record, rec.join("\n") + "\n")
            .map_err(|e| fail(trf("inst.err.write", &[("path", &record.display()), ("error", &e)])))
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
        return Err(fail(trf("inst.err.not_installed", &[("dir", &dir.display()), ("record", &RECORD)])));
    }
    let lines = |kind: &str| -> Vec<String> {
        rec.iter().filter_map(|l| l.strip_prefix(kind).and_then(|r| r.strip_prefix(' ')).map(str::to_string)).collect()
    };
    let mut summary = vec![];
    for k in lines("value") {
        if !remove_if_ours(&HSTRING::from(&k))? {
            summary.push(trf("inst.un.left", &[("key", &format!("HKCU\\{k}"))]));
        }
    }
    let mut keys = lines("key");
    keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
    for k in &keys {
        delete_empty(k);
    }
    register::notify();
    summary.insert(0, trf("inst.un.restored", &[("explorer", &"explorer.exe")]));
    let exe = dir.join(EXE);
    for f in lines("file").iter().map(PathBuf::from).filter(|f| *f != exe) {
        let _ = std::fs::remove_file(f);
    }
    let _ = std::fs::remove_file(dir.join(RECORD));
    // The exe, and the previous one an update moved aside while it ran.
    for (i, f) in [exe.clone(), dir.join(OLD)].iter().enumerate() {
        if f.exists() && std::fs::remove_file(f).is_err() {
            // Running: it can still be moved out of the way; the next rexplorer start deletes it.
            let aside = std::env::temp_dir().join(format!("rexplorer-{}-{i}.old", std::process::id()));
            if std::fs::rename(f, &aside).is_err() {
                summary.push(trf("inst.un.close", &[("path", &f.display())]));
            }
        }
    }
    for d in lines("dir") {
        let _ = std::fs::remove_dir(d);
    }
    summary.push(trf("inst.un.removed", &[("path", &exe.display())]));
    Ok(summary.join("\n"))
}

fn process_alive(pid: u32) -> bool {
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: plain handle open and close; a pid that no longer exists fails the open.
    unsafe {
        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
            .map(|h| {
                let _ = windows::Win32::Foundation::CloseHandle(h);
            })
            .is_ok()
    }
}

/// Deletes copies moved aside by `install` (next to the exe) and `uninstall` (in %TEMP%). Touches the disk:
/// call on a worker thread.
pub fn cleanup_old() {
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        let _ = std::fs::remove_file(dir.join(OLD));
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
        // archive_view::session_dir() of an instance that crashed or was killed: its exit cleanup never ran.
        if let Some(pid) = name.strip_prefix("rexplorer-show-").and_then(|p| p.parse::<u32>().ok())
            && !process_alive(pid)
        {
            let _ = std::fs::remove_dir_all(e.path());
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
        set_sz(&ap, windows::core::w!("Path"), r"C:\previous\rexplorer").unwrap();
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

    #[test]
    fn plan_and_summaries_keep_paths_and_commands_verbatim() {
        let dir = Path::new(r"C:\x\app");
        let real = plan(dir, REAL_REG_BASE);
        for want in [r"Copy rexplorer to C:\x\app\rexplorer.exe", r"HKCU\Software\Classes\{Folder, Directory, Drive}\shell\open\command", register::EXPLORER_CLSID, r"HKCU\Software\Microsoft\Windows\CurrentVersion\App Paths\explorer.exe", "Start menu", "rexplorer --uninstall", "HKLM"] {
            assert!(real.contains(want), "{want}\n{real}");
        }
        assert!(!plan(dir, r"Software\rexplorer-test").contains("Start menu"), "a test base leaves the Start menu alone");
        let s = Scratch::new("summary");
        let sum = install(&s.dir, &s.base).unwrap();
        assert!(sum.starts_with("Copied to ") && sum.contains("Programs that start explorer.exe by name get rexplorer"), "{sum}");
        let un = uninstall(&s.dir).unwrap();
        assert!(un.contains("open Windows Explorer again") && un.contains("Removed "), "{un}");
        let err = uninstall(&s.dir).unwrap_err().message();
        assert!(err.contains("is not installed in") && err.contains("install.txt"), "{err}");
    }

    #[test]
    fn rollback_summary_lists_what_it_could_not_restore() {
        use std::os::windows::fs::OpenOptionsExt;
        let s = Scratch::new("summary-fail");
        let failed = s.failing_install("end");
        // Held open without sharing, so deleting it fails and the rollback lists it.
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(s.dir.join(EXE)).unwrap();
        let err = failed.rollback.run().unwrap_err().message();
        drop(held);
        assert!(err.starts_with("Roll back could not restore these; fix them by hand:"), "{err}");
        assert!(err.contains("rexplorer.exe: delete it"), "{err}");
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

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryExW(name: PCWSTR, file: isize, flags: u32) -> isize;
        fn FreeLibrary(module: isize) -> i32;
    }

    /// Maps `exe` as an image, like a running exe: it can be renamed but not deleted until `FreeLibrary`.
    fn run_image(exe: &Path) -> isize {
        let image = unsafe { LoadLibraryExW(PCWSTR(HSTRING::from(exe.as_os_str()).as_ptr()), 0, 0x20) };
        assert_ne!(image, 0);
        image
    }

    #[test]
    fn a_successful_update_over_a_running_exe_moves_it_to_old() {
        let s = Scratch::new("running");
        install(&s.dir, &s.base).unwrap();
        let image = run_image(&s.dir.join(EXE));
        install(&s.dir, &s.base).unwrap();
        unsafe { FreeLibrary(image) };
        assert_eq!(files(&s.dir), ["install.txt", "rexplorer.exe", "rexplorer.old"]);
    }

    #[test]
    fn shortcut_folders_come_from_the_overrides_or_stay_off_under_a_test_base() {
        let (p, s) = (|| Some(OsString::from(r"C:\scratch\Programs")), || Some(OsString::from(r"C:\scratch\Startup")));
        let both = Some((PathBuf::from(r"C:\scratch\Programs"), PathBuf::from(r"C:\scratch\Startup")));
        assert_eq!(shortcut_dirs(r"Software\rexplorer-test", p(), s()).unwrap(), both);
        assert_eq!(shortcut_dirs(REAL_REG_BASE, p(), s()).unwrap(), both);
        assert_eq!(shortcut_dirs(r"Software\rexplorer-test", p(), None).unwrap(), None);
        assert_eq!(shortcut_dirs(r"Software\rexplorer-test", None, s()).unwrap(), None);
        assert_eq!(shortcut_dirs(r"Software\rexplorer-test", None, None).unwrap(), None);
    }

    /// #64 and #65 in a scratch folder: install with both shortcuts, update over a running copy, uninstall; every
    /// key, value, shortcut and file the install created is gone.
    #[test]
    fn install_update_over_a_running_copy_and_uninstall_leave_nothing() {
        let s = Scratch::new("lifecycle");
        let scratch = s.dir.parent().unwrap().to_path_buf();
        let links = (scratch.join("Programs"), scratch.join("Startup"));
        std::fs::create_dir_all(&links.0).unwrap();
        std::fs::create_dir_all(&links.1).unwrap();
        let lnks = [links.0.join("rexplorer.lnk"), links.1.join("rexplorer desktop menu.lnk")];
        install_to(&s.dir, &s.base, Some(&links)).unwrap();
        let record = std::fs::read_to_string(s.dir.join(RECORD)).unwrap();
        for l in &lnks {
            assert!(l.is_file() && record.contains(&format!("file {}", l.display())), "{}\n{record}", l.display());
        }
        let image = run_image(&s.dir.join(EXE));
        install_to(&s.dir, &s.base, Some(&links)).unwrap();
        assert_eq!(files(&s.dir), ["install.txt", "rexplorer.exe", "rexplorer.old"], "the running copy is moved aside");
        let me = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        assert!(std::fs::read(s.dir.join(EXE)).unwrap() == me, "the next launch runs the new copy");
        unsafe { FreeLibrary(image) };
        uninstall(&s.dir).unwrap();
        assert!(lnks.iter().all(|l| !l.exists()), "both shortcuts are gone");
        assert_eq!(s.registry(), Vec::<String>::new());
        assert!(!key_exists(&s.root), "every key the install created is gone");
        assert!(!s.dir.exists(), "the exe, the old copy, install.txt and the folder are gone");
    }

    #[test]
    fn install_then_uninstall_under_a_test_base() {
        let base = format!(r"Software\rexplorer-test\install-{}", std::process::id());
        let dir = std::env::temp_dir().join(format!("rexplorer-install-{}", std::process::id())).join("app");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
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
        assert!(!dir.parent().unwrap().exists(), "the folder and the parent install created are gone");
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
