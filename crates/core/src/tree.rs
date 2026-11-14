//! The shell's own folder tree (NamespaceTreeControl) hosted as a child window.
use std::cell::RefCell;
use std::rc::Rc;

use windows::Win32::Foundation::{E_NOTIMPL, HWND, LPARAM, POINTL, RECT, S_FALSE, WPARAM};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IDataObject};
use windows::Win32::System::Ole::{DROPEFFECT, IDropTarget, IDropTarget_Impl, IOleWindow, RegisterDragDrop, RevokeDragDrop};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{
    IContextMenu, INameSpaceTreeControl2, INameSpaceTreeControlDropHandler, INameSpaceTreeControlDropHandler_Impl,
    INameSpaceTreeControlEvents, INameSpaceTreeControlEvents_Impl, IShellItem, IShellItemArray, NSTCGNI_PARENT, NSTCIS_EXPANDED, NSTCIS_SELECTED,
    NSTCRS_EXPANDED, NSTCRS_VISIBLE, NSTCS_AUTOHSCROLL, NSTCS_FADEINOUTEXPANDOS,
    NSTCS_FULLROWSELECT, NSTCS_HASEXPANDOS, NSTCS_NOINFOTIP, NSTCS_ROOTHASEXPANDO, NSTCS_SHOWSELECTIONALWAYS,
    NamespaceTreeControl, SHCONTF_FOLDERS, SHCONTF_INCLUDEHIDDEN, SHCreateItemFromIDList, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
    SHGetIDListFromObject,
};
use windows::Win32::UI::Controls::{TVGN_CARET, TVM_SELECTITEM};
use windows::Win32::UI::WindowsAndMessaging::{DestroyWindow, FindWindowExW, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetWindowPos};
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

/// The virtual folders whose way the tree may open, by parsing name: the desktop (its root has none), This PC, Home,
/// Quick access and the user's files. Their ancestors are the desktop and This PC, quick to list; a folder below one
/// of them (a phone under This PC, a library) is not.
const QUICK_ROOTS: [&str; 5] = [
    "",
    "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}",
    "::{F874310E-B6B7-47DC-BC84-B9E6B38F5903}",
    "::{679F85CB-0220-4080-B29B-5540CC05AAB6}",
    "::{59031a47-3f72-44a7-89c5-5595fe6b30ee}",
];

/// Whether the tree may open the ancestors of the folder `name` (a parsing name): only with Explorer's "Expand to
/// open folder" on (`option`), and only where the control's synchronous listing of each ancestor is quick: one of
/// the `QUICK_ROOTS`, or a folder on a fixed or RAM disk (`drive`: GetDriveTypeW of its root) none of whose
/// ancestors, itself included, is a junction, symbolic link or mount point (`reparse`, asked root first and never
/// past the first one: a link to a share or a mounted volume would list them). Otherwise the tree selects the
/// folder only when it is already shown.
pub fn may_expand(option: bool, name: &str, drive: impl FnOnce(&str) -> u32, reparse: impl Fn(&str) -> bool) -> bool {
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_RAMDISK: u32 = 6;
    let path = std::path::Path::new(name);
    if !option || name.starts_with(r"\\") {
        return false;
    }
    if !path.is_absolute() {
        return QUICK_ROOTS.iter().any(|r| r.eq_ignore_ascii_case(name));
    }
    let local = name.get(..3).is_some_and(|root| matches!(drive(root), DRIVE_FIXED | DRIVE_RAMDISK));
    let mut down: Vec<&std::path::Path> = path.ancestors().collect();
    down.reverse();
    local && !down.iter().skip(1).any(|a| reparse(&a.to_string_lossy()))
}

/// GetDriveTypeW of a drive root such as `C:\`.
pub fn drive_type(root: &str) -> u32 {
    unsafe { windows::Win32::Storage::FileSystem::GetDriveTypeW(&HSTRING::from(root)) }
}

