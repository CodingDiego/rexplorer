//! MSAA (IAccessible) for the custom-drawn controls. One shared object per control window; the control
//! answers two private messages, so the object never holds a pointer into the control's state.
use std::cell::Cell;

use windows::Win32::Foundation::{
    CO_E_OBJNOTCONNECTED, DISP_E_MEMBERNOTFOUND, E_INVALIDARG, E_NOTIMPL, HWND, LPARAM, LRESULT, POINT, RECT, S_FALSE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
use windows::Win32::System::Com::{CoDisconnectObject, IDispatch, IDispatch_Impl, ITypeInfo};
use windows::Win32::System::Variant::{VARIANT, VT_I4};
use windows::Win32::UI::Accessibility::{
    AccessibleObjectFromWindow, IAccessible, IAccessible_Impl, LresultFromObject, NAVDIR_FIRSTCHILD, NAVDIR_LASTCHILD,
    NAVDIR_NEXT, NAVDIR_PREVIOUS, NotifyWinEvent,
};
use windows::Win32::UI::Controls::{STATE_SYSTEM_FOCUSABLE, STATE_SYSTEM_INVISIBLE, STATE_SYSTEM_OFFSCREEN, STATE_SYSTEM_UNAVAILABLE};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, IsWindowEnabled};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowRect, IsWindowVisible, OBJID_CLIENT, OBJID_WINDOW, PostMessageW, STATE_SYSTEM_FOCUSED,
    STATE_SYSTEM_SELECTED, SendMessageW, WM_APP,
};
use windows::core::{BSTR, ComObject, GUID, IUnknown, Interface, Result, implement};

/// Sent by the object; lparam = *mut Tree to fill. The control returns LRESULT(1) when it filled it.
pub const WM_ACC_ITEMS: u32 = WM_APP + 0x3F00;
/// Posted by accDoDefaultAction; wparam = item index (0-based). The control does what a click on it does.
pub const WM_ACC_INVOKE: u32 = WM_APP + 0x3F01;

#[derive(Default)]
pub struct Tree {
    pub name: String,
    pub role: u32,
    pub focusable: bool,
    pub focus: Option<usize>,
    pub items: Vec<Item>,
}

#[derive(Default, Clone)]
pub struct Item {
    pub name: String,
    pub description: String,
    pub role: u32,
    pub state: u32,
    /// Client coordinates.
    pub rect: RECT,
    /// "" = no default action.
    pub action: &'static str,
    /// "" = none, e.g. "Alt+Left".
    pub shortcut: String,
}

/// One per control window, owned by the control's State; that is the control's only strong reference.
pub struct Server(ComObject<Accessible>);

impl Server {
    pub fn new(hwnd: HWND) -> Server {
        Server(ComObject::new(Accessible { hwnd: Cell::new(hwnd) }))
    }

    pub fn answer(&self, wparam: WPARAM) -> LRESULT {
        unsafe { LresultFromObject(&IAccessible::IID, wparam, &self.0.to_interface::<IAccessible>()) }
    }
}

impl Drop for Server {
    // Clients may keep the object after the window is gone: cut the window link and the remote proxies.
    fn drop(&mut self) {
        self.0.hwnd.set(HWND::default());
        let _ = unsafe { CoDisconnectObject(&self.0.to_interface::<IUnknown>(), None) };
    }
}

pub fn is_client(lparam: LPARAM) -> bool {
    lparam.0 as u32 as i32 == OBJID_CLIENT.0
}

pub fn child_id(item: Option<usize>) -> i32 {
    item.map_or(0, |i| i as i32 + 1)
}

/// Call after the state change is complete, never inside a borrow of the control's State.
pub fn event(event: u32, hwnd: HWND, item: Option<usize>) {
    unsafe { NotifyWinEvent(event, hwnd, OBJID_CLIENT.0, child_id(item)) };
}

