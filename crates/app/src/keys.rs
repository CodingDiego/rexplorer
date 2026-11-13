//! `[keys]` in settings.ini: `cmd.new_tab=Ctrl+Y` replaces that command's default accelerators.

use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::{ACCEL, ACCEL_VIRT_FLAGS, FALT, FCONTROL, FSHIFT, FVIRTKEY};

use crate::*;

/// The commands a `[keys]` line can name.
const NAMES: [(&str, u16); 43] = [
    ("cmd.back", CMD_BACK),
    ("cmd.forward", CMD_FORWARD),
    ("cmd.up", CMD_UP),
    ("cmd.refresh", CMD_REFRESH),
    ("cmd.address", CMD_ADDRESS),
    ("cmd.address_history", CMD_ADDRESS_HISTORY),
    ("cmd.explorer", CMD_EXPLORER),
    ("cmd.enter", CMD_ENTER),
    ("cmd.escape", CMD_ESCAPE),
    ("cmd.properties", CMD_PROPERTIES),
    ("cmd.new_tab", CMD_NEW_TAB),
    ("cmd.close_tab", CMD_CLOSE_TAB),
    ("cmd.next_tab", CMD_NEXT_TAB),
    ("cmd.prev_tab", CMD_PREV_TAB),
    ("cmd.palette", CMD_PALETTE),
    ("cmd.terminal", CMD_TERMINAL),
    ("cmd.toggle_sidebar", CMD_TOGGLE_SIDEBAR),
    ("cmd.toggle_preview", CMD_TOGGLE_PREVIEW),
    ("cmd.toggle_details", CMD_TOGGLE_DETAILS),
    ("cmd.search", CMD_SEARCH),
    ("cmd.pin", CMD_PIN),
    ("cmd.view_details", CMD_VIEW1),
    ("cmd.view_icons", CMD_VIEW1 + 1),
    ("cmd.view_thumbnails", CMD_VIEW1 + 2),
    ("cmd.view_extra_large", CMD_VIEW_XL),
    ("cmd.view_medium", CMD_VIEW_MEDIUM),
    ("cmd.view_small", CMD_VIEW_SMALL),
    ("cmd.view_list", CMD_VIEW_LIST),
    ("cmd.view_tiles", CMD_VIEW_TILES),
    ("cmd.view_content", CMD_VIEW_CONTENT),
    ("cmd.new_folder", CMD_NEW_FOLDER),
    ("cmd.open_tab", CMD_OPEN_TAB),
    ("cmd.fullscreen", CMD_FULLSCREEN),
    ("cmd.invert_selection", CMD_INVERT),
    ("cmd.next_pane", CMD_FOCUS_NEXT),
    ("cmd.prev_pane", CMD_FOCUS_PREV),
    ("cmd.new_window", CMD_NEW_WINDOW),
    ("cmd.undo", CMD_UNDO),
    ("cmd.redo", CMD_REDO),
    ("cmd.copy_path", CMD_COPY_PATH),
    ("cmd.fit_columns", CMD_FIT_COLUMNS),
    ("cmd.paste_shortcut", CMD_PASTE_SHORTCUT),
    ("cmd.run_as_admin", CMD_RUN_AS),
];

/// `Ctrl+Shift+X`, `F5`, `Alt+Left`: (modifier flags, virtual key); None when it is not one of those.
fn parse_chord(s: &str) -> Option<(u8, VIRTUAL_KEY)> {
    let mut flags = 0;
    let mut parts = s.split('+').map(str::trim).peekable();
    let mut key = None;
    while let Some(p) = parts.next() {
        let lower = p.to_ascii_lowercase();
        if parts.peek().is_some() {
            flags |= match lower.as_str() {
                "ctrl" | "control" => FCONTROL.0,
                "alt" => FALT.0,
                "shift" => FSHIFT.0,
                _ => return None,
            };
        } else {
            key = Some(match lower.as_str() {
                "left" => VK_LEFT,
                "right" => VK_RIGHT,
                "up" => VK_UP,
                "down" => VK_DOWN,
                "enter" | "return" => VK_RETURN,
                "esc" | "escape" => VK_ESCAPE,
                "tab" => VK_TAB,
                "space" => VK_SPACE,
                "backspace" => VK_BACK,
                "delete" | "del" => VK_DELETE,
                "insert" => VK_INSERT,
                "home" => VK_HOME,
                "end" => VK_END,
                "pageup" => VK_PRIOR,
                "pagedown" => VK_NEXT,
                k if k.len() >= 2 && k.starts_with('f') && k[1..].parse::<u16>().is_ok_and(|n| (1..=24).contains(&n)) => {
                    VIRTUAL_KEY(VK_F1.0 + k[1..].parse::<u16>().ok()? - 1)
                }
                // A character key needs Ctrl or Alt: bare (or shifted) it would eat typing in the address bar and the list.
                _ if flags & (FCONTROL.0 | FALT.0) == 0 => return None,
                "`" => VK_OEM_3,
                k if k.len() == 1 && k.as_bytes()[0].is_ascii_alphanumeric() => VIRTUAL_KEY(k.as_bytes()[0].to_ascii_uppercase() as u16),
                _ => return None,
            });
        }
    }
    key.map(|k| (flags, k))
}

