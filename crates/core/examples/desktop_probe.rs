//! Read-only check of the desktop view from outside Explorer: its items, the selection, and the item at a point.
//! Usage: cargo run --release -p rexplorer-core --example desktop_probe -- [x y]
use rexplorer_core::com::ComInit;
use rexplorer_core::desktop::DesktopView;
use rexplorer_core::item::ShellItem;
use windows::Win32::Foundation::{HWND, POINT};

fn main() {
    if std::env::args().any(|a| a == "--aware") {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn SetProcessDpiAwarenessContext(context: isize) -> i32;
        }
        // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, as the desktop helper runs.
        unsafe { SetProcessDpiAwarenessContext(-4) };
    }
    let _c = ComInit::sta();
    let root = unsafe { windows::Win32::UI::Shell::SHGetDesktopFolder() }.expect("desktop root");
    let view = DesktopView::find().expect("desktop view");
    let name = |i: usize| view.item(i).map(|pidl| ShellItem { parent: root.clone(), pidl }.display_name());
    for i in 0.. {
        match name(i) {
            Ok(n) => println!("item {i}: {n}"),
            Err(_) => break,
        }
    }
    let list = find_list().expect("desktop list window");
    println!("selection: {:?}", DesktopView::selection(list));
    println!("icon size: {:?}", view.icon_size());
    for (name, f) in rexplorer_core::desktop::ARRANGE {
        println!("{name}: {:?}", view.has_flag(f));
    }
    if std::env::args().any(|a| a == "--refresh") {
        println!("refresh: {:?}", view.refresh());
    }
    if let Some(n) = std::env::args().skip_while(|a| a != "--select").nth(1).and_then(|s| s.parse::<usize>().ok()) {
        println!("select_for_menu({n}): {:?}", view.select_for_menu(list, Some(n)));
        println!("selection after: {:?}", DesktopView::selection(list));
    }
    for i in [0usize, 5] {
        let p = view.item_point(list, i).expect("item position");
        let probe = POINT { x: p.x + 20, y: p.y + 20 };
        println!("item {i} at ({},{}); hit test at ({},{}) -> {:?}", p.x, p.y, probe.x, probe.y, DesktopView::index_at(list, probe));
    }
    let r = view.item_point(list, 0).unwrap();
    println!("hit test far below item 0 -> {:?}", DesktopView::index_at(list, POINT { x: r.x + 3000, y: r.y + 3000 }));
}

/// The desktop's list: Progman's, or the WorkerW that hosts it once the wallpaper has animated.
fn find_list() -> Option<HWND> {
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, FindWindowW};
    use windows::core::w;
    unsafe {
        let mut host = FindWindowW(w!("Progman"), None).ok();
        while let Some(h) = host {
            if let Ok(dv) = FindWindowExW(Some(h), None, w!("SHELLDLL_DefView"), None) {
                return FindWindowExW(Some(dv), None, w!("SysListView32"), None).ok();
            }
            host = FindWindowExW(None, Some(h), w!("WorkerW"), None).ok();
        }
        None
    }
}