//! List views (details, icons, thumbnails), the icon-view image list fed by the thumbs worker, and icon themes.
//! Icon and thumbnail images are rendered on the thumbs worker; the UI thread only copies finished bitmaps.

use super::*;
use std::collections::HashMap;

use rexplorer_core::icons::{self, IconTheme};
use rexplorer_core::images::ImageCache;
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, AlphaBlend, BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC,
    CreateDIBSection, DIB_RGB_COLORS, DeleteDC, GdiFlush, GetObjectW, HALFTONE, HBITMAP, SRCCOPY, SelectObject,
    SetStretchBltMode, StretchBlt,
};

pub(crate) const WM_THUMB_DONE: u32 = WM_APP + 7;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum View {
    Details,
    Icons,
    Thumbnails,
}

impl View {
    pub fn parse(s: &str) -> View {
        match s.trim() {
            "icons" => View::Icons,
            "thumbnails" => View::Thumbnails,
            _ => View::Details,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            View::Details => "details",
            View::Icons => "icons",
            View::Thumbnails => "thumbnails",
        }
    }

    pub fn next(self) -> View {
        match self {
            View::Details => View::Icons,
            View::Icons => View::Thumbnails,
            View::Thumbnails => View::Details,
        }
    }

    /// Image edge in 96-DPI pixels for the icon views.
    fn px(self) -> i32 {
        match self {
            View::Details => 0,
            View::Icons => 96,
            View::Thumbnails => 128,
        }
    }
}

const UNKNOWN: i32 = -1;
const REQUESTED: i32 = -2;
const LOOKAHEAD: i32 = 16;
// shortcut: past this many images the list is wiped and visible items re-added (cache hits); a ring of slots would avoid the flash
const MAX_IMAGES: i32 = 1500;

struct ThemeIcons {
    theme: IconTheme,
    list: HIMAGELIST,
    /// Image list index per theme bitmap (keyed by HBITMAP value).
    index: HashMap<isize, i32>,
}

pub(crate) struct Views {
    pub view: View,
    /// LVSIL_NORMAL list for the icon views; index 0 is the transparent placeholder.
    normal: HIMAGELIST,
    size: i32,
    /// Image index per item of the active listing, or UNKNOWN / REQUESTED.
    slots: Vec<i32>,
    /// Goes into the high bits of each request id, so deliveries for an older listing are recognised.
    token: usize,
    thumbs: Option<thumbs::Thumbs>,
    cache: ImageCache,
    icon_theme: Option<ThemeIcons>,
    /// The list small-icon indices refer to: the system image list, or the icon theme's.
    small: HIMAGELIST,
    /// What LVSIL_SMALL really holds: each small icon centered in a row-high cell, so details rows get the
    /// height of these images (28 px at 96 DPI) instead of the icons' 16 px.
    tall: HIMAGELIST,
    /// Small-icon index to `tall` index, filled on first use.
    tall_index: HashMap<i32, i32>,
}

impl Views {
    pub fn new(view: View) -> Views {
        Views {
            view,
            normal: HIMAGELIST::default(),
            size: 0,
            slots: Vec::new(),
            token: 0,
            thumbs: None,
            cache: ImageCache::new(512),
            icon_theme: None,
            small: HIMAGELIST::default(),
            tall: HIMAGELIST::default(),
            tall_index: HashMap::new(),
        }
    }
}

/// Details row height in 96-DPI pixels.
const ROW: i32 = 28;
/// What the list adds to its image height for a row, in pixels.
const ROW_PAD: i32 = 1;

/// Makes `src` the list small-icon indices refer to, shown through the row-high list.
pub(crate) unsafe fn set_small_source(app: *mut App, src: HIMAGELIST) {
    (*app).views.small = src;
    (*app).image_list_set = true;
    rebuild_tall(app);
}

