//! Icon themes: a folder of PNG files named by extension, decoded with WIC into
//! 32-bit premultiplied BGRA DIB bitmaps ready for an image list.
use std::collections::HashMap;
use std::path::Path;

use windows::Win32::Foundation::{E_FAIL, GENERIC_READ, GENERIC_WRITE};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, HBITMAP,
};
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

/// Bitmaps keyed by lowercase file stem. Owns and deletes them.
pub struct IconTheme {
    bitmaps: HashMap<String, HBITMAP>,
    size: i32,
}

/// Loads every `*.png` in `dir` at `size` x `size`.
pub fn load_theme(dir: &Path, size: i32) -> Result<IconTheme> {
    let mut theme = IconTheme { bitmaps: HashMap::new(), size };
    let rd = std::fs::read_dir(dir).map_err(|e| Error::new(E_FAIL, e.to_string()))?;
    for entry in rd.flatten() {
        let p = entry.path();
        let is_png = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"));
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()).filter(|_| is_png) else {
            continue;
        };
        // A broken PNG is skipped rather than failing the whole theme.
        if let Ok(h) = load_png(&p, size) {
            if let Some(old) = theme.bitmaps.insert(stem.to_lowercase(), h) {
                unsafe {
                    let _ = DeleteObject(old.into());
                }
            }
        }
    }
    Ok(theme)
}

impl IconTheme {
    /// Folders use "folder"; files use their extension's key, else "file".
    pub fn bitmap_for(&self, name: &str, is_folder: bool) -> Option<HBITMAP> {
        if is_folder {
            return self.bitmaps.get("folder").copied();
        }
        Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .and_then(|e| self.bitmaps.get(&e.to_lowercase()))
            .or_else(|| self.bitmaps.get("file"))
            .copied()
    }

    pub fn len(&self) -> usize {
        self.bitmaps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bitmaps.is_empty()
    }

    pub fn size(&self) -> i32 {
        self.size
    }
}

impl Drop for IconTheme {
    fn drop(&mut self) {
        for h in self.bitmaps.values() {
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
        let t = load_theme(&dir, 16).unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!(t.size(), 16);
        assert!(t.bitmap_for("main.rs", false).is_some());
        assert_eq!(t.bitmap_for("x.unknownext", false), t.bitmaps.get("file").copied());
        assert_eq!(t.bitmap_for("docs", true), t.bitmaps.get("folder").copied());
        drop(t);
        std::fs::remove_dir_all(&dir).ok();
    }
}
