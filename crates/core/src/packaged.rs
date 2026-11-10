//! Commands that MSIX and sparse packages add to Explorer's context menu (manifest extension
//! windows.fileExplorerContextMenus). They are IExplorerCommand COM handlers. The shell's own IContextMenu lists
//! some of them (Terminal and Code on a folder background, Code and Notepad on files) but not others (folder items),
//! so the menu code asks for those itself, skipping what the shell already listed (`to_query`). Nothing here
//! invokes a command except `invoke`.
use std::ffi::c_void;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::sync::{Mutex, Once, OnceLock};
use std::time::{Duration, Instant};

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP,
    SelectObject,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, CoCreateInstance};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    ExtractIconExW, IExplorerCommand, IShellItemArray, SHCreateShellItemArrayFromIDLists, SHParseDisplayName,
};
use windows::Win32::UI::WindowsAndMessaging::{DI_NORMAL, DestroyIcon, DrawIconEx, GetSystemMetrics, HICON, SM_CXSMICON};
use windows::core::{GUID, HSTRING, PWSTR, Result};

use crate::com::{ComInit, take_cotaskmem_string};

/// The late handlers are skipped after this long: cold activation is 220 to 350 ms each, but they run side by side.
const DEADLINE: Duration = Duration::from_millis(350);

/// One handler a package registers and the item types it is registered for (lowercase: "*", ".py", "directory",
/// "directory\\background").
#[derive(Debug, PartialEq)]
pub struct Registration {
    pub clsid: GUID,
    pub types: Vec<String>,
}

/// What a menu is built for.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// A file; its extension lowercase with the dot ("" when it has none).
    File(String),
    Folder,
    Background,
}

impl Registration {
    /// shortcut: "*" never matches a folder (Code and Notepad list `Directory` next to it); read as inferred from
    /// manifests, not from documentation. Upgrade if a handler turns up that lists only "*" and shows on folders.
    pub fn matches(&self, t: &Target) -> bool {
        self.types.iter().any(|ty| match t {
            Target::File(ext) => ty == "*" || ty == ext,
            Target::Folder => ty == "directory",
            Target::Background => ty == "directory\\background",
        })
    }
}

/// A command ready to show.
pub struct Command {
    pub clsid: GUID,
    /// Localized, with its & accelerator.
    pub title: String,
    pub icon: Option<HBITMAP>,
    /// Activation plus state, title and icon, on its worker.
    pub took: Duration,
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(i) = tag[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        if !tag[..at].ends_with(|c: char| c.is_ascii_whitespace()) {
            continue;
        }
        let Some(rest) = tag[from..].trim_start().strip_prefix('=') else { continue };
        let rest = rest.trim_start();
        let q = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let val = &rest[1..];
        return val.find(q).map(|e| &val[..e]);
    }
    None
}

/// The handlers of one AppxManifest.xml: each Verb's Clsid gets the Type of the ItemType around it. Any namespace
/// prefix works (Notepad mixes desktop4 and desktop5 in one element). Merged into `out` by CLSID.
/// shortcut: a tag scan, not an XML parser; comments and CDATA are not skipped. Upgrade if a manifest breaks it.
pub fn parse_manifest(xml: &str, out: &mut Vec<Registration>) {
    let mut ty: Option<String> = None;
    for tag in xml.split('<').skip(1) {
        let tag = tag.split('>').next().unwrap_or("");
        let closing = tag.starts_with('/');
        let name = tag.trim_start_matches('/').split(|c: char| c.is_ascii_whitespace() || c == '/').next().unwrap_or("");
        let local = name.rsplit(':').next().unwrap_or("").to_ascii_lowercase();
        match local.as_str() {
            "itemtype" if !closing => {
                ty = attr(tag, "Type").map(|t| t.to_ascii_lowercase());
                if tag.ends_with('/') {
                    ty = None;
                }
            }
            "itemtype" => ty = None,
            "verb" if !closing => {
                let Some(ty) = &ty else { continue };
                let Some(Ok(clsid)) = attr(tag, "Clsid").map(|c| GUID::try_from(c.trim_matches(|c| c == '{' || c == '}'))) else { continue };
                match out.iter_mut().find(|r| r.clsid == clsid) {
                    Some(r) if r.types.contains(ty) => {}
                    Some(r) => r.types.push(ty.clone()),
                    None => out.push(Registration { clsid, types: vec![ty.clone()] }),
                }
            }
            _ => {}
        }
    }
}