/// (Re)creates the row-high list for the window's DPI; its images are made again as rows ask for them.
pub(crate) unsafe fn rebuild_tall(app: *mut App) {
    let a = &mut *app;
    if a.views.small.is_invalid() {
        return;
    }
    let (mut w, mut h) = (0, 0);
    let _ = ImageList_GetIconSize(a.views.small, Some(&mut w), Some(&mut h));
    let tall = ImageList_Create(w, (ROW * GetDpiForWindow(a.hwnd) as i32 / 96 - ROW_PAD).max(h), ILC_COLOR32, 64, 64);
    let old = std::mem::replace(&mut a.views.tall, tall);
    a.views.tall_index.clear();
    SendMessageW(a.list, LVM_SETIMAGELIST, Some(WPARAM(LVSIL_SMALL as usize)), Some(LPARAM(tall.0)));
    if !old.is_invalid() {
        let _ = ImageList_Destroy(Some(old));
    }
}

/// The row-high image showing small icon `index`.
pub(crate) unsafe fn tall_icon(app: *mut App, index: i32) -> i32 {
    let v = &mut (*app).views;
    if v.tall.is_invalid() || index < 0 {
        return index;
    }
    if let Some(&t) = v.tall_index.get(&index) {
        return t;
    }
    let (mut w, mut h, mut sw, mut sh) = (0, 0, 0, 0);
    let _ = ImageList_GetIconSize(v.tall, Some(&mut w), Some(&mut h));
    let _ = ImageList_GetIconSize(v.small, Some(&mut sw), Some(&mut sh));
    let Some((bmp, _)) = dib(w, h) else { return 0 };
    let dc = CreateCompatibleDC(None);
    let old = SelectObject(dc, bmp.into());
    let _ = ImageList_Draw(v.small, index, dc, 0, (h - sh) / 2, ILD_TRANSPARENT | ILD_PRESERVEALPHA);
    SelectObject(dc, old);
    let _ = DeleteDC(dc);
    let t = ImageList_Add(v.tall, bmp, None).max(0);
    let _ = DeleteObject(bmp.into());
    v.tall_index.insert(index, t);
    t
}

/// After the list exists: starts the worker, loads the icon theme and applies the saved view.
pub(crate) unsafe fn init(app: *mut App) {
    let a = &mut *app;
    a.views.thumbs = Some(thumbs::start(a.hwnd, WM_THUMB_DONE));
    if let Some(name) = a.theme.icon_theme.clone() {
        let dpi = GetDpiForWindow(a.hwnd) as i32;
        a.views.icon_theme = load_icon_theme(&settings::dir().join("icons").join(name), 16 * dpi / 96);
        if let Some(list) = a.views.icon_theme.as_ref().map(|t| t.list) {
            set_small_source(app, list);
        }
    }
    apply(app);
}

/// None when the folder is missing or has neither `folder.png` nor `file.png`: the system icons stay.
unsafe fn load_icon_theme(dir: &std::path::Path, size: i32) -> Option<ThemeIcons> {
    if !dir.is_dir() {
        return None;
    }
    let theme = icons::load_theme(dir, size).ok()?;
    if theme.bitmap_for("x", true).is_none() && theme.bitmap_for("x.\u{1}", false).is_none() {
        return None;
    }
    let list = ImageList_Create(size, size, ILC_COLOR32, 16, 16);
    (!list.is_invalid()).then(|| ThemeIcons { theme, list, index: HashMap::new() })
}

/// Small-icon index from the icon theme, or None when no theme is active.
pub(crate) unsafe fn theme_icon(app: *mut App, item: &ShellItem) -> Option<i32> {
    (*app).views.icon_theme.as_ref()?;
    theme_icon_for(app, &item.display_name(), item.is_folder())
}

