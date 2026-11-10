//! Read-only dump of the shell context menu rexplorer builds (GetUIObjectOf / CreateViewObject, then
//! QueryContextMenu with CMF_EXPLORE | CMF_CANRENAME): every top-level entry with its canonical verb.
//! Usage: cargo run --release -p rexplorer-core --example menu_probe -- <folder path>
use rexplorer_core::com::ComInit;
use rexplorer_core::folder::ShellFolder;
use rexplorer_core::packaged::{self, Target};
use windows::Win32::UI::Shell::{GCS_VERBW, IContextMenu};
use windows::Win32::UI::WindowsAndMessaging::{CreatePopupMenu, DestroyMenu, GetMenuItemCount, GetMenuItemID, GetMenuStringW, MF_BYPOSITION};
use windows::Win32::Foundation::HWND;
use windows::core::PSTR;

/// Prints the menu's top-level entries and returns their verbs.
fn dump(label: &str, menu: &IContextMenu) -> Vec<String> {
    let mut verbs = Vec::new();
    unsafe {
        let h = CreatePopupMenu().expect("menu");
        menu.QueryContextMenu(h, 0, 1, 0x7FFF, flags()).ok().expect("QueryContextMenu");
        println!("== {label}");
        for i in 0..GetMenuItemCount(Some(h)) {
            let mut text = [0u16; 256];
            let n = GetMenuStringW(h, i as u32, Some(&mut text), MF_BYPOSITION);
            let id = GetMenuItemID(h, i);
            let mut verb = [0u16; 64];
            let v = if id != u32::MAX && id >= 1 && menu.GetCommandString(id as usize - 1, GCS_VERBW, None, PSTR(verb.as_mut_ptr() as *mut u8), 64).is_ok() {
                String::from_utf16_lossy(&verb[..verb.iter().position(|&c| c == 0).unwrap_or(64)])
            } else {
                String::new()
            };
            let t = String::from_utf16_lossy(&text[..n.max(0) as usize]);
            verbs.push(v.clone());
            println!("  {:>3} {:<45} verb={v}", i, if t.is_empty() { "(separator or submenu)".into() } else { t });
        }
        let _ = DestroyMenu(h);
    }
    verbs
}

fn main() {
    let path = std::env::args().nth(1).expect("folder path");
    let _c = ComInit::sta();
    if path == "--cmd" {
        let a: Vec<String> = std::env::args().collect();
        return probe_command(&a[2], &a[3]);
    }
    let parent = ShellFolder::parse(std::path::Path::new(&path).parent().expect("parent").to_str().unwrap()).expect("parent folder");
    let item = parent.enumerate().expect("enumerate").into_iter().find(|i| i.parsing_name().eq_ignore_ascii_case(&path)).expect("item in parent");
    let menu: IContextMenu = unsafe { parent.folder.GetUIObjectOf(HWND::default(), &[item.pidl.as_ptr()], None) }.expect("item menu");
    let item_verbs = dump(&format!("item menu for {path}"), &menu);
    let mut bg_verbs = Vec::new();
    if std::path::Path::new(&path).is_dir() {
        let folder = ShellFolder::parse(&path).expect("folder");
        let bg: IContextMenu = unsafe { folder.folder.CreateViewObject(HWND::default()) }.expect("background menu");
        bg_verbs = dump(&format!("background menu of {path}"), &bg);
    }
    let t = std::time::Instant::now();
    let regs = packaged::registrations();
    println!("== packaged: {} registrations scanned in {:?}", regs.len(), t.elapsed());
    for r in regs {
        println!("  {:?} {:?}", r.clsid, r.types);
    }
    let p = std::path::Path::new(&path);
    let target = if p.is_dir() { Target::Folder } else { Target::File(p.extension().map_or(String::new(), |e| format!(".{}", e.to_string_lossy().to_lowercase()))) };
    packaged_dump("as item", &packaged::to_query(regs, &item_verbs), &[target], &path);
    if p.is_dir() {
        packaged_dump("as background", &packaged::to_query(regs, &bg_verbs), &[Target::Background], &path);
    }
}

/// What `ContextMenu::show_in` adds: the same query, timed as a whole and per command.
fn packaged_dump(label: &str, regs: &[&packaged::Registration], targets: &[Target], path: &str) {
    let t = std::time::Instant::now();
    let cmds = packaged::query(regs, targets, &[path.to_string()]);
    println!("  {label}: {} commands in {:?}", cmds.len(), t.elapsed());
    for c in cmds {
        println!("    {:<30} {} took={:?} icon={}", c.title, format!("{:?}", c.clsid), c.took, c.icon.is_some());
    }
}

/// QueryContextMenu flags: the optional second argument (hex), else rexplorer's CMF_EXPLORE | CMF_CANRENAME.
fn flags() -> u32 {
    std::env::args().nth(2).and_then(|f| u32::from_str_radix(f.trim_start_matches("0x"), 16).ok()).unwrap_or(0x4 | 0x10)
}

/// `--cmd {clsid} <path>`: creates a packaged File Explorer command handler and asks it, read-only, for its title,
/// state and flags on `path` (nothing is invoked).
#[allow(dead_code)]
pub fn probe_command(clsid: &str, path: &str) {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, CoCreateInstance, CoTaskMemFree};
    use windows::Win32::UI::Shell::{IExplorerCommand, IShellItem, IShellItemArray, SHCreateItemFromParsingName, SHCreateShellItemArrayFromShellItem};
    use windows::core::{GUID, HSTRING};
    unsafe {
        let id = GUID::try_from(clsid.trim_matches(|c| c == '{' || c == '}')).expect("clsid");
        let t0 = std::time::Instant::now();
        let cmd: IExplorerCommand = match CoCreateInstance(&id, None, CLSCTX_LOCAL_SERVER | CLSCTX_INPROC_SERVER) {
            Ok(c) => c,
            Err(e) => return println!("{clsid}: CoCreateInstance failed: {e}"),
        };
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None).expect("item");
        let array: IShellItemArray = SHCreateShellItemArrayFromShellItem(&item).expect("array");
        let title = cmd.GetTitle(&array).map(|p| { let s = p.to_string().unwrap_or_default(); CoTaskMemFree(Some(p.0 as _)); s });
        let state = cmd.GetState(&array, true);
        let flags = cmd.GetFlags();
        let icon = cmd.GetIcon(&array).map(|p| { let s = p.to_string().unwrap_or_default(); CoTaskMemFree(Some(p.0 as _)); s });
        let first = t0.elapsed();
        let t1 = std::time::Instant::now();
        for _ in 0..5 {
            let c: IExplorerCommand = CoCreateInstance(&id, None, CLSCTX_LOCAL_SERVER | CLSCTX_INPROC_SERVER).expect("again");
            let _ = c.GetState(&array, false);
            if let Ok(p) = c.GetTitle(&array) { CoTaskMemFree(Some(p.0 as _)); }
        }
        println!("{clsid}: title={title:?} state={state:?} flags={flags:?} icon={icon:?} first={first:?} warm_avg={:?}", t1.elapsed() / 5);
    }
}
