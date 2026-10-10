//! Icon themes: a folder of PNG files named by extension, decoded with WIC into
//! 32-bit premultiplied BGRA DIB bitmaps ready for an image list. `export_system_icons` seeds such a
//! folder with Windows' own icon for every registered extension.
//!
//! App fallback: for an extension the shell gives its blank page (registered, but no icon of its own, like
//! `.ps1` on a machine whose PowerShell file type was never registered), the icon of the app that opens it.
//! Explorer shows the page; Finder and Nautilus show the app. `app_fallback` serves the system image list
//! indices (details rows, search and archive rows, the export); `app_icon_bitmap` the icon views.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{E_FAIL, GENERIC_READ, GENERIC_WRITE};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetObjectW, HBITMAP, SelectObject,
};
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES, SearchPathW};
use windows::Win32::System::Registry::{HKEY_CLASSES_ROOT, RegEnumKeyExW};
use windows::Win32::UI::Controls::{
    HIMAGELIST, IImageList, ILD_PRESERVEALPHA, ILD_TRANSPARENT, ImageList_Draw, ImageList_GetIconSize,
};
use windows::Win32::UI::Shell::{
    ASSOCF_INIT_IGNOREUNKNOWN, ASSOCSTR_COMMAND, AssocQueryStringW, SHFILEINFOW, SHGFI_FLAGS, SHGFI_PIDL, SHGFI_SMALLICON, SHGFI_SYSICONINDEX,
    SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW, SHGetImageList, SHGetNameFromIDList, SHIL_EXTRALARGE, SHIL_JUMBO,
    SIGDN_PARENTRELATIVEPARSING,
};
use windows::core::{PWSTR, w};

use crate::com::{Pidl, take_cotaskmem_string};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatPng, GUID_WICPixelFormat32bppBGRA,
    GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapDitherTypeNone,
    WICBitmapEncoderNoCache, WICBitmapInterpolationModeHighQualityCubic,
    WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::StructuredStorage::IPropertyBag2;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IStream};
use windows::core::{Error, HSTRING, Interface, PCWSTR, Result};

fn factory() -> Result<IWICImagingFactory> {
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
}

/// Decodes a PNG, scales it to `size` x `size` and returns a top-down 32-bit
/// premultiplied BGRA DIB section. The caller owns the bitmap.
pub fn load_png(path: &Path, size: i32) -> Result<HBITMAP> {
    if size <= 0 {
        return Err(Error::new(E_FAIL, "icon size must be positive"));
    }
    unsafe {
        let f = factory()?;
        let wide = HSTRING::from(path.as_os_str());
        let decoder = f.CreateDecoderFromFilename(
            PCWSTR(wide.as_ptr()),
            None,
            GENERIC_READ,
            WICDecodeMetadataCacheOnDemand,
        )?;
        let frame = decoder.GetFrame(0)?;
        let scaler = f.CreateBitmapScaler()?;
        scaler.Initialize(
            &frame,
            size as u32,
            size as u32,
            WICBitmapInterpolationModeHighQualityCubic,
        )?;
        let conv = f.CreateFormatConverter()?;
        conv.Initialize(
            &scaler,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )?;
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let hbm = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let buf = std::slice::from_raw_parts_mut(bits as *mut u8, (size * size * 4) as usize);
        if let Err(e) = conv.CopyPixels(std::ptr::null(), (size * 4) as u32, buf) {
            let _ = DeleteObject(hbm.into());
            return Err(e);
        }
        Ok(hbm)
    }
}

/// Writes tightly packed straight-alpha BGRA pixels as a PNG.
pub fn save_png(bits_bgra: &[u8], width: u32, height: u32, path: &Path) -> Result<()> {
    if bits_bgra.len() != width as usize * height as usize * 4 {
        return Err(Error::new(E_FAIL, "pixel buffer does not match width x height"));
    }
    unsafe {
        let f = factory()?;
        // CreateEncoder with the PNG container GUID selects the same encoder as CLSID_WICPngEncoder.
        let enc = f.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        let stream = f.CreateStream()?;
        let wide = HSTRING::from(path.as_os_str());
        stream.InitializeFromFilename(PCWSTR(wide.as_ptr()), GENERIC_WRITE.0)?;
        let istream: IStream = stream.cast()?;
        enc.Initialize(&istream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        enc.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.ok_or_else(|| Error::new(E_FAIL, "no encoder frame"))?;
        frame.Initialize(None::<&IPropertyBag2>)?;
        frame.SetSize(width, height)?;
        let mut fmt = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut fmt)?;
        frame.WritePixels(height, width * 4, bits_bgra)?;
        frame.Commit()?;
        enc.Commit()?;
        Ok(())
    }
}

