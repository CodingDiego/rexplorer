//! Data operations against temp folders, run quietly through IFileOperation, the shell's copy engine.
//!
//! Paste, drop, Shift+Delete and the New menu hand their work to that engine, so "the same result as Explorer" is
//! asserted as "the same final tree as a direct IFileOperation run", with the answer the conflict dialog would get
//! given as flags (Replace = FOF_NO_UI, Keep both = FOF_RENAMEONCOLLISION) and a sink that cancels. Skip has no flag:
//! it is pressed in the shell's own conflict dialog (`answer_conflict`), which also checks Replace against its flag.
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
    FILEOPERATION_FLAGS, FOF_NO_UI, FOF_RENAMEONCOLLISION, FOFX_RECYCLEONDELETE, FileOperation, IFileOperation, IFileOperationProgressSink,
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

/// Answers Cancel at the first item, as the progress dialog's Cancel does; keeps what a delete it failed to stop
/// put in the Recycle Bin.
#[implement(IFileOperationProgressSink)]
struct CancelAtFirst(std::rc::Rc<std::cell::RefCell<Vec<IShellItem>>>);

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
    fn PostDeleteItem(&self, _: u32, _: Ref<IShellItem>, _: HRESULT, recycled: Ref<IShellItem>) -> Result<()> {
        if let Ok(i) = recycled.ok() {
            self.0.borrow_mut().push(i.clone());
        }
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
    let recycled = std::rc::Rc::default();
    let sink: IFileOperationProgressSink = CancelAtFirst(std::rc::Rc::clone(&recycled)).into();
    for moving in [false, true] {
        let r = op(FOF_NO_UI, Some(&sink), |o| unsafe {
            if moving { o.MoveItem(&item(&folder), &item(&d), PCWSTR::null(), None) } else { o.CopyItem(&item(&folder), &item(&d), PCWSTR::null(), None) }
        });
        assert!(r.is_err() || r == Ok(true), "the cancel is reported: {r:?}");
        assert_eq!(tree(&fx.0.join("src")), src_before, "moving: {moving}");
        assert!(tree(&d).is_empty(), "moving: {moving}");
    }
    // A delete cancelled at its first item, to the bin or for good: everything is still there.
    for flags in [FOF_NO_UI | FOFX_RECYCLEONDELETE, FOF_NO_UI] {
        let r = op(flags, Some(&sink), |o| unsafe { o.DeleteItem(&item(&folder), None) });
        // A cancel not honoured put the folder in the real Recycle Bin: out of it for good before any assert.
        let in_bin: Vec<IShellItem> = recycled.borrow_mut().drain(..).collect();
        if !in_bin.is_empty() {
            let _ = op(FOF_NO_UI, None, |o| in_bin.iter().try_for_each(|i| unsafe { o.DeleteItem(i, None) }));
        }
        assert!(in_bin.is_empty(), "the cancelled delete still recycled {} item(s)", in_bin.len());
        assert!(r.is_err() || r == Ok(true), "the cancel is reported: {r:?}");
        assert_eq!(tree(&fx.0.join("src")), src_before);
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

/// The answers of the shell's own "Replace or Skip Files" dialog, in its order.
const REPLACE: usize = 0;
const SKIP: usize = 1;

/// The push buttons with an action in the MSAA tree under `acc`, grouped by parent, in order (the title bar's
/// buttons are simple children without an object of their own).
fn action_buttons(acc: &windows::Win32::UI::Accessibility::IAccessible, depth: usize, out: &mut Vec<Vec<windows::Win32::UI::Accessibility::IAccessible>>) {
    use windows::Win32::System::Com::IDispatch;
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::{AccessibleChildren, IAccessible, ROLE_SYSTEM_PUSHBUTTON};
    use windows::core::Interface;
    let n = unsafe { acc.accChildCount() }.unwrap_or(0).clamp(0, 256) as usize;
    let mut kids = vec![VARIANT::default(); n];
    let mut got = 0;
    if depth > 12 || unsafe { AccessibleChildren(acc, 0, &mut kids, &mut got) }.is_err() {
        return;
    }
    let me = VARIANT::from(0i32);
    let mut mine = Vec::new();
    for k in kids.iter().take(got.max(0) as usize) {
        let Ok(sub) = IDispatch::try_from(k).and_then(|d| d.cast::<IAccessible>()) else { continue };
        let role = unsafe { sub.get_accRole(&me) }.ok().and_then(|v| i32::try_from(&v).ok());
        let action = unsafe { sub.get_accDefaultAction(&me) }.map(|b| !b.is_empty()).unwrap_or(false);
        if role == Some(ROLE_SYSTEM_PUSHBUTTON as i32) && action {
            mine.push(sub.clone());
        }
        action_buttons(&sub, depth + 1, out);
    }
    if !mine.is_empty() {
        out.push(mine);
    }
}

/// The answers of the copy engine's conflict panel, Replace, Skip and Compare: three action buttons under one
/// parent and none elsewhere. The progress panel has three too, but apart (the details toggle under the dialog,
/// Pause and Cancel under the panel), so the structure tells them apart in any UI language.
fn conflict_answers(groups: Vec<Vec<windows::Win32::UI::Accessibility::IAccessible>>) -> Option<Vec<windows::Win32::UI::Accessibility::IAccessible>> {
    match <[_; 1]>::try_from(groups) {
        Ok([one]) if one.len() == 3 => Some(one),
        _ => None,
    }
}

/// The OperationStatusWindow windows of this process, with whether each is visible.
fn status_windows() -> Vec<(isize, bool)> {
    unsafe extern "system" fn status(h: HWND, found: LPARAM) -> BOOL {
        let mut pid = 0;
        let mut class = [0u16; 32];
        let n = unsafe {
            GetWindowThreadProcessId(h, Some(&mut pid));
            GetClassNameW(h, &mut class)
        } as usize;
        if pid == std::process::id() && String::from_utf16_lossy(&class[..n]) == "OperationStatusWindow" {
            unsafe { (*(found.0 as *mut Vec<(isize, bool)>)).push((h.0 as isize, IsWindowVisible(h).as_bool())) };
        }
        BOOL(1)
    }
    let mut found: Vec<(isize, bool)> = Vec::new();
    let _ = unsafe { EnumWindows(Some(status), LPARAM(&mut found as *mut _ as isize)) };
    found
}

/// Presses answer `pick` (REPLACE, SKIP) in the first conflict panel of the copy engine this process shows within
/// 15 s: the dialog a person answers in Explorer, found by its window class and pressed through its accessibility
/// tree (language-neutral: by structure and position). Err names the buttons last seen when it pressed none.
/// Join it before any assert or unwrap: a thread left running would press a button in the next test's dialog.
fn answer_conflict(pick: usize) -> std::thread::JoinHandle<std::result::Result<(), String>> {
    use windows::Win32::UI::Accessibility::{AccessibleObjectFromWindow, IAccessible};
    use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, WM_CLOSE};
    use windows::core::Interface;
    std::thread::spawn(move || {
        let _c = ComInit::mta();
        let me = windows::Win32::System::Variant::VARIANT::from(0i32);
        let mut last = String::from("no visible OperationStatusWindow");
        for _ in 0..150 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let Some(&(found, _)) = status_windows().iter().find(|w| w.1) else { continue };
            let mut p = std::ptr::null_mut();
            if unsafe { AccessibleObjectFromWindow(HWND(found as *mut _), OBJID_CLIENT.0 as u32, &IAccessible::IID, &mut p) }.is_err() || p.is_null() {
                continue;
            }
            let acc = unsafe { IAccessible::from_raw(p) };
            let mut groups = Vec::new();
            action_buttons(&acc, 0, &mut groups);
            let names: Vec<Vec<String>> = groups.iter().map(|g| g.iter().map(|b| unsafe { b.get_accName(&me) }.map(|n| n.to_string()).unwrap_or_default()).collect()).collect();
            last = format!("action buttons by parent: {names:?}");
            // The progress panel may show first; the answers once the conflict is asked.
            if let Some(answers) = conflict_answers(groups) {
                return unsafe { answers[pick].accDoDefaultAction(&me) }.map_err(|e| format!("{e} pressing in {last}"));
            }
        }
        // Never answered: closing every such window, shown or not, cancels the operation, so the test fails
        // instead of waiting for a person.
        for (h, _) in status_windows() {
            let _ = unsafe { PostMessageW(Some(HWND(h as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        }
        Err(format!("no conflict panel within 15 s; {last}"))
    })
}

/// Waits up to 10 s for `done`.
fn eventually(done: impl Fn() -> bool) -> bool {
    (0..100).any(|_| {
        let ok = done();
        if !ok {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        ok
    })
}

/// src\x.txt "new" over dst\x.txt "old", and src\y.txt "y" whose name is free.
fn conflict_and_free(fx: &Fixture) -> (PathBuf, PathBuf, PathBuf) {
    let (s, d) = conflict(fx);
    let y = fx.0.join("src").join("y.txt");
    std::fs::write(&y, "y").unwrap();
    (s, y, d)
}

#[test]
fn paste_with_skip_in_the_shells_dialog_keeps_both_sides_and_does_the_rest() {
    let _l = SHELL.lock();
    let _c = ComInit::sta();
    for moving in [false, true] {
        let fx = fixture(if moving { "skip-move" } else { "skip-copy" });
        let (x, y, d) = conflict_and_free(&fx);
        let (items, to) = ([item(&x), item(&y)], item(&d));
        let answer = answer_conflict(SKIP);
        // No flags: the engine asks, as for a paste or drop in Explorer.
        let r = op(FILEOPERATION_FLAGS(0), None, |o| unsafe {
            for f in &items {
                if moving { o.MoveItem(f, &to, PCWSTR::null(), None)? } else { o.CopyItem(f, &to, PCWSTR::null(), None)? }
            }
            Ok(())
        });
        let answered = answer.join().unwrap();
        assert_eq!(answered, Ok(()), "the conflict dialog was answered (moving: {moving})");
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(tree(&d), BTreeMap::from([("x.txt".into(), "old".into()), ("y.txt".into(), "y".into())]), "moving: {moving}");
        // The skipped file stays where it was, moving or not; the free one moved only on a move.
        let src: BTreeMap<String, String> = if moving { BTreeMap::from([("x.txt".into(), "new".into())]) } else { BTreeMap::from([("x.txt".into(), "new".into()), ("y.txt".into(), "y".into())]) };
        assert_eq!(tree(&fx.0.join("src")), src, "moving: {moving}");
    }
}

#[test]
fn the_dialogs_replace_matches_the_no_ui_flag() {
    let _l = SHELL.lock();
    let _c = ComInit::sta();
    let fx = fixture("replace-dialog");
    let (x, y, d) = conflict_and_free(&fx);
    let (ix, iy, id) = (item(&x), item(&y), item(&d));
    let answer = answer_conflict(REPLACE);
    let r = op(FILEOPERATION_FLAGS(0), None, |o| unsafe {
        o.CopyItem(&ix, &id, PCWSTR::null(), None)?;
        o.CopyItem(&iy, &id, PCWSTR::null(), None)
    });
    let answered = answer.join().unwrap();
    r.unwrap();
    assert_eq!(answered, Ok(()));
    let by_dialog = tree(&d);
    let fx2 = fixture("replace-flag");
    let (x, y, d) = conflict_and_free(&fx2);
    copy(&x, &d, FOF_NO_UI).unwrap();
    copy(&y, &d, FOF_NO_UI).unwrap();
    assert_eq!(by_dialog, tree(&d));
    assert_eq!(by_dialog, BTreeMap::from([("x.txt".into(), "new".into()), ("y.txt".into(), "y".into())]));
}

#[test]
fn a_drop_on_a_folder_does_what_a_paste_does() {
    use rexplorer_core::dragdrop::{data_object_of_paths, folder_drop_target};
    use windows::Win32::Foundation::POINTL;
    use windows::Win32::System::Ole::{DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE};
    use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
    let _l = SHELL.lock();
    let _c = ComInit::ole();
    // The folder's own drop target, as Explorer's view and rexplorer's list hand a drop to it; the effect it took.
    let drop = |paths: &[&Path], dest: &Path, effect: DROPEFFECT| -> Result<DROPEFFECT> {
        let data = data_object_of_paths(&paths.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>())?;
        let target = folder_drop_target(&ShellFolder::parse(&dest.to_string_lossy())?, HWND::default())?;
        let (keys, pt) = (MODIFIERKEYS_FLAGS(0), POINTL::default());
        let mut e = effect;
        unsafe {
            target.DragEnter(&data, keys, pt, &mut e)?;
            let took = e;
            target.Drop(&data, keys, pt, &mut e)?;
            Ok(took)
        }
    };
    let want = BTreeMap::from([("x.txt".into(), "old".into()), ("y.txt".into(), "y".into())]);
    for (effect, moving) in [(DROPEFFECT_COPY, false), (DROPEFFECT_MOVE, true)] {
        // No conflict: the file lands as a paste lands it.
        let fx = fixture(if moving { "drop-move" } else { "drop-copy" });
        let (x, y, d) = conflict_and_free(&fx);
        assert_eq!(drop(&[&y], &d, effect).unwrap(), effect, "the folder takes the offered effect");
        assert!(eventually(|| d.join("y.txt").exists() && (!moving || !y.exists())), "moving: {moving}");
        // A conflict asks the same dialog; Skip leaves both sides as they were.
        std::fs::write(&y, "y").unwrap();
        std::fs::remove_file(d.join("y.txt")).unwrap();
        let answer = answer_conflict(SKIP);
        let dropped = drop(&[&x, &y], &d, effect);
        let answered = answer.join().unwrap();
        assert_eq!(dropped.unwrap(), effect, "moving: {moving}");
        assert_eq!(answered, Ok(()), "moving: {moving}");
        assert!(eventually(|| tree(&d) == want && (!moving || !y.exists())), "moving: {moving}: {:?}", tree(&d));
        assert_eq!(std::fs::read_to_string(&x).unwrap(), "new", "the skipped source stays");
    }
}

#[test]
fn new_text_document_names_match_the_engine() {
    use windows::Win32::UI::Shell::{ASSOCF_NONE, ASSOCSTR_FRIENDLYDOCNAME, AssocQueryStringW};
    use windows::core::PWSTR;
    let _l = SHELL.lock();
    let _c = ComInit::sta();
    let fx = fixture("new-txt");
    let (a, b) = (fx.0.join("src"), fx.0.join("dst"));
    // The New submenu names the text entry after the .txt type, as the shell's New menu does.
    let mut buf = [0u16; 260];
    let mut n = buf.len() as u32;
    let _ = unsafe { AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_FRIENDLYDOCNAME, &HSTRING::from(".txt"), PCWSTR::null(), Some(PWSTR(buf.as_mut_ptr())), &mut n) };
    let kind = String::from_utf16_lossy(&buf[..(n as usize).saturating_sub(1)]);
    let f = ShellFolder::parse(a.to_str().unwrap()).unwrap();
    let entries = ContextMenu::for_background(&f, HWND::default()).unwrap().new_items().unwrap();
    let Some((id, _)) = entries.iter().find(|(_, t)| t.replace('&', "").eq_ignore_ascii_case(&kind)).cloned() else {
        panic!("no New entry named {kind:?}: {entries:?}");
    };
    // The app's New > Text Document, twice, on a fresh menu each time as the app builds one per use.
    for _ in 0..2 {
        let m = ContextMenu::for_background(&f, HWND::default()).unwrap();
        let items = m.new_items().unwrap();
        let id = items.iter().find(|(_, t)| t.replace('&', "").eq_ignore_ascii_case(&kind)).map_or(id, |e| e.0);
        m.invoke_command(HWND::default(), id).unwrap();
    }
    assert!(eventually(|| tree(&a).len() == 2), "{:?}", tree(&a));
    let made = tree(&a);
    assert!(made.keys().all(|k| k.ends_with(".txt")) && made.values().all(String::is_empty), "{made:?}");
    // The engine, asked for the first name twice, picks the same second name and makes the same empty file.
    let first = made.keys().min_by_key(|k| k.len()).unwrap().clone();
    for _ in 0..2 {
        op(FOF_NO_UI | FOF_RENAMEONCOLLISION, None, |o| unsafe { o.NewItem(&item(&b), 0, &HSTRING::from(&first), PCWSTR::null(), None) }).unwrap();
    }
    assert_eq!(tree(&b), made);
}
