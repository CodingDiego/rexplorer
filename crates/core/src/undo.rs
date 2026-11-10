//! Undo and redo of the file operations made in rexplorer: each entry records what was done, and undoing it runs the inverse through IFileOperation, so the shell's progress, conflict and confirmation UI apply.
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Shell::{
    FILEOPERATION_FLAGS, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NO_UI, FOF_WANTNUKEWARNING, FOFX_RECYCLEONDELETE, FileOperation, IFileOperation,
    IFileOperationProgressSink, IFileOperationProgressSink_Impl, IShellItem, SHCreateItemFromParsingName, SHELLSTATEA, SHGetSetSettings,
    SIGDN_DESKTOPABSOLUTEPARSING, SSF_NOCONFIRMRECYCLE,
};
use windows::core::{HRESULT, HSTRING, PCWSTR, Ref, Result, implement};

use crate::com::take_cotaskmem_string;
use crate::folder::ShellFolder;
use crate::item::ShellItem;
use crate::menu::ContextMenu;

/// One thing that was done. Paths are full file system paths, as `ShellItem::parsing_name` gives them.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// `from` was renamed to `to` (same folder).
    Rename { from: String, to: String },
    /// These items were made (New, or copies of a paste or drop); undo sends them to the Recycle Bin.
    Created(Vec<String>),
    /// (original path, the Recycle Bin item's parsing name); undo restores them.
    Recycled(Vec<(String, String)>),
    /// (from, to); undo moves each back.
    Moved(Vec<(String, String)>),
    /// A paste or drop, (source, destination), recorded before it ran: copy or move is decided at undo time
    /// (the source still exists: a copy).
    Transfer(Vec<(String, String)>),
}

/// The undo and redo stacks. Every successful `take` is followed by exactly one `put`; `busy` stops a second Ctrl+Z
/// while the shell's progress dialog pumps messages.
#[derive(Default)]
pub struct Log {
    undo: Vec<Vec<Op>>,
    redo: Vec<Vec<Op>>,
    busy: bool,
}

const MAX_ENTRIES: usize = 32;
const RECYCLE_BIN: &str = "::{645FF040-5081-101B-9F08-00AA002F954E}";

fn abs(p: &str) -> bool {
    Path::new(p).is_absolute()
}

impl Op {
    /// False for an Op that lists nothing, names a virtual (non file system) path, or renames onto itself.
    fn recordable(&self) -> bool {
        match self {
            Op::Rename { from, to } => from != to && abs(from) && abs(to),
            Op::Created(v) => !v.is_empty() && v.iter().all(|p| abs(p)),
            Op::Recycled(v) => !v.is_empty() && v.iter().all(|(p, _)| abs(p)),
            Op::Moved(v) | Op::Transfer(v) => !v.is_empty() && v.iter().all(|(a, b)| abs(a) && abs(b)),
        }
    }
}