/// Names a standard control that has no label (an Edit) for screen readers: an oleacc annotation that lives as
/// long as the window. COM must be up on the calling thread.
pub fn set_name(hwnd: HWND, name: &str) -> Result<()> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::Win32::UI::Accessibility::{CAccPropServices, IAccPropServices, PROPID_ACC_NAME};
    unsafe {
        let svc: IAccPropServices = CoCreateInstance(&CAccPropServices, None, CLSCTX_INPROC_SERVER)?;
        svc.SetHwndPropStr(hwnd, OBJID_CLIENT.0 as u32, 0, PROPID_ACC_NAME, &windows::core::HSTRING::from(name))
    }
}

pub fn object(hwnd: HWND) -> Result<IAccessible> {
    let mut p = std::ptr::null_mut();
    unsafe { AccessibleObjectFromWindow(hwnd, OBJID_CLIENT.0 as u32, &IAccessible::IID, &mut p)? };
    Ok(unsafe { IAccessible::from_raw(p) })
}

// Not agile: the Cell is unsynchronised, so every call must come through a proxy onto the window's thread.
#[implement(IAccessible, Agile = false)]
pub struct Accessible {
    hwnd: Cell<HWND>,
}

impl Accessible {
    fn tree(&self) -> Result<(HWND, Tree)> {
        let hwnd = self.hwnd.get();
        let mut tree = Tree::default();
        let ok = !hwnd.is_invalid()
            && unsafe { SendMessageW(hwnd, WM_ACC_ITEMS, None, Some(LPARAM(&mut tree as *mut Tree as isize))) }.0 == 1;
        if ok { Ok((hwnd, tree)) } else { Err(CO_E_OBJNOTCONNECTED.into()) }
    }

    fn client_rect(hwnd: HWND) -> RECT {
        let mut r = RECT::default();
        let _ = unsafe { GetClientRect(hwnd, &mut r) };
        r
    }

    fn state_of(hwnd: HWND, tree: &Tree, item: Option<usize>) -> u32 {
        let focused = unsafe { GetFocus() } == hwnd;
        match item {
            None => {
                let mut s = 0;
                if tree.focusable {
                    s |= STATE_SYSTEM_FOCUSABLE.0;
                }
                if focused {
                    s |= STATE_SYSTEM_FOCUSED;
                }
                if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
                    s |= STATE_SYSTEM_INVISIBLE.0;
                }
                if !unsafe { IsWindowEnabled(hwnd) }.as_bool() {
                    s |= STATE_SYSTEM_UNAVAILABLE.0;
                }
                s
            }
            Some(i) => {
                let it = &tree.items[i];
                let mut s = it.state;
                let c = Self::client_rect(hwnd);
                let r = it.rect;
                let hit = r.left < c.right && r.right > c.left && r.top < c.bottom && r.bottom > c.top;
                if s & STATE_SYSTEM_INVISIBLE.0 == 0 && !hit {
                    s |= STATE_SYSTEM_OFFSCREEN.0;
                }
                if tree.focusable {
                    s |= STATE_SYSTEM_FOCUSABLE.0;
                    if focused && tree.focus == Some(i) {
                        s |= STATE_SYSTEM_FOCUSED;
                    }
                }
                s
            }
        }
    }
}

/// VT_I4 0 = the object itself, 1..=n = items, anything else is invalid.
fn child(v: &VARIANT, n: usize) -> Result<Option<usize>> {
    if v.vt() != VT_I4 {
        return Err(E_INVALIDARG.into());
    }
    let id = unsafe { v.Anonymous.Anonymous.Anonymous.lVal };
    match id {
        0 => Ok(None),
        1.. if id as usize <= n => Ok(Some(id as usize - 1)),
        _ => Err(E_INVALIDARG.into()),
    }
}

fn empty() -> Result<VARIANT> {
    Ok(VARIANT::default())
}