fn reg_string(key: &str, value: &str) -> Option<String> {
    let (k, v) = (HSTRING::from(key), HSTRING::from(value));
    let mut cb = 0u32;
    unsafe { RegGetValueW(HKEY_CURRENT_USER, &k, &v, RRF_RT_REG_SZ, None, None, Some(&mut cb)) }.ok().ok()?;
    let mut buf = vec![0u16; cb as usize / 2 + 1];
    unsafe { RegGetValueW(HKEY_CURRENT_USER, &k, &v, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut cb)) }.ok().ok()?;
    Some(String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(buf.len())]))
}

static REGISTRATIONS: OnceLock<Vec<Registration>> = OnceLock::new();

/// Every handler the user's packages register, scanned once (hundreds of manifests: keep the first call off the UI
/// thread, see `warm`). shortcut: the registry has no index of these verbs, so the manifests are read; packages
/// installed later show up after a restart.
pub fn registrations() -> &'static [Registration] {
    REGISTRATIONS.get_or_init(|| {
        let root = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages";
        let mut out = Vec::new();
        let mut key = HKEY::default();
        if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(root), None, KEY_READ, &mut key) }.is_err() {
            return out;
        }
        for index in 0.. {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            if unsafe { RegEnumKeyExW(key, index, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) }.is_err() {
                break;
            }
            let full = String::from_utf16_lossy(&name[..len as usize]);
            let Some(dir) = reg_string(&format!(r"{root}\{full}"), "PackageRootFolder") else { continue };
            // A sparse package (VS Code) keeps its manifest in the staged copy, not in its external root folder.
            let staged = std::path::Path::new(&std::env::var("ProgramFiles").unwrap_or_default()).join("WindowsApps").join(&full);
            let Some(xml) = [std::path::PathBuf::from(&dir), staged].iter().find_map(|d| std::fs::read_to_string(d.join("AppxManifest.xml")).ok()) else { continue };
            if xml.to_ascii_lowercase().contains("fileexplorercontextmenus") {
                parse_manifest(&xml, &mut out);
            }
        }
        unsafe {
            let _ = RegCloseKey(key);
        }
        out
    })
}

/// The registrations when the scan has finished, else None (never blocks).
pub fn ready() -> Option<&'static [Registration]> {
    REGISTRATIONS.get().map(|v| &v[..])
}

/// Starts the scan on a thread, once; call at startup so the first menu already has the list.
pub fn warm() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        std::thread::spawn(|| {
            registrations();
        });
    });
}

/// The selection as the handlers want it. Paths must exist on the file system.
pub(crate) fn array_of(paths: &[String]) -> Result<IShellItemArray> {
    struct Abs(*mut ITEMIDLIST);
    impl Drop for Abs {
        fn drop(&mut self) {
            unsafe { windows::Win32::UI::Shell::ILFree(Some(self.0)) };
        }
    }
    let mut pidls = Vec::new();
    for p in paths {
        let mut abs = std::ptr::null_mut();
        unsafe { SHParseDisplayName(&HSTRING::from(p.as_str()), None, &mut abs, 0, None)? };
        pidls.push(Abs(abs));
    }
    let raw: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| p.0 as *const _).collect();
    unsafe { SHCreateShellItemArrayFromIDLists(&raw) }
}

fn create(clsid: &GUID) -> Result<IExplorerCommand> {
    unsafe { CoCreateInstance(clsid, None, CLSCTX_LOCAL_SERVER | CLSCTX_INPROC_SERVER) }
}

/// A 16 px (at 96 DPI) premultiplied ARGB bitmap of the icon a handler names ("path,index" or "path"; a negative
/// index is a resource id). The caller deletes it. Menu icons use the small size at the system DPI.
fn icon_bitmap(spec: &str) -> Option<HBITMAP> {
    let spec = spec.trim().trim_matches('"');
    let (path, index) = match spec.rsplit_once(',').and_then(|(p, i)| Some((p, i.trim().parse::<i32>().ok()?))) {
        Some(pi) => pi,
        None => (spec, 0),
    };
    let mut icon = HICON::default();
    if unsafe { ExtractIconExW(&HSTRING::from(path), index, None, Some(&mut icon), 1) } == 0 || icon.is_invalid() {
        return None;
    }
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    unsafe {
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok();
        if let Some(bmp) = bmp {
            let dc = CreateCompatibleDC(None);
            let old = SelectObject(dc, bmp.into());
            let _ = DrawIconEx(dc, 0, 0, icon, size, size, 0, None, DI_NORMAL);
            SelectObject(dc, old);
            let _ = DeleteDC(dc);
        }
        let _ = DestroyIcon(icon);
        bmp
    }
}