/// Extensions registered under HKEY_CLASSES_ROOT, without the dot, lowercase, in registry order.
pub fn registered_extensions() -> Vec<String> {
    let mut out = Vec::new();
    for index in 0.. {
        let mut name = [0u16; 256];
        let mut len = name.len() as u32;
        let hr = unsafe { RegEnumKeyExW(HKEY_CLASSES_ROOT, index, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) };
        if hr.is_err() {
            break;
        }
        let name = String::from_utf16_lossy(&name[..len as usize]);
        // The extension becomes a file name (`txt.png`) and a theme key (the last extension only).
        if let Some(ext) = name.strip_prefix('.') {
            if !ext.is_empty() && !ext.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|', '.']) {
                out.push(ext.to_lowercase());
            }
        }
    }
    out
}

/// Index into the system image lists of the icon for `path`, or for a `.ext` name alone with
/// `SHGFI_USEFILEATTRIBUTES` in `extra` (the registry decides, nothing is read from disk).
fn sys_index(path: &str, extra: SHGFI_FLAGS) -> Option<i32> {
    let name = HSTRING::from(path);
    let mut fi = SHFILEINFOW::default();
    let flags = SHGFI_SYSICONINDEX | SHGFI_SMALLICON | extra;
    let himl = unsafe { SHGetFileInfoW(PCWSTR(name.as_ptr()), FILE_ATTRIBUTE_NORMAL, Some(&mut fi), size_of::<SHFILEINFOW>() as u32, flags) };
    (himl != 0).then_some(fi.iIcon)
}

fn system_icon_index(ext: &str) -> Option<i32> {
    sys_index(&format!("x.{ext}"), SHGFI_USEFILEATTRIBUTES)
}

/// The blank page the shell gives a file whose type has no icon of its own.
fn generic_index() -> Option<i32> {
    static GENERIC: OnceLock<Option<i32>> = OnceLock::new();
    *GENERIC.get_or_init(|| system_icon_index("rexplorer-no-such-extension"))
}

/// The executable the shell's "open" command for `.ext` runs. A registered path that is gone (a WinSxS copy
/// a Windows update removed, as PowerShell's here) is searched again by file name on PATH.
fn opener_exe(ext: &str) -> Option<String> {
    let assoc = HSTRING::from(format!(".{ext}"));
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // IGNOREUNKNOWN: an unregistered extension has no opener, not the "Open with" dialog.
    unsafe { AssocQueryStringW(ASSOCF_INIT_IGNOREUNKNOWN, ASSOCSTR_COMMAND, PCWSTR(assoc.as_ptr()), w!("open"), Some(PWSTR(buf.as_mut_ptr())), &mut len) }.ok().ok()?;
    let cmd = String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)]);
    // shortcut: an unquoted path with spaces is cut at the first space; quote-less commands are rare
    let exe = match cmd.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => cmd.split(' ').next()?,
    };
    if Path::new(exe).is_file() {
        return Some(exe.to_string());
    }
    let file = HSTRING::from(Path::new(exe).file_name()?);
    let mut found = [0u16; 1024];
    let n = unsafe { SearchPathW(PCWSTR::null(), PCWSTR(file.as_ptr()), PCWSTR::null(), Some(&mut found), None) } as usize;
    (0 < n && n < found.len()).then(|| String::from_utf16_lossy(&found[..n]))
}

/// System image list index of the icon of the app that opens `.ext` files; None without one, or when the
/// app's icon is the blank page too. Resolved once per extension.
pub fn app_icon_index(ext: &str) -> Option<i32> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<i32>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let ext = ext.to_lowercase();
    if let Some(i) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&ext) {
        return *i;
    }
    let index = opener_exe(&ext).and_then(|exe| sys_index(&exe, SHGFI_FLAGS(0))).filter(|i| Some(*i) != generic_index());
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(ext, index);
    index
}

