//! Thumbnails and large icons via `IShellItemImageFactory`, plus a small LRU bitmap cache.
//!
//! All bitmaps are 32-bit DIBs; `ImageList_Add` accepts them as-is. `GetImage` returns straight alpha (edge pixels
//! carry color above their alpha), so anything drawn with `AlphaBlend` goes through `premultiply` first.
use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::size_of;

use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{BITMAP, DeleteObject, GetObjectW, HBITMAP};
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromIDList, SIIGBF, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
    SIIGBF_RESIZETOFIT,
};
use windows::core::Result;

use crate::com::Pidl;

/// Renders `abs` at `size` x `size`; the caller owns the returned 32-bit straight-alpha DIB.
pub fn image(abs: &Pidl, size: i32, flags: SIIGBF) -> Result<HBITMAP> {
    let factory: IShellItemImageFactory = unsafe { SHCreateItemFromIDList(abs.as_ptr())? };
    unsafe { factory.GetImage(SIZE { cx: size, cy: size }, flags) }
}

/// Content thumbnail (falls back to the icon when the item has none). An item the shell would show as the
/// blank page gets its opening app's icon (`icons::app_icon_bitmap`), larger than `size`: callers fit it.
pub fn thumbnail(abs: &Pidl, size: i32) -> Result<HBITMAP> {
    if let Some(b) = crate::icons::app_icon_bitmap(abs, size) {
        return Ok(b);
    }
    image(abs, size, SIIGBF_RESIZETOFIT | SIIGBF_BIGGERSIZEOK)
}

/// The item's icon only, never a content thumbnail; the opening app's icon in place of the blank page.
pub fn icon(abs: &Pidl, size: i32) -> Result<HBITMAP> {
    if let Some(b) = crate::icons::app_icon_bitmap(abs, size) {
        return Ok(b);
    }
    image(abs, size, SIIGBF_ICONONLY | SIIGBF_RESIZETOFIT)
}

/// Converts a 32-bit DIB section in place to premultiplied alpha, as `AlphaBlend` with `AC_SRC_ALPHA` expects.
/// Straight-alpha edge pixels would otherwise add their full color and halo the image.
pub fn premultiply(bmp: HBITMAP) {
    let mut bm = BITMAP::default();
    let n = unsafe { GetObjectW(bmp.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void)) };
    if n == 0 || bm.bmBitsPixel != 32 || bm.bmBits.is_null() {
        return;
    }
    let count = (bm.bmWidthBytes / 4 * bm.bmHeight.abs()) as usize;
    let px = unsafe { std::slice::from_raw_parts_mut(bm.bmBits as *mut [u8; 4], count) };
    for p in px {
        let a = p[3] as u32;
        if a < 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
}

/// LRU cache that owns its bitmaps and deletes them on eviction, `clear` and drop.
pub struct ImageCache {
    capacity: usize,
    tick: u64,
    map: HashMap<String, (HBITMAP, u64)>,
}

impl ImageCache {
    pub fn new(capacity: usize) -> Self {
        ImageCache { capacity, tick: 0, map: HashMap::new() }
    }

    /// Returns the cached bitmap (still owned by the cache) and marks it recently used.
    pub fn get(&mut self, key: &str) -> Option<HBITMAP> {
        self.tick += 1;
        let tick = self.tick;
        self.map.get_mut(key).map(|e| {
            e.1 = tick;
            e.0
        })
    }