impl Log {
    pub fn record(&mut self, op: Op) {
        // While a reversal runs its shell UI pumps messages: an entry recorded then would land out of order.
        if self.busy || !op.recordable() {
            return;
        }
        self.undo.push(vec![op]);
        if self.undo.len() > MAX_ENTRIES {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// The newest entry of undo (or redo); None while another one runs.
    pub fn take(&mut self, redo: bool) -> Option<Vec<Op>> {
        if self.busy {
            return None;
        }
        let e = if redo { self.redo.pop() } else { self.undo.pop() };
        self.busy = e.is_some();
        e
    }

    /// What the reversal did goes to the opposite stack; redo is not cleared here.
    pub fn put(&mut self, redo: bool, done: Vec<Op>) {
        self.busy = false;
        if !done.is_empty() {
            if redo { &mut self.undo } else { &mut self.redo }.push(done);
        }
    }
}

/// What a paste or drop of `sources` into `dest` may create: only the names free now (never touches a file already there).
pub fn plan_transfer(sources: &[String], dest: &str, exists: impl Fn(&str) -> bool) -> Vec<(String, String)> {
    sources
        .iter()
        .filter_map(|s| {
            let to = Path::new(dest).join(Path::new(s).file_name()?).to_string_lossy().into_owned();
            (!exists(&to)).then(|| (s.clone(), to))
        })
        .collect()
}

/// (copies to recycle, moves to undo) of a recorded transfer: a destination that exists, whose source still does (copy) or not (move).
/// A transfer is recorded before it runs, so it may never have happened: a destination next to a source it is not a
/// copy of (`same`) is somebody else's file and is left alone.
fn split_transfer(pairs: Vec<(String, String)>, exists: impl Fn(&str) -> bool, same: impl Fn(&str, &str) -> bool) -> (Vec<String>, Vec<(String, String)>) {
    let (mut copies, mut moves) = (Vec::new(), Vec::new());
    for (s, d) in pairs.into_iter().filter(|(_, d)| exists(d)) {
        if !exists(&s) {
            moves.push((s, d));
        } else if same(&s, &d) {
            copies.push(d);
        }
    }
    (copies, moves)
}

/// Whether `b` looks like the shell's copy of `a`: the same size and last-write time (2 s apart at most, FAT's step).
/// shortcut: two folders always match, upgrade to comparing their contents if a wrong folder is ever recycled.
fn same_file(a: &str, b: &str) -> bool {
    let (Ok(a), Ok(b)) = (std::fs::metadata(a), std::fs::metadata(b)) else { return false };
    if a.is_dir() || b.is_dir() {
        return a.is_dir() && b.is_dir();
    }
    let (Ok(ta), Ok(tb)) = (a.modified(), b.modified()) else { return false };
    a.len() == b.len() && ta.max(tb).duration_since(ta.min(tb)).is_ok_and(|d| d.as_secs() <= 2)
}

/// Explorer skips "Move to Recycle Bin?" when the user turned it off; a permanent delete still warns.
fn recycle_flags(confirm: bool) -> FILEOPERATION_FLAGS {
    let f = FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE | FOF_WANTNUKEWARNING;
    if confirm { f } else { f | FOF_NOCONFIRMATION }
}

fn confirms_recycle() -> bool {
    let mut ss = SHELLSTATEA::default();
    unsafe { SHGetSetSettings(Some(&mut ss), SSF_NOCONFIRMRECYCLE, false) };
    // fNoConfirmRecycle is bit 2 of the packed bitfield.
    let bits = { ss._bitfield1 };
    bits & 4 == 0
}

fn item(path: &str) -> Result<IShellItem> {
    unsafe { SHCreateItemFromParsingName(&HSTRING::from(path), None) }
}

fn name_of(i: &IShellItem) -> String {
    unsafe { i.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING) }.map(|p| unsafe { take_cotaskmem_string(p) }).unwrap_or_default()
}

fn file_name(p: &str) -> String {
    Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

type Pairs = Rc<RefCell<Vec<(String, String)>>>;

/// Collects (original, Recycle Bin item) of every successful delete.
#[implement(IFileOperationProgressSink)]
struct Sink(Pairs);

impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn StartOperations(&self) -> Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, _: HRESULT) -> Result<()> {
        Ok(())
    }
    fn PreRenameItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> Result<()> {
        Ok(())
    }
    fn PostRenameItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR, _: HRESULT, _: Ref<IShellItem>) -> Result<()> {
        Ok(())
    }
    fn PreMoveItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR) -> Result<()> {
        Ok(())
    }
    fn PostMoveItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR, _: HRESULT, _: Ref<IShellItem>) -> Result<()> {
        Ok(())
    }
    fn PreCopyItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR) -> Result<()> {
        Ok(())
    }
    fn PostCopyItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR, _: HRESULT, _: Ref<IShellItem>) -> Result<()> {
        Ok(())
    }
    fn PreDeleteItem(&self, _: u32, _: Ref<IShellItem>) -> Result<()> {
        Ok(())
    }
    fn PostDeleteItem(&self, _: u32, item: Ref<IShellItem>, hr: HRESULT, created: Ref<IShellItem>) -> Result<()> {
        // `created` is the item now in the Recycle Bin, and null when the delete was permanent.
        if let (true, Some(i), Some(c)) = (hr.is_ok(), item.as_ref(), created.as_ref()) {
            self.0.borrow_mut().push((name_of(i), name_of(c)));
        }
        Ok(())
    }
    fn PreNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> Result<()> {
        Ok(())
    }
    fn PostNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR, _: &PCWSTR, _: u32, _: HRESULT, _: Ref<IShellItem>) -> Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, _: u32, _: u32) -> Result<()> {
        Ok(())
    }
    fn ResetTimer(&self) -> Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> Result<()> {
        Ok(())
    }
}

