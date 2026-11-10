//! OLE drag and drop through the shell's own data objects and drop targets.
use std::cell::RefCell;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND, POINT, POINTL};
use windows::Win32::System::Threading::Sleep;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{
    DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE,
    IDropSource, IDropTarget, IDropTarget_Impl, OleGetClipboard, RegisterDragDrop, RevokeDragDrop, CF_HDROP, CF_UNICODETEXT,
    ReleaseStgMedium,
};
use windows::Win32::UI::Shell::{BHID_DataObject, CLSID_DragDropHelper, DROPFILES, DragQueryFileW, HDROP, IDropTargetHelper, IShellFolder, SHBindToParent, SHDoDragDrop};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::core::{Interface, Ref, Result, implement, w};

use crate::com::Pidl;
use crate::folder::ShellFolder;
use crate::item::ShellItem;

/// The shell's IDataObject (CF_HDROP, shell ID list...) for items of `folder`.
pub fn data_object(folder: &ShellFolder, items: &[&ShellItem]) -> Result<IDataObject> {
    let pidls: Vec<_> = items.iter().map(|i| i.pidl.as_ptr()).collect();
    unsafe { folder.folder.GetUIObjectOf(HWND::default(), &pidls, None) }
}

/// Drop target for the folder background (dropping into the folder itself).
pub fn folder_drop_target(folder: &ShellFolder, hwnd: HWND) -> Result<IDropTarget> {
    unsafe { folder.folder.CreateViewObject(hwnd) }
}

/// The shell's data object for files of different folders (search results), by parsing name.
pub fn data_object_of_paths(paths: &[String]) -> Result<IDataObject> {
    unsafe { crate::packaged::array_of(paths)?.BindToHandler(None, &BHID_DataObject) }
}

/// Drop target of the item at absolute `pidl` (a folder, a drive, the Recycle Bin), as its parent folder gives it;
/// the same object a drop on that item in its folder's list gets.
pub fn pidl_drop_target(pidl: &Pidl, hwnd: HWND) -> Result<IDropTarget> {
    let mut last = std::ptr::null_mut();
    unsafe {
        let parent: IShellFolder = SHBindToParent(pidl.as_ptr(), Some(&mut last))?;
        parent.GetUIObjectOf(hwnd, &[last as *const _], None)
    }
}

/// Drop target for one item (a subfolder, an executable...).
pub fn item_drop_target(folder: &ShellFolder, item: &ShellItem, hwnd: HWND) -> Result<IDropTarget> {
    unsafe { folder.folder.GetUIObjectOf(hwnd, &[item.pidl.as_ptr()], None) }
}

/// Explorer writes DROPEFFECT 2 (move) for a cut and 5 (copy | link) for a copy.
fn is_move(effect: u32) -> bool {
    effect & DROPEFFECT_MOVE.0 != 0 && effect & DROPEFFECT_COPY.0 == 0
}