/// Small-icon index from the icon theme for a name, or None when no theme is active.
pub(crate) unsafe fn theme_icon_for(app: *mut App, name: &str, folder: bool) -> Option<i32> {
    let t = (*app).views.icon_theme.as_mut()?;
    // A theme without folder.png still gives folders its file icon.
    let bmp = t.theme.bitmap_for(name, folder).or_else(|| t.theme.bitmap_for(name, false))?;
    let list = t.list;
    Some(*t.index.entry(bmp.0 as isize).or_insert_with(|| ImageList_Add(list, bmp, None).max(0)))
}

/// Switches the list to `view`, saving it to the session and the profile.
pub(crate) unsafe fn set_view(app: *mut App, view: View) {
    let a = &mut *app;
    if a.views.view == view {
        return;
    }
    a.views.view = view;
    a.session.view = view.name().to_string();
    a.profile.view = view.name().to_string();
    save_profile_key(&a.profile.name, "view", view.name());
    apply(app);
    nav::session_changed(app);
}

/// Puts the current view into the ListView (also after a DPI change).
pub(crate) unsafe fn apply(app: *mut App) {
    let a = &mut *app;
    let list = a.list;
    let view = a.views.view;
    toolbar::set_checked(a.tools, CMD_VIEW1, view == View::Details);
    toolbar::set_checked(a.tools, CMD_VIEW_GRID, view != View::Details);
    if view == View::Details {
        SendMessageW(list, LVM_SETVIEW, Some(WPARAM(LV_VIEW_DETAILS as usize)), None);
    } else {
        let dpi = GetDpiForWindow(a.hwnd) as i32;
        let size = view.px() * dpi / 96;
        if size != a.views.size || a.views.normal.is_invalid() {
            let old = a.views.normal;
            a.views.normal = ImageList_Create(size, size, ILC_COLOR32, 64, 64);
            a.views.size = size;
            SendMessageW(list, LVM_SETIMAGELIST, Some(WPARAM(LVSIL_NORMAL as usize)), Some(LPARAM(a.views.normal.0)));
            if !old.is_invalid() {
                let _ = ImageList_Destroy(Some(old));
            }
        }
        // Distance between tile origins: image plus room for two lines of label.
        let (cx, cy) = (size + 40 * dpi / 96, size + 44 * dpi / 96);
        SendMessageW(list, LVM_SETICONSPACING, Some(WPARAM(0)), Some(LPARAM(((cy as isize) << 16) | cx as isize)));
        SendMessageW(list, LVM_SETVIEW, Some(WPARAM(LV_VIEW_ICON as usize)), None);
    }
    let count = nav::source(app).map_or(0, |s| s.len());
    reset(app, count);
    let _ = InvalidateRect(Some(list), None, true);
}

/// A new listing (navigation, refresh, tab switch) or a new view: every item goes back to the placeholder.
pub(crate) unsafe fn reset(app: *mut App, count: usize) {
    let v = &mut (*app).views;
    v.token = (v.token + 1) & 0x7FFF_FFFF;
    if let Some(t) = &v.thumbs {
        t.invalidate();
    }
    v.slots = vec![UNKNOWN; count];
    if !v.normal.is_invalid() {
        let _ = ImageList_Remove(v.normal, -1);
        if let Some((bmp, bits)) = dib(v.size, v.size) {
            // An image whose alpha is all zero is drawn opaque (black); one barely-there pixel keeps it transparent.
            *bits.add(3) = 1;
            ImageList_Add(v.normal, bmp, None);
            let _ = DeleteObject(bmp.into());
        }
    }
}

/// LVN_GETDISPINFO in an icon view: the item's image, requesting it when unknown.
pub(crate) unsafe fn image(app: *mut App, i: usize) -> i32 {
    match (&(*app).views.slots).get(i) {
        Some(&UNKNOWN) => request(app, i),
        Some(&s) => s.max(0),
        None => 0,
    }
}

