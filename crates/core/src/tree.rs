//! The shell's own folder tree (NamespaceTreeControl) hosted as a child window.
use std::cell::RefCell;
use std::rc::Rc;

use windows::Win32::Foundation::{E_NOTIMPL, HWND, LPARAM, POINTL, RECT, S_FALSE, WPARAM};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IDataObject};
use windows::Win32::System::Ole::{DROPEFFECT, IDropTarget, IDropTarget_Impl, IOleWindow, RegisterDragDrop, RevokeDragDrop};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{
    IContextMenu, INameSpaceTreeControl2, INameSpaceTreeControlDropHandler, INameSpaceTreeControlDropHandler_Impl,
    INameSpaceTreeControlEvents, INameSpaceTreeControlEvents_Impl, IShellItem, IShellItemArray, NSTCIS_EXPANDED, NSTCIS_SELECTED,
    NSTCRS_EXPANDED, NSTCRS_VISIBLE, NSTCS_AUTOHSCROLL, NSTCS_FADEINOUTEXPANDOS,
    NSTCS_FULLROWSELECT, NSTCS_HASEXPANDOS, NSTCS_NOINFOTIP, NSTCS_ROOTHASEXPANDO, NSTCS_SHOWSELECTIONALWAYS,
    NamespaceTreeControl, SHCONTF_FOLDERS, SHCONTF_INCLUDEHIDDEN, SHCreateItemFromIDList, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
    SHGetIDListFromObject,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyWindow, FindWindowExW, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos};
use windows::core::{BOOL, GUID, HSTRING, Interface, PCWSTR, PWSTR, Ref, Result, implement, w};

use crate::com::{Pidl, take_cotaskmem_string};
use crate::dragdrop::{AfterDrop, dropped_effect};

/// The hosted folder tree; selection changes are reported through the callback given to `create`.
pub struct TreePane {
    tree: INameSpaceTreeControl2,
    cookie: u32,
}

#[implement(INameSpaceTreeControlEvents, INameSpaceTreeControlDropHandler)]
struct Events {
    on_select: Box<dyn Fn(Pidl)>,
    drop: Rc<DropState>,
}

#[allow(non_snake_case)]
impl INameSpaceTreeControlEvents_Impl for Events_Impl {
    fn OnItemClick(&self, _: Ref<IShellItem>, _: u32, _: u32) -> Result<()> { Ok(()) }
    fn OnPropertyItemCommit(&self, _: Ref<IShellItem>) -> Result<()> { Ok(()) }
    fn OnItemStateChanging(&self, _: Ref<IShellItem>, _: u32, _: u32) -> Result<()> { Ok(()) }
    fn OnItemStateChanged(&self, _: Ref<IShellItem>, _: u32, _: u32) -> Result<()> { Ok(()) }
    fn OnSelectionChanged(&self, sel: Ref<IShellItemArray>) -> Result<()> {
        if let Some(sel) = sel.as_ref() {
            unsafe {
                if sel.GetCount()? > 0 {
                    let item = sel.GetItemAt(0)?;
                    (self.on_select)(Pidl::from_raw(SHGetIDListFromObject(&item)?));
                }
            }
        }
        Ok(())
    }
    fn OnKeyboardInput(&self, _: u32, _: WPARAM, _: LPARAM) -> Result<()> { Err(S_FALSE.into()) }
    fn OnBeforeExpand(&self, _: Ref<IShellItem>) -> Result<()> { Ok(()) }
    fn OnAfterExpand(&self, _: Ref<IShellItem>) -> Result<()> { Ok(()) }
    fn OnBeginLabelEdit(&self, _: Ref<IShellItem>) -> Result<()> { Ok(()) }
    fn OnEndLabelEdit(&self, _: Ref<IShellItem>) -> Result<()> { Ok(()) }
    fn OnGetToolTip(&self, _: Ref<IShellItem>, _: PWSTR, _: i32) -> Result<()> { Ok(()) }
    fn OnBeforeItemDelete(&self, _: Ref<IShellItem>) -> Result<()> { Ok(()) }
    fn OnItemAdded(&self, _: Ref<IShellItem>, _: BOOL) -> Result<()> { Ok(()) }
    fn OnItemDeleted(&self, _: Ref<IShellItem>, _: BOOL) -> Result<()> { Ok(()) }
    fn OnBeforeContextMenu(&self, _: Ref<IShellItem>, _: *const GUID, _: *mut *mut core::ffi::c_void) -> Result<()> { Ok(()) }
    fn OnAfterContextMenu(&self, _: Ref<IShellItem>, _: Ref<IContextMenu>, _: *const GUID, _: *mut *mut core::ffi::c_void) -> Result<()> { Ok(()) }
    fn OnBeforeStateImageChange(&self, _: Ref<IShellItem>) -> Result<()> { Ok(()) }
    fn OnGetDefaultIconIndex(&self, _: Ref<IShellItem>, _: *mut i32, _: *mut i32) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
}

