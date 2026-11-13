//! Data operations against temp folders, run quietly through IFileOperation, the shell's copy engine.
//!
//! Paste, drop, Shift+Delete and the New menu hand their work to that engine, so "the same result as Explorer" is
//! asserted as "the same final tree as a direct IFileOperation run", with the answer the conflict dialog would get
//! given as flags (Replace = FOF_NO_UI, Keep both = FOF_RENAMEONCOLLISION) and a sink that cancels.
//! These touch COM and the Recycle Bin: one at a time, never alongside other shell tests under load.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rexplorer_core::ShellFolder;
use rexplorer_core::com::ComInit;
use rexplorer_core::menu::ContextMenu;
use rexplorer_core::undo::{self, Op};
use windows::Win32::Foundation::{E_ABORT, HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::TDM_CLICK_BUTTON;
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible, PostMessageW};
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Shell::{
    FILEOPERATION_FLAGS, FOF_NO_UI, FOF_RENAMEONCOLLISION, FileOperation, IFileOperation, IFileOperationProgressSink,
    IFileOperationProgressSink_Impl, IShellItem, SHCreateItemFromParsingName,
};
use windows::core::{BOOL, HRESULT, HSTRING, PCWSTR, Ref, Result, implement};

static SHELL: Mutex<()> = Mutex::new(());

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture(tag: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("rexplorer-dataops-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("dst")).unwrap();
    Fixture(dir)
}

fn item(p: &Path) -> IShellItem {
    unsafe { SHCreateItemFromParsingName(&HSTRING::from(p.as_os_str()), None) }.unwrap()
}

/// Relative path (lowercase) to file content, "<dir>" for folders: the whole tree under `root`.
fn tree(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut todo = vec![root.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let rel = e.path().strip_prefix(root).unwrap().to_string_lossy().to_lowercase();
            if e.path().is_dir() {
                out.insert(rel, "<dir>".into());
                todo.push(e.path());
            } else {
                out.insert(rel, std::fs::read_to_string(e.path()).unwrap_or_default());
            }
        }
    }
    out
}

fn op(flags: FILEOPERATION_FLAGS, sink: Option<&IFileOperationProgressSink>, add: impl FnOnce(&IFileOperation) -> Result<()>) -> Result<bool> {
    unsafe {
        let o: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)?;
        o.SetOperationFlags(flags)?;
        let cookie = sink.map(|s| o.Advise(s)).transpose()?;
        add(&o)?;
        let r = o.PerformOperations();
        if let Some(c) = cookie {
            let _ = o.Unadvise(c);
        }
        let aborted = o.GetAnyOperationsAborted().map(|b| b.as_bool()).unwrap_or(false);
        r.map(|_| aborted)
    }
}