/// Sends `paths` to the Recycle Bin through IFileOperation (never a permanent delete without the shell's warning);
/// returns (original, bin item) of what went. `quiet` shows no UI (tests).
pub fn recycle(hwnd: HWND, paths: &[String], quiet: bool) -> Vec<(String, String)> {
    if paths.is_empty() {
        return Vec::new();
    }
    let got: Pairs = Rc::default();
    let go = || -> Result<()> {
        unsafe {
            let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)?;
            op.SetOperationFlags(if quiet { FOF_NO_UI | FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE } else { recycle_flags(confirms_recycle()) })?;
            if !hwnd.is_invalid() {
                op.SetOwnerWindow(hwnd)?;
            }
            let sink: IFileOperationProgressSink = Sink(got.clone()).into();
            let cookie = op.Advise(&sink)?;
            // A path gone meanwhile (a stale search result) is skipped, not a reason to delete none of the rest.
            let queued = paths.iter().filter_map(|p| item(p).ok()).try_for_each(|i| op.DeleteItem(&i, None::<&IFileOperationProgressSink>));
            // A cancel returns Err; what the sink saw is still recorded.
            let _ = queued.and_then(|_| op.PerformOperations());
            let _ = op.Unadvise(cookie);
        }
        Ok(())
    };
    let _ = go();
    let done = got.borrow().iter().filter(|(o, _)| paths.iter().any(|p| p.eq_ignore_ascii_case(o))).cloned().collect();
    done
}

/// Puts Recycle Bin items back with the bin's "undelete" verb; returns the originals invoked. Never onto a name that is taken.
fn restore(hwnd: HWND, pairs: &[(String, String)]) -> Vec<String> {
    let want: Vec<&(String, String)> = pairs.iter().filter(|(o, _)| !Path::new(o).exists()).collect();
    if want.is_empty() {
        return Vec::new();
    }
    let Ok(bin) = ShellFolder::parse(RECYCLE_BIN) else { return Vec::new() };
    let Ok(items) = bin.enumerate() else { return Vec::new() };
    let picked: Vec<(&ShellItem, &str)> =
        items.iter().filter_map(|i| want.iter().find(|(_, b)| b.eq_ignore_ascii_case(&i.parsing_name())).map(|(o, _)| (i, o.as_str()))).collect();
    if picked.is_empty() {
        return Vec::new();
    }
    let its: Vec<&ShellItem> = picked.iter().map(|p| p.0).collect();
    let ok = ContextMenu::for_items(&bin, &its, hwnd).and_then(|m| m.invoke_verb(hwnd, "undelete", false)).is_ok();
    if ok { picked.iter().map(|p| p.1.to_string()).collect() } else { Vec::new() }
}

/// One IFileOperation: `add` queues the work. `quiet` shows no UI (tests).
fn run(hwnd: HWND, quiet: bool, add: impl FnOnce(&IFileOperation) -> Result<()>) -> Result<()> {
    unsafe {
        let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)?;
        op.SetOperationFlags(if quiet { FOF_NO_UI } else { FOF_ALLOWUNDO })?;
        if !hwnd.is_invalid() {
            op.SetOwnerWindow(hwnd)?;
        }
        add(&op)?;
        op.PerformOperations()
    }
}