impl IDispatch_Impl for Accessible_Impl {
    fn GetTypeInfoCount(&self) -> Result<u32> {
        Ok(0)
    }
    fn GetTypeInfo(&self, _: u32, _: u32) -> Result<ITypeInfo> {
        Err(E_NOTIMPL.into())
    }
    fn GetIDsOfNames(&self, _: *const GUID, _: *const windows::core::PCWSTR, _: u32, _: u32, _: *mut i32) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn Invoke(
        &self,
        _: i32,
        _: *const GUID,
        _: u32,
        _: windows::Win32::System::Com::DISPATCH_FLAGS,
        _: *const windows::Win32::System::Com::DISPPARAMS,
        _: *mut VARIANT,
        _: *mut windows::Win32::System::Com::EXCEPINFO,
        _: *mut u32,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
}

#[allow(non_snake_case, clippy::not_unsafe_ptr_arg_deref)] // COM vtable method: the signature is fixed by IAccessible_Impl
impl IAccessible_Impl for Accessible_Impl {
    fn accParent(&self) -> Result<IDispatch> {
        let (hwnd, _) = self.tree()?;
        let mut p = std::ptr::null_mut();
        unsafe { AccessibleObjectFromWindow(hwnd, OBJID_WINDOW.0 as u32, &IDispatch::IID, &mut p)? };
        Ok(unsafe { IDispatch::from_raw(p) })
    }

    fn accChildCount(&self) -> Result<i32> {
        Ok(self.tree()?.1.items.len() as i32)
    }

    fn get_accChild(&self, varchild: &VARIANT) -> Result<IDispatch> {
        let (_, t) = self.tree()?;
        match child(varchild, t.items.len())? {
            Some(_) => Err(S_FALSE.into()),
            None => Err(E_INVALIDARG.into()),
        }
    }

    fn get_accName(&self, varchild: &VARIANT) -> Result<BSTR> {
        let (_, t) = self.tree()?;
        Ok(match child(varchild, t.items.len())? {
            None => BSTR::from(t.name.as_str()),
            Some(i) => BSTR::from(t.items[i].name.as_str()),
        })
    }

    fn get_accValue(&self, _: &VARIANT) -> Result<BSTR> {
        Err(DISP_E_MEMBERNOTFOUND.into())
    }

    fn get_accDescription(&self, varchild: &VARIANT) -> Result<BSTR> {
        let (_, t) = self.tree()?;
        Ok(match child(varchild, t.items.len())? {
            None => BSTR::new(),
            Some(i) => BSTR::from(t.items[i].description.as_str()),
        })
    }

    fn get_accRole(&self, varchild: &VARIANT) -> Result<VARIANT> {
        let (_, t) = self.tree()?;
        Ok(VARIANT::from(match child(varchild, t.items.len())? {
            None => t.role,
            Some(i) => t.items[i].role,
        } as i32))
    }

    fn get_accState(&self, varchild: &VARIANT) -> Result<VARIANT> {
        let (hwnd, t) = self.tree()?;
        let item = child(varchild, t.items.len())?;
        Ok(VARIANT::from(Accessible::state_of(hwnd, &t, item) as i32))
    }

    fn get_accHelp(&self, _: &VARIANT) -> Result<BSTR> {
        Err(DISP_E_MEMBERNOTFOUND.into())
    }

    fn get_accHelpTopic(&self, _: *mut BSTR, _: &VARIANT) -> Result<i32> {
        Err(DISP_E_MEMBERNOTFOUND.into())
    }

    fn get_accKeyboardShortcut(&self, varchild: &VARIANT) -> Result<BSTR> {
        let (_, t) = self.tree()?;
        match child(varchild, t.items.len())? {
            Some(i) if !t.items[i].shortcut.is_empty() => Ok(BSTR::from(t.items[i].shortcut.as_str())),
            _ => Err(DISP_E_MEMBERNOTFOUND.into()),
        }
    }

    fn accFocus(&self) -> Result<VARIANT> {
        let (hwnd, t) = self.tree()?;
        if unsafe { GetFocus() } != hwnd {
            return empty();
        }
        Ok(VARIANT::from(child_id(t.focus.filter(|&f| f < t.items.len()))))
    }