/// Paths a cut put on data: its CF_HDROP files when "Preferred DropEffect" says move.
pub fn cut_paths(data: &IDataObject) -> Vec<String> {
    let fmt = |cf: u16| FORMATETC { cfFormat: cf, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
    unsafe {
        let Ok(mut m) = data.GetData(&fmt(RegisterClipboardFormatW(w!("Preferred DropEffect")) as u16)) else { return Vec::new() };
        // The data comes from any program: only a memory block of at least the 4 bytes is read.
        let mut moved = false;
        if m.tymed == TYMED_HGLOBAL.0 as u32 && GlobalSize(m.u.hGlobal) >= 4 {
            let p = GlobalLock(m.u.hGlobal) as *const u32;
            moved = !p.is_null() && is_move(p.read_unaligned());
            let _ = GlobalUnlock(m.u.hGlobal);
        }
        ReleaseStgMedium(&mut m);
        if !moved {
            return Vec::new();
        }
        hdrop_paths(data)
    }
}

/// The CF_HDROP paths `data` carries; none when it has no file list.
pub fn hdrop_paths(data: &IDataObject) -> Vec<String> {
    let fmt = FORMATETC { cfFormat: CF_HDROP.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
    unsafe {
        let Ok(mut m) = data.GetData(&fmt) else { return Vec::new() };
        // From any program: not a memory block, or one shorter than the DROPFILES header, is no file list.
        if m.tymed != TYMED_HGLOBAL.0 as u32 || GlobalSize(m.u.hGlobal) < std::mem::size_of::<DROPFILES>() {
            ReleaseStgMedium(&mut m);
            return Vec::new();
        }
        let h = HDROP(m.u.hGlobal.0);
        let paths = (0..DragQueryFileW(h, u32::MAX, None))
            .map(|i| {
                let mut buf = vec![0u16; DragQueryFileW(h, i, None) as usize + 1];
                let n = DragQueryFileW(h, i, Some(&mut buf)) as usize;
                String::from_utf16_lossy(&buf[..n])
            })
            .collect();
        ReleaseStgMedium(&mut m);
        paths
    }
}

/// The clipboard's cut files, from any program; none on a copy or when it cannot be read.
pub fn clipboard_cut_paths() -> Vec<String> {
    unsafe { OleGetClipboard() }.map(|d| cut_paths(&d)).unwrap_or_default()
}

/// The clipboard's file list (CF_HDROP), copy or cut; empty when it has none or cannot be read.
pub fn clipboard_paths() -> Vec<String> {
    unsafe { OleGetClipboard() }.map(|d| hdrop_paths(&d)).unwrap_or_default()
}

/// Opens the clipboard for `owner`, retrying once: a clipboard manager may hold it for a moment.
fn open_clipboard(owner: HWND) -> bool {
    unsafe {
        OpenClipboard(Some(owner)).is_ok() || {
            Sleep(50);
            OpenClipboard(Some(owner)).is_ok()
        }
    }
}

/// Replaces the clipboard's content with `text` (CF_UNICODETEXT).
pub fn set_clipboard_text(owner: HWND, text: &str) -> Result<()> {
    unsafe {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let block = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2)?;
        let p = GlobalLock(block) as *mut u16;
        if p.is_null() {
            let _ = GlobalFree(Some(block));
            return Err(windows::core::Error::from_thread());
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
        let _ = GlobalUnlock(block);
        if !open_clipboard(owner) {
            let _ = GlobalFree(Some(block));
            return Err(windows::core::Error::from_thread());
        }
        let set = EmptyClipboard().and_then(|()| SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(block.0))).map(|_| ()));
        // The clipboard owns the block once SetClipboardData took it.
        if set.is_err() {
            let _ = GlobalFree(Some(block));
        }
        let _ = CloseClipboard();
        set
    }
}

/// Empties the clipboard (Esc after a cut); does nothing when it cannot be opened.
pub fn clear_clipboard(owner: HWND) {
    unsafe {
        if open_clipboard(owner) {
            let _ = EmptyClipboard();
            let _ = CloseClipboard();
        }
    }
}

/// Runs the shell's modal drag loop for `data`, with its drag image (taken from `hwnd`, a ListView answers
/// DI_GETDRAGIMAGE) and its drop source; returns the effect the target applied (NONE if cancelled).
pub fn drag(data: &IDataObject, hwnd: HWND) -> Result<DROPEFFECT> {
    unsafe { SHDoDragDrop(Some(hwnd), data, None::<&IDropSource>, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK) }
}

type Pick = Box<dyn Fn(POINTL) -> Option<IDropTarget>>;
/// Called with the dropped data just before the target runs the drop.
pub type OnDrop = Box<dyn Fn(POINTL, &IDataObject)>;

