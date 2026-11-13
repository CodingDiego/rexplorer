//! `--install` / `--uninstall` (rexplorer as a normal per-user program) and `--register` / `--unregister` (rexplorer
//! in place of Explorer's folder windows; never the desktop shell). Everything lives under HKCU: no elevation.
//!
//! `install` (standalone) copies the running exe to `<dir>\rexplorer.exe`, adds a Start menu shortcut and an Apps
//! and Features entry (`HKCU\<base>\Microsoft\Windows\CurrentVersion\Uninstall\rexplorer`), and records every key,
//! value, folder and file it created in `<dir>\install.txt`. It writes no folder, drive or App Paths key: Windows
//! Explorer still opens folders.
//!
//! `take_over` (`--register`) is the separate step that makes rexplorer the file manager: register.rs's Folder,
//! Directory, Drive and File Explorer CLSID verbs, `App Paths\explorer.exe` (ShellExecute callers of `explorer.exe`
//! by bare name; CreateProcess callers still get Explorer) and a Startup shortcut to `rexplorer --desktop` (the
//! desktop's context menu). It records them in `<dir>\takeover.txt`; `give_back` (`--unregister`) reverts them, and
//! `uninstall` always gives back first. Shortcuts go to the real Start menu and Startup folders, or under a test base
//! to the folders `REXPLORER_PROGRAMS_DIR` and `REXPLORER_STARTUP_DIR` name (none without both).
//!
//! Both journal each change before making it (a value's prior data of any type, the keys and folders they create, a
//! replaced file moved aside to `<path>.bak`), so a failure hands back a `Rollback` that restores the previous setup
//! exactly. scripts/restore-explorer.ps1 undoes the same records without a working exe; `known_keys` is its fixed
//! list for when the records are gone.
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::E_FAIL;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
use windows::Win32::UI::Shell::{
    FOLDERID_Programs, FOLDERID_Startup, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink,
};
use windows::core::{Error, HSTRING, Interface, PCWSTR, Result};

use crate::com::{ComInit, take_cotaskmem_string};
use crate::i18n::{tr, trf};
use crate::register::{
    self, delete_empty, delete_value, dword, is_foreign, key_exists, read_raw, remove_if_ours, set_raw, show_raw, sz,
};

/// The real registry base, relative to HKCU.
pub const REAL_REG_BASE: &str = "Software";
/// What `install` created (the program).
const RECORD: &str = "install.txt";
/// What `take_over` changed (rexplorer in Explorer's place).
const TAKEOVER: &str = "takeover.txt";
const EXE: &str = "rexplorer.exe";
/// The previous exe, moved aside by an update while it ran.
const OLD: &str = "rexplorer.old";
const START_LNK: &str = "rexplorer.lnk";
const DESKTOP_LNK: &str = "rexplorer desktop menu.lnk";
/// The Apps and Features entry's Publisher, as in the winget manifests.
const PUBLISHER: &str = "CodingDiego";

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

/// The per-user classes key under `reg_base`, which register.rs writes into.
pub fn classes(reg_base: &str) -> String {
    format!(r"{reg_base}\Classes")
}

fn app_paths(reg_base: &str) -> String {
    format!(r"{reg_base}\Microsoft\Windows\CurrentVersion\App Paths\explorer.exe")
}

/// The Apps and Features (Settings > Apps) entry.
fn arp(reg_base: &str) -> String {
    format!(r"{reg_base}\Microsoft\Windows\CurrentVersion\Uninstall\rexplorer")
}

/// Every key rexplorer can write under `HKCU\<reg_base>`. scripts/restore-explorer.ps1 keeps the same list for a
/// lost journal; `restore_script_knows_every_key` ties the two.
pub fn known_keys(reg_base: &str) -> Vec<String> {
    let mut v: Vec<String> = register::command_keys(&classes(reg_base)).into_iter().map(|(k, _)| k.to_string()).collect();
    v.push(app_paths(reg_base));
    v.push(arp(reg_base));
    v
}

pub fn is_installed(dir: &Path) -> bool {
    dir.join(RECORD).is_file()
}

/// What `install` changes, for the confirmation dialog.
pub fn plan(dir: &Path, reg_base: &str) -> String {
    let mut s = format!("{}\n", trf("inst.plan.copy", &[("path", &dir.join(EXE).display())]));
    if reg_base == REAL_REG_BASE {
        s.push_str(&format!("{}\n", tr("inst.plan.start")));
    }
    s.push_str(&format!("\n{}\nHKCU\\{}\n", tr("inst.plan.apps"), arp(reg_base)));
    s.push_str(&format!(
        "\n{}",
        trf("inst.plan.footer", &[("undo", &"rexplorer --uninstall"), ("register", &"rexplorer --register")])
    ));
    s
}

