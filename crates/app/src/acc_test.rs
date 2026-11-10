//! Test helpers for the MSAA servers: read a control's accessible tree as a client would, and record the
//! WinEvents it raises (in-process, this thread only, so parallel tests do not see each other's events).
#![allow(dead_code)]
use std::cell::RefCell;

use rexplorer_core::access;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, IAccessible, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, OBJID_CLIENT, PM_REMOVE, PeekMessageW, TranslateMessage, WINEVENT_OUTOFCONTEXT,
};
use windows::core::Result;

pub struct Kid {
    pub name: String,
    pub description: String,
    pub role: u32,
    pub state: u32,
    /// Screen ltrb.
    pub rect: RECT,
}

pub fn object(hwnd: HWND) -> IAccessible {
    access::object(hwnd).unwrap()
}

fn id(n: i32) -> VARIANT {
    VARIANT::from(n)
}

fn i4(v: &VARIANT) -> i32 {
    unsafe { v.Anonymous.Anonymous.Anonymous.lVal }
}

pub fn kid(acc: &IAccessible, n: i32) -> Kid {
    unsafe {
        let v = id(n);
        let (mut x, mut y, mut w, mut h) = (0, 0, 0, 0);
        acc.accLocation(&mut x, &mut y, &mut w, &mut h, &v).unwrap();
        Kid {
            name: acc.get_accName(&v).map(|b| b.to_string()).unwrap_or_default(),
            description: acc.get_accDescription(&v).map(|b| b.to_string()).unwrap_or_default(),
            role: i4(&acc.get_accRole(&v).unwrap()) as u32,
            state: i4(&acc.get_accState(&v).unwrap()) as u32,
            rect: RECT { left: x, top: y, right: x + w, bottom: y + h },
        }
    }
}

pub fn kids(acc: &IAccessible) -> Vec<Kid> {
    let n = unsafe { acc.accChildCount() }.unwrap();
    (1..=n).map(|i| kid(acc, i)).collect()
}

pub fn root(acc: &IAccessible) -> Kid {
    kid(acc, 0)
}

pub fn screen(hwnd: HWND, r: RECT) -> RECT {
    let (mut a, mut b) = (POINT { x: r.left, y: r.top }, POINT { x: r.right, y: r.bottom });
    unsafe {
        let _ = ClientToScreen(hwnd, &mut a);
        let _ = ClientToScreen(hwnd, &mut b);
    }
    RECT { left: a.x, top: a.y, right: b.x, bottom: b.y }
}

pub fn pump() {
    unsafe {
        let mut m = MSG::default();
        while PeekMessageW(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
}

pub fn invoke(acc: &IAccessible, n: i32) -> Result<()> {
    let r = unsafe { acc.accDoDefaultAction(&id(n)) };
    pump();
    r
}

thread_local! {
    static EVENTS: RefCell<Vec<(u32, isize, i32)>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "system" fn record(_: HWINEVENTHOOK, event: u32, hwnd: HWND, id_object: i32, id_child: i32, _: u32, _: u32) {
    if id_object == OBJID_CLIENT.0 {
        EVENTS.with(|e| e.borrow_mut().push((event, hwnd.0 as isize, id_child)));
    }
}

pub struct Hook(HWINEVENTHOOK);

/// EVENT_OBJECT_REORDER ..= EVENT_OBJECT_NAMECHANGE (focus, selection, state change are in between).
pub fn hook() -> Hook {
    EVENTS.with(|e| e.borrow_mut().clear());
    let h = unsafe {
        SetWinEventHook(0x8004, 0x800C, None, Some(record), std::process::id(), GetCurrentThreadId(), WINEVENT_OUTOFCONTEXT)
    };
    Hook(h)
}

impl Hook {
    /// Pumps, then returns and clears what was recorded as (event, hwnd, child id).
    pub fn take(&self) -> Vec<(u32, HWND, i32)> {
        pump();
        EVENTS.with(|e| e.borrow_mut().drain(..).map(|(ev, h, c)| (ev, HWND(h as *mut _), c)).collect())
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        let _ = unsafe { UnhookWinEvent(self.0) };
    }
}
