//! Profiles and color themes stored as plain ini files under `%APPDATA%\rexplorer`.
//! Pure Rust, no Win32.
//!
//! `settings.ini` holds `[general] profile=<name>` plus one `[profile <name>]` section per
//! profile; `themes\<name>.ini` holds a single `[theme]` section.

use std::path::{Path, PathBuf};

type Section = (String, Vec<(String, String)>);

/// Parses ini text into ordered sections; keys before any header go under `""`.
/// Blank lines and lines starting with `;` or `#` are skipped.
pub fn parse_ini(text: &str) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    let mut cur: Section = (String::new(), Vec::new());
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            // The unnamed leading section only exists if it has keys.
            if !(cur.0.is_empty() && cur.1.is_empty()) {
                out.push(cur);
            }
            cur = (name.trim().to_string(), Vec::new());
        } else if let Some((k, v)) = line.split_once('=') {
            cur.1.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    if !(cur.0.is_empty() && cur.1.is_empty()) {
        out.push(cur);
    }
    out
}

/// Inverse of [`parse_ini`] (comments are not preserved).
pub fn write_ini(sections: &[Section]) -> String {
    let mut s = String::new();
    for (name, keys) in sections {
        if !name.is_empty() {
            s.push_str(&format!("[{name}]\n"));
        }
        for (k, v) in keys {
            s.push_str(&format!("{k}={v}\n"));
        }
        s.push('\n');
    }
    s
}

/// `#RRGGBB` to a COLORREF (`0x00BBGGRR`).
pub fn parse_color(s: &str) -> Option<u32> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(((v & 0xFF) << 16) | (v & 0xFF00) | (v >> 16))
}

/// COLORREF to `#RRGGBB`.
pub fn format_color(c: u32) -> String {
    format!("#{:02X}{:02X}{:02X}", c & 0xFF, (c >> 8) & 0xFF, (c >> 16) & 0xFF)
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub name: String,
    pub dark: bool,
    pub mica: bool,
    pub background: u32,
    /// Content cards (list, preview, fields); the window frame, sidebar and header use `background`.
    pub surface: u32,
    pub text: u32,
    pub secondary_text: u32,
    pub accent: u32,
    pub selection: u32,
    pub border: u32,
    pub icon_theme: Option<String>,
    pub font: String,
    pub font_size: i32,
}

/// Mix `a` toward `b` by `t` (0..1), per 0x00BBGGRR channel.
fn mix(a: u32, b: u32, t: f32) -> u32 {
    (0..3).fold(0, |acc, i| {
        let (x, y) = (((a >> (8 * i)) & 0xFF) as f32, ((b >> (8 * i)) & 0xFF) as f32);
        acc | (((x + (y - x) * t).round() as u32) << (8 * i))
    })
}

impl Theme {
    /// Selection is the accent at 18% over the surface; border is the background 8% toward the text.
    fn palette(name: &str, dark: bool, [background, surface, text, secondary_text, accent]: [&str; 5]) -> Theme {
        let c = |s| parse_color(s).unwrap();
        let (background, surface, text, accent) = (c(background), c(surface), c(text), c(accent));
        Theme {
            name: name.into(),
            dark,
            mica: true,
            background,
            surface,
            text,
            secondary_text: c(secondary_text),
            accent,
            selection: mix(surface, accent, 0.18),
            border: mix(background, text, 0.08),
            icon_theme: None,
            font: "Segoe UI Variable Text".into(),
            font_size: 10,
        }
    }

    pub fn light() -> Theme {
        Theme::palette("light", false, ["#F5F5F7", "#FFFFFF", "#1D1D1F", "#6E6E73", "#0A84FF"])
    }

    pub fn dark() -> Theme {
        Theme::palette("dark", true, ["#1E1E1E", "#262626", "#F2F2F7", "#98989D", "#0A84FF"])
    }

    /// A theme file written by an earlier version with its built-in values unchanged (no `surface` key).
    fn is_legacy_default(text: &str) -> bool {
        let t = Theme::from_ini("", text);
        let colors = [t.background, t.text, t.secondary_text, t.accent, t.selection, t.border].map(format_color);
        let old = [
            ["#FFFFFF", "#1B1B1B", "#6B6B6B", "#0067C0", "#CCE4F7", "#E5E5E5"],
            ["#202020", "#FFFFFF", "#A0A0A0", "#4CC2FF", "#3A3A3A", "#333333"],
        ];
        !text.contains("surface") && t.icon_theme.is_none() && t.font_size == 9 && old.iter().any(|o| colors == *o)
    }