/// What `take_over` changes, for its confirmation dialog.
pub fn takeover_plan(reg_base: &str, exe: &Path) -> String {
    let c = classes(reg_base);
    let ex = "explorer.exe";
    let mut s = format!(
        "{}\n\
         HKCU\\{c}\\{{Folder, Directory, Drive}}\\shell\\open\\command\n\
         HKCU\\{c}\\CLSID\\{}\\shell\\{{open, opennewwindow}}\\command\n\
         = \"{}\"\n\n{}\nHKCU\\{}\n",
        tr("inst.plan.open"),
        register::EXPLORER_CLSID,
        exe.display(),
        trf("inst.plan.app_paths", &[("explorer", &ex)]),
        app_paths(reg_base)
    );
    if reg_base == REAL_REG_BASE {
        s.push_str(&format!("\n{}\n", tr("inst.plan.desktop")));
    }
    s.push_str(&format!(
        "\n{}",
        trf("inst.plan.give_back", &[("undo", &"rexplorer --unregister"), ("script", &r"scripts\restore-explorer.ps1")])
    ));
    s
}

fn read_record(dir: &Path, name: &str) -> Vec<String> {
    std::fs::read_to_string(dir.join(name)).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default()
}

/// What the records list as changed, one registry key or file per line, for the uninstall confirmation.
pub fn recorded(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for line in read_record(dir, TAKEOVER).into_iter().chain(read_record(dir, RECORD)) {
        let item = match line.split_once(' ') {
            Some(("value", k)) => format!("HKCU\\{k}"),
            Some(("named", kn)) => format!("HKCU\\{}", kn.rsplit_once('|').map_or(kn, |(k, _)| k)),
            Some(("file", f)) => f.to_string(),
            _ => continue,
        };
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

fn push(rec: &mut Vec<String>, line: String) {
    if !rec.contains(&line) {
        rec.push(line);
    }
}

fn fail(text: String) -> Error {
    Error::new(E_FAIL, text)
}

/// One change `install` or `take_over` made, and how to revert it.
#[derive(Debug)]
enum Undo {
    /// `prior` None: the value did not exist; else its registry type and bytes. `name` "" is the default value.
    Value { key: String, name: &'static str, prior: Option<(u32, Vec<u8>)> },
    /// A key the run created: deleted again once empty.
    Key(String),
    /// A folder the run created: removed again once empty.
    Dir(PathBuf),
    /// A file the run wrote; `backup` is the file it replaced, moved aside.
    File { path: PathBuf, backup: Option<PathBuf> },
}

/// The changes of a failed run, oldest first: `run` reverts them.
#[derive(Debug)]
pub struct Rollback(Vec<Undo>);

/// A failed install or take-over: what went wrong and how to revert what it had changed by then.
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

    /// Sets the `name` value of `HKCU\key` to `(type, bytes)`, journaling its prior value and every key the write
    /// creates.
    fn set(&mut self, key: &str, name: &'static str, (ty, data): (u32, Vec<u8>)) -> Result<()> {
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
        self.0.push(Undo::Value { key: key.to_string(), name, prior: read_raw(&k, PCWSTR(n.as_ptr())) });
        step(name)?;
        set_raw(&k, PCWSTR(n.as_ptr()), ty, &data)
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

    /// After a successful run, or a failed one whose changes are kept: deletes the backups. A running old exe
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
                        Some((ty, data)) => (
                            set_raw(&k, PCWSTR(n.as_ptr()), ty, &data).is_ok(),
                            trf(
                                "inst.undo.set_back",
                                &[("key", &format!("HKCU\\{key}")), ("name", &name), ("value", &show_raw(ty, &data))],
                            ),
                        ),
                        None => (
                            delete_value(&k, PCWSTR(n.as_ptr())).is_ok(),
                            trf("inst.undo.delete", &[("key", &format!("HKCU\\{key}")), ("name", &name)]),
                        ),
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

/// `app_id`: the shortcut carries the process's AppUserModelID (the Start menu entry, so its pin and jump list
/// match the running windows).
fn shortcut(target: &Path, args: &str, lnk: &Path, app_id: bool) -> Result<()> {
    let _com = ComInit::sta();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(target.as_os_str()))?;
        link.SetArguments(&HSTRING::from(args))?;
        link.SetDescription(&HSTRING::from(tr("inst.shortcut_desc")))?;
        if app_id {
            crate::jumplist::stamp_link(&link)?;
        }
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

fn env_shortcut_dirs(reg_base: &str) -> Result<Option<(PathBuf, PathBuf)>> {
    shortcut_dirs(reg_base, std::env::var_os("REXPLORER_PROGRAMS_DIR"), std::env::var_os("REXPLORER_STARTUP_DIR"))
}

/// Runs `apply` with a journal: Ok keeps the changes, Err carries their rollback.
fn journaled(dir: &Path, apply: impl FnOnce(&mut Rollback) -> Result<String>) -> std::result::Result<String, Failed> {
    let mut journal = Rollback(vec![]);
    match apply(&mut journal) {
        Ok(summary) => {
            journal.commit(dir);
            Ok(summary)
        }
        Err(error) => Err(Failed { error, rollback: journal }),
    }
}

/// Records the missing keys on the path to `leaf` (deepest first after sorting at removal).
fn record_missing_keys(rec: &mut Vec<String>, leaf: &str) {
    let mut k = leaf;
    while !key_exists(k) {
        push(rec, format!("key {k}"));
        let Some((parent, _)) = k.rsplit_once('\\') else { break };
        k = parent;
    }
}

fn ensure_dir(dir: &Path, rec: &mut Vec<String>, journal: &mut Rollback) -> Result<()> {
    if !dir.exists() {
        // Deepest first, the order uninstall removes them in (%LOCALAPPDATA%\Programs too, on a fresh profile).
        for d in dir.ancestors().take_while(|d| !d.as_os_str().is_empty() && !d.exists()) {
            push(rec, format!("dir {}", d.display()));
        }
        journal.create_dir(dir)?;
    }
    Ok(())
}

/// Writes the record `<dir>\<name>` even after a failure, so that keeping the changes leaves the undo able to revert
/// them; then reports `result`.
fn save(dir: &Path, name: &str, rec: &[String], journal: &mut Rollback, result: Result<()>) -> Result<()> {
    let record = dir.join(name);
    let wrote = journal.file(&record, || {
        std::fs::write(&record, rec.join("\n") + "\n")
            .map_err(|e| fail(trf("inst.err.write", &[("path", &record.display()), ("error", &e)])))
    });
    result?;
    wrote?;
    step("end")
}

/// Installs the running exe into `dir` as a normal program (exe, Start menu shortcut, Apps and Features entry under
/// `HKCU\<reg_base>`). Returns a summary. On failure the `Failed` carries the rollback of what had changed by then.
pub fn install(dir: &Path, reg_base: &str) -> std::result::Result<String, Failed> {
    let links = env_shortcut_dirs(reg_base).map_err(|error| Failed { error, rollback: Rollback(vec![]) })?;
    install_to(dir, reg_base, links.as_ref().map(|(p, _)| p.as_path()))
}

fn install_to(dir: &Path, reg_base: &str, programs: Option<&Path>) -> std::result::Result<String, Failed> {
    journaled(dir, |journal| apply_program(dir, reg_base, programs, journal))
}

fn apply_program(dir: &Path, reg_base: &str, programs: Option<&Path>, journal: &mut Rollback) -> Result<String> {
    let ap = arp(reg_base);
    let target = dir.join(EXE);
    let mut rec = read_record(dir, RECORD);
    record_missing_keys(&mut rec, &ap);
    let mut summary = vec![];
    let result = (|| -> Result<()> {
        ensure_dir(dir, &mut rec, journal)?;
        let src = std::env::current_exe().map_err(|e| fail(trf("inst.err.no_exe", &[("error", &e)])))?;
        push(&mut rec, format!("file {}", target.display()));
        if !same_file(&src, &target) {
            let copy = || std::fs::copy(&src, &target).map(|_| ());
            journal.file(&target, || copy().map_err(|e| fail(trf("inst.err.copy", &[("path", &target.display()), ("error", &e)]))))?;
        }
        summary.push(trf("inst.sum.copied", &[("path", &target.display())]));
        if let Some(programs) = programs {
            let lnk = programs.join(START_LNK);
            push(&mut rec, format!("file {}", lnk.display()));
            journal.file(&lnk, || shortcut(&target, "", &lnk, true))?;
            summary.push(tr("inst.sum.start_menu").to_string());
        }
        let exe = target.display().to_string();
        let values: [(&'static str, (u32, Vec<u8>)); 8] = [
            ("DisplayName", sz("rexplorer")),
            ("DisplayVersion", sz(env!("CARGO_PKG_VERSION"))),
            ("Publisher", sz(PUBLISHER)),
            ("InstallLocation", sz(&dir.display().to_string())),
            ("DisplayIcon", sz(&exe)),
            ("UninstallString", sz(&format!("\"{exe}\" --uninstall --quiet"))),
            ("NoModify", dword(1)),
            ("NoRepair", dword(1)),
        ];
        for (name, v) in values {
            push(&mut rec, format!("named {ap}|{name}"));
            journal.set(&ap, name, v)?;
        }
        summary.push(trf("inst.sum.apps", &[("uninstall", &"rexplorer --uninstall")]));
        summary.push(trf("inst.sum.still_explorer", &[("register", &"rexplorer --register")]));
        Ok(())
    })();
    save(dir, RECORD, &rec, journal, result)?;
    Ok(summary.join("\n"))
}

/// Makes `exe` the file manager under `HKCU\<reg_base>` (folders, drives, File Explorer's verbs, App Paths, the
/// desktop helper at sign-in), journaled in `<dir>\takeover.txt`. Refuses, changing nothing, when another program
/// already overrides one of the entry points. On failure the `Failed` carries the rollback.
pub fn take_over(dir: &Path, reg_base: &str, exe: &Path) -> std::result::Result<String, Failed> {
    let links = env_shortcut_dirs(reg_base).map_err(|error| Failed { error, rollback: Rollback(vec![]) })?;
    take_over_to(dir, reg_base, exe, links.as_ref().map(|(_, s)| s.as_path()))
}

fn take_over_to(dir: &Path, reg_base: &str, exe: &Path, startup: Option<&Path>) -> std::result::Result<String, Failed> {
    journaled(dir, |journal| apply_takeover(dir, reg_base, exe, startup, journal))
}

fn apply_takeover(dir: &Path, reg_base: &str, exe: &Path, startup: Option<&Path>, journal: &mut Rollback) -> Result<String> {
    let classes = classes(reg_base);
    let ap = app_paths(reg_base);
    if let Some(k) = register::foreign_override(&classes) {
        return Err(fail(trf("inst.err.overrides", &[("key", &format!("HKCU\\{k}"))])));
    }
    if is_foreign(&HSTRING::from(&ap)) {
        return Err(fail(trf("inst.err.answers", &[("key", &format!("HKCU\\{ap}"))])));
    }
    let commands: Vec<String> = register::command_keys(&classes).into_iter().map(|(k, _)| k.to_string()).collect();
    let mut rec = read_record(dir, TAKEOVER);
    for leaf in commands.iter().chain(Some(&ap)) {
        record_missing_keys(&mut rec, leaf);
    }
    let mut summary = vec![];
    let result = (|| -> Result<()> {
        ensure_dir(dir, &mut rec, journal)?;
        for k in &commands {
            push(&mut rec, format!("value {k}"));
        }
        for (k, name, value) in register::writes(&classes, exe) {
            journal.set(&k.to_string(), name, sz(&value))?;
        }
        register::notify();
        summary.push(tr("inst.sum.open").to_string());
        push(&mut rec, format!("value {ap}"));
        journal.set(&ap, "", sz(&exe.display().to_string()))?;
        journal.set(&ap, "Path", sz(&exe.parent().unwrap_or(dir).display().to_string()))?;
        summary.push(trf("inst.sum.app_paths", &[("explorer", &"explorer.exe")]));
        if let Some(startup) = startup {
            let lnk = startup.join(DESKTOP_LNK);
            push(&mut rec, format!("file {}", lnk.display()));
            journal.file(&lnk, || shortcut(exe, "--desktop", &lnk, false))?;
            summary.push(tr("inst.sum.desktop").to_string());
        }
        Ok(())
    })();
    save(dir, TAKEOVER, &rec, journal, result)?;
    Ok(summary.join("\n"))
}

/// Reverts the registry and file lines of a record: values that still name rexplorer, the named values it wrote,
/// then the keys it created (only if empty) and the files other than `keep`. Returns what it left alone.
fn revert(rec: &[String], keep: &[PathBuf]) -> Result<Vec<String>> {
    let lines = |kind: &str| -> Vec<String> {
        rec.iter().filter_map(|l| l.strip_prefix(kind).and_then(|r| r.strip_prefix(' ')).map(str::to_string)).collect()
    };
    let mut left = vec![];
    for k in lines("value") {
        if !remove_if_ours(&HSTRING::from(&k))? {
            left.push(trf("inst.un.left", &[("key", &format!("HKCU\\{k}"))]));
        }
    }
    for kn in lines("named") {
        if let Some((k, n)) = kn.rsplit_once('|') {
            delete_value(&HSTRING::from(k), PCWSTR(HSTRING::from(n).as_ptr()))?;
        }
    }
    let mut keys = lines("key");
    keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
    for k in &keys {
        delete_empty(k);
    }
    for f in lines("file").iter().map(PathBuf::from).filter(|f| !keep.contains(f)) {
        let _ = std::fs::remove_file(f);
    }
    Ok(left)
}

fn remove_dirs(rec: &[String]) {
    for d in rec.iter().filter_map(|l| l.strip_prefix("dir ")) {
        let _ = std::fs::remove_dir(d);
    }
}

/// Gives folders back to Windows Explorer: reverts what `<dir>\takeover.txt` lists. Without it (an older build's
/// `--register`, or a lost record) removes the values of `known_keys` that name rexplorer and the desktop helper's
/// Startup shortcut. Leaves the program installed. Returns a summary.
pub fn give_back(dir: &Path, reg_base: &str) -> Result<String> {
    let rec = read_record(dir, TAKEOVER);
    let mut summary = vec![trf("inst.un.restored", &[("explorer", &"explorer.exe")])];
    if rec.is_empty() {
        register::unregister(&classes(reg_base))?;
        let ap = app_paths(reg_base);
        if !remove_if_ours(&HSTRING::from(&ap))? {
            summary.push(trf("inst.un.left", &[("key", &format!("HKCU\\{ap}"))]));
        }
        if let Ok(Some((_, startup))) = env_shortcut_dirs(reg_base) {
            let _ = std::fs::remove_file(startup.join(DESKTOP_LNK));
        }
    } else {
        summary.extend(revert(&rec, &[])?);
        let _ = std::fs::remove_file(dir.join(TAKEOVER));
        remove_dirs(&rec);
    }
    register::notify();
    Ok(summary.join("\n"))
}

/// Gives folders back to Explorer first, then reverts what `<dir>\install.txt` lists: the Apps and Features entry,
/// the keys install created (only if empty), the files, the exe last. Returns a summary.
pub fn uninstall(dir: &Path, reg_base: &str) -> Result<String> {
    let rec = read_record(dir, RECORD);
    let had_takeover = dir.join(TAKEOVER).is_file();
    let given_back = give_back(dir, reg_base)?;
    if rec.is_empty() {
        return if had_takeover {
            Ok(given_back)
        } else {
            Err(fail(trf("inst.err.not_installed", &[("dir", &dir.display()), ("record", &RECORD)])))
        };
    }
    let exe = dir.join(EXE);
    let mut summary = vec![given_back];
    summary.extend(revert(&rec, std::slice::from_ref(&exe))?);
    register::notify();
    if reg_base == REAL_REG_BASE {
        let _com = crate::com::ComInit::sta();
        let _ = crate::jumplist::delete();
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
    remove_dirs(&rec);
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

fn remove_old_in(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(OLD));
}

/// Deletes copies moved aside by `install` (next to the exe) and `uninstall` (in %TEMP%). Touches the disk:
/// call on a worker thread.
pub fn cleanup_old() {
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        remove_old_in(&dir);
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
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteTreeW};
    use windows::core::w;

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

        /// Every value install or take-over writes that exists now, as `key\name=data` lines.
        fn registry(&self) -> Vec<String> {
            let names = ["", "DelegateExecute", "Path", "DisplayName", "DisplayVersion", "Publisher", "InstallLocation", "DisplayIcon", "UninstallString", "NoModify", "NoRepair"];
            let mut out = vec![];
            for k in known_keys(&self.base) {
                for name in names {
                    let n = HSTRING::from(name);
                    if let Some((ty, data)) = read_raw(&HSTRING::from(&k), PCWSTR(n.as_ptr())) {
                        out.push(format!(r"{k}\{name}={ty}:{}", show_raw(ty, &data)));
                    }
                }
            }
            out
        }

        fn exe(&self) -> PathBuf {
            self.dir.join(EXE)
        }

        fn failing(&self, at: &'static str, takeover: bool) -> Failed {
            FAIL_AT.set(Some(at));
            let r = if takeover { take_over(&self.dir, &self.base, &self.exe()) } else { install(&self.dir, &self.base) };
            FAIL_AT.set(None);
            r.expect_err("the run was made to fail")
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
        let failed = s.failing("end", false);
        assert!(s.exe().is_file() && is_installed(&s.dir), "the install got to its end");
        assert_eq!(s.registry().len(), 8, "{:?}", s.registry());
        failed.rollback.run().unwrap();
        assert!(!key_exists(&s.root), "every key the install created is gone");
        assert!(!s.dir.parent().unwrap().exists(), "the install dir and the parent it created are gone");
    }

    #[test]
    fn rollback_after_an_update_restores_the_previous_install() {
        let s = Scratch::new("update");
        install(&s.dir, &s.base).unwrap();
        // The previous install, made distinguishable from what the update writes, a DWORD included.
        std::fs::write(s.exe(), "previous exe").unwrap();
        std::fs::write(s.dir.join(RECORD), "previous record\n").unwrap();
        let ap = HSTRING::from(arp(&s.base));
        register::set_sz(&ap, w!("DisplayVersion"), "0.0.1").unwrap();
        let (ty, data) = dword(0);
        set_raw(&ap, w!("NoRepair"), ty, &data).unwrap();
        let before = s.registry();
        assert!(before.iter().any(|v| v.ends_with(r"\NoRepair=4:0")), "{before:?}");
        // Fails at install.txt, the last step: the exe and every value are rewritten by then.
        let failed = s.failing(RECORD, false);
        assert_ne!(std::fs::read(s.exe()).unwrap(), b"previous exe", "the update replaced the exe");
        assert_ne!(s.registry(), before, "the update rewrote values");
        failed.rollback.run().unwrap();
        assert_eq!(std::fs::read(s.exe()).unwrap(), b"previous exe");
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
        let failed = s.failing("Path", true);
        assert!(s.registry().iter().any(|v| v.contains(&s.dir.display().to_string())), "the take-over rewrote values");
        failed.rollback.run().unwrap();
        assert_eq!(s.registry(), before);
        assert!(!key_exists(&app_paths(&s.base)) && !s.dir.exists());
    }

    #[test]
    fn plan_and_summaries_keep_paths_and_commands_verbatim() {
        let dir = Path::new(r"C:\x\app");
        let real = plan(dir, REAL_REG_BASE);
        for want in [r"Copy rexplorer to C:\x\app\rexplorer.exe", "Start menu", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\rexplorer", "rexplorer --uninstall", "rexplorer --register", "HKLM"] {
            assert!(real.contains(want), "{want}\n{real}");
        }
        for not in ["Classes", "App Paths", "Win+E"] {
            assert!(!real.contains(not), "the standalone plan changes no association: {not}\n{real}");
        }
        assert!(!plan(dir, r"Software\rexplorer-test").contains("Start menu"), "a test base leaves the Start menu alone");
        let tp = takeover_plan(REAL_REG_BASE, &dir.join(EXE));
        for want in [r"HKCU\Software\Classes\{Folder, Directory, Drive}\shell\open\command", register::EXPLORER_CLSID, r"HKCU\Software\Microsoft\Windows\CurrentVersion\App Paths\explorer.exe", r#""C:\x\app\rexplorer.exe""#, "rexplorer --unregister", r"scripts\restore-explorer.ps1", "desktop"] {
            assert!(tp.contains(want), "{want}\n{tp}");
        }
        let s = Scratch::new("summary");
        let sum = install(&s.dir, &s.base).unwrap();
        assert!(sum.starts_with("Copied to ") && sum.contains("Windows Explorer still opens folders"), "{sum}");
        let sum = take_over(&s.dir, &s.base, &s.exe()).unwrap();
        assert!(sum.contains("Programs that start explorer.exe by name get rexplorer"), "{sum}");
        let un = uninstall(&s.dir, &s.base).unwrap();
        assert!(un.contains("open Windows Explorer again") && un.contains("Removed "), "{un}");
        let err = uninstall(&s.dir, &s.base).unwrap_err().message();
        assert!(err.contains("is not installed in") && err.contains("install.txt"), "{err}");
    }

    #[test]
    fn rollback_summary_lists_what_it_could_not_restore() {
        use std::os::windows::fs::OpenOptionsExt;
        let s = Scratch::new("summary-fail");
        let failed = s.failing("end", false);
        // Held open without sharing, so deleting it fails and the rollback lists it.
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(s.exe()).unwrap();
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
        let user = user_files(&s);
        let image = run_image(&s.exe());
        install(&s.dir, &s.base).unwrap();
        unsafe { FreeLibrary(image) };
        for (p, bytes) in user {
            assert_eq!(std::fs::read(&p).unwrap(), bytes, "settings carry over: {}", p.display());
        }
        assert_eq!(files(&s.dir), ["install.txt", "rexplorer.exe", "rexplorer.old"]);
        remove_old_in(&s.dir);
        assert_eq!(files(&s.dir), ["install.txt", "rexplorer.exe"], "the next start deletes the old copy");
    }

    /// Settings, themes and the session file as the app keeps them (outside the install dir), plus their bytes.
    fn user_files(s: &Scratch) -> Vec<(PathBuf, Vec<u8>)> {
        let data = s.dir.parent().unwrap().join("appdata");
        std::fs::create_dir_all(data.join("themes")).unwrap();
        let paths = [data.join("settings.ini"), data.join("themes").join("mine.ini"), data.join("session.ini")];
        paths.into_iter().map(|p| { std::fs::write(&p, format!("keep {}", p.display())).unwrap(); let b = std::fs::read(&p).unwrap(); (p, b) }).collect()
    }

    /// #64 and #65 from a 0.1.4 install: its one record holds the folder keys and the Startup shortcut and there is no
    /// takeover.txt. The new --install, then --unregister, then --uninstall leave the base as it was before.
    #[test]
    fn a_0_1_4_install_upgrades_unregisters_and_uninstalls_to_the_original_base() {
        let s = Scratch::new("v014");
        let scratch = s.dir.parent().unwrap().to_path_buf();
        let (programs, startup) = (scratch.join("Programs"), scratch.join("Startup"));
        std::fs::create_dir_all(&programs).unwrap();
        std::fs::create_dir_all(&startup).unwrap();
        let user = user_files(&s);
        assert!(!key_exists(&s.root));
        // What 0.1.4 left: keys, folder, exe, the verbs and App Paths, both shortcuts, and install.txt listing them.
        let (classes, ap) = (classes(&s.base), app_paths(&s.base));
        let leaves: Vec<String> = register::command_keys(&classes).into_iter().map(|(k, _)| k.to_string()).chain(Some(ap.clone())).collect();
        let mut rec = vec![];
        for leaf in &leaves {
            record_missing_keys(&mut rec, leaf);
        }
        std::fs::create_dir_all(&s.dir).unwrap();
        std::fs::copy(std::env::current_exe().unwrap(), s.exe()).unwrap();
        register::register(&classes, &s.exe()).unwrap();
        register::set_sz(&HSTRING::from(&ap), PCWSTR::null(), &s.exe().display().to_string()).unwrap();
        register::set_sz(&HSTRING::from(&ap), w!("Path"), &s.dir.display().to_string()).unwrap();
        let (start_lnk, desktop_lnk) = (programs.join(START_LNK), startup.join(DESKTOP_LNK));
        for f in [&start_lnk, &desktop_lnk] {
            std::fs::write(f, "0.1.4 shortcut").unwrap();
        }
        rec.push(format!("dir {}", s.dir.display()));
        rec.push(format!("file {}", s.exe().display()));
        rec.extend(leaves.iter().map(|k| format!("value {k}")));
        rec.extend([&start_lnk, &desktop_lnk].map(|f| format!("file {}", f.display())));
        std::fs::write(s.dir.join(RECORD), rec.join("\n") + "\n").unwrap();
        assert!(!s.dir.join(TAKEOVER).exists());

        install_to(&s.dir, &s.base, Some(&programs)).unwrap();
        let record = std::fs::read_to_string(s.dir.join(RECORD)).unwrap();
        assert!(rec.iter().all(|l| record.lines().any(|r| r == l)), "the upgrade keeps every line of the old record\n{record}");
        assert!(record.contains(&format!("named {}|DisplayName", arp(&s.base))), "{record}");
        assert!(desktop_lnk.is_file(), "the upgrade does not touch the Startup shortcut");

        give_back(&s.dir, &s.base).unwrap();
        assert_eq!(s.registry().len(), 8, "only the Apps and Features entry is left: {:?}", s.registry());
        assert!(s.exe().is_file(), "--unregister leaves the program");

        uninstall(&s.dir, &s.base).unwrap();
        assert!(!start_lnk.exists() && !desktop_lnk.exists(), "both shortcuts, the Startup one from the old record");
        assert_eq!(s.registry(), Vec::<String>::new());
        assert!(!key_exists(&s.root), "the base is as before the old install");
        assert!(!s.dir.exists());
        for (p, bytes) in user {
            assert_eq!(std::fs::read(&p).unwrap(), bytes, "{} is untouched", p.display());
        }
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

    /// #64 and #65 in a scratch folder: install (Start menu shortcut only), take over (Startup shortcut), update over
    /// a running copy, uninstall; every key, value, shortcut and file either step created is gone.
    #[test]
    fn install_update_over_a_running_copy_and_uninstall_leave_nothing() {
        let s = Scratch::new("lifecycle");
        let scratch = s.dir.parent().unwrap().to_path_buf();
        let (programs, startup) = (scratch.join("Programs"), scratch.join("Startup"));
        std::fs::create_dir_all(&programs).unwrap();
        std::fs::create_dir_all(&startup).unwrap();
        let (start_lnk, desktop_lnk) = (programs.join(START_LNK), startup.join(DESKTOP_LNK));
        install_to(&s.dir, &s.base, Some(&programs)).unwrap();
        let record = std::fs::read_to_string(s.dir.join(RECORD)).unwrap();
        assert!(start_lnk.is_file() && record.contains(&format!("file {}", start_lnk.display())), "{record}");
        assert!(!desktop_lnk.exists(), "a standalone install adds no desktop helper");
        assert_eq!(crate::jumplist::link_app_id(&start_lnk).as_deref(), Some(crate::jumplist::APP_ID), "the Start shortcut shares the windows' taskbar ID");
        take_over_to(&s.dir, &s.base, &s.exe(), Some(&startup)).unwrap();
        let record = std::fs::read_to_string(s.dir.join(TAKEOVER)).unwrap();
        assert!(desktop_lnk.is_file() && record.contains(&format!("file {}", desktop_lnk.display())), "{record}");
        let image = run_image(&s.exe());
        install_to(&s.dir, &s.base, Some(&programs)).unwrap();
        assert_eq!(files(&s.dir), ["install.txt", "rexplorer.exe", "rexplorer.old", "takeover.txt"], "the running copy is moved aside");
        let me = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        assert!(std::fs::read(s.exe()).unwrap() == me, "the next launch runs the new copy");
        unsafe { FreeLibrary(image) };
        uninstall(&s.dir, &s.base).unwrap();
        assert!(!start_lnk.exists() && !desktop_lnk.exists(), "both shortcuts are gone");
        assert_eq!(s.registry(), Vec::<String>::new());
        assert!(!key_exists(&s.root), "every key the install created is gone");
        assert!(!s.dir.exists(), "the exe, the old copy, both records and the folder are gone");
    }

    #[test]
    fn standalone_install_writes_only_the_program_and_take_over_is_separate() {
        let s = Scratch::new("standalone");
        install(&s.dir, &s.base).unwrap();
        assert!(s.exe().is_file() && is_installed(&s.dir));
        for k in register::command_keys(&classes(&s.base)).into_iter().map(|(k, _)| k.to_string()).chain(Some(app_paths(&s.base))) {
            assert!(!key_exists(&k), "a standalone install writes no association: {k}");
        }
        assert!(!key_exists(&classes(&s.base)));
        let ap = HSTRING::from(arp(&s.base));
        let exe = s.exe().display().to_string();
        assert_eq!(read_raw(&ap, w!("NoModify")), Some(dword(1)));
        assert_eq!(read_raw(&ap, w!("NoRepair")), Some(dword(1)));
        assert_eq!(register::read_sz(&ap, w!("UninstallString")), Some(format!("\"{exe}\" --uninstall --quiet")));
        assert_eq!(register::read_sz(&ap, w!("DisplayIcon")), Some(exe.clone()));
        assert_eq!(register::read_sz(&ap, w!("InstallLocation")), Some(s.dir.display().to_string()));
        assert_eq!(register::read_sz(&ap, w!("DisplayName")).as_deref(), Some("rexplorer"));
        take_over(&s.dir, &s.base, &s.exe()).unwrap();
        for (k, _) in register::command_keys(&classes(&s.base)) {
            assert!(read_default(&k).is_some_and(|v| v.contains(&exe)), "{k}");
        }
        assert!(read_default(&HSTRING::from(app_paths(&s.base))).is_some());
        // A second take-over (after an update) keeps the record of what the first one created.
        take_over(&s.dir, &s.base, &s.exe()).unwrap();
        // Another program takes the Drive verb meanwhile: giving back leaves it.
        let drive = HSTRING::from(format!(r"{}\Drive\shell\open\command", classes(&s.base)));
        register::set_sz(&drive, PCWSTR::null(), r"C:\other.exe %1").unwrap();
        let g = give_back(&s.dir, &s.base).unwrap();
        assert!(g.contains("another program"), "{g}");
        assert_eq!(read_default(&drive).as_deref(), Some(r"C:\other.exe %1"));
        assert!(!key_exists(&app_paths(&s.base)) && !key_exists(&format!(r"{}\CLSID", classes(&s.base))));
        assert!(s.exe().is_file() && key_exists(&arp(&s.base)) && !s.dir.join(TAKEOVER).exists(), "the program stays");
        // Refused while another program overrides a verb, changing nothing.
        let refused = take_over(&s.dir, &s.base, &s.exe()).unwrap_err();
        assert!(refused.rollback.is_empty() && refused.error.message().contains("Another program"), "{}", refused.error.message());
        assert!(!key_exists(&app_paths(&s.base)));
        register::delete_value(&drive, PCWSTR::null()).unwrap();
        uninstall(&s.dir, &s.base).unwrap();
        assert!(!key_exists(&arp(&s.base)) && !s.dir.parent().unwrap().exists());
        assert!(uninstall(&s.dir, &s.base).is_err(), "nothing left to uninstall");
    }

    #[test]
    fn give_back_without_a_record_uses_the_fixed_list() {
        let s = Scratch::new("norecord");
        let exe = Path::new(r"C:\old\rexplorer.exe");
        register::register(&classes(&s.base), exe).unwrap();
        let ap = HSTRING::from(app_paths(&s.base));
        register::set_sz(&ap, PCWSTR::null(), &exe.display().to_string()).unwrap();
        register::set_sz(&ap, w!("Path"), r"C:\old").unwrap();
        give_back(&s.dir, &s.base).unwrap();
        assert_eq!(s.registry(), Vec::<String>::new());
        assert!(!s.dir.exists(), "no folder is made to give back");
    }

    #[test]
    fn take_over_without_an_install_journals_its_folder() {
        let s = Scratch::new("takeonly");
        take_over(&s.dir, &s.base, Path::new(r"C:\dev\rexplorer.exe")).unwrap();
        assert!(s.dir.join(TAKEOVER).is_file() && !is_installed(&s.dir));
        assert!(recorded(&s.dir).iter().any(|l| l.ends_with(r"Folder\shell\open\command")), "{:?}", recorded(&s.dir));
        // Uninstall still gives back, and reports success since something was undone.
        uninstall(&s.dir, &s.base).unwrap();
        assert_eq!(s.registry(), Vec::<String>::new());
        assert!(!key_exists(&s.root) && !s.dir.parent().unwrap().exists());
    }

    /// scripts/restore-explorer.ps1 works without the exe or its records, from its own copy of `known_keys`.
    #[test]
    fn restore_script_knows_every_key() {
        let script = include_str!("../../../scripts/restore-explorer.ps1");
        for k in known_keys("BASE") {
            let rel = k.strip_prefix(r"BASE\").unwrap();
            assert!(script.contains(&format!("'{rel}'")), "restore-explorer.ps1 misses '{rel}'");
        }
        for name in [RECORD, TAKEOVER, START_LNK, DESKTOP_LNK] {
            assert!(script.contains(name), "restore-explorer.ps1 misses {name}");
        }
        assert!(script.contains(&format!("'{}'", crate::jumplist::APP_ID)), "restore-explorer.ps1 misses the jump list's app id");
    }
}
