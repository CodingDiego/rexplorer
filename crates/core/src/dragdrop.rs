//! OLE drag and drop through the shell's own data objects and drop targets.
use std::cell::RefCell;

use windows::Win32::Foundation::{
    DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND, POINTL,
};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Ole::{
    DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE, DoDragDrop,
    IDropSource, IDropSource_Impl, IDropTarget, IDropTarget_Impl, RegisterDragDrop, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MK_RBUTTON, MODIFIERKEYS_FLAGS};
use windows::core::{BOOL, HRESULT, Interface, Ref, Result, implement};

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

/// Drop target for one item (a subfolder, an executable...).
pub fn item_drop_target(folder: &ShellFolder, item: &ShellItem, hwnd: HWND) -> Result<IDropTarget> {
    unsafe { folder.folder.GetUIObjectOf(hwnd, &[item.pidl.as_ptr()], None) }
}

#[implement(IDropSource)]
struct DropSource;

impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, fescapepressed: BOOL, grfkeystate: MODIFIERKEYS_FLAGS) -> HRESULT {
        if fescapepressed.as_bool() {
            DRAGDROP_S_CANCEL
        } else if grfkeystate.0 & (MK_LBUTTON.0 | MK_RBUTTON.0) == 0 {
            DRAGDROP_S_DROP
        } else {
            HRESULT(0)
        }
    }

    fn GiveFeedback(&self, _dweffect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Runs the modal drag loop for `data`; returns the effect the target applied (NONE if cancelled).
pub fn drag(data: &IDataObject) -> Result<DROPEFFECT> {
    let source: IDropSource = DropSource.into();
    let mut effect = DROPEFFECT_NONE;
    let hr = unsafe { DoDragDrop(data, &source, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK, &mut effect) };
    if hr == DRAGDROP_S_CANCEL {
        return Ok(DROPEFFECT_NONE);
    }
    hr.ok()?; // S_OK and DRAGDROP_S_DROP are both success codes
    Ok(effect)
}

type Pick = Box<dyn Fn(POINTL) -> Option<IDropTarget>>;

/// One drop target registered on a window; forwards to whichever shell target `pick` returns.
#[implement(IDropTarget)]
pub struct DropForwarder {
    pick: Pick,
    current: RefCell<Option<IDropTarget>>,
    data: RefCell<Option<IDataObject>>,
}

impl DropForwarder {
    pub fn new(pick: Pick) -> Self {
        DropForwarder { pick, current: RefCell::new(None), data: RefCell::new(None) }
    }
}

impl IDropTarget_Impl for DropForwarder_Impl {
    fn DragEnter(&self, pdataobj: Ref<IDataObject>, grfkeystate: MODIFIERKEYS_FLAGS, pt: &POINTL, pdweffect: *mut DROPEFFECT) -> Result<()> {
        *self.data.borrow_mut() = pdataobj.as_ref().cloned();
        let target = (self.pick)(*pt);
        *self.current.borrow_mut() = target.clone();
        match target {
            Some(t) => unsafe { t.DragEnter(self.data.borrow().as_ref(), grfkeystate, *pt, pdweffect) },
            None => {
                unsafe { *pdweffect = DROPEFFECT_NONE };
                Ok(())
            }
        }
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
                unsafe { t.DragEnter(self.data.borrow().as_ref(), grfkeystate, *pt, pdweffect)? };
            }
        }
        match target {
            Some(t) => unsafe { t.DragOver(grfkeystate, *pt, pdweffect) },
            None => {
                unsafe { *pdweffect = DROPEFFECT_NONE };
                Ok(())
            }
        }
    }

    fn DragLeave(&self) -> Result<()> {
        *self.data.borrow_mut() = None;
        match self.current.borrow_mut().take() {
            Some(t) => unsafe { t.DragLeave() },
            None => Ok(()),
        }
    }

    fn Drop(&self, pdataobj: Ref<IDataObject>, grfkeystate: MODIFIERKEYS_FLAGS, pt: &POINTL, pdweffect: *mut DROPEFFECT) -> Result<()> {
        *self.data.borrow_mut() = None;
        match self.current.borrow_mut().take() {
            Some(t) => unsafe { t.Drop(pdataobj.as_ref(), grfkeystate, *pt, pdweffect) },
            None => {
                unsafe { *pdweffect = DROPEFFECT_NONE };
                Ok(())
            }
        }
    }
}

/// Registers a forwarding drop target on `hwnd`. OLE must be initialized (`ComInit::ole`).
pub fn register(hwnd: HWND, pick: Pick) -> Result<()> {
    let target: IDropTarget = DropForwarder::new(pick).into();
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
    use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
    use windows::Win32::System::Ole::CF_HDROP;

    struct Fixture(std::path::PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fixture(tag: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!("rexplorer-dragdrop-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        Fixture(dir)
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
        let _c = ComInit::ole();
        let fx = fixture("fwd");
        let f = ShellFolder::parse(fx.0.to_str().unwrap()).unwrap();
        let items = f.enumerate().unwrap();
        let file = items.iter().find(|i| !i.is_folder()).unwrap();
        let data = data_object(&f, &[file]).unwrap();
        let target = folder_drop_target(&f, HWND::default()).unwrap();
        let fwd: IDropTarget = DropForwarder::new(Box::new(move |p| (p.x < 100).then(|| target.clone()))).into();
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
        }
    }
}