    /// Starts from the dark or light builtin per the `dark` key, then applies present keys.
    /// Unparseable values are ignored.
    pub fn from_ini(name: &str, text: &str) -> Theme {
        let sections = parse_ini(text);
        let keys: &[(String, String)] = sections
            .iter()
            .find(|(n, _)| n == "theme")
            .map_or(&[], |(_, k)| k.as_slice());
        let get = |key: &str| keys.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        let mut t = if get("dark").and_then(parse_bool).unwrap_or(true) { Theme::dark() } else { Theme::light() };
        t.name = name.to_string();
        let color = |key: &str, slot: &mut u32| {
            if let Some(c) = get(key).and_then(parse_color) {
                *slot = c;
            }
        };
        if let Some(b) = get("mica").and_then(parse_bool) {
            t.mica = b;
        }
        color("background", &mut t.background);
        // A file from before `surface` existed keeps its look: cards in its background color.
        t.surface = get("surface").or(get("background")).and_then(parse_color).unwrap_or(t.surface);
        color("text", &mut t.text);
        color("secondary_text", &mut t.secondary_text);
        color("accent", &mut t.accent);
        color("selection", &mut t.selection);
        color("border", &mut t.border);
        if let Some(v) = get("icon_theme").filter(|v| !v.is_empty()) {
            t.icon_theme = Some(v.to_string());
        }
        if let Some(v) = get("font").filter(|v| !v.is_empty()) {
            t.font = v.to_string();
        }
        if let Some(n) = get("font_size").and_then(|v| v.parse().ok()) {
            t.font_size = n;
        }
        t
    }

    pub fn to_ini(&self) -> String {
        let mut k = vec![
            ("dark".to_string(), self.dark.to_string()),
            ("mica".to_string(), self.mica.to_string()),
            ("background".to_string(), format_color(self.background)),
            ("surface".to_string(), format_color(self.surface)),
            ("text".to_string(), format_color(self.text)),
            ("secondary_text".to_string(), format_color(self.secondary_text)),
            ("accent".to_string(), format_color(self.accent)),
            ("selection".to_string(), format_color(self.selection)),
            ("border".to_string(), format_color(self.border)),
        ];
        if let Some(i) = &self.icon_theme {
            k.push(("icon_theme".into(), i.clone()));
        }
        k.push(("font".into(), self.font.clone()));
        k.push(("font_size".into(), self.font_size.to_string()));
        write_ini(&[("theme".to_string(), k)])
    }
}

/// The shell's Home ("Inicio"): Quick Access pins and recent files. The default start folder.
pub const HOME_START: &str = "shell:::{F874310E-B6B7-47DC-BC84-B9E6B38F5903}";

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    pub start: String,
    pub theme: String,
    pub view: String,
    pub show_tree: bool,
    pub show_preview: bool,
    pub tree_width: i32,
    pub preview_width: i32,
    pub bookmarks: Vec<(String, String)>,
}

impl Profile {
    pub fn default_profile() -> Profile {
        Profile {
            name: "default".into(),
            start: HOME_START.into(),
            theme: "dark".into(),
            view: "details".into(),
            show_tree: false,
            show_preview: false,
            tree_width: 220,
            preview_width: 360,
            // The sidebar's Favorites come from Quick Access with the shell's names; bookmarks are the user's own.
            bookmarks: Vec::new(),
        }
    }

    fn to_section(&self) -> Section {
        let mut k = vec![
            ("start".to_string(), self.start.clone()),
            ("theme".to_string(), self.theme.clone()),
            ("view".to_string(), self.view.clone()),
            ("show_tree".to_string(), self.show_tree.to_string()),
            ("show_preview".to_string(), self.show_preview.to_string()),
            ("tree_width".to_string(), self.tree_width.to_string()),
            ("preview_width".to_string(), self.preview_width.to_string()),
        ];
        k.extend(self.bookmarks.iter().map(|(l, t)| (format!("bookmark.{l}"), t.clone())));
        (format!("profile {}", self.name), k)
    }

    /// Missing keys take the default profile's values, except bookmarks (only those listed).
    fn from_section(name: &str, keys: &[(String, String)]) -> Profile {
        let mut p = Profile { name: name.to_string(), bookmarks: Vec::new(), ..Profile::default_profile() };
        for (k, v) in keys {
            match k.as_str() {
                "start" => p.start = v.clone(),
                "theme" => p.theme = v.clone(),
                "view" => p.view = v.clone(),
                "show_tree" => p.show_tree = parse_bool(v).unwrap_or(p.show_tree),
                "show_preview" => p.show_preview = parse_bool(v).unwrap_or(p.show_preview),
                "tree_width" => p.tree_width = v.parse().unwrap_or(p.tree_width),
                "preview_width" => p.preview_width = v.parse().unwrap_or(p.preview_width),
                _ => {
                    if let Some(label) = k.strip_prefix("bookmark.") {
                        p.bookmarks.push((label.to_string(), v.clone()));
                    }
                }
            }
        }
        p
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub active: String,
    pub profiles: Vec<Profile>,
    pub themes: Vec<Theme>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { active: "default".into(), profiles: vec![Profile::default_profile()], themes: vec![Theme::light(), Theme::dark()] }
    }
}