/// The default table with each overridden command's accelerators replaced by its valid `Ctrl+X[, Alt+Y]` entries;
/// a command whose entries are all invalid keeps its defaults.
pub fn apply(defaults: &[ACCEL], overrides: &[(String, String)]) -> Vec<ACCEL> {
    let mut table = defaults.to_vec();
    for (name, value) in overrides {
        let Some(&(_, cmd)) = NAMES.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) else { continue };
        let chords: Vec<ACCEL> = value
            .split(',')
            .filter_map(parse_chord)
            .map(|(flags, key)| ACCEL { fVirt: ACCEL_VIRT_FLAGS(FVIRTKEY.0 | flags), key: key.0, cmd })
            .collect();
        if !chords.is_empty() {
            // A user's chord wins over another command's default.
            table.retain(|a| a.cmd != cmd && !chords.iter().any(|c| c.fVirt == a.fVirt && c.key == a.key));
            table.extend(chords);
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords() {
        assert_eq!(parse_chord("Ctrl+Shift+X"), Some((FCONTROL.0 | FSHIFT.0, VK_X)));
        assert_eq!(parse_chord("f5"), Some((0, VK_F5)));
        assert_eq!(parse_chord(" Alt + Left "), Some((FALT.0, VK_LEFT)));
        assert_eq!(parse_chord("Ctrl+`"), Some((FCONTROL.0, VK_OEM_3)));
        assert_eq!(parse_chord("F24"), Some((0, VIRTUAL_KEY(VK_F1.0 + 23))));
        assert_eq!(parse_chord("Alt+A"), Some((FALT.0, VK_A)));
        for bad in ["", "Ctrl+", "Win+X", "Ctrl+F25", "F0", "Ctrl+Foo", "X+Ctrl", "a", "Shift+A", "`", "Shift+`"] {
            assert_eq!(parse_chord(bad), None, "{bad}");
        }
    }

    #[test]
    fn overrides_replace_only_that_command() {
        let defaults = ACCELS.to_vec();
        assert_eq!(apply(&defaults, &[]).len(), defaults.len());
        let t = apply(&defaults, &[("cmd.new_tab".into(), "Ctrl+Y".into()), ("cmd.refresh".into(), "junk".into()), ("cmd.bogus".into(), "F1".into())]);
        let of = |cmd: u16| t.iter().filter(|a| a.cmd == cmd).map(|a| (a.fVirt.0, a.key)).collect::<Vec<_>>();
        assert_eq!(of(CMD_NEW_TAB), vec![(FVIRTKEY.0 | FCONTROL.0, VK_Y.0)]);
        assert_eq!(of(CMD_REFRESH).len(), 2, "invalid entry keeps F5 and Ctrl+R");
        assert_eq!(of(CMD_CLOSE_TAB).len(), 1);
        let t = apply(&defaults, &[("cmd.address".into(), "Ctrl+L, Alt+D, nope".into())]);
        assert_eq!(t.iter().filter(|a| a.cmd == CMD_ADDRESS).count(), 2);
    }

    #[test]
    fn explorer_view_chords() {
        let t = apply(&ACCELS, &[]);
        let want = [CMD_VIEW_XL, CMD_VIEW1 + 1, CMD_VIEW_MEDIUM, CMD_VIEW_SMALL, CMD_VIEW_LIST, CMD_VIEW1, CMD_VIEW_TILES, CMD_VIEW_CONTENT];
        for (i, cmd) in want.into_iter().enumerate() {
            let key = VK_1.0 + i as u16;
            let on: Vec<u16> = t.iter().filter(|a| a.fVirt.0 == FVIRTKEY.0 | FCONTROL.0 | FSHIFT.0 && a.key == key).map(|a| a.cmd).collect();
            assert_eq!(on, [cmd], "Ctrl+Shift+{}", i + 1);
        }
        assert!(!t.iter().any(|a| a.cmd == CMD_VIEW1 + 2), "Thumbnails has no default chord");
        let t = apply(&ACCELS, &[("cmd.view_small".into(), "Ctrl+Alt+S".into())]);
        assert_eq!(t.iter().filter(|a| a.cmd == CMD_VIEW_SMALL).count(), 1);
    }

    #[test]
    fn new_view_commands_are_rebindable() {
        let of = |t: &[ACCEL], cmd: u16| t.iter().filter(|a| a.cmd == cmd).map(|a| (a.fVirt.0, a.key)).collect::<Vec<_>>();
        let t = apply(&ACCELS, &[("cmd.view_tiles".into(), "Ctrl+Alt+T".into())]);
        assert_eq!(of(&t, CMD_VIEW_TILES), [(FVIRTKEY.0 | FCONTROL.0 | FALT.0, VK_T.0)]);
        assert!(!t.iter().any(|a| a.fVirt.0 == FVIRTKEY.0 | FCONTROL.0 | FSHIFT.0 && a.key == VK_7.0));
        let t = apply(&ACCELS, &[("cmd.view_thumbnails".into(), "Ctrl+Shift+3".into())]);
        assert_eq!(of(&t, CMD_VIEW1 + 2), [(FVIRTKEY.0 | FCONTROL.0 | FSHIFT.0, VK_3.0)]);
        assert!(of(&t, CMD_VIEW_MEDIUM).is_empty());
        for (name, cmd) in [("cmd.view_list", CMD_VIEW_LIST), ("cmd.view_content", CMD_VIEW_CONTENT)] {
            let t = apply(&ACCELS, &[(name.into(), "Ctrl+Alt+K".into())]);
            assert_eq!(of(&t, cmd), [(FVIRTKEY.0 | FCONTROL.0 | FALT.0, VK_K.0)], "{name}");
        }
    }

    #[test]
    fn details_pane_chord() {
        let t = apply(&ACCELS, &[]);
        let has = |flags: u8, key: VIRTUAL_KEY, cmd: u16| t.iter().any(|a| a.fVirt.0 == FVIRTKEY.0 | flags && a.key == key.0 && a.cmd == cmd);
        assert!(has(FALT.0 | FSHIFT.0, VK_P, CMD_TOGGLE_DETAILS));
        assert!(has(FALT.0, VK_P, CMD_TOGGLE_PREVIEW));
        let t = apply(&ACCELS, &[("cmd.toggle_details".into(), "Ctrl+Alt+D".into())]);
        let d: Vec<_> = t.iter().filter(|a| a.cmd == CMD_TOGGLE_DETAILS).map(|a| (a.fVirt.0, a.key)).collect();
        assert_eq!(d, vec![(FVIRTKEY.0 | FCONTROL.0 | FALT.0, VK_D.0)]);
    }

    #[test]
    fn batch_c_chords() {
        let t = apply(&ACCELS, &[]);
        let has = |flags: u8, key: VIRTUAL_KEY, cmd: u16| t.iter().any(|a| a.fVirt.0 == FVIRTKEY.0 | flags && a.key == key.0 && a.cmd == cmd);
        assert!(has(FCONTROL.0 | FSHIFT.0, VK_C, CMD_COPY_PATH));
        assert!(has(FCONTROL.0, VK_ADD, CMD_FIT_COLUMNS));
        assert!(has(0, VK_TAB, CMD_FOCUS_NEXT) && has(FSHIFT.0, VK_TAB, CMD_FOCUS_PREV));
        assert!(has(0, VK_F6, CMD_FOCUS_NEXT) && has(FSHIFT.0, VK_F6, CMD_FOCUS_PREV), "F6 stays");
        assert!(!t.iter().any(|a| a.fVirt.0 == FVIRTKEY.0 | FCONTROL.0 && a.key == VK_C.0), "plain Ctrl+C is the list's own key");
        let t = apply(&ACCELS, &[("cmd.copy_path".into(), "Alt+C".into())]);
        let c: Vec<_> = t.iter().filter(|a| a.cmd == CMD_COPY_PATH).map(|a| (a.fVirt.0, a.key)).collect();
        assert_eq!(c, vec![(FVIRTKEY.0 | FALT.0, VK_C.0)]);
        let t = apply(&ACCELS, &[("cmd.fit_columns".into(), "Alt+F".into())]);
        assert_eq!(t.iter().filter(|a| a.cmd == CMD_FIT_COLUMNS).count(), 1);
    }

    #[test]
    fn rename_edits_keep_tab() {
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, MSG, WINDOW_EX_STYLE, WM_KEYDOWN, WS_CHILD, WS_OVERLAPPEDWINDOW};
        unsafe {
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 200, 100, None, None, None, None).unwrap();
            let edit = CreateWindowExW(WINDOW_EX_STYLE(0), w!("EDIT"), PCWSTR::null(), WS_CHILD, 0, 0, 100, 20, Some(parent), None, None, None).unwrap();
            let msg = |hwnd, vk: VIRTUAL_KEY| MSG { hwnd, message: WM_KEYDOWN, wParam: WPARAM(vk.0 as usize), ..Default::default() };
            assert!(crate::edit_keeps_tab(&msg(edit, VK_TAB)), "a rename edit in the tree or the embedded view commits on Tab");
            assert!(!crate::edit_keeps_tab(&msg(edit, VK_F5)));
            assert!(!crate::edit_keeps_tab(&msg(parent, VK_TAB)), "Tab moves on from the panes themselves");
            let _ = DestroyWindow(parent);
        }
    }

    #[test]
    fn no_duplicate_default_chords() {
        for (i, a) in ACCELS.iter().enumerate() {
            assert!(!ACCELS[..i].iter().any(|b| b.fVirt == a.fVirt && b.key == a.key), "chord of command {} repeats", a.cmd);
        }
    }

    #[test]
    fn undo_redo_chords() {
        let t = apply(&ACCELS, &[]);
        let has = |key: VIRTUAL_KEY, cmd: u16| t.iter().any(|a| a.fVirt.0 == FVIRTKEY.0 | FCONTROL.0 && a.key == key.0 && a.cmd == cmd);
        assert!(has(VK_Z, CMD_UNDO) && has(VK_Y, CMD_REDO));
        let t = apply(&ACCELS, &[("cmd.undo".into(), "Alt+Z".into())]);
        let u: Vec<_> = t.iter().filter(|a| a.cmd == CMD_UNDO).map(|a| (a.fVirt.0, a.key)).collect();
        assert_eq!(u, vec![(FVIRTKEY.0 | FALT.0, VK_Z.0)]);
    }

    #[test]
    fn f4_opens_the_address_history() {
        let t = apply(&ACCELS, &[]);
        let on_f4: Vec<u16> = t.iter().filter(|a| a.fVirt.0 == FVIRTKEY.0 && a.key == VK_F4.0).map(|a| a.cmd).collect();
        assert_eq!(on_f4, [CMD_ADDRESS_HISTORY]);
        let t = apply(&ACCELS, &[("cmd.address_history".into(), "Alt+H".into())]);
        assert_eq!(t.iter().filter(|a| a.cmd == CMD_ADDRESS_HISTORY).map(|a| (a.fVirt.0, a.key)).collect::<Vec<_>>(), [(FVIRTKEY.0 | FALT.0, VK_H.0)]);
    }

    #[test]
    fn user_chord_wins_over_a_default() {
        let t = apply(&ACCELS, &[("cmd.new_tab".into(), "Ctrl+Y".into())]);
        assert!(!t.iter().any(|a| a.cmd == CMD_REDO && a.key == VK_Y.0 && a.fVirt.0 == FVIRTKEY.0 | FCONTROL.0));
        let y: Vec<_> = t.iter().filter(|a| a.key == VK_Y.0 && a.fVirt.0 == FVIRTKEY.0 | FCONTROL.0).collect();
        assert_eq!(y.len(), 1);
        assert_eq!(y[0].cmd, CMD_NEW_TAB);
    }
}