/// LVN_ODCACHEHINT: request the rows about to be drawn plus a lookahead.
pub(crate) unsafe fn on_cache_hint(app: *mut App, from: i32, to: i32) {
    let v = &mut (*app).views;
    if v.view == View::Details || v.normal.is_invalid() {
        return;
    }
    if ImageList_GetImageCount(v.normal) >= MAX_IMAGES {
        let n = v.slots.len();
        reset(app, n);
        let _ = InvalidateRect(Some((*app).list), None, false);
    }
    let count = (*app).views.slots.len();
    for i in thumbs::visible_range(from, to - from + 1, count, LOOKAHEAD) {
        if (&(*app).views.slots)[i] == UNKNOWN {
            request(app, i);
        }
    }
}

fn cache_key(view: View, size: i32, parsing: &str) -> String {
    format!("{}:{size}:{parsing}", view.name())
}

/// From the cache when possible (image index), else queued on the worker (placeholder until it lands).
unsafe fn request(app: *mut App, i: usize) -> i32 {
    let Some(l) = listing(app) else { return 0 };
    let Some(item) = l.items.get(i) else { return 0 };
    let v = &mut (*app).views;
    let key = cache_key(v.view, v.size, &item.parsing_name());
    if let Some(bmp) = v.cache.get(&key) {
        let index = ImageList_Add(v.normal, bmp, None).max(0);
        v.slots[i] = index;
        return index;
    }
    let Some(t) = &v.thumbs else { return 0 };
    t.request((v.token << 32) | i, l.folder.child_pidl(item), v.size, v.view == View::Icons);
    v.slots[i] = REQUESTED;
    0
}

/// WM_THUMB_DONE: copy the bitmap into the image list, or drop it when it belongs to an older listing.
pub(crate) unsafe fn on_thumb_done(app: *mut App, wparam: WPARAM, lparam: LPARAM) {
    let d = thumbs::done(wparam, lparam);
    let (token, i) = (d.id >> 32, d.id & 0xFFFF_FFFF);
    let v = &mut (*app).views;
    let live = v.thumbs.is_some() && token == v.token && v.view != View::Details && v.slots.get(i) == Some(&REQUESTED);
    let fitted = if live { fit(d.bitmap, v.size) } else { None };
    let _ = DeleteObject(d.bitmap.into());
    if !live {
        return;
    }
    let Some(bmp) = fitted else {
        v.slots[i] = 0; // placeholder for good, so it is not requested again
        return;
    };
    let index = ImageList_Add(v.normal, bmp, None).max(0);
    v.slots[i] = index;
    let parsing = listing(app).and_then(|l| l.items.get(i)).map(|it| it.parsing_name());
    let v = &mut (*app).views;
    match parsing {
        Some(p) => v.cache.insert(cache_key(v.view, v.size, &p), bmp),
        None => {
            let _ = DeleteObject(bmp.into());
        }
    }
    SendMessageW((*app).list, LVM_REDRAWITEMS, Some(WPARAM(i)), Some(LPARAM(i as isize)));
}

/// WM_DESTROY: stop the worker, delete bitmaps already queued for us, free the image lists.
pub(crate) unsafe fn destroy(app: *mut App) {
    let a = &mut *app;
    a.views.thumbs = None;
    let mut msg = MSG::default();
    while PeekMessageW(&mut msg, Some(a.hwnd), WM_THUMB_DONE, WM_THUMB_DONE, PM_REMOVE).as_bool() {
        let _ = DeleteObject(thumbs::done(msg.wParam, msg.lParam).bitmap.into());
    }
    if !a.views.normal.is_invalid() {
        let _ = ImageList_Destroy(Some(a.views.normal));
        a.views.normal = HIMAGELIST::default();
    }
    if let Some(t) = a.views.icon_theme.take() {
        let _ = ImageList_Destroy(Some(t.list));
    }
    if !a.views.tall.is_invalid() {
        let _ = ImageList_Destroy(Some(a.views.tall));
        a.views.tall = HIMAGELIST::default();
    }
}

