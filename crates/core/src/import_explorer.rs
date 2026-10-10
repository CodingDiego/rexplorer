//! One-way import of Windows Explorer's current settings into a rexplorer profile (`--import-explorer`).
//! The registry is only read; `map` and `decide` are pure so they are tested without it.
//!
//! Sources: `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced` (Hidden, HideFileExt,
//! ShowStatusBar, ShowPreviewHandlers, AutoCheckSelect, LaunchTo), `...\Explorer` (ShowRecent, ShowFrequent) and
//! the `ShellState` binary there (double-click vs single-click). A value Windows has not written is absent: it
//! maps to no change.

use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RRF_RT_REG_DWORD, RegGetValueW};
use windows::core::HSTRING;

use crate::settings::{HOME_START, Profile};

const EXPLORER: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer";
const ADVANCED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";

/// Raw values as stored; None when absent or of another registry type.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Raw {
    pub hidden: Option<u32>,
    pub hide_file_ext: Option<u32>,
    pub show_status_bar: Option<u32>,
    pub show_preview_handlers: Option<u32>,
    pub auto_check_select: Option<u32>,
    pub launch_to: Option<u32>,
    pub show_recent: Option<u32>,
    pub show_frequent: Option<u32>,
    pub shell_state: Option<Vec<u8>>,
}

fn dword(key: &str, name: &str) -> Option<u32> {
    let mut v = 0u32;
    let mut cb = 4u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(name), RRF_RT_REG_DWORD, None, Some((&mut v as *mut u32).cast()), Some(&mut cb))
    }
    .ok()
    .ok()?;
    Some(v)
}

fn binary(key: &str, name: &str) -> Option<Vec<u8>> {
    let (k, n) = (HSTRING::from(key), HSTRING::from(name));
    let mut cb = 0u32;
    unsafe { RegGetValueW(HKEY_CURRENT_USER, &k, &n, RRF_RT_REG_BINARY, None, None, Some(&mut cb)) }.ok().ok()?;
    let mut buf = vec![0u8; cb as usize];
    unsafe { RegGetValueW(HKEY_CURRENT_USER, &k, &n, RRF_RT_REG_BINARY, None, Some(buf.as_mut_ptr().cast()), Some(&mut cb)) }.ok().ok()?;
    buf.truncate(cb as usize);
    Some(buf)
}

/// Whether Windows' "app mode" is dark (Settings > Personalization > Colors), which Explorer's menus follow.
pub fn apps_use_dark() -> bool {
    dword(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme") == Some(0)
}

/// Reads Explorer's values from the real HKCU (read only).
pub fn read() -> Raw {
    Raw {
        hidden: dword(ADVANCED, "Hidden"),
        hide_file_ext: dword(ADVANCED, "HideFileExt"),
        show_status_bar: dword(ADVANCED, "ShowStatusBar"),
        show_preview_handlers: dword(ADVANCED, "ShowPreviewHandlers"),
        auto_check_select: dword(ADVANCED, "AutoCheckSelect"),
        launch_to: dword(ADVANCED, "LaunchTo"),
        show_recent: dword(EXPLORER, "ShowRecent"),
        show_frequent: dword(EXPLORER, "ShowFrequent"),
        shell_state: binary(EXPLORER, "ShellState"),
    }
}

fn flag(v: Option<u32>, invert: bool) -> Option<String> {
    match v? {
        n @ (0 | 1) => Some(((n == 1) != invert).to_string()),
        _ => None,
    }
}

/// `SHELLSTATE`: cbSize, then a flags dword whose bit 5 is fDoubleClickInWebView (set = double-click opens).
fn single_click(state: &[u8]) -> Option<bool> {
    let cb = u32::from_le_bytes(state.get(..4)?.try_into().ok()?) as usize;
    if cb < 8 || cb > state.len() {
        return None;
    }
    Some(state[4] & 0x20 == 0)
}

/// The profile keys Explorer's values map to, as ini text. Unknown or missing values are left out.
pub fn map(r: &Raw) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let mut put = |k, v: Option<String>| out.extend(v.map(|v| (k, v)));
    put("show_hidden", match r.hidden { Some(1) => Some("true".into()), Some(2) => Some("false".into()), _ => None });
    put("show_extensions", flag(r.hide_file_ext, true));
    put("show_status", flag(r.show_status_bar, false));
    put("preview_handlers", flag(r.show_preview_handlers, false));
    put("item_checkboxes", flag(r.auto_check_select, false));
    put(
        "start",
        match r.launch_to {
            Some(1) => Some("shell:MyComputerFolder".into()),
            Some(2) => Some(HOME_START.into()),
            Some(3) => Some("shell:Downloads".into()),
            _ => None,
        },
    );
    put("show_recent", flag(r.show_recent, false));
    put("show_frequent", flag(r.show_frequent, false));
    put("single_click", r.shell_state.as_deref().and_then(single_click).map(|b| b.to_string()));
    out
}

