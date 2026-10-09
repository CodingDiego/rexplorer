//! Embedded Explorer view (`IExplorerBrowser`): the fallback for folders our own list cannot show.

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Ole::IOleWindow;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    EBO_NOBORDER, EBO_NOTRAVELLOG, ExplorerBrowser, FOLDERSETTINGS, FVM_DETAILS, FWF_NOCLIENTEDGE,
    FWF_NOWEBVIEW, IExplorerBrowser, IExplorerBrowserEvents, IExplorerBrowserEvents_Impl, ILClone,
    IShellView, SBSP_ABSOLUTE,
};
use windows::core::{Interface, Ref, Result, implement};

use crate::com::Pidl;

#[implement(IExplorerBrowserEvents)]
struct Events {
    on_navigated: Box<dyn Fn(Pidl)>,
}

impl IExplorerBrowserEvents_Impl for Events_Impl {
    fn OnNavigationPending(&self, _: *const ITEMIDLIST) -> Result<()> {
        Ok(())
    }

    fn OnViewCreated(&self, _: Ref<IShellView>) -> Result<()> {
        Ok(())
    }

    fn OnNavigationComplete(&self, pidlfolder: *const ITEMIDLIST) -> Result<()> {
        if !pidlfolder.is_null() {
            let p = unsafe { ILClone(pidlfolder) };
            if !p.is_null() {
                (self.on_navigated)(unsafe { Pidl::from_raw(p) });
            }
        }
        Ok(())
    }

    fn OnNavigationFailed(&self, _: *const ITEMIDLIST) -> Result<()> {
        Ok(())
    }
}

/// An Explorer view hosted inside a parent window.
pub struct EmbeddedBrowser {
    browser: IExplorerBrowser,
    cookie: u32,
}

/// Creates the view inside `parent`; `on_navigated` gets the folder PIDL after each navigation completes.
pub fn create(parent: HWND, rect: RECT, on_navigated: Box<dyn Fn(Pidl)>) -> Result<EmbeddedBrowser> {
    unsafe {
        let browser: IExplorerBrowser = CoCreateInstance(&ExplorerBrowser, None, CLSCTX_INPROC_SERVER)?;
        browser.SetOptions(EBO_NOBORDER | EBO_NOTRAVELLOG)?;
        // FWF_NOBORDER does not exist in the bindings; NOCLIENTEDGE is the border flag.
        let fs = FOLDERSETTINGS {
            ViewMode: FVM_DETAILS.0 as u32,
            fFlags: (FWF_NOCLIENTEDGE.0 | FWF_NOWEBVIEW.0) as u32,
        };
        browser.Initialize(parent, &rect, Some(&fs))?;
        let events: IExplorerBrowserEvents = Events { on_navigated }.into();
        match browser.Advise(&events) {
            Ok(cookie) => Ok(EmbeddedBrowser { browser, cookie }),
            Err(e) => {
                let _ = browser.Destroy();
                Err(e)
            }
        }
    }
}

impl EmbeddedBrowser {
    /// Navigates to an absolute PIDL.
    pub fn browse(&self, pidl: &Pidl) -> Result<()> {
        unsafe { self.browser.BrowseToIDList(pidl.as_ptr(), SBSP_ABSOLUTE) }
    }

    /// The browser's own window; null if it cannot be queried.
    pub fn hwnd(&self) -> HWND {
        self.browser
            .cast::<IOleWindow>()
            .and_then(|w| unsafe { w.GetWindow() })
            .unwrap_or_default()
    }

    /// Moves the view inside its parent.
    pub fn set_rect(&self, rect: RECT) -> Result<()> {
        unsafe { self.browser.SetRect(None, rect) }
    }
}

impl Drop for EmbeddedBrowser {
    fn drop(&mut self) {
        unsafe {
            let _ = self.browser.Unadvise(self.cookie);
            let _ = self.browser.Destroy();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::ComInit;
    use crate::folder::ShellFolder;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, WINDOW_EX_STYLE,
        WS_OVERLAPPEDWINDOW,
    };
    use windows::core::w;

    #[test]
    fn browser_creates_and_browses() {
        let _c = ComInit::sta();
        let parent = unsafe {
            CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("host"), WS_OVERLAPPEDWINDOW, 0, 0, 600, 400, None, None, None, None)
        }
        .unwrap();
        let rect = RECT { left: 0, top: 0, right: 600, bottom: 400 };
        {
            let b = create(parent, rect, Box::new(|_| {})).unwrap();
            b.browse(&ShellFolder::parse(r"C:\Windows").unwrap().pidl).unwrap();
            for _ in 0..50 {
                let mut msg = MSG::default();
                while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                    unsafe { DispatchMessageW(&msg) };
                }
            }
            assert!(!b.hwnd().is_invalid());
            b.set_rect(RECT { left: 0, top: 0, right: 300, bottom: 200 }).unwrap();
        }
        unsafe { DestroyWindow(parent).unwrap() };
    }
}