/// `index` unless it is the blank page, in which case the opening app's icon for `name`'s extension when
/// there is one. `name` is only asked for in that case.
pub fn app_fallback(index: i32, name: impl FnOnce() -> String) -> i32 {
    if Some(index) != generic_index() {
        return index;
    }
    Path::new(&name()).extension().and_then(|e| e.to_str()).and_then(app_icon_index).unwrap_or(index)
}

/// System icon `index` drawn from shell image list `shil` into a top-down 32-bit DIB (premultiplied, as the
/// list draws) the caller owns.
fn draw_system_icon(shil: u32, index: i32) -> Option<HBITMAP> {
    let list: IImageList = unsafe { SHGetImageList(shil as i32) }.ok()?;
    let himl = HIMAGELIST(list.as_raw() as isize);
    let (mut w, mut h) = (0, 0);
    unsafe { ImageList_GetIconSize(himl, Some(&mut w), Some(&mut h)) }.ok().ok()?;
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let hbm = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        let dc = CreateCompatibleDC(None);
        let old = SelectObject(dc, hbm.into());
        let drawn = ImageList_Draw(himl, index, dc, 0, 0, ILD_TRANSPARENT | ILD_PRESERVEALPHA).as_bool();
        SelectObject(dc, old);
        let _ = DeleteDC(dc);
        if !drawn {
            let _ = DeleteObject(hbm.into());
            return None;
        }
        Some(hbm)
    }
}

/// The pixels of a 32-bit DIB section (top-down BGRA rows) and its width and height.
fn dib_pixels<'a>(bmp: HBITMAP) -> Option<(&'a mut [u8], u32, u32)> {
    let mut bm = BITMAP::default();
    let n = unsafe { GetObjectW(bmp.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) };
    if n == 0 || bm.bmBitsPixel != 32 || bm.bmBits.is_null() {
        return None;
    }
    let len = (bm.bmWidthBytes * bm.bmHeight.abs()) as usize;
    Some((unsafe { std::slice::from_raw_parts_mut(bm.bmBits as *mut u8, len) }, bm.bmWidth as u32, bm.bmHeight.unsigned_abs()))
}

