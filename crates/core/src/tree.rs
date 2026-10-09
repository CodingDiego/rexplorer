//! The shell's own folder tree (NamespaceTreeControl) hosted as a child window.
use windows::Win32::Foundation::{E_NOTIMPL, HWND, LPARAM, RECT, S_FALSE, WPARAM};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Ole::IOleWindow;
use windows::Win32::UI::Shell::{
    IContextMenu, INameSpaceTreeControl2, INameSpaceTreeControlEvents,
    INameSpaceTreeControlEvents_Impl, IShellItem, IShellItemArray, NSTCIS_SELECTED,
    NSTCRS_EXPANDED, NSTCRS_VISIBLE, NSTCS_AUTOHSCROLL, NSTCS_FADEINOUTEXPANDOS,
    NSTCS_FULLROWSELECT, NSTCS_HASEXPANDOS, NSTCS_NOINFOTIP, NSTCS_ROOTHASEXPANDO,
    NamespaceTreeControl, SHCONTF_FOLDERS, SHCreateItemFromIDList, SHCreateItemFromParsingName,
    SHGetIDListFromObject,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyWindow, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos};
use windows::core::{BOOL, GUID, HSTRING, Interface, PCWSTR, PWSTR, Ref, Result, implement};


use crate::com::Pidl;

/// The hosted folder tree; selection changes are reported through the callback given to `create`.
pub struct TreePane {
    tree: INameSpaceTreeControl2,
    cookie: u32,
}

#[implement(INameSpaceTreeControlEvents)]
struct Events {
    on_select: Box<dyn Fn(Pidl)>,
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

fn item_from_name(name: &str) -> Result<IShellItem> {
    let wide = HSTRING::from(name);
    unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None) }
}

/// Creates the tree under `parent`, with Quick Access (if available) and This PC as roots.
pub fn create(parent: HWND, rect: RECT, on_select: Box<dyn Fn(Pidl)>) -> Result<TreePane> {
    unsafe {
        let tree: INameSpaceTreeControl2 = CoCreateInstance(&NamespaceTreeControl, None, CLSCTX_INPROC_SERVER)?;
        let style = NSTCS_HASEXPANDOS.0
            | NSTCS_ROOTHASEXPANDO.0
            | NSTCS_AUTOHSCROLL.0
            | NSTCS_FULLROWSELECT.0
            | NSTCS_NOINFOTIP.0
            | NSTCS_FADEINOUTEXPANDOS.0;
        tree.Initialize(parent, &rect, style as u32)?;
        let events: INameSpaceTreeControlEvents = Events { on_select }.into();
        let cookie = tree.TreeAdvise(&events)?;
        let pane = TreePane { tree, cookie }; // from here Drop unadvises and destroys on error
        let folders = SHCONTF_FOLDERS.0 as u32;
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
        let pane = create(p, RECT400, Box::new(|_| {})).unwrap();
        let h = pane.hwnd();
        assert!(!h.is_invalid());
        assert!(unsafe { IsWindow(Some(h)) }.as_bool());
        drop(pane);
        unsafe { DestroyWindow(p).unwrap() };
    }

    #[test]
    fn select_this_pc_does_not_fail() {
        let _c = ComInit::sta();
        let p = parent();
        let pane = create(p, RECT400, Box::new(|_| {})).unwrap();
        pane.select(&ShellFolder::parse("shell:MyComputerFolder").unwrap().pidl).unwrap();
        drop(pane);
        unsafe { DestroyWindow(p).unwrap() };
    }
}