    fn accSelection(&self) -> Result<VARIANT> {
        let (_, t) = self.tree()?;
        let sel = |i: usize| t.items.get(i).is_some_and(|it| it.state & STATE_SYSTEM_SELECTED != 0);
        // shortcut: one child even when several are selected (Home Ctrl+A); return an IEnumVARIANT when a reader needs the whole selection
        let pick = match t.focus {
            Some(f) if sel(f) => Some(f),
            _ => (0..t.items.len()).find(|&i| sel(i)),
        };
        match pick {
            Some(i) => Ok(VARIANT::from(child_id(Some(i)))),
            None => empty(),
        }
    }

    fn get_accDefaultAction(&self, varchild: &VARIANT) -> Result<BSTR> {
        let (_, t) = self.tree()?;
        match child(varchild, t.items.len())? {
            Some(i) if !t.items[i].action.is_empty() => Ok(BSTR::from(t.items[i].action)),
            _ => Err(DISP_E_MEMBERNOTFOUND.into()),
        }
    }

    fn accSelect(&self, _: i32, _: &VARIANT) -> Result<()> {
        Err(DISP_E_MEMBERNOTFOUND.into())
    }

    fn accLocation(&self, pxleft: *mut i32, pytop: *mut i32, pcxwidth: *mut i32, pcyheight: *mut i32, varchild: &VARIANT) -> Result<()> {
        let (hwnd, t) = self.tree()?;
        let (x, y, w, h) = match child(varchild, t.items.len())? {
            None => {
                let mut r = RECT::default();
                unsafe { GetWindowRect(hwnd, &mut r)? };
                (r.left, r.top, r.right - r.left, r.bottom - r.top)
            }
            Some(i) => {
                let r = t.items[i].rect;
                let mut p = POINT { x: r.left, y: r.top };
                let _ = unsafe { ClientToScreen(hwnd, &mut p) };
                (p.x, p.y, r.right - r.left, r.bottom - r.top)
            }
        };
        unsafe {
            *pxleft = x;
            *pytop = y;
            *pcxwidth = w;
            *pcyheight = h;
        }
        Ok(())
    }

    fn accNavigate(&self, navdir: i32, varstart: &VARIANT) -> Result<VARIANT> {
        let (_, t) = self.tree()?;
        let n = t.items.len();
        let id = match (navdir as u32, child(varstart, n)?) {
            (NAVDIR_FIRSTCHILD, None) if n > 0 => 1,
            (NAVDIR_LASTCHILD, None) if n > 0 => n as i32,
            (NAVDIR_NEXT, Some(i)) if i + 1 < n => i as i32 + 2,
            (NAVDIR_PREVIOUS, Some(i)) if i > 0 => i as i32,
            _ => return empty(),
        };
        Ok(VARIANT::from(id))
    }