/// Performs the inverse of `entry` and returns what that did, as an entry for the opposite stack.
pub fn reverse(hwnd: HWND, entry: Vec<Op>) -> Vec<Op> {
    reverse_with(hwnd, entry, false)
}

fn reverse_with(hwnd: HWND, entry: Vec<Op>, quiet: bool) -> Vec<Op> {
    entry.into_iter().rev().flat_map(|op| reverse_op(hwnd, op, quiet)).collect()
}

fn reverse_op(hwnd: HWND, op: Op, quiet: bool) -> Vec<Op> {
    let exists = |p: &str| Path::new(p).exists();
    let out = match op {
        Op::Rename { from, to } => {
            if !exists(&to) || (exists(&from) && !from.eq_ignore_ascii_case(&to)) {
                return Vec::new();
            }
            let _ = run(hwnd, quiet, |o| unsafe { o.RenameItem(&item(&to)?, &HSTRING::from(file_name(&from)), None::<&IFileOperationProgressSink>) });
            if exists(&from) { vec![Op::Rename { from: to, to: from }] } else { Vec::new() }
        }
        Op::Created(paths) => {
            let live: Vec<String> = paths.into_iter().filter(|p| exists(p)).collect();
            vec![Op::Recycled(recycle(hwnd, &live, quiet))]
        }
        Op::Recycled(pairs) => vec![Op::Created(restore(hwnd, &pairs))],
        Op::Moved(pairs) => {
            let mut done = Vec::new();
            for (from, to) in pairs.into_iter().filter(|(f, t)| exists(t) && !exists(f)) {
                let parent = Path::new(&from).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                let _ = run(hwnd, quiet, |o| unsafe { o.MoveItem(&item(&to)?, &item(&parent)?, &HSTRING::from(file_name(&from)), None::<&IFileOperationProgressSink>) });
                if exists(&from) {
                    done.push((to, from));
                }
            }
            vec![Op::Moved(done)]
        }
        Op::Transfer(pairs) => {
            let (copies, moves) = split_transfer(pairs, exists, same_file);
            let mut v = reverse_op(hwnd, Op::Created(copies), quiet);
            v.extend(reverse_op(hwnd, Op::Moved(moves), quiet));
            v
        }
    };
    // A reversal that did nothing is no entry.
    out.into_iter().filter(|o| match o {
        Op::Created(v) => !v.is_empty(),
        Op::Recycled(v) => !v.is_empty(),
        Op::Moved(v) | Op::Transfer(v) => !v.is_empty(),
        Op::Rename { .. } => true,
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::ComInit;

    fn s(p: &str) -> String {
        p.to_string()
    }

    struct Fixture(std::path::PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fixture(tag: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!("rexplorer-undo-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Fixture(dir)
    }

    fn p(dir: &Path, n: &str) -> String {
        dir.join(n).to_string_lossy().into_owned()
    }

    #[test]
    fn plan_transfer_skips_names_already_taken() {
        let src = vec![s("C:\\a\\x.txt"), s("C:\\a\\y.txt"), s("C:\\")];
        let got = plan_transfer(&src, "D:\\b", |p| p == "D:\\b\\y.txt");
        assert_eq!(got, vec![(s("C:\\a\\x.txt"), s("D:\\b\\x.txt"))]);
        assert!(plan_transfer(&[s("D:\\b\\z.txt")], "D:\\b", |p| p == "D:\\b\\z.txt").is_empty());
    }

    #[test]
    fn split_transfer_tells_copies_from_moves() {
        let pairs = vec![(s("s1"), s("d1")), (s("s2"), s("d2")), (s("s3"), s("d3"))];
        let got = split_transfer(pairs, |p| matches!(p, "s1" | "d1" | "d2"), |_, _| true);
        assert_eq!(got, (vec![s("d1")], vec![(s("s2"), s("d2"))]));
    }

    #[test]
    fn split_transfer_skips_a_destination_that_is_not_the_copy() {
        // A cancelled drop was recorded, then another file took the name: undo must leave it alone.
        let got = split_transfer(vec![(s("s1"), s("d1"))], |_| true, |_, _| false);
        assert_eq!(got, (vec![], vec![]));
    }

    #[test]
    fn same_file_compares_size_and_time() {
        let dir = std::env::temp_dir().join(format!("rexplorer-same-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b, c) = (dir.join("a.txt"), dir.join("b.txt"), dir.join("c.txt"));
        std::fs::write(&a, "hello").unwrap();
        std::fs::copy(&a, &b).unwrap();
        std::fs::write(&c, "other text").unwrap();
        let p = |x: &std::path::PathBuf| x.to_string_lossy().into_owned();
        assert!(same_file(&p(&a), &p(&b)));
        assert!(!same_file(&p(&a), &p(&c)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_ignores_a_record_while_busy() {
        let mut l = Log::default();
        l.record(Op::Created(vec![s("C:\\a")]));
        assert!(l.take(false).is_some());
        l.record(Op::Created(vec![s("C:\\b")]));
        l.put(false, vec![Op::Recycled(vec![(s("C:\\a"), s("bin"))])]);
        assert!(l.take(false).is_none());
        assert!(l.take(true).is_some());
    }

    #[test]
    fn log_moves_entries_between_stacks() {
        let mut l = Log::default();
        let r1 = Op::Rename { from: s("C:\\a\\1"), to: s("C:\\a\\2") };
        let c = Op::Created(vec![s("C:\\a\\n")]);
        let rec = Op::Recycled(vec![(s("C:\\a\\n"), s("bin"))]);
        l.record(r1);
        l.record(c.clone());
        assert_eq!(l.take(false), Some(vec![c]));
        l.put(false, vec![rec.clone()]);
        assert_eq!(l.take(true), Some(vec![rec]));
        let back = vec![Op::Created(vec![s("C:\\a\\n")])];
        l.put(true, back.clone());
        assert_eq!(l.take(false), Some(back));
        l.put(false, vec![]);
        assert_eq!(l.take(true), None);
        l.record(Op::Created(vec![s("C:\\a\\m")]));
        assert_eq!(l.take(true), None);
    }

    #[test]
    fn log_ignores_virtual_empty_and_trims_oldest() {
        let mut l = Log::default();
        l.record(Op::Created(vec![]));
        l.record(Op::Rename { from: s("::{x}\\a"), to: s("::{x}\\b") });
        l.record(Op::Rename { from: s("C:\\a"), to: s("C:\\a") });
        assert_eq!(l.take(false), None);
        for i in 0..40 {
            l.record(Op::Created(vec![format!("C:\\a\\{i}")]));
        }
        assert_eq!(l.take(false), Some(vec![Op::Created(vec![s("C:\\a\\39")])]));
        l.put(false, vec![]);
        assert_eq!(l.undo.len(), 31);
    }

    #[test]
    fn log_take_refuses_while_busy() {
        let mut l = Log::default();
        l.record(Op::Created(vec![s("C:\\a")]));
        l.record(Op::Created(vec![s("C:\\b")]));
        assert!(l.take(false).is_some());
        assert!(l.take(false).is_none());
        l.put(false, vec![]);
        assert!(l.take(false).is_some());
    }

    #[test]
    fn recycle_flags_follow_the_confirmation_setting() {
        let has = |f: FILEOPERATION_FLAGS, x: FILEOPERATION_FLAGS| f.0 & x.0 == x.0;
        let (on, off) = (recycle_flags(true), recycle_flags(false));
        assert!(!has(on, FOF_NOCONFIRMATION) && has(on, FOF_WANTNUKEWARNING) && has(on, FOF_ALLOWUNDO));
        assert!(has(off, FOF_NOCONFIRMATION) && has(off, FOF_WANTNUKEWARNING));
    }

    #[test]
    fn rename_reverses_and_returns_its_inverse() {
        let _c = ComInit::sta();
        let fx = fixture("rename");
        std::fs::write(fx.0.join("b.txt"), b"x").unwrap();
        let (a, b) = (p(&fx.0, "a.txt"), p(&fx.0, "b.txt"));
        let r = reverse_with(HWND::default(), vec![Op::Rename { from: a.clone(), to: b.clone() }], true);
        assert!(Path::new(&a).exists() && !Path::new(&b).exists());
        assert_eq!(r, vec![Op::Rename { from: b.clone(), to: a.clone() }]);
        reverse_with(HWND::default(), r, true);
        assert!(Path::new(&b).exists() && !Path::new(&a).exists());
    }

    #[test]
    fn moved_back_never_overwrites() {
        let _c = ComInit::sta();
        let fx = fixture("moved");
        std::fs::create_dir_all(fx.0.join("src")).unwrap();
        std::fs::create_dir_all(fx.0.join("dst")).unwrap();
        let (sx, dx) = (p(&fx.0, "src\\x.txt"), p(&fx.0, "dst\\x.txt"));
        std::fs::write(&dx, "new").unwrap();
        let r = reverse_with(HWND::default(), vec![Op::Moved(vec![(sx.clone(), dx.clone())])], true);
        assert_eq!(std::fs::read_to_string(&sx).unwrap(), "new");
        assert_eq!(r, vec![Op::Moved(vec![(dx.clone(), sx.clone())])]);
        std::fs::write(&dx, "other").unwrap();
        let r = reverse_with(HWND::default(), r, true);
        assert!(r.is_empty());
        assert_eq!(std::fs::read_to_string(&sx).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(&dx).unwrap(), "other");
    }

    #[test]
    fn transfer_copy_goes_to_the_bin_and_comes_back() {
        let _c = ComInit::sta();
        let fx = fixture("transfer");
        std::fs::create_dir_all(fx.0.join("src")).unwrap();
        std::fs::create_dir_all(fx.0.join("dst")).unwrap();
        let n = format!("rexplorer-undo-test-{}.txt", std::process::id());
        let (sx, dx) = (p(&fx.0, &format!("src\\{n}")), p(&fx.0, &format!("dst\\{n}")));
        std::fs::write(&sx, "x").unwrap();
        std::fs::write(&dx, "x").unwrap();
        let r = reverse_with(HWND::default(), vec![Op::Transfer(vec![(sx.clone(), dx.clone())])], true);
        if r.is_empty() && !Path::new(&dx).exists() {
            println!("skipped: no Recycle Bin");
            return;
        }
        assert!(Path::new(&sx).exists() && !Path::new(&dx).exists());
        let Some(Op::Recycled(v)) = r.first() else { panic!("{r:?}") };
        assert_eq!(v[0].0, dx);
        let r2 = reverse_with(HWND::default(), r, true);
        for _ in 0..50 {
            if Path::new(&dx).exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert_eq!(r2, vec![Op::Created(vec![dx.clone()])]);
        assert!(Path::new(&dx).exists());
    }

    #[test]
    fn recycle_skips_a_path_that_is_gone() {
        let _c = ComInit::sta();
        let fx = fixture("gone");
        let n = format!("rexplorer-undo-test-gone-{}.txt", std::process::id());
        let (gone, here) = (p(&fx.0, "gone.txt"), p(&fx.0, &n));
        std::fs::write(&here, "x").unwrap();
        let done = recycle(HWND::default(), &[gone, here.clone()], true);
        if done.is_empty() && !Path::new(&here).exists() {
            println!("skipped: no Recycle Bin");
            return;
        }
        assert!(!Path::new(&here).exists(), "the file that exists still goes");
        assert_eq!(done.iter().map(|d| d.0.clone()).collect::<Vec<_>>(), vec![here.clone()]);
        // Out of the user's bin again.
        restore(HWND::default(), &done);
        for _ in 0..50 {
            if Path::new(&here).exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(Path::new(&here).exists());
    }
}