/// `%APPDATA%\rexplorer`, or `<temp>\rexplorer` when APPDATA is unset.
pub fn dir() -> PathBuf {
    std::env::var_os("APPDATA").map_or_else(std::env::temp_dir, PathBuf::from).join("rexplorer")
}

fn theme_path(dir: &Path, name: &str) -> PathBuf {
    dir.join("themes").join(format!("{name}.ini"))
}

impl Settings {
    pub fn load() -> Settings {
        Settings::load_from(&dir())
    }

    /// Missing or unreadable files yield defaults; builtin themes are always present and
    /// replaced by same-named theme files.
    pub fn load_from(dir: &Path) -> Settings {
        let mut s = Settings::default();
        let sections = std::fs::read_to_string(dir.join("settings.ini")).map(|t| parse_ini(&t)).unwrap_or_default();
        let profiles: Vec<Profile> = sections
            .iter()
            .filter_map(|(n, k)| n.strip_prefix("profile ").map(|name| Profile::from_section(name.trim(), k)))
            .collect();
        if !profiles.is_empty() {
            s.profiles = profiles;
        }
        s.active = sections
            .iter()
            .find(|(n, _)| n == "general")
            .and_then(|(_, k)| k.iter().find(|(k, _)| k == "profile"))
            .map_or_else(|| s.profiles[0].name.clone(), |(_, v)| v.clone());

        let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join("themes"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ini")))
            .collect();
        files.sort();
        for p in files {
            let (Some(name), Ok(text)) = (p.file_stem().and_then(|n| n.to_str()), std::fs::read_to_string(&p)) else { continue };
            let t = Theme::from_ini(name, &text);
            match s.themes.iter_mut().find(|x| x.name == name) {
                Some(slot) => *slot = t,
                None => s.themes.push(t),
            }
        }
        s
    }

    pub fn save(&self) {
        let _ = self.save_to(&dir());
    }

    pub fn save_to(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir.join("themes"))?;
        std::fs::write(dir.join("settings.ini"), self.settings_text())?;
        for t in &self.themes {
            std::fs::write(theme_path(dir, &t.name), t.to_ini())?;
        }
        Ok(())
    }

    fn settings_text(&self) -> String {
        let mut sections = vec![("general".to_string(), vec![("profile".to_string(), self.active.clone())])];
        sections.extend(self.profiles.iter().map(Profile::to_section));
        write_ini(&sections)
    }

    /// The active profile, or the first one if the name is unknown.
    pub fn active_profile(&self) -> &Profile {
        self.profiles.iter().find(|p| p.name == self.active).unwrap_or(&self.profiles[0])
    }

    /// Falls back to "dark", then to the first theme.
    pub fn theme(&self, name: &str) -> &Theme {
        let find = |n: &str| self.themes.iter().find(|t| t.name == n);
        find(name).or_else(|| find("dark")).unwrap_or(&self.themes[0])
    }