/// The control asks its events object for this (QueryInterface); every method succeeds, so the control still runs
/// each drop itself. OnDrop comes inside the control's own Drop, before it moves anything: the folder it drops on is
/// planned there, with the data `TreeDrop` kept for it (the control passes no item array for a drag from a file view).
impl INameSpaceTreeControlDropHandler_Impl for Events_Impl {
    fn OnDragEnter(&self, _: Ref<IShellItem>, _: Ref<IShellItemArray>, _: BOOL, _: u32, _: *mut u32) -> Result<()> {
        Ok(())
    }
    fn OnDragOver(&self, _: Ref<IShellItem>, _: Ref<IShellItemArray>, _: u32, _: *mut u32) -> Result<()> {
        Ok(())
    }
    fn OnDragPosition(&self, _: Ref<IShellItem>, _: Ref<IShellItemArray>, _: i32, _: i32) -> Result<()> {
        Ok(())
    }
    fn OnDrop(&self, over: Ref<IShellItem>, _: Ref<IShellItemArray>, _: i32, _: u32, _: *mut u32) -> Result<()> {
        let dest = over.as_ref().and_then(|i| unsafe { i.GetDisplayName(SIGDN_FILESYSPATH) }.ok()).map(|p| unsafe { take_cotaskmem_string(p) });
        let data = self.drop.data.borrow().clone();
        if let (Some(plan), Some(dest), Some(data)) = (&self.drop.plan, dest, data) {
            let after = plan(&data, dest);
            *self.drop.after.borrow_mut() = after;
        }
        Ok(())
    }
    fn OnDropPosition(&self, _: Ref<IShellItem>, _: Ref<IShellItemArray>, _: i32, _: i32) -> Result<()> {
        Ok(())
    }
    fn OnDragLeave(&self, _: Ref<IShellItem>) -> Result<()> {
        Ok(())
    }
}

/// Reads a drop on the tree before the control runs it: the data and the folder, and what to do with the effect.
pub type PlanDrop = Box<dyn Fn(&IDataObject, String) -> Option<AfterDrop>>;

/// What the drop target and the drop handler share during one drop.
#[derive(Default)]
struct DropState {
    plan: Option<PlanDrop>,
    data: RefCell<Option<IDataObject>>,
    after: RefCell<Option<AfterDrop>>,
}

/// Registered on the tree's window in place of the control's own target, which it calls for everything; after the
/// control's Drop it hands the plan the effect the drop had.
#[implement(IDropTarget)]
struct TreeDrop {
    inner: IDropTarget,
    state: Rc<DropState>,
}

#[allow(clippy::not_unsafe_ptr_arg_deref)] // COM vtable methods: signatures fixed by IDropTarget_Impl
impl IDropTarget_Impl for TreeDrop_Impl {
    fn DragEnter(&self, data: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        unsafe { self.inner.DragEnter(data.as_ref(), keys, *pt, effect) }
    }
    fn DragOver(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        unsafe { self.inner.DragOver(keys, *pt, effect) }
    }
    fn DragLeave(&self) -> Result<()> {
        unsafe { self.inner.DragLeave() }
    }
    fn Drop(&self, data: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        *self.state.data.borrow_mut() = data.as_ref().cloned();
        *self.state.after.borrow_mut() = None;
        // No borrow is held across the call: a right-drag menu pumps messages inside it.
        let r = unsafe { self.inner.Drop(data.as_ref(), keys, *pt, effect) };
        *self.state.data.borrow_mut() = None;
        let after = self.state.after.borrow_mut().take();
        if let Some(a) = after {
            a(dropped_effect(r.is_ok(), unsafe { *effect }, data.as_ref()));
        }
        r
    }
}