/// A `Command` with the icon as a plain integer, so it can cross threads (HBITMAP is a pointer).
type Raw = (GUID, String, Bmp, Duration);

/// A bitmap handle that is deleted on drop unless `release` took it: a result nobody accepts frees its icon.
struct Bmp(isize);

impl Bmp {
    fn release(&mut self) -> Option<HBITMAP> {
        (std::mem::take(&mut self.0) != 0).then_some(HBITMAP(self.0 as *mut c_void))
    }
}

impl Drop for Bmp {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                let _ = DeleteObject(HBITMAP(self.0 as *mut c_void).into());
            }
        }
    }
}

/// One handler's answer for the selection; None when it is hidden for it, has no title or has subcommands.
fn ask(clsid: GUID, paths: &[String]) -> Option<Raw> {
    let t0 = Instant::now();
    let cmd = create(&clsid).ok()?;
    let array = array_of(paths).ok()?;
    // 2 = ECS_HIDDEN
    if unsafe { cmd.GetState(&array, true) }.ok()? == 2 {
        return None;
    }
    // 1 = ECF_HASSUBCOMMANDS: a submenu would need its own rendering.
    if unsafe { cmd.GetFlags() }.ok()? & 1 != 0 {
        return None;
    }
    let title = unsafe { take_cotaskmem_string(cmd.GetTitle(&array).ok()?) };
    if title.is_empty() {
        return None;
    }
    let icon = unsafe { cmd.GetIcon(&array).map(|p| take_cotaskmem_string(p)) }.ok().and_then(|s| icon_bitmap(&s));
    Some((clsid, title, Bmp(icon.map_or(0, |b| b.0 as isize)), t0.elapsed()))
}

/// The registrations the shell has not already put in the menu: its items' verbs are the handler CLSIDs in
/// `{GUID}` form (compared case-insensitively).
pub fn to_query<'a>(regs: &'a [Registration], shell_verbs: &[String]) -> Vec<&'a Registration> {
    regs.iter().filter(|r| !shell_verbs.iter().any(|v| v.eq_ignore_ascii_case(&format!("{{{:?}}}", r.clsid)))).collect()
}

/// Handlers still being asked by an earlier menu: a hung one must not pile up a thread per menu.
static IN_FLIGHT: Mutex<Vec<u128>> = Mutex::new(Vec::new());

/// The commands of `regs` that match every target and are visible for `paths`, in registration order. Each handler
/// is asked on its own thread (own MTA) so the cold activations overlap; those not back within the deadline are
/// skipped. `targets` and `paths` describe the same selection.
pub fn query(regs: &[&Registration], targets: &[Target], paths: &[String]) -> Vec<Command> {
    // A package's handler can live in a folder the user can write: never load one into an elevated rexplorer.
    if crate::elevation::is_elevated() {
        return Vec::new();
    }
    let (tx, rx) = channel::<(usize, Option<Raw>)>();
    let mut pending = 0;
    for (i, r) in regs.iter().enumerate().filter(|(_, r)| targets.iter().all(|t| r.matches(t))) {
        let key = r.clsid.to_u128();
        {
            let mut busy = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
            if busy.contains(&key) {
                continue;
            }
            busy.push(key);
        }
        let (tx, paths, clsid) = (tx.clone(), paths.to_vec(), r.clsid);
        pending += 1;
        std::thread::spawn(move || {
            let _com = ComInit::mta();
            let answer = ask(clsid, &paths);
            IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner()).retain(|k| *k != key);
            // Sent too late, the answer drops with the channel and its bitmap with it.
            let _ = tx.send((i, answer));
        });
    }
    drop(tx);
    let end = Instant::now() + DEADLINE;
    let mut got = Vec::new();
    while got.len() < pending {
        match rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            Ok((i, Some((clsid, title, mut bmp, took)))) => got.push((i, Command { clsid, title, icon: bmp.release(), took })),
            Ok((_, None)) => pending -= 1,
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
        }
    }
    got.sort_by_key(|(i, _)| *i);
    got.into_iter().map(|(_, c)| c).collect()
}

