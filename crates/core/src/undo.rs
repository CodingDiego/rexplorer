//! Undo and redo of the file operations made in rexplorer: each entry records what was done, and undoing it runs the inverse through IFileOperation, so the shell's progress, conflict and confirmation UI apply.
//! An undo only touches an item it can prove it made or moved: one with the file id it recorded, or for a paste or
//! drop, a name that was not there before holding what the source held. Such an item goes to the Recycle Bin as a copy
//! only while the source is still there as planned; the source itself (same id) is only ever moved back.
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::SystemTime;

use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx, GetVolumeInformationByHandleW,
};
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

/// A file system item's identity, (volume serial, 128-bit file id). It survives renames and edits, never a delete and
/// re-create (a creation time would: NTFS tunnels it to a file re-made under the same name).
pub type Id = (u64, [u8; 16]);

/// One thing that was done. Paths are full file system paths, as `ShellItem::parsing_name` gives them.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// `from` was renamed to `to` (same folder); `id` is the item at `to`.
    Rename { from: String, to: String, id: Id },
    /// These items were made (New, shortcuts, copies of a paste or drop); undo sends to the Recycle Bin each one
    /// still there.
    Created(Vec<(String, Id)>),
    /// (original path, the Recycle Bin item's parsing name); undo restores them.
    Recycled(Vec<(String, String)>),
    /// (from, to, the item at `to`); undo moves each back.
    Moved(Vec<(String, String, Id)>),
    /// A paste or drop, read before it ran: it may be cancelled, skipped or still running. Undo works out from the
    /// destination then what it made and, from its id and the source path, whether it copied or moved.
    Transfer(Vec<Sent>),
}

/// One item of a paste or drop, read before the shell ran it.
#[derive(Clone, Debug, PartialEq)]
pub struct Sent {
    pub from: String,
    /// The destination folder.
    pub dest: String,
    /// The names (lowercase) in `dest` then that the shell could give this item (`could_be`): never claimed by an undo.
    taken: Vec<String>,
    /// The source then, which what it became must still show.
    was: Was,
}

/// A source of a paste or drop as it was planned.
#[derive(Clone, Debug, PartialEq)]
enum Was {
    /// Its size and last write (a copy or a move keeps both), and its id (a move within the volume keeps it).
    File(u64, SystemTime, Option<Id>),
    /// Its id (a move within the volume keeps it), the lowercase names directly inside it, sorted, and when it was
    /// planned (a copy, or a move to another volume, makes a new folder that holds the same names).
    Folder { id: Option<Id>, kids: Vec<String>, at: SystemTime },
}

/// FILE_SUPPORTS_OPEN_BY_FILE_ID: the volume's file ids are stable (not FAT's or exFAT's directory slots).
const STABLE_IDS: u32 = 0x0100_0000;