/// A zeroed (fully transparent) top-down 32-bit DIB and its pixels.
unsafe fn dib(w: i32, h: i32) -> Option<(HBITMAP, *mut u8)> {
    if w <= 0 || h <= 0 {
        return None;
    }
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
    let bmp = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
    std::ptr::write_bytes(bits as *mut u8, 0, (w * h * 4) as usize);
    Some((bmp, bits as *mut u8))
}

/// Shrinks `src` to fit a `size` square (never enlarges) and centers it: image lists need exact-size images.
unsafe fn fit(src: HBITMAP, size: i32) -> Option<HBITMAP> {
    let mut bm = BITMAP::default();
    if GetObjectW(src.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void)) == 0 {
        return None;
    }
    let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
    if w <= 0 || h <= 0 {
        return None;
    }
    let (dst, bits) = dib(size, size)?;
    let scale = (size as f64 / w.max(h) as f64).min(1.0);
    let (dw, dh) = (((w as f64 * scale).round() as i32).max(1), ((h as f64 * scale).round() as i32).max(1));
    let (x, y) = ((size - dw) / 2, (size - dh) / 2);
    let dc = CreateCompatibleDC(None);
    let sdc = CreateCompatibleDC(None);
    let old_d = SelectObject(dc, dst.into());
    let old_s = SelectObject(sdc, src.into());
    let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
    let blended = bm.bmBitsPixel == 32 && AlphaBlend(dc, x, y, dw, dh, sdc, 0, 0, w, h, bf).as_bool();
    let _ = GdiFlush();
    let px = std::slice::from_raw_parts_mut(bits, (size * size * 4) as usize);
    // A source without alpha (all zero) blends to nothing: copy it opaque instead.
    if !blended || px.chunks_exact(4).all(|p| p[3] == 0) {
        SetStretchBltMode(dc, HALFTONE);
        let _ = StretchBlt(dc, x, y, dw, dh, Some(sdc), 0, 0, w, h, SRCCOPY);
        let _ = GdiFlush();
        for row in y..y + dh {
            for col in x..x + dw {
                px[((row * size + col) * 4 + 3) as usize] = 255;
            }
        }
    }
    SelectObject(sdc, old_s);
    SelectObject(dc, old_d);
    let _ = DeleteDC(sdc);
    let _ = DeleteDC(dc);
    Some(dst)
}

// ---- persistence and the starter icon theme ------------------------------------------------------

/// Sets `key=value` in `[section]` of ini text, keeping every other line (comments included).
fn set_ini_key(text: &str, section: &str, key: &str, value: &str) -> String {
    let header = |l: &str| l.trim().strip_prefix('[').and_then(|l| l.strip_suffix(']')).map(|n| n.trim().to_string());
    let is_section = |n: &str| match (n.strip_prefix("profile "), section.strip_prefix("profile ")) {
        (Some(a), Some(b)) => a.trim() == b.trim(),
        _ => n == section,
    };
    let mut out: Vec<String> = Vec::new();
    let (mut inside, mut done) = (false, false);
    for line in text.lines() {
        if let Some(name) = header(line) {
            inside = is_section(&name);
            out.push(line.to_string());
            if inside && !done {
                out.push(format!("{key}={value}"));
                done = true;
            }
            continue;
        }
        if inside && line.split_once('=').is_some_and(|(k, _)| k.trim() == key) {
            continue; // replaced by the line written under the header
        }
        out.push(line.to_string());
    }
    if !done {
        out.push(format!("[{section}]"));
        out.push(format!("{key}={value}"));
    }
    out.join("\n") + "\n"
}

unsafe fn save_profile_key(profile: &str, key: &str, value: &str) {
    let path = settings::dir().join("settings.ini");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = std::fs::write(&path, set_ini_key(&text, &format!("profile {profile}"), key, value));
}

const STARTER: [&str; 8] = ["folder", "file", "rs", "txt", "png", "zip", "rar", "pdf"];