/// Runs a command on the selection. Called on the thread that shows the menu, with a fresh instance (nothing
/// crosses apartments).
pub fn invoke(clsid: &GUID, paths: &[String]) -> Result<()> {
    let cmd = create(clsid)?;
    let array = array_of(paths)?;
    unsafe { cmd.Invoke(&array, None) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTEPAD: &str = r#"<Package xmlns:desktop4="a" xmlns:desktop5="b"><Applications><Application>
    <Extensions><desktop4:Extension Category="windows.fileExplorerContextMenus">
      <desktop4:FileExplorerContextMenus>
        <desktop4:ItemType Type="*">
          <desktop4:Verb Id="OpenInNotepad" Clsid="CA6CC9F1-867A-481E-951E-A28C5E4F01EA" />
        </desktop4:ItemType>
        <desktop5:ItemType Type="Directory">
          <desktop5:Verb Id="OpenInNotepad" Clsid="{ca6cc9f1-867a-481e-951e-a28c5e4f01ea}" />
        </desktop5:ItemType>
      </desktop4:FileExplorerContextMenus>
    </desktop4:Extension></Extensions></Application></Applications></Package>"#;

    fn parse(xml: &str) -> Vec<Registration> {
        let mut v = Vec::new();
        parse_manifest(xml, &mut v);
        v
    }

    #[test]
    fn mixed_namespaces_dedupe_by_clsid() {
        let v = parse(NOTEPAD);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].clsid, GUID::try_from("CA6CC9F1-867A-481E-951E-A28C5E4F01EA").unwrap());
        assert_eq!(v[0].types, ["*", "directory"]);
    }

    #[test]
    fn background_and_extension_types_lowercase() {
        let xml = r#"<desktop4:FileExplorerContextMenus><desktop4:ItemType Type="Directory\Background">
            <desktop4:Verb Id="a" Clsid="9f156763-7844-4dc4-b2b1-901f640f5155"/></desktop4:ItemType>
            <desktop4:ItemType Type=".PY"><desktop4:Verb Id="b" Clsid="C7E29CB0-9691-4DE8-B72B-6719DDC0B4A1"/>
            </desktop4:ItemType></desktop4:FileExplorerContextMenus>"#;
        let v = parse(xml);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].types, ["directory\\background"]);
        assert_eq!(v[1].types, [".py"]);
    }

    #[test]
    fn manifest_without_context_menus_is_empty() {
        assert!(parse(r#"<Package><Applications><Application Id="x"/></Applications></Package>"#).is_empty());
    }

    #[test]
    fn targets_match_their_types() {
        let reg = |t: &[&str]| Registration { clsid: GUID::zeroed(), types: t.iter().map(|s| s.to_string()).collect() };
        let star = reg(&["*"]);
        assert!(star.matches(&Target::File(".md".into())) && star.matches(&Target::File(String::new())));
        assert!(!star.matches(&Target::Folder) && !star.matches(&Target::Background));
        let dir = reg(&["directory", "directory\\background"]);
        assert!(dir.matches(&Target::Folder) && dir.matches(&Target::Background) && !dir.matches(&Target::File(".md".into())));
        let py = reg(&[".py"]);
        assert!(py.matches(&Target::File(".py".into())) && !py.matches(&Target::File(".pyw".into())));
        assert!(!reg(&["directory"]).matches(&Target::Background));
    }

    #[test]
    fn closing_tag_ends_the_type_and_equals_may_have_spaces() {
        let xml = r#"<a:ItemType Type = "x"><a:Verb Clsid = "9f156763-7844-4dc4-b2b1-901f640f5155"/></a:ItemType>
            <a:Verb Clsid="C7E29CB0-9691-4DE8-B72B-6719DDC0B4A1"/>"#;
        let v = parse(xml);
        assert_eq!(v.len(), 1, "a Verb after </ItemType> has no type");
        assert_eq!(v[0].types, ["x"]);
    }

    #[test]
    fn shell_listed_handlers_are_not_asked_again() {
        let reg = |c: &str| Registration { clsid: GUID::try_from(c).unwrap(), types: vec!["directory".into()] };
        let regs = [reg("9F156763-7844-4DC4-B2B1-901F640F5155"), reg("D28D30E7-17F3-41D9-93F0-9BAE6DC4884B")];
        let left = to_query(&regs, &["open".into(), "{9f156763-7844-4dc4-b2b1-901f640f5155}".into()]);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].clsid, regs[1].clsid);
        assert!(to_query(&regs, &["{9F156763-7844-4DC4-B2B1-901F640F5155}".into(), "{D28D30E7-17F3-41D9-93F0-9BAE6DC4884B}".into()]).is_empty());
    }
}