    fn accHitTest(&self, xleft: i32, ytop: i32) -> Result<VARIANT> {
        let (hwnd, t) = self.tree()?;
        let mut wr = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut wr)? };
        if xleft < wr.left || xleft >= wr.right || ytop < wr.top || ytop >= wr.bottom {
            return empty();
        }
        let mut p = POINT { x: xleft, y: ytop };
        let _ = unsafe { ScreenToClient(hwnd, &mut p) };
        let hit = t.items.iter().position(|it| {
            let r = it.rect;
            it.state & STATE_SYSTEM_INVISIBLE.0 == 0 && p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
        });
        Ok(VARIANT::from(child_id(hit)))
    }

    fn accDoDefaultAction(&self, varchild: &VARIANT) -> Result<()> {
        let (hwnd, t) = self.tree()?;
        match child(varchild, t.items.len())? {
            Some(i) if !t.items[i].action.is_empty() && t.items[i].state & STATE_SYSTEM_UNAVAILABLE.0 == 0 => {
                // Posted, never sent: a click can open a menu or a modal loop and would block the client's call.
                unsafe { PostMessageW(Some(hwnd), WM_ACC_INVOKE, WPARAM(i), LPARAM(0)) }
            }
            _ => Err(DISP_E_MEMBERNOTFOUND.into()),
        }
    }

    fn put_accName(&self, _: &VARIANT, _: &BSTR) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn put_accValue(&self, _: &VARIANT, _: &BSTR) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::ComInit;
    use std::cell::RefCell;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows::Win32::UI::Accessibility::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::w;

    thread_local! {
        static SERVER: RefCell<Option<Server>> = const { RefCell::new(None) };
        static TREE: RefCell<Option<Tree>> = const { RefCell::new(None) };
        static INVOKED: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    }

    fn rc(l: i32, t: i32, r: i32, b: i32) -> RECT {
        RECT { left: l, top: t, right: r, bottom: b }
    }

    fn test_tree() -> Tree {
        let it = |name: &str, desc: &str, role, state, rect, action| Item {
            name: name.into(),
            description: desc.into(),
            role,
            state,
            rect,
            action,
            shortcut: String::new(),
        };
        Tree {
            name: "Test".into(),
            role: ROLE_SYSTEM_TOOLBAR,
            focusable: true,
            focus: Some(1),
            items: vec![
                it("Alpha", "first", ROLE_SYSTEM_PUSHBUTTON, 0, rc(10, 10, 60, 30), "Open"),
                Item { shortcut: "Ctrl+B".into(), ..it("Beta", "", ROLE_SYSTEM_LISTITEM, STATE_SYSTEM_SELECTED | STATE_SYSTEM_SELECTABLE, rc(70, 10, 120, 30), "Open") },
                it("Gamma", "", ROLE_SYSTEM_PUSHBUTTON, STATE_SYSTEM_UNAVAILABLE.0, rc(10, 300, 60, 320), "Press"),
                it("Hidden", "", ROLE_SYSTEM_PUSHBUTTON, STATE_SYSTEM_INVISIBLE.0, RECT::default(), "Open"),
            ],
        }
    }

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_GETOBJECT if is_client(lparam) => {
                return SERVER.with(|s| s.borrow_mut().get_or_insert_with(|| Server::new(hwnd)).answer(wparam));
            }
            WM_ACC_ITEMS => {
                return TREE.with(|t| match &*t.borrow() {
                    Some(tree) => {
                        let out = unsafe { &mut *(lparam.0 as *mut Tree) };
                        out.name = tree.name.clone();
                        out.role = tree.role;
                        out.focusable = tree.focusable;
                        out.focus = tree.focus;
                        out.items = tree.items.clone();
                        LRESULT(1)
                    }
                    None => LRESULT(0),
                });
            }
            WM_ACC_INVOKE => {
                INVOKED.with(|v| v.borrow_mut().push(wparam.0));
                return LRESULT(0);
            }
            // Same as every control: State dies here and takes the server with it.
            WM_NCDESTROY => drop(SERVER.with(|s| s.borrow_mut().take())),
            _ => {}
        }
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    fn window() -> HWND {
        unsafe {
            let wc = WNDCLASSW { lpfnWndProc: Some(proc), lpszClassName: w!("RexAccTest"), ..Default::default() };
            RegisterClassW(&wc);
            TREE.with(|t| *t.borrow_mut() = Some(test_tree()));
            INVOKED.with(|v| v.borrow_mut().clear());
            CreateWindowExW(WINDOW_EX_STYLE(0), w!("RexAccTest"), w!("t"), WS_POPUP, 100, 100, 300, 200, None, None, None, None).unwrap()
        }
    }

    fn pump() {
        unsafe {
            let mut m = MSG::default();
            while PeekMessageW(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = DispatchMessageW(&m);
            }
        }
    }

    fn id(n: i32) -> VARIANT {
        VARIANT::from(n)
    }

    fn screen(h: HWND, x: i32, y: i32) -> POINT {
        let mut p = POINT { x, y };
        unsafe {
            let _ = ClientToScreen(h, &mut p);
        }
        p
    }

    fn i4(v: &VARIANT) -> i32 {
        assert_eq!(v.vt(), VT_I4);
        unsafe { v.Anonymous.Anonymous.Anonymous.lVal }
    }

    fn state(acc: &IAccessible, n: i32) -> u32 {
        i4(&unsafe { acc.get_accState(&id(n)) }.unwrap()) as u32
    }

    fn name(acc: &IAccessible, n: i32) -> String {
        unsafe { acc.get_accName(&id(n)) }.unwrap().to_string()
    }

    fn loc(acc: &IAccessible, n: i32) -> (i32, i32, i32, i32) {
        let (mut x, mut y, mut w, mut h) = (0, 0, 0, 0);
        unsafe { acc.accLocation(&mut x, &mut y, &mut w, &mut h, &id(n)) }.unwrap();
        (x, y, w, h)
    }

    #[test]
    fn answers_wm_getobject_with_the_items() {
        let _com = ComInit::sta();
        let h = window();
        let acc = object(h).unwrap();
        assert_eq!(unsafe { acc.accChildCount() }.unwrap(), 4);
        assert_eq!(["Alpha", "Beta", "Gamma", "Hidden"], [1, 2, 3, 4].map(|n| name(&acc, n)));
        let role = |n| i4(&unsafe { acc.get_accRole(&id(n)) }.unwrap()) as u32;
        assert_eq!(role(1), ROLE_SYSTEM_PUSHBUTTON);
        assert_eq!(role(2), ROLE_SYSTEM_LISTITEM);
        assert_eq!(role(0), ROLE_SYSTEM_TOOLBAR);
        assert_eq!(unsafe { acc.get_accDescription(&id(1)) }.unwrap().to_string(), "first");
        assert_eq!(unsafe { acc.get_accKeyboardShortcut(&id(2)) }.unwrap().to_string(), "Ctrl+B");
        assert_eq!(unsafe { acc.get_accKeyboardShortcut(&id(1)) }.unwrap_err().code(), DISP_E_MEMBERNOTFOUND);
        let s3 = state(&acc, 3);
        assert_ne!(s3 & STATE_SYSTEM_UNAVAILABLE.0, 0);
        assert_ne!(s3 & STATE_SYSTEM_OFFSCREEN.0, 0);
        let s2 = state(&acc, 2);
        assert_eq!(s2 & (STATE_SYSTEM_SELECTED | STATE_SYSTEM_SELECTABLE), STATE_SYSTEM_SELECTED | STATE_SYSTEM_SELECTABLE);
        assert_ne!(s2 & STATE_SYSTEM_FOCUSABLE.0, 0);
        assert_eq!(s2 & STATE_SYSTEM_FOCUSED != 0, unsafe { GetFocus() } == h);
        assert_eq!(name(&acc, 0), "Test");
        let s0 = state(&acc, 0);
        assert_ne!(s0 & STATE_SYSTEM_FOCUSABLE.0, 0);
        assert_ne!(s0 & STATE_SYSTEM_INVISIBLE.0, 0);
        let p = screen(h, 10, 10);
        assert_eq!(loc(&acc, 1), (p.x, p.y, 50, 20));
        let mut wr = RECT::default();
        unsafe { GetWindowRect(h, &mut wr).unwrap() };
        assert_eq!(loc(&acc, 0), (wr.left, wr.top, wr.right - wr.left, wr.bottom - wr.top));
        assert!(unsafe { acc.accParent() }.is_ok());
        drop(acc);
        unsafe { DestroyWindow(h).unwrap() };
    }

    #[test]
    fn set_name_names_a_plain_edit() {
        let _com = ComInit::sta();
        let edit = unsafe { CreateWindowExW(WINDOW_EX_STYLE(0), w!("EDIT"), w!(""), WS_POPUP, 0, 0, 100, 20, None, None, None, None) }.unwrap();
        let acc = object(edit).unwrap();
        // Not asserted empty: oleacc borrows a nearby label's text, and parallel tests make windows of their own.
        assert_ne!(name(&acc, 0), "Search");
        set_name(edit, "Search").unwrap();
        assert_eq!(name(&object(edit).unwrap(), 0), "Search");
        drop(acc);
        unsafe { DestroyWindow(edit).unwrap() };
    }

    #[test]
    fn navigation_and_hit_testing() {
        let _com = ComInit::sta();
        let h = window();
        let acc = object(h).unwrap();
        let nav = |d: u32, n: i32| unsafe { acc.accNavigate(d as i32, &id(n)) }.unwrap();
        assert_eq!(i4(&nav(NAVDIR_FIRSTCHILD, 0)), 1);
        assert_eq!(i4(&nav(NAVDIR_LASTCHILD, 0)), 4);
        assert_eq!(i4(&nav(NAVDIR_NEXT, 1)), 2);
        assert!(nav(NAVDIR_NEXT, 4).is_empty());
        assert!(nav(NAVDIR_PREVIOUS, 1).is_empty());
        assert!(nav(NAVDIR_LEFT, 1).is_empty());
        assert!(nav(NAVDIR_NEXT, 0).is_empty());
        let hit = |x, y| unsafe { acc.accHitTest(x, y) }.unwrap();
        let p = screen(h, 95, 20);
        assert_eq!(i4(&hit(p.x, p.y)), 2);
        let p = screen(h, 200, 150);
        assert_eq!(i4(&hit(p.x, p.y)), 0);
        let p = screen(h, 0, 0);
        assert_eq!(i4(&hit(p.x, p.y)), 0);
        let p = screen(h, -50, 20);
        assert!(hit(p.x, p.y).is_empty());
        assert_eq!(i4(&unsafe { acc.accSelection() }.unwrap()), 2);
        let focus = unsafe { acc.accFocus() }.unwrap();
        if unsafe { GetFocus() } == h {
            assert_eq!(i4(&focus), 2)
        } else {
            assert!(focus.is_empty())
        }
        drop(acc);
        unsafe { DestroyWindow(h).unwrap() };
    }

    #[test]
    fn bad_child_ids_are_invalid_arg() {
        let _com = ComInit::sta();
        let h = window();
        let acc = object(h).unwrap();
        for v in [id(5), id(-1), VARIANT::from("x")] {
            assert_eq!(unsafe { acc.get_accName(&v) }.unwrap_err().code(), E_INVALIDARG);
        }
        assert!(unsafe { acc.get_accChild(&id(1)) }.is_err());
        // The raw HRESULT must reach the client as S_FALSE (simple element), not become E_FAIL.
        let mut out = std::ptr::null_mut();
        let hr = unsafe { (Interface::vtable(&acc).get_accChild)(Interface::as_raw(&acc), id(1), &mut out) };
        assert_eq!(hr, S_FALSE);
        assert_eq!(unsafe { acc.get_accValue(&id(0)) }.unwrap_err().code(), DISP_E_MEMBERNOTFOUND);
        drop(acc);
        unsafe { DestroyWindow(h).unwrap() };
    }

    #[test]
    fn default_action_is_posted_and_refused_when_unavailable() {
        let _com = ComInit::sta();
        let h = window();
        let acc = object(h).unwrap();
        assert_eq!(unsafe { acc.get_accDefaultAction(&id(1)) }.unwrap().to_string(), "Open");
        assert_eq!(unsafe { acc.get_accDefaultAction(&id(0)) }.unwrap_err().code(), DISP_E_MEMBERNOTFOUND);
        unsafe { acc.accDoDefaultAction(&id(1)) }.unwrap();
        assert!(INVOKED.with(|v| v.borrow().is_empty()));
        pump();
        assert_eq!(INVOKED.with(|v| v.borrow().clone()), [0]);
        INVOKED.with(|v| v.borrow_mut().clear());
        assert_eq!(unsafe { acc.accDoDefaultAction(&id(3)) }.unwrap_err().code(), DISP_E_MEMBERNOTFOUND);
        pump();
        assert!(INVOKED.with(|v| v.borrow().is_empty()));
        drop(acc);
        unsafe { DestroyWindow(h).unwrap() };
    }

    #[test]
    fn a_held_object_outlives_the_window() {
        let _com = ComInit::sta();
        let h = window();
        let acc = object(h).unwrap();
        let saved = TREE.with(|t| t.borrow_mut().take());
        assert_eq!(unsafe { acc.accChildCount() }.unwrap_err().code(), CO_E_OBJNOTCONNECTED);
        TREE.with(|t| *t.borrow_mut() = saved);
        assert_eq!(unsafe { acc.accChildCount() }.unwrap(), 4);
        unsafe { DestroyWindow(h).unwrap() };
        assert_eq!(unsafe { acc.accChildCount() }.unwrap_err().code(), CO_E_OBJNOTCONNECTED);
        assert_eq!(unsafe { acc.get_accName(&id(0)) }.unwrap_err().code(), CO_E_OBJNOTCONNECTED);
        assert_eq!(unsafe { acc.accDoDefaultAction(&id(1)) }.unwrap_err().code(), CO_E_OBJNOTCONNECTED);
        drop(acc);
    }

    #[test]
    fn another_thread_gets_a_proxy_to_the_window_thread() {
        use windows::Win32::System::Com::IAgileObject;
        let _com = ComInit::sta();
        let h = window();
        let target = h.0 as isize;
        let (tx, rx) = std::sync::mpsc::channel();
        let (go_tx, go) = std::sync::mpsc::channel::<()>();
        let client = std::thread::spawn(move || {
            let _com = ComInit::mta();
            let acc = object(HWND(target as *mut _)).unwrap();
            // A proxy: every call runs on the window's thread, never on this one next to the window's own code.
            let agile = acc.cast::<IAgileObject>().is_ok();
            let names = [0, 1, 4].map(|n| unsafe { acc.get_accName(&id(n)) }.map(|b| b.to_string()));
            let role = unsafe { acc.get_accRole(&id(2)) }.map(|v| i4(&v) as u32);
            tx.send((agile, names, role)).unwrap();
            // The window is gone: the proxy is cut off (CoDisconnectObject), it does not reach a dead or reused HWND.
            go.recv().unwrap();
            unsafe { acc.accChildCount() }.is_err()
        });
        let (agile, names, role) = loop {
            pump();
            if let Ok(r) = rx.try_recv() {
                break r;
            }
            std::thread::yield_now();
        };
        assert!(!agile, "another thread's calls must reach the window thread through a proxy");
        // Not even on its own thread: the object (a Cell, no locks) must never be handed out as free-threaded.
        let direct = SERVER.with(|s| s.borrow().as_ref().unwrap().0.to_interface::<IUnknown>());
        assert!(direct.cast::<IAgileObject>().is_err());
        assert_eq!(names.map(|n| n.unwrap()), ["Test", "Alpha", "Hidden"]);
        assert_eq!(role.unwrap(), ROLE_SYSTEM_LISTITEM);
        drop(direct);
        unsafe { DestroyWindow(h).unwrap() };
        go_tx.send(()).unwrap();
        while !client.is_finished() {
            pump();
            std::thread::yield_now();
        }
        assert!(client.join().unwrap(), "a call after the window died must fail");
    }

    #[test]
    fn ids_and_objids() {
        assert_eq!(child_id(None), 0);
        assert_eq!(child_id(Some(0)), 1);
        assert!(is_client(LPARAM(-4)));
        assert!(is_client(LPARAM(0xFFFF_FFFC)));
        assert!(!is_client(LPARAM(-25)));
        assert!(!is_client(LPARAM(0)));
    }
}