/// Puts `TreeDrop` on the control's tree view in place of the control's target; on any failure the control keeps its
/// own (drops still work, only without undo).
fn wrap_drop_target(tree: &INameSpaceTreeControl2, outer: HWND, state: Rc<DropState>) -> Result<()> {
    unsafe {
        let inner: IDropTarget = tree.cast()?;
        let view = FindWindowExW(Some(outer), None, w!("SysTreeView32"), None)?;
        RevokeDragDrop(view)?;
        let ours: IDropTarget = TreeDrop { inner: inner.clone(), state }.into();
        RegisterDragDrop(view, &ours).inspect_err(|_| {
            let _ = RegisterDragDrop(view, &inner);
        })
    }
}

fn item_from_name(name: &str) -> Result<IShellItem> {
    let wide = HSTRING::from(name);
    unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None) }
}

/// Creates the tree under `parent`, with Quick Access (if available) and This PC as roots; `hidden` lists hidden
/// folders too. `plan` reads each drop on a folder before the control runs it (for undo).
/// shortcut: `hidden` is read once, when the tree is made; a later change of the setting shows after a restart.
pub fn create(parent: HWND, rect: RECT, hidden: bool, on_select: Box<dyn Fn(Pidl)>, plan: Option<PlanDrop>) -> Result<TreePane> {
    unsafe {
        let tree: INameSpaceTreeControl2 = CoCreateInstance(&NamespaceTreeControl, None, CLSCTX_INPROC_SERVER)?;
        let style = NSTCS_HASEXPANDOS.0
            | NSTCS_ROOTHASEXPANDO.0
            | NSTCS_AUTOHSCROLL.0
            | NSTCS_FULLROWSELECT.0
            | NSTCS_NOINFOTIP.0
            | NSTCS_FADEINOUTEXPANDOS.0
            // The current folder stays marked while the list has the focus.
            | NSTCS_SHOWSELECTIONALWAYS.0;
        tree.Initialize(parent, &rect, style as u32)?;
        let drop = Rc::new(DropState { plan, ..Default::default() });
        let events: INameSpaceTreeControlEvents = Events { on_select, drop: drop.clone() }.into();
        let cookie = tree.TreeAdvise(&events)?;
        let pane = TreePane { tree, cookie }; // from here Drop unadvises and destroys on error
        if drop.plan.is_some() {
            let _ = wrap_drop_target(&pane.tree, pane.hwnd(), drop);
        }
        let folders = (SHCONTF_FOLDERS.0 | if hidden { SHCONTF_INCLUDEHIDDEN.0 } else { 0 }) as u32;
        // Quick Access is absent on older Windows; its failure must not fail the pane.
        if let Ok(qa) = item_from_name("shell:::{679f85cb-0220-4080-b29b-5540cc05aab6}") {
            let _ = pane.tree.AppendRoot(&qa, folders, NSTCRS_VISIBLE.0 as u32, None);
        }
        let pc = item_from_name("shell:MyComputerFolder")?;
        pane.tree.AppendRoot(&pc, folders, (NSTCRS_EXPANDED.0 | NSTCRS_VISIBLE.0) as u32, None)?;
        Ok(pane)
    }
}

impl TreePane {
    /// The tree control's window.
    pub fn hwnd(&self) -> HWND {
        self.tree
            .cast::<IOleWindow>()
            .and_then(|w| unsafe { w.GetWindow() })
            .unwrap_or_default()
    }