/// Whether `path` is a reparse point (or unreadable). GetFileAttributesW answers for a link itself, not its target,
/// so asked of a path with no link above it this is a metadata read on the path's own volume.
pub fn is_reparse_point(path: &str) -> bool {
    use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_REPARSE_POINT, GetFileAttributesW, INVALID_FILE_ATTRIBUTES};
    let a = unsafe { GetFileAttributesW(&HSTRING::from(path)) };
    a == INVALID_FILE_ATTRIBUTES || a & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
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

    /// Selects the item for an absolute PIDL and scrolls it into view. With `expand` its ancestors are opened first
    /// (the control lists each one on this thread); without, for a folder the tree does not show now the tree selects
    /// nothing: a highlight left on the folder before would take Down and Enter relative to a place the window has
    /// left, and selecting an ancestor instead would navigate there when the tree has the focus (`on_select`).
    pub fn select(&self, pidl: &Pidl, expand: bool) -> Result<()> {
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
            let sel = NSTCIS_SELECTED.0 as u32;
            if expand {
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
            } else if !self.shows(&item) {
                // The control keeps a selection SetItemState cannot drop; its tree view's own message can.
                if let Ok(view) = FindWindowExW(Some(self.hwnd()), None, w!("SysTreeView32"), None) {
                    SendMessageW(view, TVM_SELECTITEM, Some(WPARAM(TVGN_CARET as usize)), Some(LPARAM(0)));
                }
                return Ok(());
            }
            self.tree.SetItemState(&item, sel, sel)?;
            self.tree.EnsureItemVisible(&item)
        }
    }

    /// The item is in the tree and every folder above it is expanded. GetItemRect cannot tell: it answers for an
    /// item under a collapsed folder too; GetItemState fails for one the tree does not hold.
    fn shows(&self, item: &IShellItem) -> bool {
        let open = NSTCIS_EXPANDED.0 as u32;
        unsafe {
            if self.tree.GetItemState(item, open).is_err() {
                return false;
            }
            let mut at = item.clone();
            while let Ok(p) = self.tree.GetNextItem(&at, NSTCGNI_PARENT) {
                if !self.tree.GetItemState(&p, open).is_ok_and(|s| s & open != 0) {
                    return false;
                }
                at = p;
            }
            true
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

    /// Dispatches what the control posted to this thread before a test destroys it: its background work reports by
    /// posting to its window, and these tests run no message loop (#94, a suspicion, not a traced cause).
    fn settle() {
        use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MSG, PM_REMOVE, PeekMessageW};
        let mut m = MSG::default();
        for _ in 0..3 {
            while unsafe { PeekMessageW(&mut m, None, 0, 0, PM_REMOVE) }.as_bool() {
                unsafe { DispatchMessageW(&m) };
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn tree_creates_under_hidden_window() {
        let _l = crate::com::SHELL_UI.lock();
        let _c = ComInit::sta();
        let p = parent();
        let pane = create(p, RECT400, false, Box::new(|_| {}), None).unwrap();
        let h = pane.hwnd();
        assert!(!h.is_invalid());
        assert!(unsafe { IsWindow(Some(h)) }.as_bool());
        settle();
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
        let _l = crate::com::SHELL_UI.lock();
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
        let _l = crate::com::SHELL_UI.lock();
        let _c = ComInit::ole();
        let p = parent();
        let view = |pane: &TreePane| unsafe { FindWindowExW(Some(pane.hwnd()), None, w!("SysTreeView32"), None) }.unwrap();
        let pane = create(p, RECT400, false, Box::new(|_| {}), Some(Box::new(|_, _| None))).unwrap();
        // The control's own was revoked to make room for ours: a target is still there (revoking succeeds).
        assert!(unsafe { RevokeDragDrop(view(&pane)) }.is_ok());
        settle();
        drop(pane);
        let pane = create(p, RECT400, false, Box::new(|_| {}), None).unwrap();
        assert!(unsafe { RevokeDragDrop(view(&pane)) }.is_ok(), "the control's own target stays");
        settle();
        drop(pane);
        unsafe { DestroyWindow(p).unwrap() };
    }

    #[test]
    fn select_this_pc_does_not_fail() {
        let _l = crate::com::SHELL_UI.lock();
        let _c = ComInit::sta();
        let p = parent();
        let pane = create(p, RECT400, false, Box::new(|_| {}), None).unwrap();
        pane.select(&ShellFolder::parse("shell:MyComputerFolder").unwrap().pidl, true).unwrap();
        settle();
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
        let _l = crate::com::SHELL_UI.lock();
        let _c = ComInit::sta();
        let p = parent();
        // A folder no root shows until its parents are expanded; a hidden one only when the tree lists hidden folders.
        let windir = std::env::var("SystemRoot").unwrap();
        let deep = format!(r"{windir}\System32\drivers\etc");
        let pane = create(p, RECT400, false, Box::new(|_| {}), None).unwrap();
        pane.select(&ShellFolder::parse(&deep).unwrap().pidl, true).unwrap();
        assert!(selected(&pane).eq_ignore_ascii_case(&deep), "{}", selected(&pane));
        settle();
        drop(pane);
        // The shell answers with long names; %TEMP% can be a short one (RUNNER~1 on CI).
        let temp = std::fs::canonicalize(std::env::temp_dir()).unwrap().to_string_lossy().trim_start_matches(r"\\?\").to_string();
        let pane = create(p, RECT400, true, Box::new(|_| {}), None).unwrap();
        pane.select(&ShellFolder::parse(&temp).unwrap().pidl, true).unwrap();
        assert!(selected(&pane).eq_ignore_ascii_case(&temp), "{}", selected(&pane));
        settle();
        drop(pane);
        unsafe { DestroyWindow(p).unwrap() };
    }

    #[test]
    fn without_expand_only_a_shown_folder_is_selected() {
        let _l = crate::com::SHELL_UI.lock();
        let _c = ComInit::sta();
        let p = parent();
        let windir = std::env::var("SystemRoot").unwrap();
        let (sys, deep) = (format!(r"{windir}\System32"), format!(r"{windir}\System32\drivers\etc"));
        let pidl = |s: &str| ShellFolder::parse(s).unwrap().pidl;
        let pane = create(p, RECT400, false, Box::new(|_| {}), None).unwrap();
        let sel = |pane: &TreePane| unsafe {
            pane.tree.GetSelectedItems().ok().and_then(|a| a.GetItemAt(0).ok()).and_then(|i| i.GetDisplayName(SIGDN_FILESYSPATH).ok()).map(|s| take_cotaskmem_string(s))
        };
        pane.select(&pidl(&deep), false).unwrap();
        let hidden = sel(&pane);
        // Opened once, then the way to it is collapsed again: it is in the tree but not shown.
        pane.select(&pidl(&deep), true).unwrap();
        let open = NSTCIS_EXPANDED.0 as u32;
        let sys_item = item_from_name(&format!("::{{20D04FE0-3AEA-1069-A2D8-08002B30309D}}\\{sys}")).unwrap();
        unsafe { pane.tree.SetItemState(&sys_item, open, 0) }.unwrap();
        pane.select(&pidl(&windir), false).unwrap();
        let shown = sel(&pane);
        pane.select(&pidl(&deep), false).unwrap();
        let collapsed = sel(&pane);
        settle();
        drop(pane);
        unsafe { DestroyWindow(p).unwrap() };
        assert!(!hidden.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(&deep)), "a folder no expanded parent shows: {hidden:?}");
        assert!(shown.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(&windir)), "a shown folder is selected: {shown:?}");
        assert_eq!(collapsed, None, "under a collapsed folder: the folder left before is no longer selected");
    }

    #[test]
    fn without_expand_a_folder_the_tree_cannot_show_clears_the_selection() {
        let _l = crate::com::SHELL_UI.lock();
        let _c = ComInit::sta();
        let p = parent();
        let windir = std::env::var("SystemRoot").unwrap();
        let pane = create(p, RECT400, false, Box::new(|_| {}), None).unwrap();
        pane.select(&ShellFolder::parse(&windir).unwrap().pidl, true).unwrap();
        let count = |pane: &TreePane| unsafe { pane.tree.GetSelectedItems().and_then(|a| a.GetCount()).unwrap_or(0) };
        let before = count(&pane);
        // Control Panel: under the desktop, which is no root here, so under nothing the tree holds.
        pane.select(&ShellFolder::parse("::{26EE0668-A00A-44D7-9371-BEB064C98683}").unwrap().pidl, false).unwrap();
        let after = count(&pane);
        settle();
        drop(pane);
        unsafe { DestroyWindow(p).unwrap() };
        assert_eq!((before, after), (1, 0));
    }

    #[test]
    fn only_quick_local_paths_expand_and_only_with_the_option() {
        let fixed = |_: &str| 3;
        let plain = |_: &str| false;
        assert!(may_expand(true, r"C:\a\b", fixed, plain));
        assert!(may_expand(true, r"C:\", fixed, |_| unreachable!()), "a drive root has no ancestor to ask about");
        for root in ["", "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}", "::{679f85cb-0220-4080-b29b-5540cc05aab6}"] {
            assert!(may_expand(true, root, |_| unreachable!(), plain), "a quick virtual root: {root:?}");
        }
        for deep in [r"::{20D04FE0-3AEA-1069-A2D8-08002B30309D}\\?\usb#vid_04e8&pid_6860#phone", r"::{031E4825-7B94-4DC3-B131-E946B44C8DD5}\Documents.library-ms"] {
            assert!(!may_expand(true, deep, |_| unreachable!(), plain), "a virtual folder below one: {deep}");
        }
        assert!(may_expand(true, r"R:\x", |_| 6, plain), "a RAM disk");
        assert!(!may_expand(false, r"C:\a\b", fixed, plain), "Explorer's option off");
        assert!(!may_expand(false, "", fixed, plain));
        for remote in [4, 2, 5, 0, 1] {
            assert!(!may_expand(true, r"Z:\share\deep", |_| remote, plain), "drive type {remote}");
        }
        assert!(!may_expand(true, r"\\server\share\x", |_| unreachable!(), plain), "UNC: never asked");
        // A junction, symbolic link or mount point on the way (or the folder itself): asked root first, and nothing
        // past the first one is asked (that would go through the link).
        let asked = RefCell::new(Vec::new());
        let link_at_a = |p: &str| {
            asked.borrow_mut().push(p.to_string());
            p.eq_ignore_ascii_case(r"C:\a")
        };
        assert!(!may_expand(true, r"C:\a\b\c", fixed, link_at_a));
        assert_eq!(*asked.borrow(), [r"C:\a"]);
        assert!(!may_expand(true, r"C:\x\link", fixed, |p| p.ends_with("link")), "the folder itself");
        assert_eq!(drive_type(&std::env::var("SystemDrive").unwrap_or("C:".into()).replace(':', ":\\")), 3);
    }

    #[test]
    fn a_junction_reads_as_a_reparse_point_and_a_plain_folder_does_not() {
        let dir = std::env::temp_dir().join(format!("rexplorer-reparse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("real")).unwrap();
        let link = dir.join("link");
        let made = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&link).arg(dir.join("real")).output().is_ok_and(|o| o.status.success());
        let (plain, junction) = (is_reparse_point(&dir.join("real").to_string_lossy()), is_reparse_point(&link.to_string_lossy()));
        let missing = is_reparse_point(&dir.join("missing").to_string_lossy());
        let _ = std::fs::remove_dir(&link);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!plain);
        assert!(missing, "unreadable counts as one: never expanded");
        assert!(!made || junction, "a junction");
    }
}