/// Premultiplied to straight alpha in place: the inverse of `images::premultiply`, so a list-drawn icon
/// keeps the contract of the shell's own images.
fn straighten(bmp: HBITMAP) {
    let Some((px, _, _)) = dib_pixels(bmp) else { return };
    for p in px.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if 0 < a && a < 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

/// Whether every visible pixel of `bmp` lies inside its top-left `edge` x `edge` corner: the jumbo list
/// draws an icon that has no 256 px image unscaled there.
fn confined_to_corner(bmp: HBITMAP, edge: u32) -> bool {
    let Some((px, w, _)) = dib_pixels(bmp) else { return false };
    px.chunks_exact(4).enumerate().all(|(i, p)| p[3] == 0 || (i as u32 % w < edge && i as u32 / w < edge))
}

/// For an item the shell gives the blank page: the opening app's icon as a straight-alpha DIB at least
/// `size` px (48 or 256 wide; the caller scales). None when the shell has an icon of its own for it.
pub fn app_icon_bitmap(abs: &Pidl, size: i32) -> Option<HBITMAP> {
    let mut fi = SHFILEINFOW::default();
    let flags = SHGFI_PIDL | SHGFI_SYSICONINDEX | SHGFI_SMALLICON;
    let himl = unsafe { SHGetFileInfoW(PCWSTR(abs.as_ptr() as *const u16), FILE_FLAGS_AND_ATTRIBUTES(0), Some(&mut fi), size_of::<SHFILEINFOW>() as u32, flags) };
    if himl == 0 || Some(fi.iIcon) != generic_index() {
        return None;
    }
    let name = unsafe { take_cotaskmem_string(SHGetNameFromIDList(abs.as_ptr(), SIGDN_PARENTRELATIVEPARSING).ok()?) };
    let index = Path::new(&name).extension().and_then(|e| e.to_str()).and_then(app_icon_index)?;
    let mut bmp = None;
    if size > 48 {
        bmp = draw_system_icon(SHIL_JUMBO, index).filter(|&b| {
            let ok = !confined_to_corner(b, 48);
            if !ok {
                unsafe {
                    let _ = DeleteObject(b.into());
                }
            }
            ok
        });
    }
    let bmp = bmp.or_else(|| draw_system_icon(SHIL_EXTRALARGE, index))?;
    straighten(bmp);
    Some(bmp)
}

/// Writes `<ext>.png` into `dir` for every registered extension that has an icon of its own (or an opening
/// app with one) and no PNG there yet, from the shell's 48 px image list. Returns the extensions written.
/// Takes seconds: keep it off the UI thread.
pub fn export_system_icons(dir: &Path) -> Vec<String> {
    let mut written = Vec::new();
    for ext in registered_extensions() {
        let out = dir.join(format!("{ext}.png"));
        if out.exists() {
            continue;
        }
        // Extensions left with the blank page keep the theme's file.png.
        let Some(index) = system_icon_index(&ext).map(|i| app_fallback(i, || format!("x.{ext}"))).filter(|i| Some(*i) != generic_index()) else { continue };
        let Some(bmp) = draw_system_icon(SHIL_EXTRALARGE, index) else { continue };
        straighten(bmp);
        if let Some((px, w, h)) = dib_pixels(bmp) {
            if save_png(px, w, h, &out).is_ok() {
                written.push(ext);
            }
        }
        unsafe {
            let _ = DeleteObject(bmp.into());
        }
    }
    written
}

/// A theme folder; each `<key>.png` is decoded at `size` x `size` on first use (a folder of hundreds would
/// cost startup otherwise). Owns and deletes the bitmaps.
pub struct IconTheme {
    dir: PathBuf,
    size: i32,
    /// None: the PNG is missing or broken, remembered so it is not retried.
    bitmaps: HashMap<String, Option<HBITMAP>>,
}

pub fn load_theme(dir: &Path, size: i32) -> Result<IconTheme> {
    if !dir.is_dir() {
        return Err(Error::new(E_FAIL, "icon theme folder not found"));
    }
    Ok(IconTheme { dir: dir.to_path_buf(), size, bitmaps: HashMap::new() })
}

impl IconTheme {
    /// The extension's PNG when there is one (a zip is a folder to the shell, but its own icon wins), else
    /// "folder" or "file".
    pub fn bitmap_for(&mut self, name: &str, is_folder: bool) -> Option<HBITMAP> {
        let ext = Path::new(name).extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase());
        ext.and_then(|e| self.get(&e)).or_else(|| self.get(if is_folder { "folder" } else { "file" }))
    }

    fn get(&mut self, key: &str) -> Option<HBITMAP> {
        if let Some(h) = self.bitmaps.get(key) {
            return *h;
        }
        // Names from archives and shell namespaces may hold what NTFS forbids; `z:a` would join as a drive-relative path.
        if key.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']) {
            return None;
        }
        let h = load_png(&self.dir.join(format!("{key}.png")), self.size).ok();
        self.bitmaps.insert(key.to_string(), h);
        h
    }
}