/// The item at `p` now, the link itself for a link; None when it cannot be opened, or on a file system whose ids are
/// not stable (FAT and exFAT reuse a deleted file's slot) or that reports none.
pub fn id_of(p: &str) -> Option<Id> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    // No access right is needed to read the id; backup semantics opens folders too.
    let f = std::fs::OpenOptions::new().access_mode(0).custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0).open(p).ok()?;
    let h = HANDLE(f.as_raw_handle());
    let mut flags = 0u32;
    unsafe { GetVolumeInformationByHandleW(h, None, None, None, Some(&mut flags), None) }.ok()?;
    if flags & STABLE_IDS == 0 {
        return None;
    }
    // The 128-bit id: ReFS ids need more than the 64-bit file index.
    let mut i = FILE_ID_INFO::default();
    unsafe { GetFileInformationByHandleEx(h, FileIdInfo, (&mut i as *mut FILE_ID_INFO).cast(), size_of::<FILE_ID_INFO>() as u32) }.ok()?;
    (i.FileId.Identifier != [0; 16]).then_some((i.VolumeSerialNumber, i.FileId.Identifier))
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
    /// The items at `paths` now, just made; one that cannot be identified is left out (no undo could prove it ours).
    pub fn created(paths: Vec<String>) -> Op {
        Op::Created(paths.into_iter().filter_map(|p| id_of(&p).map(|id| (p, id))).collect())
    }

    /// A rename just done; None when the renamed item cannot be identified.
    pub fn rename(from: String, to: String) -> Option<Op> {
        let id = id_of(&to)?;
        Some(Op::Rename { from, to, id })
    }

    /// (from, to) of moves just done, with the item now at `to`.
    fn moved(pairs: Vec<(String, String)>) -> Op {
        Op::Moved(pairs.into_iter().filter_map(|(f, t)| id_of(&t).map(|id| (f, t, id))).collect())
    }

    /// False for an Op that lists nothing, names a virtual (non file system) path, or renames onto itself.
    fn recordable(&self) -> bool {
        match self {
            Op::Rename { from, to, .. } => from != to && abs(from) && abs(to),
            Op::Created(v) => !v.is_empty() && v.iter().all(|(p, _)| abs(p)),
            Op::Recycled(v) => !v.is_empty() && v.iter().all(|(p, _)| abs(p)),
            Op::Moved(v) => !v.is_empty() && v.iter().all(|(a, b, _)| abs(a) && abs(b)),
            Op::Transfer(v) => !v.is_empty() && v.iter().all(|s| abs(&s.from) && abs(&s.dest)),
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

fn names_in(dir: &str) -> Option<Vec<String>> {
    Some(std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
}

/// A paste or drop of `sources` into `dest`, read before the shell runs it: each source as it is now, and the names
/// in `dest` its copy could get that are already taken. Sources that are gone, and a `dest` that cannot be read, give none.
pub fn plan_transfer(sources: &[String], dest: &str) -> Vec<Sent> {
    let Some(names) = names_in(dest) else { return Vec::new() };
    // Lowercase: a Replace may give the existing item the source's case, and it must still count as taken.
    let lower: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
    let at = SystemTime::now();
    sources
        .iter()
        .filter_map(|s| {
            let name = file_name(s).to_lowercase();
            let m = std::fs::metadata(s).ok().filter(|_| !name.is_empty())?;
            let was = if m.is_dir() { Was::Folder { id: id_of(s), kids: kids(s)?, at } } else { Was::File(m.len(), m.modified().ok()?, id_of(s)) };
            let taken = lower.iter().filter(|l| could_be(l, &name, m.is_dir())).cloned().collect();
            Some(Sent { from: s.clone(), dest: dest.to_string(), taken, was })
        })
        .collect()
}

/// The lowercase names directly inside folder `p`, sorted.
fn kids(p: &str) -> Option<Vec<String>> {
    let mut v: Vec<String> = names_in(p)?.iter().map(|n| n.to_lowercase()).collect();
    v.sort();
    Some(v)
}

/// Whether the shell could have given a copy of `name` the name `n`, both lowercase: the name itself or, for "Keep
/// both" and a copy into its own folder, the stem followed by " (" or " - " ("x (2).txt", "x - copy.txt",
/// "x - copia.txt"). A folder's name has no extension.
fn could_be(n: &str, name: &str, folder: bool) -> bool {
    let split = |s: &str| s.rfind('.').filter(|&i| i > 0 && !folder).unwrap_or(s.len());
    let ((ns, ne), (stem, ext)) = (n.split_at(split(n)), name.split_at(split(name)));
    ne == ext && ns.strip_prefix(stem).is_some_and(|r| r.is_empty() || r.starts_with(" (") || r.starts_with(" - "))
}

/// Whether `p` holds what the source held: for a file the same size and last write (2 s apart at most, FAT's step);
/// for a folder the source itself (moved within its volume), or a folder made since the plan holding the same names
/// (the shell's copy engine gives a copied folder a new creation time). A copy of an empty folder is never proven
/// (any empty folder made since would pass), so its undo leaves it: the safe side.
/// shortcut: a folder's names are compared one level deep, upgrade to a deeper compare if a wrong folder is ever claimed.
fn holds(p: &str, was: &Was) -> bool {
    let Ok(m) = std::fs::metadata(p) else { return false };
    let near = |u: SystemTime, t: SystemTime| u.max(t).duration_since(u.min(t)).is_ok_and(|d| d.as_secs() <= 2);
    match was {
        Was::File(len, t, _) => !m.is_dir() && m.len() == *len && m.modified().is_ok_and(|u| near(u, *t)),
        Was::Folder { kids: k, at, .. } => {
            m.is_dir()
                && (is_source(p, was) || (!k.is_empty() && m.created().is_ok_and(|c| c >= *at || near(c, *at)) && kids(p).as_ref() == Some(k)))
        }
    }
}

/// The item at `p` is the planned source itself (moved within its volume: the id is kept).
fn is_source(p: &str, was: &Was) -> bool {
    let (Was::File(_, _, id) | Was::Folder { id, .. }) = was;
    id.is_some() && id_of(p) == *id
}

/// The source path still holds what the plan read there: the same size and last write for a file, the same names
/// for a folder. A move that left part of a folder behind (a locked file) does not.
fn intact(p: &str, was: &Was) -> bool {
    match was {
        Was::File(..) => holds(p, was),
        Was::Folder { kids: k, .. } => Path::new(p).is_dir() && kids(p).as_ref() == Some(k),
    }
}

/// Where `s` landed, read after the shell ran it from `names` (its destination's names now, and lowercase): a name
/// that was not taken before, that the shell could have given it, holds what the source held and no other source of
/// the transfer `claimed`. Its own name first; else only a single such name proves which is ours. None for a
/// skipped, cancelled or replaced item.
fn landed(s: &Sent, names: &[(String, String)], claimed: &[String]) -> Option<String> {
    let name = file_name(&s.from).to_lowercase();
    let fresh: Vec<(String, &str)> = names
        .iter()
        .filter(|(_, l)| could_be(l, &name, matches!(s.was, Was::Folder { .. })) && !s.taken.contains(l))
        .map(|(n, l)| (Path::new(&s.dest).join(n).to_string_lossy().into_owned(), l.as_str()))
        .filter(|(p, _)| !claimed.contains(p) && holds(p, &s.was))
        .collect();
    let own = fresh.iter().find(|(_, l)| *l == name);
    own.or(fresh.first().filter(|_| fresh.len() == 1)).map(|(p, _)| p.clone())
}

/// The names in `dir` now, each with its lowercase form.
fn names_lower(dir: &str) -> Vec<(String, String)> {
    names_in(dir).unwrap_or_default().into_iter().map(|n| {
        let l = n.to_lowercase();
        (n, l)
    }).collect()
}

/// Explorer skips "Move to Recycle Bin?" when the user turned it off; a permanent delete still warns.
fn recycle_flags(confirm: bool) -> FILEOPERATION_FLAGS {
    let f = FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE | FOF_WANTNUKEWARNING;
    if confirm { f } else { f | FOF_NOCONFIRMATION }
}

fn too_long_to_recycle(path: &str) -> bool {
    path.encode_utf16().count() >= 260
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
            // The bin takes no path over MAX_PATH: the shell then deletes for good, and with the confirmation off it
            // does so without its nuke warning. Such a delete always asks (#34).
            let ask = confirms_recycle() || paths.iter().any(|p| too_long_to_recycle(p));
            op.SetOperationFlags(if quiet { FOF_NO_UI | FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE } else { recycle_flags(ask) })?;
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
    got.borrow().iter().filter(|(o, _)| paths.iter().any(|p| p.eq_ignore_ascii_case(o))).cloned().collect()
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
    // invoke_verb runs "undelete" synchronously (CMIC_MASK_NOASYNC): see there.
    if ContextMenu::for_items(&bin, &its, hwnd).and_then(|m| m.invoke_verb(hwnd, "undelete", false)).is_err() {
        return Vec::new();
    }
    // A bin that still restores on a thread of its own gets 2 s, so redo can identify what came back.
    for _ in 0..40 {
        if picked.iter().all(|p| Path::new(p.1).exists()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    for (i, o) in &picked {
        drop_record(&i.parsing_name(), o);
    }
    picked.iter().map(|p| p.1.to_string()).collect()
}

/// The undelete verb run outside Explorer moves the item's $R file back but leaves its $I record in the bin, hidden
/// and never cleaned up. Removes that record once the item is back at `original` and the record names it.
fn drop_record(bin_item: &str, original: &str) {
    let Some(i) = record_of(bin_item) else { return };
    if Path::new(bin_item).exists() || !Path::new(original).exists() {
        return;
    }
    if std::fs::read(&i).ok().and_then(|b| record_path(&b)).is_some_and(|p| p.eq_ignore_ascii_case(original)) {
        let _ = std::fs::remove_file(&i);
    }
}

/// The $I record beside a bin item's $R file.
fn record_of(bin_item: &str) -> Option<std::path::PathBuf> {
    let r = Path::new(bin_item);
    let rest = r.file_name()?.to_str()?.strip_prefix("$R")?;
    Some(r.with_file_name(format!("$I{rest}")))
}

/// The original path a Recycle Bin $I record holds: version 2 is (version, size, time, length in UTF-16 units, the
/// path), version 1 a fixed MAX_PATH field after the same first three.
fn record_path(b: &[u8]) -> Option<String> {
    let u16s = |s: &[u8]| -> String {
        let w: Vec<u16> = s.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).take_while(|&c| c != 0).collect();
        String::from_utf16_lossy(&w)
    };
    match b.get(..8)? {
        [1, 0, 0, 0, 0, 0, 0, 0] => Some(u16s(b.get(24..24 + 520)?)),
        [2, 0, 0, 0, 0, 0, 0, 0] => {
            let n = u32::from_le_bytes(b.get(24..28)?.try_into().ok()?) as usize;
            Some(u16s(b.get(28..28 + n.checked_mul(2)?)?))
        }
        _ => None,
    }
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
    // The item recorded is still the one at `p`: a path another item took since is never touched.
    let is = |p: &str, id: Id| id_of(p) == Some(id);
    let out = match op {
        Op::Rename { from, to, id } => {
            if !is(&to, id) || (exists(&from) && !from.eq_ignore_ascii_case(&to)) {
                return Vec::new();
            }
            let _ = run(hwnd, quiet, |o| unsafe { o.RenameItem(&item(&to)?, &HSTRING::from(file_name(&from)), None::<&IFileOperationProgressSink>) });
            Op::rename(to, from).into_iter().collect()
        }
        Op::Created(made) => {
            let live: Vec<String> = made.into_iter().filter(|(p, id)| is(p, *id)).map(|(p, _)| p).collect();
            vec![Op::Recycled(recycle(hwnd, &live, quiet))]
        }
        Op::Recycled(pairs) => vec![Op::created(restore(hwnd, &pairs))],
        Op::Moved(moves) => {
            let mut done = Vec::new();
            for (from, to, _) in moves.into_iter().filter(|(f, t, id)| is(t, *id) && !exists(f)) {
                let parent = Path::new(&from).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                let _ = run(hwnd, quiet, |o| unsafe { o.MoveItem(&item(&to)?, &item(&parent)?, &HSTRING::from(file_name(&from)), None::<&IFileOperationProgressSink>) });
                if exists(&from) {
                    done.push((to, from));
                }
            }
            vec![Op::moved(done)]
        }
        Op::Transfer(sent) => {
            let (mut claimed, mut copies, mut moves) = (Vec::new(), Vec::new(), Vec::new());
            // One plan has one destination: read it once.
            let names = sent.first().map(|s| names_lower(&s.dest)).unwrap_or_default();
            for s in &sent {
                let Some(d) = landed(s, &names, &claimed) else { continue };
                claimed.push(d.clone());
                // The source itself landed: a move, even when something took its old path since (then the move back
                // is skipped). A copy only when the source is still there as planned; else neither is proven.
                if is_source(&d, &s.was) || !exists(&s.from) {
                    moves.push((s.from.clone(), d));
                } else if intact(&s.from, &s.was) {
                    copies.push(d);
                }
            }
            let mut v = reverse_op(hwnd, Op::created(copies), quiet);
            v.extend(reverse_op(hwnd, Op::moved(moves), quiet));
            v
        }
    };
    // A reversal that did nothing is no entry.
    out.into_iter().filter(Op::recordable).collect()
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

    /// A fixture with `src\x.txt` ("hello") and an empty `dst`: (fixture, source, dst folder).
    fn transfer_fixture(tag: &str) -> (Fixture, String, String) {
        let fx = fixture(tag);
        std::fs::create_dir_all(fx.0.join("src")).unwrap();
        std::fs::create_dir_all(fx.0.join("dst")).unwrap();
        let src = p(&fx.0, "src\\x.txt");
        std::fs::write(&src, "hello").unwrap();
        let dst = p(&fx.0, "dst");
        (fx, src, dst)
    }

    #[test]
    fn could_be_takes_the_shells_names_for_a_copy() {
        for n in ["x.txt", "X.TXT", "x (2).txt", "x - Copy.txt", "x - copia (3).txt"] {
            assert!(could_be(&n.to_lowercase(), "x.txt", false), "{n}");
        }
        for n in ["xy.txt", "x (2).doc", "x.txt.bak", "y.txt", "x final.txt"] {
            assert!(!could_be(&n.to_lowercase(), "x.txt", false), "{n}");
        }
        // A folder's dot is part of its name.
        assert!(could_be("a.b (2)", "a.b", true));
        assert!(!could_be("a (2).b", "a.b", true));
        assert!(!could_be("a.b backup", "a.b", true));
        assert!(could_be("makefile (2)", "makefile", false));
        assert!(could_be(".gitignore (2)", ".gitignore", false));
    }

    /// `landed` against its destination as it is now.
    fn landed_now(s: &Sent, claimed: &[String]) -> Option<String> {
        landed(s, &names_lower(&s.dest), claimed)
    }

    #[test]
    fn plan_transfer_reads_sources_and_taken_names() {
        let (fx, src, dst) = transfer_fixture("plan");
        std::fs::write(p(&fx.0, "dst\\x.txt"), "theirs").unwrap();
        std::fs::write(p(&fx.0, "dst\\y.txt"), "other").unwrap();
        let got = plan_transfer(&[src.clone(), p(&fx.0, "src\\gone.txt"), s("C:\\")], &dst);
        assert_eq!(got.len(), 1, "a gone source and a drive root give nothing");
        assert_eq!((got[0].from.as_str(), got[0].dest.as_str(), got[0].taken.clone()), (src.as_str(), dst.as_str(), vec![s("x.txt")]));
        assert!(matches!(got[0].was, Was::File(5, _, Some(_))));
        assert!(plan_transfer(&[src], &p(&fx.0, "missing")).is_empty());
    }

    #[test]
    fn landed_finds_a_plain_copy_and_a_keep_both_copy() {
        let (fx, src, dst) = transfer_fixture("landed");
        let plan = plan_transfer(std::slice::from_ref(&src), &dst);
        std::fs::copy(&src, p(&fx.0, "dst\\x.txt")).unwrap();
        assert_eq!(landed_now(&plan[0],&[]), Some(p(&fx.0, "dst\\x.txt")));
        // "Keep both" (a Spanish Windows names a copy into its own folder "x - copia.txt"): the shell's name, not the planned one.
        let plan = plan_transfer(std::slice::from_ref(&src), &dst);
        std::fs::copy(&src, p(&fx.0, "dst\\x - copia.txt")).unwrap();
        assert_eq!(landed_now(&plan[0],&[]), Some(p(&fx.0, "dst\\x - copia.txt")));
    }

    #[test]
    fn landed_refuses_what_it_cannot_prove() {
        let (fx, src, dst) = transfer_fixture("refuse");
        std::fs::write(p(&fx.0, "dst\\x.txt"), "theirs").unwrap();
        let plan = plan_transfer(std::slice::from_ref(&src), &dst);
        // Skip at the conflict: nothing new, and the file that was there is not ours.
        assert_eq!(landed_now(&plan[0],&[]), None);
        // Replace, which may leave the name in another case: still the file that was there.
        std::fs::remove_file(p(&fx.0, "dst\\x.txt")).unwrap();
        std::fs::copy(&src, p(&fx.0, "dst\\X.TXT")).unwrap();
        assert_eq!(landed_now(&plan[0],&[]), None);
        std::fs::remove_file(p(&fx.0, "dst\\X.TXT")).unwrap();
        std::fs::write(p(&fx.0, "dst\\x.txt"), "theirs").unwrap();
        // A new file of a fitting name with other content is somebody else's.
        std::fs::write(p(&fx.0, "dst\\x (2).txt"), "not hello").unwrap();
        assert_eq!(landed_now(&plan[0],&[]), None);
        // Two new copies: which one is ours cannot be told.
        std::fs::copy(&src, p(&fx.0, "dst\\x (3).txt")).unwrap();
        std::fs::copy(&src, p(&fx.0, "dst\\x (4).txt")).unwrap();
        assert_eq!(landed_now(&plan[0],&[]), None);
        // One of them claimed by another source of the same transfer leaves the other.
        assert_eq!(landed_now(&plan[0],&[p(&fx.0, "dst\\x (3).txt")]), Some(p(&fx.0, "dst\\x (4).txt")));
    }

    #[test]
    fn landed_finds_a_moved_source_by_what_it_held() {
        let (fx, src, dst) = transfer_fixture("landedmove");
        std::fs::write(p(&fx.0, "dst\\x.txt"), "theirs").unwrap();
        let plan = plan_transfer(std::slice::from_ref(&src), &dst);
        std::fs::rename(&src, p(&fx.0, "dst\\x (2).txt")).unwrap();
        assert_eq!(landed_now(&plan[0],&[]), Some(p(&fx.0, "dst\\x (2).txt")));
        // A folder lands under its own name.
        std::fs::create_dir_all(fx.0.join("src\\sub")).unwrap();
        let plan = plan_transfer(&[p(&fx.0, "src\\sub")], &dst);
        std::fs::rename(fx.0.join("src\\sub"), fx.0.join("dst\\sub")).unwrap();
        assert_eq!(landed_now(&plan[0],&[]), Some(p(&fx.0, "dst\\sub")));
    }

    #[test]
    fn a_folder_counts_only_when_it_is_the_source_or_a_copy_of_it() {
        let (fx, _, dst) = transfer_fixture("folders");
        std::fs::create_dir_all(fx.0.join("src\\Photos")).unwrap();
        std::fs::write(fx.0.join("src\\Photos\\a.jpg"), "a").unwrap();
        let photos = p(&fx.0, "src\\Photos");
        // Copy shape: the source stays. Another program later makes a folder of a fitting name: not ours.
        let plan = plan_transfer(std::slice::from_ref(&photos), &dst);
        std::fs::create_dir_all(fx.0.join("dst\\Photos - Old")).unwrap();
        std::fs::write(fx.0.join("dst\\Photos - Old\\mine.doc"), "x").unwrap();
        assert_eq!(landed_now(&plan[0], &[]), None);
        // A copy made since the plan holds the same names: ours.
        std::fs::create_dir_all(fx.0.join("dst\\Photos")).unwrap();
        std::fs::write(fx.0.join("dst\\Photos\\A.JPG"), "a").unwrap();
        assert_eq!(landed_now(&plan[0], &[]), Some(p(&fx.0, "dst\\Photos")));
        std::fs::remove_dir_all(fx.0.join("dst\\Photos")).unwrap();
        // Move shape: the source went elsewhere, and the fitting folder is still someone else's.
        let plan = plan_transfer(std::slice::from_ref(&photos), &dst);
        std::fs::rename(&photos, fx.0.join("elsewhere")).unwrap();
        std::fs::create_dir_all(fx.0.join("dst\\Photos (2)")).unwrap();
        assert_eq!(landed_now(&plan[0], &[]), None);
        // Moved in within the volume: the item itself, whatever it holds now.
        std::fs::rename(fx.0.join("elsewhere"), fx.0.join("dst\\Photos")).unwrap();
        std::fs::write(fx.0.join("dst\\Photos\\new.txt"), "n").unwrap();
        assert_eq!(landed_now(&plan[0], &[]), Some(p(&fx.0, "dst\\Photos")));
    }

    #[test]
    fn id_of_tells_a_re_made_file_apart() {
        let fx = fixture("id");
        let f = p(&fx.0, "a.txt");
        std::fs::write(&f, "a").unwrap();
        let id = id_of(&f).unwrap();
        std::fs::write(&f, "edited").unwrap();
        std::fs::rename(&f, p(&fx.0, "b.txt")).unwrap();
        assert_eq!(id_of(&p(&fx.0, "b.txt")), Some(id), "an edit and a rename keep the item");
        std::fs::write(&f, "a").unwrap();
        assert_ne!(id_of(&f), Some(id));
        assert_eq!(id_of(&p(&fx.0, "missing.txt")), None);
        assert!(id_of(&fx.0.to_string_lossy()).is_some(), "folders too");
    }

    #[test]
    fn undo_refuses_an_item_it_did_not_make_or_move() {
        // Each refusal happens before any shell call.
        let fx = fixture("proof");
        let (a, b) = (p(&fx.0, "a.txt"), p(&fx.0, "b.txt"));
        std::fs::write(&a, "made").unwrap();
        let made = Op::created(vec![a.clone()]);
        std::fs::write(&b, "moved").unwrap();
        let moved = Op::moved(vec![(p(&fx.0, "c.txt"), b.clone())]);
        let renamed = Op::rename(p(&fx.0, "d.txt"), b.clone()).unwrap();
        // Both replaced by other items under the same names.
        std::fs::remove_file(&a).unwrap();
        std::fs::remove_file(&b).unwrap();
        std::fs::write(&a, "theirs").unwrap();
        std::fs::write(&b, "theirs").unwrap();
        for op in [made, moved, renamed] {
            assert!(reverse_with(HWND::default(), vec![op.clone()], true).is_empty(), "{op:?}");
        }
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "theirs");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "theirs");
        assert!(!Path::new(&p(&fx.0, "c.txt")).exists() && !Path::new(&p(&fx.0, "d.txt")).exists());
    }

    #[test]
    fn undo_of_a_skipped_transfer_does_nothing() {
        let (fx, src, dst) = transfer_fixture("skipped");
        std::fs::write(p(&fx.0, "dst\\x.txt"), "theirs").unwrap();
        let plan = plan_transfer(std::slice::from_ref(&src), &dst);
        assert!(reverse_with(HWND::default(), vec![Op::Transfer(plan)], true).is_empty());
        assert_eq!(std::fs::read_to_string(p(&fx.0, "dst\\x.txt")).unwrap(), "theirs");
        assert!(Path::new(&src).exists());
    }

    /// A Created entry for the log tests, which never touch the disk.
    fn made(p: &str) -> Op {
        Op::Created(vec![(s(p), (1, [1; 16]))])
    }

    #[test]
    fn log_ignores_a_record_while_busy() {
        let mut l = Log::default();
        l.record(made("C:\\a"));
        assert!(l.take(false).is_some());
        l.record(made("C:\\b"));
        l.put(false, vec![Op::Recycled(vec![(s("C:\\a"), s("bin"))])]);
        assert!(l.take(false).is_none());
        assert!(l.take(true).is_some());
    }

    #[test]
    fn log_moves_entries_between_stacks() {
        let mut l = Log::default();
        let r1 = Op::Rename { from: s("C:\\a\\1"), to: s("C:\\a\\2"), id: (1, [2; 16]) };
        let c = made("C:\\a\\n");
        let rec = Op::Recycled(vec![(s("C:\\a\\n"), s("bin"))]);
        l.record(r1);
        l.record(c.clone());
        assert_eq!(l.take(false), Some(vec![c]));
        l.put(false, vec![rec.clone()]);
        assert_eq!(l.take(true), Some(vec![rec]));
        let back = vec![made("C:\\a\\n")];
        l.put(true, back.clone());
        assert_eq!(l.take(false), Some(back));
        l.put(false, vec![]);
        assert_eq!(l.take(true), None);
        l.record(made("C:\\a\\m"));
        assert_eq!(l.take(true), None);
    }

    #[test]
    fn log_ignores_virtual_empty_and_trims_oldest() {
        let mut l = Log::default();
        l.record(Op::Created(vec![]));
        l.record(Op::Transfer(vec![]));
        l.record(Op::Rename { from: s("::{x}\\a"), to: s("::{x}\\b"), id: (1, [1; 16]) });
        l.record(Op::Rename { from: s("C:\\a"), to: s("C:\\a"), id: (1, [1; 16]) });
        assert_eq!(l.take(false), None);
        for i in 0..40 {
            l.record(made(&format!("C:\\a\\{i}")));
        }
        assert_eq!(l.take(false), Some(vec![made("C:\\a\\39")]));
        l.put(false, vec![]);
        assert_eq!(l.undo.len(), 31);
    }

    #[test]
    fn log_take_refuses_while_busy() {
        let mut l = Log::default();
        l.record(made("C:\\a"));
        l.record(made("C:\\b"));
        assert!(l.take(false).is_some());
        assert!(l.take(false).is_none());
        l.put(false, vec![]);
        assert!(l.take(false).is_some());
    }

    #[test]
    fn long_paths_are_too_long_to_recycle() {
        assert!(!too_long_to_recycle(r"C:\a\b.txt"));
        assert!(!too_long_to_recycle(&format!(r"C:\{}", "a".repeat(256))), "259 units");
        assert!(too_long_to_recycle(&format!(r"C:\{}", "a".repeat(257))));
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
        let r = reverse_with(HWND::default(), vec![Op::rename(a.clone(), b.clone()).unwrap()], true);
        assert!(Path::new(&a).exists() && !Path::new(&b).exists());
        assert_eq!(r, vec![Op::rename(b.clone(), a.clone()).unwrap()]);
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
        let r = reverse_with(HWND::default(), vec![Op::moved(vec![(sx.clone(), dx.clone())])], true);
        assert_eq!(std::fs::read_to_string(&sx).unwrap(), "new");
        assert_eq!(r, vec![Op::moved(vec![(dx.clone(), sx.clone())])]);
        std::fs::write(&dx, "other").unwrap();
        let r = reverse_with(HWND::default(), r, true);
        assert!(r.is_empty());
        assert_eq!(std::fs::read_to_string(&sx).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(&dx).unwrap(), "other");
    }

    #[test]
    fn transfer_whose_source_is_gone_moves_back() {
        // A cut and paste that kept both: undo moves the shell's "x (2).txt" back instead of sending it to the bin.
        let _c = ComInit::sta();
        let (fx, sx, dst) = transfer_fixture("cutpaste");
        std::fs::write(p(&fx.0, "dst\\x.txt"), "theirs").unwrap();
        let plan = plan_transfer(std::slice::from_ref(&sx), &dst);
        let dx = p(&fx.0, "dst\\x (2).txt");
        std::fs::rename(&sx, &dx).unwrap();
        let r = reverse_with(HWND::default(), vec![Op::Transfer(plan)], true);
        assert_eq!(r, vec![Op::moved(vec![(dx.clone(), sx.clone())])]);
        assert_eq!(std::fs::read_to_string(&sx).unwrap(), "hello");
        assert!(!Path::new(&dx).exists());
        assert_eq!(std::fs::read_to_string(p(&fx.0, "dst\\x.txt")).unwrap(), "theirs");
    }

    #[test]
    fn a_moved_folder_stays_when_its_old_path_is_made_again() {
        // The user moves logs into the archive (same volume); a program re-makes an empty logs. Undo must not take
        // the archived folder for a copy and recycle it.
        let _c = ComInit::sta();
        let (fx, _, dst) = transfer_fixture("remade");
        let logs = p(&fx.0, "src\\logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(fx.0.join("src\\logs\\a.log"), "a").unwrap();
        let plan = plan_transfer(std::slice::from_ref(&logs), &dst);
        std::fs::rename(&logs, fx.0.join("dst\\logs")).unwrap();
        std::fs::create_dir_all(&logs).unwrap();
        assert!(reverse_with(HWND::default(), vec![Op::Transfer(plan)], true).is_empty());
        assert_eq!(std::fs::read_to_string(fx.0.join("dst\\logs\\a.log")).unwrap(), "a");
        // A moved file, its old name taken again by a look-alike.
        let x = p(&fx.0, "src\\x.txt");
        let plan = plan_transfer(std::slice::from_ref(&x), &dst);
        std::fs::rename(&x, fx.0.join("dst\\x.txt")).unwrap();
        std::fs::copy(fx.0.join("dst\\x.txt"), &x).unwrap();
        assert!(reverse_with(HWND::default(), vec![Op::Transfer(plan)], true).is_empty());
        assert_eq!(std::fs::read_to_string(fx.0.join("dst\\x.txt")).unwrap(), "hello");
    }

    #[test]
    fn a_partial_move_across_volumes_is_not_a_copy() {
        // A move to another volume copies, then deletes the source; a locked file keeps the source folder with only
        // that file. The destination is the only whole copy: undo leaves it.
        let _c = ComInit::sta();
        let (fx, _, dst) = transfer_fixture("partial");
        let proj = p(&fx.0, "src\\proj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(fx.0.join("src\\proj\\a.txt"), "a").unwrap();
        std::fs::write(fx.0.join("src\\proj\\locked.db"), "l").unwrap();
        let plan = plan_transfer(std::slice::from_ref(&proj), &dst);
        std::fs::create_dir_all(fx.0.join("dst\\proj")).unwrap();
        std::fs::write(fx.0.join("dst\\proj\\a.txt"), "a").unwrap();
        std::fs::write(fx.0.join("dst\\proj\\locked.db"), "l").unwrap();
        std::fs::remove_file(fx.0.join("src\\proj\\a.txt")).unwrap();
        assert!(reverse_with(HWND::default(), vec![Op::Transfer(plan)], true).is_empty());
        assert!(fx.0.join("dst\\proj\\a.txt").exists() && fx.0.join("dst\\proj\\locked.db").exists());
    }

    #[test]
    fn an_empty_look_alike_folder_is_not_a_copy() {
        let (fx, _, dst) = transfer_fixture("emptyalike");
        let e = p(&fx.0, "src\\empty");
        std::fs::create_dir_all(&e).unwrap();
        let plan = plan_transfer(std::slice::from_ref(&e), &dst);
        std::fs::create_dir_all(fx.0.join("dst\\empty")).unwrap();
        assert_eq!(landed_now(&plan[0], &[]), None);
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
        let plan = plan_transfer(std::slice::from_ref(&sx), &p(&fx.0, "dst"));
        std::fs::copy(&sx, &dx).unwrap();
        let r = reverse_with(HWND::default(), vec![Op::Transfer(plan)], true);
        if r.is_empty() && !Path::new(&dx).exists() {
            println!("skipped: no Recycle Bin");
            return;
        }
        assert!(Path::new(&sx).exists() && !Path::new(&dx).exists());
        let Some(Op::Recycled(v)) = r.first() else { panic!("{r:?}") };
        assert_eq!(v[0].0, dx);
        // Back with the bin's verb, synchronously: the entry for redo names it at once.
        let record = record_of(&v[0].1).unwrap();
        assert!(record.exists(), "{record:?}");
        let r2 = reverse_with(HWND::default(), r, true);
        assert!(Path::new(&dx).exists());
        assert_eq!(r2, vec![Op::created(vec![dx.clone()])]);
        assert!(matches!(&r2[0], Op::Created(v) if v.len() == 1));
        assert!(!record.exists(), "no $I record is left in the bin");
    }

    #[test]
    fn bin_records_give_their_original_path() {
        let mut v2 = vec![2, 0, 0, 0, 0, 0, 0, 0];
        v2.extend([0; 16]);
        let path: Vec<u16> = "C:\\a\\b.txt\0".encode_utf16().collect();
        v2.extend((path.len() as u32).to_le_bytes());
        v2.extend(path.iter().flat_map(|c| c.to_le_bytes()));
        assert_eq!(record_path(&v2).as_deref(), Some("C:\\a\\b.txt"));
        assert_eq!(record_path(&v2[..v2.len() - 4]), None, "cut short");
        let mut v1 = vec![1, 0, 0, 0, 0, 0, 0, 0];
        v1.extend([0; 16]);
        v1.extend(path.iter().flat_map(|c| c.to_le_bytes()));
        v1.resize(544, 0);
        assert_eq!(record_path(&v1).as_deref(), Some("C:\\a\\b.txt"));
        assert_eq!(record_path(&[3, 0, 0, 0, 0, 0, 0, 0]), None);
        assert_eq!(record_of("C:\\$RECYCLE.BIN\\S-1\\$RAB12.txt"), Some(std::path::PathBuf::from("C:\\$RECYCLE.BIN\\S-1\\$IAB12.txt")));
        assert_eq!(record_of("C:\\x\\a.txt"), None);
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