fn current(p: &Profile, key: &str) -> String {
    match key {
        "show_hidden" => p.show_hidden.to_string(),
        "show_extensions" => p.show_extensions.to_string(),
        "show_status" => p.show_status.to_string(),
        "preview_handlers" => p.preview_handlers.to_string(),
        "item_checkboxes" => p.item_checkboxes.to_string(),
        "start" => p.start.clone(),
        "show_recent" => p.show_recent.to_string(),
        "show_frequent" => p.show_frequent.to_string(),
        "single_click" => p.single_click.to_string(),
        _ => String::new(),
    }
}

/// Splits the mapped values into those to write and those to skip with the reason. A key is written when the
/// profile still has the default value for it (or `overwrite`) and the value would change it.
pub fn decide(p: &Profile, mapped: Vec<(&'static str, String)>, overwrite: bool) -> (Vec<(&'static str, String)>, Vec<(&'static str, &'static str)>) {
    let default = Profile::default_profile();
    let (mut write, mut skip) = (Vec::new(), Vec::new());
    for (k, v) in mapped {
        let cur = current(p, k);
        if cur.eq_ignore_ascii_case(&v) {
            skip.push((k, "already the same"));
        } else if !overwrite && cur != current(&default, k) {
            skip.push((k, "changed in rexplorer (use --overwrite)"));
        } else {
            write.push((k, v));
        }
    }
    (write, skip)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(m: &'a [(&'static str, String)], k: &str) -> Option<&'a str> {
        m.iter().find(|(x, _)| *x == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn hidden_and_extensions() {
        let m = map(&Raw { hidden: Some(1), hide_file_ext: Some(0), ..Default::default() });
        assert_eq!((get(&m, "show_hidden"), get(&m, "show_extensions")), (Some("true"), Some("true")));
        let m = map(&Raw { hidden: Some(2), hide_file_ext: Some(1), ..Default::default() });
        assert_eq!((get(&m, "show_hidden"), get(&m, "show_extensions")), (Some("false"), Some("false")));
        assert_eq!(map(&Raw { hidden: Some(7), hide_file_ext: Some(9), ..Default::default() }), []);
    }

    #[test]
    fn launch_to() {
        let s = |v| map(&Raw { launch_to: Some(v), ..Default::default() });
        assert_eq!(get(&s(1), "start"), Some("shell:MyComputerFolder"));
        assert_eq!(get(&s(2), "start"), Some(HOME_START));
        assert_eq!(get(&s(3), "start"), Some("shell:Downloads"));
        assert_eq!(get(&s(4), "start"), None);
    }

    #[test]
    fn toggles_and_missing() {
        assert_eq!(map(&Raw::default()), []);
        let m = map(&Raw { show_status_bar: Some(0), show_preview_handlers: Some(1), auto_check_select: Some(1), show_recent: Some(0), show_frequent: Some(1), ..Default::default() });
        assert_eq!(get(&m, "show_status"), Some("false"));
        assert_eq!(get(&m, "preview_handlers"), Some("true"));
        assert_eq!(get(&m, "item_checkboxes"), Some("true"));
        assert_eq!((get(&m, "show_recent"), get(&m, "show_frequent")), (Some("false"), Some("true")));
    }

    #[test]
    fn shell_state_click_mode() {
        let mut st = vec![0x24, 0, 0, 0, 0x37, 0x28, 0, 0];
        st.resize(36, 0);
        assert_eq!(get(&map(&Raw { shell_state: Some(st.clone()), ..Default::default() }), "single_click"), Some("false"));
        st[4] &= !0x20;
        assert_eq!(get(&map(&Raw { shell_state: Some(st), ..Default::default() }), "single_click"), Some("true"));
        for bad in [vec![], vec![1, 2, 3], vec![0x24, 0, 0, 0, 0x37], vec![4, 0, 0, 0, 0, 0, 0, 0]] {
            assert_eq!(get(&map(&Raw { shell_state: Some(bad), ..Default::default() }), "single_click"), None);
        }
    }

    #[test]
    fn decide_keeps_changed_keys_unless_overwrite() {
        let mut p = Profile::default_profile();
        p.show_status = false; // hand-changed
        let m = vec![("show_hidden", "true".to_string()), ("show_status", "true".to_string()), ("show_extensions", "true".to_string())];
        let (w, s) = decide(&p, m.clone(), false);
        assert_eq!(w, [("show_hidden", "true".to_string())]);
        assert_eq!(s.iter().map(|x| x.0).collect::<Vec<_>>(), ["show_status", "show_extensions"]);
        let (w, _) = decide(&p, m, true);
        assert_eq!(w.iter().map(|x| x.0).collect::<Vec<_>>(), ["show_hidden", "show_status"]);
    }
}