    /// Writes the default files only where missing, so the user can edit them. A built-in theme file still
    /// holding an earlier version's unedited defaults gets the current defaults.
    pub fn ensure_files(dir: &Path) {
        let d = Settings::default();
        let _ = std::fs::create_dir_all(dir.join("themes"));
        let write_new = |p: PathBuf, text: String| {
            let _ = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(p)
                .and_then(|mut f| std::io::Write::write_all(&mut f, text.as_bytes()));
        };
        write_new(dir.join("settings.ini"), d.settings_text());
        for t in &d.themes {
            let p = theme_path(dir, &t.name);
            if std::fs::read_to_string(&p).is_ok_and(|text| Theme::is_legacy_default(&text)) {
                let _ = std::fs::write(&p, t.to_ini());
            }
            write_new(p, t.to_ini());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rexplorer-settings-{}", std::process::id())).join(tag);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn cleanup(tag: &str) {
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("rexplorer-settings-{}", std::process::id())).join(tag));
    }

    #[test]
    fn ini_round_trip() {
        let text = "top = 1\n; comment\n# other\n[a b]\n  k =  v with  spaces \n\nx=y\n[empty]\n[a b]\nz=\n";
        let p = parse_ini(text);
        assert_eq!(p[0], ("".to_string(), vec![("top".to_string(), "1".to_string())]));
        assert_eq!(p[1].1[0], ("k".to_string(), "v with  spaces".to_string()));
        assert_eq!(p.len(), 4);
        assert_eq!(parse_ini(&write_ini(&p)), p);
    }

    #[test]
    fn colors() {
        assert_eq!(parse_color("#FF8000"), Some(0x0000_80FF));
        assert_eq!(parse_color("#102030"), Some(0x0030_2010));
        assert_eq!(format_color(0x0030_2010), "#102030");
        assert_eq!(parse_color("102030"), None);
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("#GG0000"), None);
    }

    #[test]
    fn empty_dir_is_defaults() {
        let d = tmp("empty");
        assert_eq!(Settings::load_from(&d), Settings::default());
        assert_eq!(Settings::load_from(&d.join("missing")), Settings::default());
        cleanup("empty");
    }

    #[test]
    fn save_then_load() {
        let d = tmp("save");
        let mut s = Settings::default();
        let mut custom = Theme::light();
        custom.name = "sepia".into();
        custom.accent = 0x0012_3456;
        custom.icon_theme = Some("fluent".into());
        custom.font_size = 11;
        s.themes.push(custom);
        s.profiles.push(Profile {
            name: "work".into(),
            theme: "sepia".into(),
            show_preview: true,
            bookmarks: vec![("Repos".into(), r"C:\Users\x\My Repos".into())],
            ..Profile::default_profile()
        });
        s.active = "work".into();
        s.save_to(&d).unwrap();
        let back = Settings::load_from(&d);
        assert_eq!(back, s);
        assert_eq!(back.active_profile().name, "work");
        assert_eq!(back.theme("nope").name, "dark");
        cleanup("save");
    }

    #[test]
    fn palettes() {
        let (l, d) = (Theme::light(), Theme::dark());
        assert_eq!(format_color(l.background), "#F5F5F7");
        assert_eq!(format_color(l.surface), "#FFFFFF");
        assert_eq!(format_color(d.surface), "#262626");
        assert_eq!(format_color(d.text), "#F2F2F7");
        // #0A84FF at 18% over #FFFFFF and over #262626; borders 8% toward the text.
        assert_eq!(format_color(l.selection), "#D3E9FF");
        assert_eq!(format_color(d.selection), "#21374D");
        assert_eq!(format_color(l.border), "#E4E4E6");
        assert_eq!(l.font_size, 10);
        // A partial file keeps the built-in surface; one that sets only a background uses it for the surface.
        assert_eq!(Theme::from_ini("x", "[theme]\ndark=false\naccent=#010203\n").surface, l.surface);
        assert_eq!(Theme::from_ini("x", "[theme]\nbackground=#101010\n").surface, 0x101010);
        assert_eq!(Theme::from_ini("x", "[theme]\nsurface=#123456\n").surface, 0x563412);
        assert_eq!(Theme::from_ini("dark", &d.to_ini()), d);
    }

    #[test]
    fn legacy_default_themes_are_replaced() {
        let d = tmp("legacy");
        std::fs::create_dir_all(d.join("themes")).unwrap();
        let old = "[theme]\ndark=true\nmica=true\nbackground=#202020\ntext=#FFFFFF\nsecondary_text=#A0A0A0\naccent=#4CC2FF\n\
                   selection=#3A3A3A\nborder=#333333\nfont=Segoe UI Variable Text\nfont_size=9\n";
        let edited = old.replace("#4CC2FF", "#FF0000");
        std::fs::write(d.join("themes/dark.ini"), old).unwrap();
        std::fs::write(d.join("themes/light.ini"), &edited).unwrap();
        Settings::ensure_files(&d);
        assert_eq!(Settings::load_from(&d).theme("dark"), &Theme::dark());
        assert_eq!(std::fs::read_to_string(d.join("themes/light.ini")).unwrap(), edited);
        cleanup("legacy");
    }

    #[test]
    fn ensure_files_keeps_edits() {
        let d = tmp("ensure");
        Settings::ensure_files(&d);
        for f in ["settings.ini", "themes/light.ini", "themes/dark.ini"] {
            assert!(d.join(f).exists(), "{f}");
        }
        std::fs::write(d.join("themes/dark.ini"), "[theme]\naccent=#010203\n").unwrap();
        Settings::ensure_files(&d);
        assert_eq!(Settings::load_from(&d).theme("dark").accent, 0x0003_0201);
        cleanup("ensure");
    }
}