    /// Moves and resizes the control (parent client coordinates).
    pub fn set_rect(&self, rect: RECT) {
        let _ = unsafe {
            SetWindowPos(
                self.hwnd(),
                None,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
    }

    /// Selects the item for an absolute PIDL and scrolls it into view.
    pub fn select(&self, pidl: &Pidl) -> Result<()> {
        unsafe {
            let item: IShellItem = SHCreateItemFromIDList(pidl.as_ptr())?;
            // A folder under the user's profile is reached from the desktop's user folder, which is no root here: the
            // tree shows it under This PC, so a file system folder is looked up by that route.
            let item = item
                .GetDisplayName(SIGDN_FILESYSPATH)
                .map(|p| take_cotaskmem_string(p))
                .and_then(|p| item_from_name(&format!("::{{20D04FE0-3AEA-1069-A2D8-08002B30309D}}\\{p}")))
                .unwrap_or(item);
            // The tree holds a folder only once its parent was expanded: open the ancestors, outermost first. Those
            // above the roots (the desktop) are not in the tree and fail harmlessly.
            let mut up = Vec::new();
            let mut at = item.clone();
            while let Ok(p) = at.GetParent() {
                up.push(p.clone());
                at = p;
            }
            let open = NSTCIS_EXPANDED.0 as u32;
            for a in up.iter().rev() {
                let _ = self.tree.SetItemState(a, open, open);
            }
            let sel = NSTCIS_SELECTED.0 as u32;
            self.tree.SetItemState(&item, sel, sel)?;
            self.tree.EnsureItemVisible(&item)
        }
    }
}

impl Drop for TreePane {
    fn drop(&mut self) {
        unsafe {
            let _ = self.tree.TreeUnadvise(self.cookie);
            // Our target holds the control: let it go with the window.
            if let Ok(view) = FindWindowExW(Some(self.hwnd()), None, w!("SysTreeView32"), None) {
                let _ = RevokeDragDrop(view);
            }
            let _ = DestroyWindow(self.hwnd()); // the control has no Destroy method
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::ComInit;
    use crate::folder::ShellFolder;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, IsWindow, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW,
    };
    use windows::core::w;

    fn parent() -> HWND {
        unsafe {
            CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!(""), WS_OVERLAPPEDWINDOW, 0, 0, 400, 400, None, None, None, None)
                .unwrap()
        }
    }

    const RECT400: RECT = RECT { left: 0, top: 0, right: 400, bottom: 400 };

    #[test]
    fn tree_creates_under_hidden_window() {
        let _c = ComInit::sta();
        let p = parent();
        let pane = create(p, RECT400, false, Box::new(|_| {}), None).unwrap();
        let h = pane.hwnd();
        assert!(!h.is_invalid());
        assert!(unsafe { IsWindow(Some(h)) }.as_bool());
        drop(pane);
        unsafe { DestroyWindow(p).unwrap() };
    }

    /// A stand-in for the control's own target: its Drop runs `during` (where the control calls OnDrop), then
    /// reports `effect`.
    #[implement(IDropTarget)]
    struct Inner {
        during: Box<dyn Fn()>,
        effect: DROPEFFECT,
    }

    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    impl IDropTarget_Impl for Inner_Impl {
        fn DragEnter(&self, _: Ref<IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, _: *mut DROPEFFECT) -> Result<()> {
            Ok(())
        }
        fn DragOver(&self, _: MODIFIERKEYS_FLAGS, _: &POINTL, _: *mut DROPEFFECT) -> Result<()> {
            Ok(())
        }
        fn DragLeave(&self) -> Result<()> {
            Ok(())
        }
        fn Drop(&self, _: Ref<IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
            (self.during)();
            unsafe { *effect = self.effect };
            Ok(())
        }
    }

    /// Drops `data` on a TreeDrop over `Inner`; whether the plan ran and the effect its result got.
    fn drop_through(planned: bool, effect: DROPEFFECT, data: &IDataObject) -> (bool, Option<DROPEFFECT>) {
        let got: Rc<RefCell<(bool, Option<DROPEFFECT>)>> = Rc::default();
        let g = got.clone();
        let plan: PlanDrop = Box::new(move |_, dest| {
            assert_eq!(dest, r"C:\dest");
            g.borrow_mut().0 = true;
            let g = g.clone();
            Some(Box::new(move |e| g.borrow_mut().1 = Some(e)))
        });
        let state = Rc::new(DropState { plan: Some(plan), ..Default::default() });
        // What the handler's OnDrop does with the folder the control drops on.
        let s = state.clone();
        let during: Box<dyn Fn()> = Box::new(move || {
            if planned {
                let data = s.data.borrow().clone().expect("the data is kept for OnDrop");
                let after = (s.plan.as_ref().unwrap())(&data, r"C:\dest".to_string());
                *s.after.borrow_mut() = after;
            }
        });
        let inner: IDropTarget = Inner { during, effect }.into();
        let target: IDropTarget = TreeDrop { inner, state: state.clone() }.into();
        let mut e = DROPEFFECT(7);
        unsafe { target.Drop(data, MODIFIERKEYS_FLAGS(0), POINTL::default(), &mut e) }.unwrap();
        assert!(state.data.borrow().is_none() && state.after.borrow().is_none(), "nothing is kept after the drop");
        *got.borrow()
    }

    #[test]
    fn a_tree_drop_reports_the_effect_the_control_had() {
        use windows::Win32::System::Ole::{DROPEFFECT_COPY, DROPEFFECT_MOVE, DROPEFFECT_NONE};
        let _c = ComInit::ole();
        let data = crate::dragdrop::data_object_of_paths(&[std::env::temp_dir().to_string_lossy().into_owned()]).unwrap();
        assert_eq!(drop_through(true, DROPEFFECT_MOVE, &data), (true, Some(DROPEFFECT_MOVE)));
        assert_eq!(drop_through(true, DROPEFFECT_COPY, &data), (true, Some(DROPEFFECT_COPY)));
        // A refused or cancelled drop reports NONE (the app records nothing for it).
        assert_eq!(drop_through(true, DROPEFFECT_NONE, &data), (true, Some(DROPEFFECT_NONE)));
        // No OnDrop (the control did not drop on a folder): no plan, nothing reported.
        assert_eq!(drop_through(false, DROPEFFECT_MOVE, &data), (false, None));
    }

    #[test]
    fn the_tree_view_keeps_a_drop_target_with_or_without_a_plan() {
        let _c = ComInit::ole();
        let p = parent();
        let view = |pane: &TreePane| unsafe { FindWindowExW(Some(pane.hwnd()), None, w!("SysTreeView32"), None) }.unwrap();
        let pane = create(p, RECT400, false, Box::new(|_| {}), Some(Box::new(|_, _| None))).unwrap();
        // The control's own was revoked to make room for ours: a target is still there (revoking succeeds).
        assert!(unsafe { RevokeDragDrop(view(&pane)) }.is_ok());
        drop(pane);
        let pane = create(p, RECT400, false, Box::new(|_| {}), None).unwrap();
        assert!(unsafe { RevokeDragDrop(view(&pane)) }.is_ok(), "the control's own target stays");
        drop(pane);
        unsafe { DestroyWindow(p).unwrap() };
    }

    #[test]
    fn select_this_pc_does_not_fail() {
        let _c = ComInit::sta();
        let p = parent();
        let pane = create(p, RECT400, false, Box::new(|_| {}), None).unwrap();
        pane.select(&ShellFolder::parse("shell:MyComputerFolder").unwrap().pidl).unwrap();
        drop(pane);
        unsafe { DestroyWindow(p).unwrap() };
    }

    /// The parsing name of the tree's selection.
    fn selected(pane: &TreePane) -> String {
        unsafe {
            let a = pane.tree.GetSelectedItems().unwrap();
            take_cotaskmem_string(a.GetItemAt(0).unwrap().GetDisplayName(SIGDN_FILESYSPATH).unwrap())
        }
    }

    #[test]
    fn select_opens_the_way_to_a_deep_folder() {
        let _c = ComInit::sta();
        let p = parent();
        // A folder no root shows until its parents are expanded; a hidden one only when the tree lists hidden folders.
        let windir = std::env::var("SystemRoot").unwrap();
        let deep = format!(r"{windir}\System32\drivers\etc");
        let pane = create(p, RECT400, false, Box::new(|_| {}), None).unwrap();
        pane.select(&ShellFolder::parse(&deep).unwrap().pidl).unwrap();
        assert!(selected(&pane).eq_ignore_ascii_case(&deep), "{}", selected(&pane));
        drop(pane);
        // The shell answers with long names; %TEMP% can be a short one (RUNNER~1 on CI).
        let temp = std::fs::canonicalize(std::env::temp_dir()).unwrap().to_string_lossy().trim_start_matches(r"\\?\").to_string();
        let pane = create(p, RECT400, true, Box::new(|_| {}), None).unwrap();
        pane.select(&ShellFolder::parse(&temp).unwrap().pidl).unwrap();
        assert!(selected(&pane).eq_ignore_ascii_case(&temp), "{}", selected(&pane));
        drop(pane);
        unsafe { DestroyWindow(p).unwrap() };
    }
}