/// Writes `icons\default\*.png` once (when the folder does not exist yet) so users see how themes work.
pub(crate) fn ensure_starter_theme(t: &Theme) {
    let dir = settings::dir().join("icons").join("default");
    if dir.exists() || std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    for name in STARTER {
        let _ = icons::save_png(&starter_icon(name, t), 32, 32, &dir.join(format!("{name}.png")));
    }
}

/// 32x32 straight-alpha BGRA: a folder silhouette, or a page in a per-type hue tinted toward the accent.
fn starter_icon(name: &str, t: &Theme) -> Vec<u8> {
    let tint = |c: u32| theme::blend(c, t.accent, 0.35);
    let color = match name {
        "folder" => t.accent,
        "file" => theme::blend(t.secondary_text, t.accent, 0.2),
        "rs" => tint(0x0010_6ADE),
        "txt" => tint(0x00A0_A0A0),
        "png" => tint(0x0060_B040),
        "zip" | "rar" => tint(0x0020_B0E0),
        _ => tint(0x0030_30D0),
    };
    // (left, top, right, bottom, radius) rounded rects in pixel units.
    let shapes: &[(f32, f32, f32, f32, f32)] = if name == "folder" {
        &[(2.0, 5.0, 14.0, 11.0, 2.0), (2.0, 8.0, 30.0, 28.0, 3.0)]
    } else {
        &[(6.0, 2.0, 26.0, 30.0, 3.0)]
    };
    let inside = |x: f32, y: f32| {
        shapes.iter().any(|&(l, tp, r, b, rad)| {
            let dx = (l + rad - x).max(x - (r - rad)).max(0.0);
            let dy = (tp + rad - y).max(y - (b - rad)).max(0.0);
            x >= l && x <= r && y >= tp && y <= b && dx * dx + dy * dy <= rad * rad
        })
    };
    let mut px = Vec::with_capacity(32 * 32 * 4);
    for y in 0..32 {
        for x in 0..32 {
            // 4x4 supersampling for smooth corners.
            let hits = (0..16).filter(|s| inside(x as f32 + (s % 4) as f32 / 4.0 + 0.125, y as f32 + (s / 4) as f32 / 4.0 + 0.125)).count();
            px.extend_from_slice(&[(color >> 16) as u8, (color >> 8) as u8, color as u8, (hits * 255 / 16) as u8]);
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_names_round_trip() {
        for v in [View::Details, View::Icons, View::Thumbnails] {
            assert_eq!(View::parse(v.name()), v);
        }
        assert_eq!(View::parse("bogus"), View::Details);
        assert_eq!(View::Thumbnails.next(), View::Details);
    }

    #[test]
    fn ini_key_replaced_inserted_and_appended() {
        let text = "; mine\n[general]\nprofile=default\n\n[profile default]\nstart=x\nview=details\n\n[profile work]\nview=icons\n";
        let out = set_ini_key(text, "profile default", "view", "thumbnails");
        assert!(out.contains("; mine"));
        assert!(out.contains("[profile default]\nview=thumbnails\nstart=x\n"));
        assert!(!out.contains("view=details"));
        assert!(out.contains("[profile work]\nview=icons"));
        let parsed = rexplorer_core::settings::parse_ini(&out);
        assert_eq!(parsed.iter().filter(|(n, _)| n == "profile default").count(), 1);
        let out = set_ini_key("[general]\nprofile=p\n", "profile p", "view", "icons");
        assert!(out.ends_with("[profile p]\nview=icons\n"));
    }

    #[test]
    fn starter_icons_have_shape() {
        let t = rexplorer_core::settings::Theme::dark();
        let px = starter_icon("folder", &t);
        assert_eq!(px.len(), 32 * 32 * 4);
        assert_eq!(px[3], 0, "corner is transparent");
        assert_eq!(px[(20 * 32 + 16) * 4 + 3], 255, "body is opaque");
        assert_eq!(&px[(20 * 32 + 16) * 4..][..3], &[(t.accent >> 16) as u8, (t.accent >> 8) as u8, t.accent as u8]);
    }
}