/// One drop target registered on a window; forwards to whichever shell target `pick` returns.
#[implement(IDropTarget)]
pub struct DropForwarder {
    hwnd: HWND,
    pick: Pick,
    on_drop: Option<OnDrop>,
    helper: Option<IDropTargetHelper>,
    current: RefCell<Option<IDropTarget>>,
    data: RefCell<Option<IDataObject>>,
}

impl DropForwarder {
    pub fn new(hwnd: HWND, pick: Pick) -> Self {
        // The shell's drag image helper keeps the image moving over our own windows.
        // It is one object for the whole process, handed to every apartment without a proxy: drive it from one thread.
        let helper = unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER) }.ok();
        DropForwarder { hwnd, pick, on_drop: None, helper, current: RefCell::new(None), data: RefCell::new(None) }
    }
}

impl IDropTarget_Impl for DropForwarder_Impl {
    fn DragEnter(&self, pdataobj: Ref<IDataObject>, grfkeystate: MODIFIERKEYS_FLAGS, pt: &POINTL, pdweffect: *mut DROPEFFECT) -> Result<()> {
        // No RefCell borrow is held across a call out: a target's Drop may pump messages and bring another drag here.
        let data = pdataobj.as_ref().cloned();
        *self.data.borrow_mut() = data.clone();
        let target = (self.pick)(*pt);
        *self.current.borrow_mut() = target.clone();
        let r = match target {
            Some(t) => unsafe { t.DragEnter(data.as_ref(), grfkeystate, *pt, pdweffect) },
            None => {
                unsafe { *pdweffect = DROPEFFECT_NONE };
                Ok(())
            }
        };
        if let Some(h) = &self.helper {
            let _ = unsafe { h.DragEnter(self.hwnd, data.as_ref(), &POINT { x: pt.x, y: pt.y }, *pdweffect) };
        }
        r
    }

    fn DragOver(&self, grfkeystate: MODIFIERKEYS_FLAGS, pt: &POINTL, pdweffect: *mut DROPEFFECT) -> Result<()> {
        let target = (self.pick)(*pt);
        let same = match (&target, &*self.current.borrow()) {
            (Some(a), Some(b)) => a.as_raw() == b.as_raw(),
            (None, None) => true,
            _ => false,
        };
        if !same {
            let old = self.current.replace(target.clone());
            if let Some(old) = old {
                let _ = unsafe { old.DragLeave() };
            }
            if let Some(t) = &target {
                let data = self.data.borrow().clone();
                unsafe { t.DragEnter(data.as_ref(), grfkeystate, *pt, pdweffect)? };
            }
        }
        let r = match target {
            Some(t) => unsafe { t.DragOver(grfkeystate, *pt, pdweffect) },
            None => {
                unsafe { *pdweffect = DROPEFFECT_NONE };
                Ok(())
            }
        };
        if let Some(h) = &self.helper {
            let _ = unsafe { h.DragOver(&POINT { x: pt.x, y: pt.y }, *pdweffect) };
        }
        r
    }

    fn DragLeave(&self) -> Result<()> {
        if let Some(h) = &self.helper {
            let _ = unsafe { h.DragLeave() };
        }
        *self.data.borrow_mut() = None;
        let current = self.current.take();
        match current {
            Some(t) => unsafe { t.DragLeave() },
            None => Ok(()),
        }
    }

    fn Drop(&self, pdataobj: Ref<IDataObject>, grfkeystate: MODIFIERKEYS_FLAGS, pt: &POINTL, pdweffect: *mut DROPEFFECT) -> Result<()> {
        // First: a right-drag drop opens the shell's menu inside the forwarded Drop, and the image must be gone by then.
        if let Some(h) = &self.helper {
            let _ = unsafe { h.Drop(pdataobj.as_ref(), &POINT { x: pt.x, y: pt.y }, *pdweffect) };
        }
        *self.data.borrow_mut() = None;
        let current = self.current.take();
        // Before the target runs: the hook reads which destination names are free.
        if let (Some(_), Some(f), Some(d)) = (&current, &self.on_drop, pdataobj.as_ref()) {
            f(*pt, d);
        }
        match current {
            Some(t) => unsafe { t.Drop(pdataobj.as_ref(), grfkeystate, *pt, pdweffect) },
            None => {
                unsafe { *pdweffect = DROPEFFECT_NONE };
                Ok(())
            }
        }
    }
}

