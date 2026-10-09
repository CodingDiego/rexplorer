//! Hosts the IPreviewHandler objects the Explorer preview pane uses, and classifies what to preview.
use std::path::Path;

use windows::Win32::Foundation::{E_FAIL, HWND, RECT};
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, CLSIDFromString, CoCreateInstance, STGM_READ,
    STGM_SHARE_DENY_NONE,
};
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithFile, IInitializeWithStream};
use windows::Win32::UI::Shell::{
    ASSOCF, ASSOCSTR_SHELLEXTENSION, AssocQueryStringW, IInitializeWithItem, IPreviewHandler,
    IShellItem, SHCreateItemFromParsingName, SHCreateStreamOnFileEx,
};
use windows::core::{Error, GUID, HSTRING, Interface, PCWSTR, PWSTR, Result, w};

#[derive(Debug, PartialEq)]
pub enum Kind {
    Text,
    Image,
    Handler,
    None,
}

const TEXT_EXTS: &[&str] = &[
    "txt", "md", "markdown", "csv", "tsv", "json", "toml", "yaml", "yml", "ini", "cfg", "conf",
    "log", "xml", "html", "htm", "css", "scss", "js", "mjs", "ts", "tsx", "jsx", "rs", "py", "rb",
    "php", "go", "java", "kt", "c", "h", "cc", "cpp", "hpp", "cs", "swift", "lua", "sh", "bash",
    "zsh", "ps1", "psm1", "bat", "cmd", "sql", "r", "m", "mm", "pl", "gradle", "dockerfile",
    "makefile", "gitignore", "editorconfig", "env",
];
const IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "ico", "tif", "tiff", "heic", "heif", "avif", "jxl",
];

/// Classifies `ext` ("rs" or ".rs", any case); the Handler arm queries the registry.
pub fn kind(ext: &str) -> Kind {
    let e = ext.trim_start_matches('.').to_ascii_lowercase();
    if TEXT_EXTS.contains(&e.as_str()) {
        Kind::Text
    } else if IMAGE_EXTS.contains(&e.as_str()) {
        Kind::Image
    } else if handler_clsid(&e).is_some() {
        Kind::Handler
    } else {
        Kind::None
    }
}

/// CLSID of the registered preview handler for `ext`, if any.
pub fn handler_clsid(ext: &str) -> Option<GUID> {
    let dotted = format!(".{}", ext.trim_start_matches('.'));
    let mut buf = [0u16; 128];
    let mut len = buf.len() as u32;
    unsafe {
        AssocQueryStringW(
            ASSOCF(0),
            ASSOCSTR_SHELLEXTENSION,
            &HSTRING::from(dotted),
            w!("{8895b1c6-b41f-4c1c-a562-0d564250836f}"),
            Some(PWSTR(buf.as_mut_ptr())),
            &mut len,
        )
        .ok()
        .ok()?;
        CLSIDFromString(PCWSTR(buf.as_ptr())).ok()
    }
}

/// A live preview handler hosted in a parent window; unloaded on drop.
pub struct Preview {
    handler: IPreviewHandler,
}

/// Creates the extension's preview handler, initializes it with `path` and previews into `host`.
pub fn open(path: &Path, host: HWND, rect: RECT) -> Result<Preview> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .ok_or_else(|| Error::new(E_FAIL, "file has no extension"))?;
    let clsid = handler_clsid(ext)
        .ok_or_else(|| Error::new(E_FAIL, format!("no preview handler for .{ext}")))?;
    unsafe {
        let handler: IPreviewHandler =
            CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER | CLSCTX_INPROC_SERVER)?;
        let wide = HSTRING::from(path.as_os_str());
        let mode = STGM_READ.0;
        if let Ok(init) = handler.cast::<IInitializeWithStream>() {
            let stream = SHCreateStreamOnFileEx(
                &wide,
                (STGM_READ | STGM_SHARE_DENY_NONE).0,
                FILE_ATTRIBUTE_NORMAL.0,
                false,
                None,
            )?;
            init.Initialize(&stream, mode)?;
        } else if let Ok(init) = handler.cast::<IInitializeWithFile>() {
            init.Initialize(&wide, mode)?;
        } else if let Ok(init) = handler.cast::<IInitializeWithItem>() {
            let item: IShellItem = SHCreateItemFromParsingName(&wide, None)?;
            init.Initialize(&item, mode)?;
        } else {
            return Err(Error::new(E_FAIL, "handler has no IInitializeWith* interface"));
        }
        handler.SetWindow(host, &rect)?;
        // Preview owns the handler from here so Drop unloads it even if DoPreview fails.
        let preview = Preview { handler };
        preview.handler.DoPreview()?;
        Ok(preview)
    }
}

impl Preview {
    pub fn set_rect(&self, rect: RECT) -> Result<()> {
        unsafe { self.handler.SetRect(&rect) }
    }

    pub fn set_focus(&self) -> Result<()> {
        unsafe { self.handler.SetFocus() }
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        let _ = unsafe { self.handler.Unload() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::ComInit;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW,
        WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW,
    };

    #[test]
    fn kind_classifies() {
        assert_eq!(kind("rs"), Kind::Text);
        assert_eq!(kind(".CSV"), Kind::Text);
        assert_eq!(kind("png"), Kind::Image);
        assert_eq!(kind("zzz-not-a-type"), Kind::None);
    }

    #[test]
    fn txt_has_a_system_preview_handler() {
        let _com = ComInit::sta();
        assert!(handler_clsid("txt").is_some());
    }

    #[test]
    fn open_txt_preview_in_hidden_window() {
        let _com = ComInit::sta();
        let dir = std::env::temp_dir().join(format!("rexplorer-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "hello preview").unwrap();
        let rect = RECT { left: 0, top: 0, right: 400, bottom: 300 };
        unsafe {
            let host = CreateWindowExW(
                WINDOW_EX_STYLE(0), w!("STATIC"), w!("host"), WS_OVERLAPPEDWINDOW,
                0, 0, 400, 300, None, None, None, None,
            )
            .unwrap();
            match open(&file, host, rect) {
                Ok(p) => {
                    let mut msg = MSG::default();
                    for _ in 0..50 {
                        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                            DispatchMessageW(&msg);
                        }
                    }
                    drop(p);
                }
                // prevhost surrogate can be unavailable in non-interactive sessions.
                Err(e) if e.code().0 as u32 == 0x80080005 || e.code().0 as u32 == 0x80040154 => {
                    eprintln!("preview unavailable in this session: {e}");
                }
                Err(e) => {
                    let _ = DestroyWindow(host);
                    let _ = std::fs::remove_dir_all(&dir);
                    panic!("open failed: {e}");
                }
            }
            let _ = DestroyWindow(host);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