impl Drop for IconTheme {
    fn drop(&mut self) {
        for h in self.bitmaps.values().flatten() {
            unsafe {
                let _ = DeleteObject((*h).into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::ComInit;
    use windows::Win32::Graphics::Gdi::{BITMAP, GetObjectW};

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rexplorer-icons-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn blue(w: u32, h: u32) -> Vec<u8> {
        [255u8, 0, 0, 255].repeat((w * h) as usize)
    }

    #[test]
    fn png_round_trip_scales() {
        let _com = ComInit::sta();
        let dir = tmp("rt");
        let p = dir.join("blue.png");
        save_png(&blue(8, 8), 8, 8, &p).unwrap();
        let h = load_png(&p, 16).unwrap();
        let mut bm = BITMAP::default();
        let n = unsafe {
            GetObjectW(h.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _))
        };
        assert!(n > 0);
        assert_eq!((bm.bmWidth, bm.bmHeight, bm.bmBitsPixel), (16, 16, 32));
        unsafe {
            let _ = DeleteObject(h.into());
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn theme_maps_extensions_and_falls_back() {
        let _com = ComInit::sta();
        let dir = tmp("theme");
        for n in ["rs", "folder", "file"] {
            save_png(&blue(8, 8), 8, 8, &dir.join(format!("{n}.png"))).unwrap();
        }
        let mut t = load_theme(&dir, 16).unwrap();
        let rs = t.bitmap_for("main.rs", false).unwrap();
        assert_eq!(t.bitmap_for("MAIN.RS", false), Some(rs), "decoded once, case-insensitive");
        let file = t.get("file");
        assert!(file.is_some());
        assert_eq!(t.bitmap_for("x.unknownext", false), file);
        assert_eq!(t.bitmap_for("noext", false), file);
        assert_eq!(t.bitmap_for("docs", true), t.get("folder"));
        assert_eq!(t.bitmap_for("v1.2", true), t.get("folder"));
        assert_eq!(t.bitmap_for("crate.rs", true), Some(rs), "a folder with a themed extension shows it");
        assert_eq!(t.bitmaps.len(), 5, "rs, file, folder and the missing unknownext and 2 are remembered");
        assert!(load_theme(&dir.join("nope"), 16).is_err());
        drop(t);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn registry_lists_txt() {
        assert!(registered_extensions().iter().any(|e| e == "txt"));
    }

    #[test]
    fn system_icon_renders_with_alpha() {
        let _com = ComInit::sta();
        let index = system_icon_index("txt").unwrap();
        assert_ne!(Some(index), generic_index());
        let bmp = draw_system_icon(SHIL_EXTRALARGE, index).unwrap();
        straighten(bmp);
        let (px, w, h) = dib_pixels(bmp).unwrap();
        assert_eq!((w, h, px.len()), (48, 48, 48 * 48 * 4));
        let alphas = px.chunks_exact(4).map(|p| p[3]);
        assert!(alphas.clone().any(|a| a == 255), "no opaque pixel");
        assert!(alphas.clone().any(|a| a == 0), "no transparent pixel");
        // Straight alpha: an anti-aliased edge keeps its color above its alpha.
        assert!(px.chunks_exact(4).any(|p| 0 < p[3] && p[3] < 255 && p[..3].iter().any(|&c| c > p[3])));
        assert!(!confined_to_corner(bmp, 16));
        assert!(confined_to_corner(bmp, 48));
        unsafe {
            let _ = DeleteObject(bmp.into());
        }
    }

    #[test]
    fn app_fallback_replaces_the_blank_page_with_the_opener() {
        let _com = ComInit::sta();
        let generic = generic_index().unwrap();
        // .txt opens with Notepad on every Windows; its icon is not the blank page.
        assert_ne!(app_fallback(generic, || "notes.txt".into()), generic);
        assert_eq!(app_fallback(generic, || "x.rexplorer-no-such-ext".into()), generic);
        assert_eq!(app_fallback(generic, || "noext".into()), generic);
        assert_eq!(app_fallback(generic + 1, || panic!("name asked for an icon of its own")), generic + 1);
        assert!(app_icon_index("rexplorer-no-such-ext").is_none());
    }

    #[test]
    fn app_icon_bitmap_only_for_blank_page_items() {
        use windows::Win32::UI::Shell::SHParseDisplayName;
        let _com = ComInit::sta();
        let dir = tmp("appicon");
        let txt = dir.join("a.txt");
        std::fs::write(&txt, "x").unwrap();
        let mut p = std::ptr::null_mut();
        unsafe { SHParseDisplayName(&HSTRING::from(txt.as_os_str()), None, &mut p, 0, None) }.unwrap();
        let abs = unsafe { Pidl::from_raw(p) };
        assert!(app_icon_bitmap(&abs, 96).is_none(), "txt has an icon of its own");
        // An extension the shell leaves blank but an app opens: machine-dependent, so only checked when present.
        let blank = registered_extensions().into_iter().find(|e| system_icon_index(e) == generic_index() && app_icon_index(e).is_some());
        if let Some(ext) = blank {
            let f = dir.join(format!("b.{ext}"));
            std::fs::write(&f, "x").unwrap();
            let mut p = std::ptr::null_mut();
            unsafe { SHParseDisplayName(&HSTRING::from(f.as_os_str()), None, &mut p, 0, None) }.unwrap();
            let abs = unsafe { Pidl::from_raw(p) };
            let bmp = app_icon_bitmap(&abs, 96).expect("opener icon");
            let (px, w, _) = dib_pixels(bmp).unwrap();
            assert!(w == 48 || w == 256, "w={w}");
            assert!(px.chunks_exact(4).any(|p| p[3] == 255), "{ext}: no opaque pixel");
            unsafe {
                let _ = DeleteObject(bmp.into());
            }
        } else {
            eprintln!("no blank-page extension with an opener on this machine; bitmap path not exercised");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