    /// Takes ownership of `bmp`; evicts the least recently used entry when over capacity.
    pub fn insert(&mut self, key: String, bmp: HBITMAP) {
        self.tick += 1;
        if let Some((old, _)) = self.map.insert(key, (bmp, self.tick)) {
            delete(old);
        }
        while self.map.len() > self.capacity {
            // shortcut: O(n) scan, fine for a few hundred entries.
            let Some(lru) = self.map.iter().min_by_key(|(_, e)| e.1).map(|(k, _)| k.clone()) else {
                break;
            };
            if let Some((old, _)) = self.map.remove(&lru) {
                delete(old);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn clear(&mut self) {
        for (_, (bmp, _)) in self.map.drain() {
            delete(bmp);
        }
    }
}

impl Drop for ImageCache {
    fn drop(&mut self) {
        self.clear();
    }
}

fn delete(bmp: HBITMAP) {
    let _ = unsafe { DeleteObject(bmp.into()) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::ComInit;
    use std::mem::size_of;
    use windows::Win32::Graphics::Gdi::{BITMAP, CreateBitmap, GetObjectW};
    use windows::Win32::UI::Shell::SHParseDisplayName;
    use windows::core::HSTRING;

    fn parse(path: &std::path::Path) -> Pidl {
        let mut p = std::ptr::null_mut();
        unsafe { SHParseDisplayName(&HSTRING::from(path.as_os_str()), None, &mut p, 0, None).unwrap() };
        unsafe { Pidl::from_raw(p) }
    }

    fn assert_bitmap(bmp: HBITMAP) {
        assert!(!bmp.is_invalid());
        let mut b = BITMAP::default();
        let n = unsafe {
            GetObjectW(bmp.into(), size_of::<BITMAP>() as i32, Some(&mut b as *mut _ as *mut _))
        };
        assert!(n > 0);
        assert!(b.bmWidth > 0 && b.bmHeight > 0);
        delete(bmp);
    }

    #[test]
    fn thumbnail_of_text_file_is_a_bitmap() {
        let _c = ComInit::sta();
        let dir = std::env::temp_dir().join(format!("rexplorer-images-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, b"abc").unwrap();
        let r = thumbnail(&parse(&file), 96);
        let _ = std::fs::remove_dir_all(&dir);
        assert_bitmap(r.unwrap());
    }

    #[test]
    fn icon_of_folder_is_a_bitmap() {
        let _c = ComInit::sta();
        assert_bitmap(icon(&parse(&std::env::temp_dir()), 48).unwrap());
    }

    #[test]
    fn premultiply_scales_color_by_alpha() {
        use windows::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS};
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: 2,
                biHeight: -1,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bmp = unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap() };
        let px = unsafe { std::slice::from_raw_parts_mut(bits as *mut [u8; 4], 2) };
        px[0] = [0, 102, 102, 10];
        px[1] = [10, 20, 30, 255];
        premultiply(bmp);
        assert_eq!(px[0], [0, 4, 4, 10], "straight edge pixel scaled by its alpha");
        assert_eq!(px[1], [10, 20, 30, 255], "opaque pixels untouched");
        delete(bmp);
    }

    #[test]
    fn shell_icons_come_straight_and_premultiply_to_valid_pixels() {
        let _c = ComInit::sta();
        let bmp = icon(&parse(&std::env::temp_dir()), 48).unwrap();
        premultiply(bmp);
        let mut b = BITMAP::default();
        unsafe { GetObjectW(bmp.into(), size_of::<BITMAP>() as i32, Some(&mut b as *mut _ as *mut c_void)) };
        let px = unsafe { std::slice::from_raw_parts(b.bmBits as *const [u8; 4], (b.bmWidthBytes / 4 * b.bmHeight.abs()) as usize) };
        assert!(px.iter().all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]), "no channel above alpha");
        delete(bmp);
    }

    #[test]
    fn cache_evicts_least_recently_used() {
        let _c = ComInit::sta();
        let mk = || unsafe { CreateBitmap(1, 1, 1, 32, None) };
        let mut cache = ImageCache::new(2);
        cache.insert("a".into(), mk());
        cache.insert("b".into(), mk());
        assert!(cache.get("a").is_some());
        cache.insert("c".into(), mk());
        assert_eq!(cache.len(), 2);
        assert!(cache.get("b").is_none());
        assert!(cache.get("a").is_some());
        assert!(cache.get("c").is_some());
    }
}
