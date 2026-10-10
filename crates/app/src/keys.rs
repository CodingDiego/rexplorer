//! `[keys]` in settings.ini: `cmd.new_tab=Ctrl+Y` replaces that command's default accelerators.

use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::{ACCEL, ACCEL_VIRT_FLAGS, FALT, FCONTROL, FSHIFT, FVIRTKEY};

use crate::*;

/// The commands a `[keys]` line can name.
const NAMES: [(&str, u16); 22] = [
    ("cmd.back", CMD_BACK),
    ("cmd.forward", CMD_FORWARD),
    ("cmd.up", CMD_UP),
    ("cmd.refresh", CMD_REFRESH),
    ("cmd.address", CMD_ADDRESS),
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
    ("cmd.search", CMD_SEARCH),
    ("cmd.pin", CMD_PIN),
    ("cmd.view_details", CMD_VIEW1),
    ("cmd.view_icons", CMD_VIEW1 + 1),
    ("cmd.view_thumbnails", CMD_VIEW1 + 2),
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
            table.retain(|a| a.cmd != cmd);
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
}