/// A drop target that only reports where data was dropped (rearranging icons inside one view): it shows MOVE while
/// the pointer is over it and ends the drop with NONE, so the drag source never deletes or moves anything.
#[implement(IDropTarget)]
struct PositionTarget(Box<dyn Fn(POINTL)>);

impl IDropTarget_Impl for PositionTarget_Impl {
    fn DragEnter(&self, _: Ref<IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, pdweffect: *mut DROPEFFECT) -> Result<()> {
        unsafe { *pdweffect = DROPEFFECT_MOVE };
        Ok(())
    }
    fn DragOver(&self, _: MODIFIERKEYS_FLAGS, _: &POINTL, pdweffect: *mut DROPEFFECT) -> Result<()> {
        unsafe { *pdweffect = DROPEFFECT_MOVE };
        Ok(())
    }
    fn DragLeave(&self) -> Result<()> {
        Ok(())
    }
    fn Drop(&self, _: Ref<IDataObject>, _: MODIFIERKEYS_FLAGS, pt: &POINTL, pdweffect: *mut DROPEFFECT) -> Result<()> {
        (self.0)(*pt);
        unsafe { *pdweffect = DROPEFFECT_NONE };
        Ok(())
    }
}

pub fn position_target(on_drop: Box<dyn Fn(POINTL)>) -> IDropTarget {
    PositionTarget(on_drop).into()
}

/// Registers a forwarding drop target on `hwnd`. OLE must be initialized (`ComInit::ole`).
pub fn register(hwnd: HWND, pick: Pick, on_drop: Option<OnDrop>) -> Result<()> {
    let target: IDropTarget = DropForwarder { on_drop, ..DropForwarder::new(hwnd, pick) }.into();
    unsafe { RegisterDragDrop(hwnd, &target) }
}