/// Clicks button `id` (6 = Yes, 7 = No) of the first dialog this process shows within 10 s: the shell's own
/// confirmation, which would otherwise wait for a person. True when it found one.
fn answer_dialog(id: usize) -> std::thread::JoinHandle<bool> {
    unsafe extern "system" fn ours(h: HWND, found: LPARAM) -> BOOL {
        let mut pid = 0;
        let mut class = [0u16; 8];
        let n = unsafe {
            GetWindowThreadProcessId(h, Some(&mut pid));
            GetClassNameW(h, &mut class)
        } as usize;
        if pid == std::process::id() && String::from_utf16_lossy(&class[..n]) == "#32770" && unsafe { IsWindowVisible(h) }.as_bool() {
            unsafe { *(found.0 as *mut isize) = h.0 as isize };
            return BOOL(0);
        }
        BOOL(1)
    }
    std::thread::spawn(move || {
        for _ in 0..100 {
            let mut found = 0isize;
            let _ = unsafe { EnumWindows(Some(ours), LPARAM(&mut found as *mut isize as isize)) };
            if found != 0 {
                let h = HWND(found as *mut _);
                return unsafe { PostMessageW(Some(h), TDM_CLICK_BUTTON.0 as u32, WPARAM(id), LPARAM(0)) }.is_ok();
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        false
    })
}

fn copy(from: &Path, to_dir: &Path, flags: FILEOPERATION_FLAGS) -> Result<bool> {
    op(flags, None, |o| unsafe { o.CopyItem(&item(from), &item(to_dir), PCWSTR::null(), None) })
}

fn mv(from: &Path, to_dir: &Path, flags: FILEOPERATION_FLAGS) -> Result<bool> {
    op(flags, None, |o| unsafe { o.MoveItem(&item(from), &item(to_dir), PCWSTR::null(), None) })
}

/// src\x.txt "new" and dst\x.txt "old": the name conflict the dialog asks about.
fn conflict(fx: &Fixture) -> (PathBuf, PathBuf) {
    let (s, d) = (fx.0.join("src").join("x.txt"), fx.0.join("dst"));
    std::fs::write(&s, "new").unwrap();
    std::fs::write(d.join("x.txt"), "old").unwrap();
    (s, d)
}

#[test]
fn copy_onto_an_existing_name_replace() {
    let _l = SHELL.lock();
    let _c = ComInit::sta();
    let fx = fixture("copy-replace");
    let (s, d) = conflict(&fx);
    copy(&s, &d, FOF_NO_UI).unwrap();
    assert_eq!(tree(&d), BTreeMap::from([("x.txt".into(), "new".into())]));
    assert_eq!(std::fs::read_to_string(&s).unwrap(), "new", "the source stays");
}

#[test]
fn copy_onto_an_existing_name_keep_both() {
    let _l = SHELL.lock();
    let _c = ComInit::sta();
    let fx = fixture("copy-both");
    let (s, d) = conflict(&fx);
    copy(&s, &d, FOF_NO_UI | FOF_RENAMEONCOLLISION).unwrap();
    let t = tree(&d);
    assert_eq!(t.get("x.txt").map(String::as_str), Some("old"), "the file already there is untouched");
    assert_eq!(t.len(), 2);
    assert!(t.values().any(|v| v == "new"));
    assert!(s.exists());
}

#[test]
fn move_onto_an_existing_name_replace_and_keep_both() {
    let _l = SHELL.lock();
    let _c = ComInit::sta();
    let fx = fixture("move");
    let (s, d) = conflict(&fx);
    mv(&s, &d, FOF_NO_UI).unwrap();
    assert!(!s.exists());
    assert_eq!(tree(&d), BTreeMap::from([("x.txt".into(), "new".into())]));
    std::fs::write(&s, "newer").unwrap();
    mv(&s, &d, FOF_NO_UI | FOF_RENAMEONCOLLISION).unwrap();
    let t = tree(&d);
    assert!(!s.exists());
    assert_eq!((t.len(), t.get("x.txt").map(String::as_str)), (2, Some("new")));
    assert!(t.values().any(|v| v == "newer"));
}

#[test]
fn delete_to_the_bin_comes_back_and_permanent_does_not_use_it() {
    let _l = SHELL.lock();
    let _c = ComInit::sta();
    let fx = fixture("delete");
    let n = format!("rexplorer-dataops-{}.txt", std::process::id());
    let a = fx.0.join("src").join(&n);
    std::fs::write(&a, "a").unwrap();
    let a_str = a.to_string_lossy().into_owned();
    // The app's Delete key: undo::recycle, here without UI.
    let done = undo::recycle(HWND::default(), std::slice::from_ref(&a_str), true);
    if done.is_empty() && !a.exists() {
        println!("skipped: no Recycle Bin");
        return;
    }
    assert!(!a.exists());
    assert_eq!(done[0].0, a_str);
    // Ctrl+Z puts it back with the bin's own verb.
    let back = undo::reverse(HWND::default(), vec![Op::Recycled(done)]);
    for _ in 0..50 {
        if a.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(back, vec![Op::created(vec![a_str.clone()])]);
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "a");
    // Shift+Delete: the same engine without FOFX_RECYCLEONDELETE; gone, and nothing to restore.
    op(FOF_NO_UI, None, |o| unsafe { o.DeleteItem(&item(&a), None) }).unwrap();
    assert!(!a.exists());
}

#[test]
fn rename_onto_an_existing_name_loses_nothing() {
    let _l = SHELL.lock();
    let _c = ComInit::sta();
    let fx = fixture("rename");
    let d = fx.0.join("dst");
    std::fs::write(d.join("a.txt"), "a").unwrap();
    std::fs::write(d.join("b.txt"), "b").unwrap();
    // Refused or renamed to a free name: either way b.txt keeps its content and a's content is still there.
    // F2 renames through ShellItem::rename (IShellFolder::SetNameOf), not IFileOperation: FOF_NO_UI would answer
    // the collision with "Yes to all" and overwrite b.txt, which the app never asks for. The shell asks, as Explorer
    // does, whether to rename to "b (2).txt"; the helper answers Yes.
    let (_, mut items) = rexplorer_core::folder::list(d.to_str().unwrap()).unwrap();
    let a_item = items.iter_mut().find(|i| i.parsing_name().to_lowercase().ends_with("a.txt")).unwrap();
    let answer = answer_dialog(6);
    let _ = a_item.rename(HWND::default(), "b.txt", true);
    let asked = answer.join().unwrap_or(false);
    let t = tree(&d);
    assert_eq!(t.get("b.txt").map(String::as_str), Some("b"));
    assert!(t.values().any(|v| v == "a"), "{t:?}");
    // Undo of a rename never lands on a taken name either.
    let a = d.join("a.txt").to_string_lossy().into_owned();
    assert!(!asked || !Path::new(&a).exists(), "Yes renames to a free name: {t:?}");
    if !Path::new(&a).exists() {
        let to = t.keys().find(|k| t[*k] == "a").map(|k| d.join(k).to_string_lossy().into_owned()).unwrap();
        std::fs::write(&a, "taken").unwrap();
        assert!(undo::reverse(HWND::default(), vec![Op::rename(a.clone(), to).unwrap()]).is_empty());
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "taken");
    }
}

/// Answers Cancel at the first item, as the progress dialog's Cancel does.
#[implement(IFileOperationProgressSink)]
struct CancelAtFirst;

impl IFileOperationProgressSink_Impl for CancelAtFirst_Impl {
    fn StartOperations(&self) -> Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, _: HRESULT) -> Result<()> {
        Ok(())
    }
    fn PreRenameItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> Result<()> {
        Err(E_ABORT.into())
    }
    fn PostRenameItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR, _: HRESULT, _: Ref<IShellItem>) -> Result<()> {
        Ok(())
    }
    fn PreMoveItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR) -> Result<()> {
        Err(E_ABORT.into())
    }
    fn PostMoveItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR, _: HRESULT, _: Ref<IShellItem>) -> Result<()> {
        Ok(())
    }
    fn PreCopyItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR) -> Result<()> {
        Err(E_ABORT.into())
    }
    fn PostCopyItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR, _: HRESULT, _: Ref<IShellItem>) -> Result<()> {
        Ok(())
    }
    fn PreDeleteItem(&self, _: u32, _: Ref<IShellItem>) -> Result<()> {
        Err(E_ABORT.into())
    }
    fn PostDeleteItem(&self, _: u32, _: Ref<IShellItem>, _: HRESULT, _: Ref<IShellItem>) -> Result<()> {
        Ok(())
    }
    fn PreNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> Result<()> {
        Err(E_ABORT.into())
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

#[test]
fn cancel_leaves_the_source_and_no_partial_destination() {
    let _l = SHELL.lock();
    let _c = ComInit::sta();
    let fx = fixture("cancel");
    let folder = fx.0.join("src").join("tree");
    std::fs::create_dir_all(folder.join("sub")).unwrap();
    std::fs::write(folder.join("one.txt"), "1").unwrap();
    std::fs::write(folder.join("sub").join("two.txt"), "2").unwrap();
    let (src_before, d) = (tree(&fx.0.join("src")), fx.0.join("dst"));
    let sink: IFileOperationProgressSink = CancelAtFirst.into();
    for moving in [false, true] {
        let r = op(FOF_NO_UI, Some(&sink), |o| unsafe {
            if moving { o.MoveItem(&item(&folder), &item(&d), PCWSTR::null(), None) } else { o.CopyItem(&item(&folder), &item(&d), PCWSTR::null(), None) }
        });
        assert!(r.is_err() || r == Ok(true), "the cancel is reported: {r:?}");
        assert_eq!(tree(&fx.0.join("src")), src_before, "moving: {moving}");
        assert!(tree(&d).is_empty(), "moving: {moving}");
    }
}

#[test]
fn new_folder_names_match_the_engine() {
    let _l = SHELL.lock();
    let _c = ComInit::sta();
    let fx = fixture("new");
    let (a, b) = (fx.0.join("src"), fx.0.join("dst"));
    // The app's New > Folder, twice: the second gets a free name.
    let f = ShellFolder::parse(a.to_str().unwrap()).unwrap();
    for _ in 0..2 {
        ContextMenu::for_background(&f, HWND::default()).unwrap().invoke_verb(HWND::default(), "NewFolder", false).unwrap();
    }
    let made = tree(&a);
    assert_eq!(made.len(), 2, "{made:?}");
    // The engine, asked for the first name twice, picks the same second name.
    let first = made.keys().min_by_key(|k| k.len()).unwrap().clone();
    for _ in 0..2 {
        op(FOF_NO_UI | FOF_RENAMEONCOLLISION, None, |o| unsafe { o.NewItem(&item(&b), FILE_ATTRIBUTE_DIRECTORY.0, &HSTRING::from(&first), PCWSTR::null(), None) }).unwrap();
    }
    assert_eq!(tree(&b), made);
}