/// Removes the drop target from `hwnd`; errors are ignored.
pub fn unregister(hwnd: HWND) {
    let _ = unsafe { RevokeDragDrop(hwnd) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::ComInit;
    use windows::Win32::System::Com::{STGMEDIUM, STGMEDIUM_0};
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc};
    use windows::Win32::System::SystemServices::MK_LBUTTON;

    struct Fixture(std::path::PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The drag image helper every DropForwarder holds is one process-wide object (the app drives it from its UI
    /// thread only); test threads using it at once corrupt the heap inside shell32. Held for a whole test.
    static HELPER: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn one_helper_user() -> std::sync::MutexGuard<'static, ()> {
        HELPER.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn fixture(tag: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!("rexplorer-dragdrop-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        // The shell answers with long names; %TEMP% can be a short one (RUNNER~1 on CI).
        let long = std::fs::canonicalize(&dir).unwrap();
        Fixture(std::path::PathBuf::from(long.to_string_lossy().trim_start_matches(r"\\?\")))
    }

    /// A shell target whose Drop pumps messages (the right-drag menu, a copy's progress) while another drag
    /// reaches the same window.
    #[implement(IDropTarget)]
    struct Pumping(std::rc::Rc<RefCell<Option<IDropTarget>>>);

    impl IDropTarget_Impl for Pumping_Impl {
        fn DragEnter(&self, _: Ref<IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, _: *mut DROPEFFECT) -> Result<()> {
            Ok(())
        }
        fn DragOver(&self, _: MODIFIERKEYS_FLAGS, _: &POINTL, _: *mut DROPEFFECT) -> Result<()> {
            Ok(())
        }
        fn DragLeave(&self) -> Result<()> {
            Ok(())
        }
        fn Drop(&self, data: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, _: &POINTL, _: *mut DROPEFFECT) -> Result<()> {
            let fwd = self.0.borrow().clone().unwrap();
            let mut effect = DROPEFFECT_COPY;
            unsafe {
                fwd.DragEnter(data.as_ref(), keys, POINTL { x: 300, y: 0 }, &mut effect)?;
                fwd.DragLeave()
            }
        }
    }

    #[test]
    fn forwarder_survives_a_drag_during_drop() {
        let _one = one_helper_user();
        let _c = ComInit::ole();
        let fx = fixture("reenter");
        let f = ShellFolder::parse(fx.0.to_str().unwrap()).unwrap();
        let items = f.enumerate().unwrap();
        let data = data_object(&f, &[items.iter().find(|i| !i.is_folder()).unwrap()]).unwrap();
        let slot = std::rc::Rc::new(RefCell::new(None));
        let inner: IDropTarget = Pumping(slot.clone()).into();
        let fwd: IDropTarget = DropForwarder::new(HWND::default(), Box::new(move |p| (p.x < 100).then(|| inner.clone()))).into();
        *slot.borrow_mut() = Some(fwd.clone());
        let keys = MODIFIERKEYS_FLAGS(MK_LBUTTON.0);
        let mut effect = DROPEFFECT_COPY;
        unsafe {
            fwd.DragEnter(&data, keys, POINTL { x: 0, y: 0 }, &mut effect).unwrap();
            fwd.Drop(&data, keys, POINTL { x: 0, y: 0 }, &mut effect).unwrap();
        }
        slot.borrow_mut().take();
    }

    /// A shell target that only notes its Drop.
    #[implement(IDropTarget)]
    struct Recorder(std::rc::Rc<RefCell<Vec<&'static str>>>);

    impl IDropTarget_Impl for Recorder_Impl {
        fn DragEnter(&self, _: Ref<IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, _: *mut DROPEFFECT) -> Result<()> {
            Ok(())
        }
        fn DragOver(&self, _: MODIFIERKEYS_FLAGS, _: &POINTL, _: *mut DROPEFFECT) -> Result<()> {
            Ok(())
        }
        fn DragLeave(&self) -> Result<()> {
            Ok(())
        }
        fn Drop(&self, _: Ref<IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, _: *mut DROPEFFECT) -> Result<()> {
            self.0.borrow_mut().push("target");
            Ok(())
        }
    }

    #[test]
    fn forwarder_calls_the_drop_hook_before_the_target() {
        let _one = one_helper_user();
        let _c = ComInit::ole();
        let fx = fixture("onDrop");
        let f = ShellFolder::parse(fx.0.to_str().unwrap()).unwrap();
        let items = f.enumerate().unwrap();
        let data = data_object(&f, &[items.iter().find(|i| !i.is_folder()).unwrap()]).unwrap();
        let log = std::rc::Rc::new(RefCell::new(Vec::new()));
        let inner: IDropTarget = Recorder(log.clone()).into();
        let log2 = log.clone();
        let fwd: IDropTarget = DropForwarder {
            on_drop: Some(Box::new(move |_, _| log2.borrow_mut().push("hook"))),
            ..DropForwarder::new(HWND::default(), Box::new(move |p| (p.x < 100).then(|| inner.clone())))
        }
        .into();
        let keys = MODIFIERKEYS_FLAGS(MK_LBUTTON.0);
        let mut effect = DROPEFFECT_COPY;
        unsafe {
            fwd.DragEnter(&data, keys, POINTL { x: 0, y: 0 }, &mut effect).unwrap();
            fwd.Drop(&data, keys, POINTL { x: 0, y: 0 }, &mut effect).unwrap();
        }
        assert_eq!(*log.borrow(), ["hook", "target"]);
        log.borrow_mut().clear();
        unsafe {
            fwd.DragEnter(&data, keys, POINTL { x: 300, y: 0 }, &mut effect).unwrap();
            fwd.Drop(&data, keys, POINTL { x: 300, y: 0 }, &mut effect).unwrap();
        }
        assert!(log.borrow().is_empty());
    }

    #[test]
    fn data_object_exposes_hdrop() {
        let _c = ComInit::ole();
        let fx = fixture("hdrop");
        let f = ShellFolder::parse(fx.0.to_str().unwrap()).unwrap();
        let items = f.enumerate().unwrap();
        let file = items.iter().find(|i| !i.is_folder()).unwrap();
        let data = data_object(&f, &[file]).unwrap();
        let fmt = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        assert!(unsafe { data.QueryGetData(&fmt) }.is_ok());
        // What browsers and chat apps read: the DROPFILES block lists exactly the dragged path.
        let mut medium = unsafe { data.GetData(&fmt) }.unwrap();
        let hdrop = HDROP(unsafe { medium.u.hGlobal }.0);
        let n = unsafe { DragQueryFileW(hdrop, u32::MAX, None) };
        let mut buf = [0u16; 512];
        let len = unsafe { DragQueryFileW(hdrop, 0, Some(&mut buf)) } as usize;
        unsafe { ReleaseStgMedium(&mut medium) };
        assert_eq!(n, 1);
        assert_eq!(String::from_utf16_lossy(&buf[..len]), fx.0.join("a.txt").to_str().unwrap());
    }

    #[test]
    fn cut_effect_is_move_only() {
        assert!(is_move(2));
        assert!([5, 1, 3, 0].into_iter().all(|e| !is_move(e)));
    }

    #[test]
    fn cut_paths_reads_a_move_data_object() {
        let _c = ComInit::ole();
        let fx = fixture("cut");
        let f = ShellFolder::parse(fx.0.to_str().unwrap()).unwrap();
        let items = f.enumerate().unwrap();
        let file = items.iter().find(|i| !i.is_folder()).unwrap();
        let data = data_object(&f, &[file]).unwrap();
        assert!(cut_paths(&data).is_empty(), "no effect means not a cut");
        let fmt = FORMATETC {
            cfFormat: unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) } as u16,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let set = |effect: u32| unsafe {
            let h = GlobalAlloc(GMEM_MOVEABLE, 4).unwrap();
            (GlobalLock(h) as *mut u32).write(effect);
            let _ = GlobalUnlock(h);
            let medium = STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: h }, pUnkForRelease: std::mem::ManuallyDrop::new(None) };
            data.SetData(&fmt, &medium, true).unwrap();
        };
        set(2);
        assert_eq!(cut_paths(&data), [fx.0.join("a.txt").to_str().unwrap()]);
        set(5);
        assert!(cut_paths(&data).is_empty());
    }

    #[test]
    fn data_object_of_paths_spans_folders() {
        let _c = ComInit::ole();
        let fx = fixture("multi");
        std::fs::write(fx.0.join("sub").join("b.txt"), b"y").unwrap();
        let paths = vec![fx.0.join("a.txt").to_str().unwrap().to_string(), fx.0.join("sub").join("b.txt").to_str().unwrap().to_string()];
        assert_eq!(hdrop_paths(&data_object_of_paths(&paths).unwrap()), paths);
    }

    #[test]
    fn pidl_drop_target_accepts_a_file() {
        let _c = ComInit::ole();
        let fx = fixture("pidltarget");
        let f = ShellFolder::parse(fx.0.to_str().unwrap()).unwrap();
        let sub = ShellFolder::parse(fx.0.join("sub").to_str().unwrap()).unwrap();
        let items = f.enumerate().unwrap();
        let file = items.iter().find(|i| !i.is_folder()).unwrap();
        let data = data_object(&f, &[file]).unwrap();
        let t = pidl_drop_target(&sub.pidl, HWND::default()).unwrap();
        let mut effect = DROPEFFECT_COPY | DROPEFFECT_MOVE;
        unsafe {
            t.DragEnter(&data, MODIFIERKEYS_FLAGS(MK_LBUTTON.0), POINTL { x: 0, y: 0 }, &mut effect).unwrap();
            assert!(effect.0 & (DROPEFFECT_COPY | DROPEFFECT_MOVE).0 != 0);
            t.DragLeave().unwrap();
        }
    }

    #[test]
    fn drop_targets_build() {
        let _c = ComInit::ole();
        let fx = fixture("targets");
        let f = ShellFolder::parse(fx.0.to_str().unwrap()).unwrap();
        let items = f.enumerate().unwrap();
        let sub = items.iter().find(|i| i.is_folder()).unwrap();
        assert!(folder_drop_target(&f, HWND::default()).is_ok());
        assert!(item_drop_target(&f, sub, HWND::default()).is_ok());
    }

    #[test]
    fn forwarder_switches_targets() {
        let _one = one_helper_user();
        let _c = ComInit::ole();
        let fx = fixture("fwd");
        let f = ShellFolder::parse(fx.0.to_str().unwrap()).unwrap();
        let items = f.enumerate().unwrap();
        let file = items.iter().find(|i| !i.is_folder()).unwrap();
        let data = data_object(&f, &[file]).unwrap();
        let target = folder_drop_target(&f, HWND::default()).unwrap();
        let fwd: IDropTarget = DropForwarder::new(HWND::default(), Box::new(move |p| (p.x < 100).then(|| target.clone()))).into();
        let keys = MODIFIERKEYS_FLAGS(MK_LBUTTON.0);
        let mut effect = DROPEFFECT_COPY | DROPEFFECT_MOVE;
        unsafe {
            fwd.DragEnter(&data, keys, POINTL { x: 10, y: 0 }, &mut effect).unwrap();
            fwd.DragOver(keys, POINTL { x: 20, y: 0 }, &mut effect).unwrap();
            fwd.DragOver(keys, POINTL { x: 200, y: 0 }, &mut effect).unwrap();
            assert_eq!(effect, DROPEFFECT_NONE);
            fwd.DragOver(keys, POINTL { x: 30, y: 0 }, &mut effect).unwrap();
            fwd.DragLeave().unwrap();
            effect = DROPEFFECT_COPY;
            fwd.DragEnter(&data, keys, POINTL { x: 300, y: 0 }, &mut effect).unwrap();
            assert_eq!(effect, DROPEFFECT_NONE);
            fwd.DragLeave().unwrap();
            // No target there: runs the drag image helper path without any file operation.
            effect = DROPEFFECT_COPY;
            fwd.DragEnter(&data, keys, POINTL { x: 300, y: 0 }, &mut effect).unwrap();
            fwd.Drop(&data, keys, POINTL { x: 300, y: 0 }, &mut effect).unwrap();
            assert_eq!(effect, DROPEFFECT_NONE);
        }
    }

    #[test]
    fn position_target_moves_nothing() {
        let seen = std::rc::Rc::new(RefCell::new(Vec::new()));
        let log = seen.clone();
        let t = position_target(Box::new(move |pt| log.borrow_mut().push((pt.x, pt.y))));
        let keys = MODIFIERKEYS_FLAGS(0);
        let mut e = DROPEFFECT_COPY;
        unsafe {
            t.DragEnter(None, keys, POINTL { x: 5, y: 6 }, &mut e).unwrap();
            assert_eq!(e, DROPEFFECT_MOVE);
            e = DROPEFFECT_COPY;
            t.DragOver(keys, POINTL { x: 5, y: 6 }, &mut e).unwrap();
            assert_eq!(e, DROPEFFECT_MOVE);
            t.Drop(None, keys, POINTL { x: 7, y: 8 }, &mut e).unwrap();
            assert_eq!(e, DROPEFFECT_NONE);
        }
        assert_eq!(*seen.borrow(), [(7, 8)]);
    }
}
