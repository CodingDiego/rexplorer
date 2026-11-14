//! Profiles and color themes stored as plain ini files under `%APPDATA%\rexplorer`.
//! Pure Rust, no Win32.
//!
//! `settings.ini` holds `[general] profile=<name>` plus one `[profile <name>]` section per
//! profile; `themes\<name>.ini` holds a single `[theme]` section.

use std::path::{Path, PathBuf};

type Section = (String, Vec<(String, String)>);

/// The format this build writes: `format=` in settings.ini's `[general]`, in a theme's `[theme]`, and a line of
/// session.txt and folders.txt. A file without one is format 0 (0.1.x), which format 1 reads as is: 1 only added keys.
/// A format only ever adds keys; one from a newer build is read for what this build knows and the rest is kept.
pub const FORMAT: u32 = 1;

/// The format of the data once loaded: the file's own when it is newer, else this build's (older ones are migrated).
pub fn loaded_format(value: Option<&str>) -> u32 {
    value.and_then(|v| v.trim().parse().ok()).unwrap_or(0).max(FORMAT)
}

/// Adds `format=` under the section's header (appending the section if it is missing) when the section has no
/// format key yet; comments, order and line endings are kept. None when there is nothing to add.
pub fn stamp_format(text: &str, section: &str) -> Option<String> {
    if parse_ini(text).iter().any(|(n, k)| n == section && k.iter().any(|(k, _)| k == "format")) {
        return None;
    }
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let (header, line) = (format!("[{section}]"), format!("format={FORMAT}"));
    let mut lines: Vec<&str> = text.lines().collect();
    match lines.iter().position(|l| l.trim_start_matches('\u{feff}').trim() == header) {
        Some(i) => lines.insert(i + 1, &line),
        None => lines.extend([header.as_str(), line.as_str()]),
    }
    Some(lines.join(eol) + eol)
}

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

/// Inverse of [`parse_ini`] (comments are not preserved). Keys of the unnamed section go first, before any header.
pub fn write_ini(sections: &[Section]) -> String {
    let mut s = String::new();
    for (name, keys) in sections.iter().filter(|(n, _)| n.is_empty()).chain(sections.iter().filter(|(n, _)| !n.is_empty())) {
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

/// The seeded tests' and fuzz/'s checks on any text as an ini file: trimmed names, keys and values split on the
/// first `=`; a settings file that always has a profile; a theme that round-trips but for its kept unknown keys
/// (a kept `[a=]` line rereads as a section header); a color that is in range or none.
#[cfg(any(test, fuzzing))]
pub fn fuzz_check(text: &str) {
    for (name, keys) in parse_ini(text) {
        assert_eq!(name, name.trim(), "{text:?}");
        assert!(!name.contains('\n'), "{text:?}");
        for (k, v) in keys {
            assert!(k == k.trim() && v == v.trim() && !k.contains('='), "{text:?}: {k:?}={v:?}");
        }
    }
    assert!(!Settings::from_ini(text).profiles.is_empty(), "{text:?}");
    let t = Theme::from_ini("fuzz", text);
    assert_eq!(t.name, "fuzz", "{text:?}");
    let back = Theme::from_ini("fuzz", &t.to_ini());
    assert_eq!(Theme { extra: Vec::new(), ..back }, Theme { extra: Vec::new(), ..t }, "{text:?}");
    assert!(parse_color(text).is_none_or(|v| v <= 0xFF_FFFF), "{text:?}");
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

/// System colors a Windows contrast theme needs beyond the `Theme` palette (COLORREFs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contrast {
    pub highlight_text: u32,
    pub hot: u32,
    pub gray: u32,
    pub button_face: u32,
    pub button_text: u32,
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
    /// Some only while a Windows contrast theme is on; never read from or written to a theme file.
    pub contrast: Option<Contrast>,
    /// See [`loaded_format`].
    pub format: u32,
    /// What this build does not read, written back as read: unknown `[theme]` keys under `theme`, then other sections.
    pub extra: Vec<Section>,
}

const THEME_KEYS: [&str; 13] =
    ["format", "dark", "mica", "background", "surface", "text", "secondary_text", "accent", "selection", "border", "icon_theme", "font", "font_size"];

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
        // Cannot fail: the strings are the #RRGGBB literals in `light`/`dark` below, parsed by the palettes test.
        #[allow(clippy::unwrap_used)]
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
            contrast: None,
            format: FORMAT,
            extra: Vec::new(),
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
        // A folder name under `icons\`, nothing more: a path here (`\\host\share`, `..\..`, `C:\x`) would make every
        // start read icons from wherever a shared theme file points.
        let one_name = |v: &&str| matches!(Path::new(v).components().collect::<Vec<_>>()[..], [std::path::Component::Normal(_)]);
        if let Some(v) = get("icon_theme").filter(one_name) {
            t.icon_theme = Some(v.to_string());
        }
        if let Some(v) = get("font").filter(|v| !v.is_empty()) {
            t.font = v.to_string();
        }
        if let Some(n) = get("font_size").and_then(|v| v.parse::<i32>().ok()) {
            t.font_size = n.clamp(6, 72); // sizes are multiplied by the display scale: a huge one overflows
        }
        t.format = loaded_format(get("format"));
        let unknown: Vec<(String, String)> = keys.iter().filter(|(k, _)| !THEME_KEYS.contains(&k.as_str())).cloned().collect();
        if !unknown.is_empty() {
            t.extra.push(("theme".into(), unknown));
        }
        t.extra.extend(sections.iter().filter(|(n, _)| n != "theme").cloned());
        t
    }

    pub fn to_ini(&self) -> String {
        let mut k = vec![
            ("format".to_string(), self.format.to_string()),
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
        k.extend(self.extra.iter().filter(|(n, _)| n == "theme").flat_map(|(_, k)| k.clone()));
        let mut sections = vec![("theme".to_string(), k)];
        sections.extend(self.extra.iter().filter(|(n, _)| n != "theme").cloned());
        write_ini(&sections)
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
    pub show_hidden: bool,
    pub show_extensions: bool,
    pub single_click: bool,
    /// Reopen the last session's tabs at a plain launch.
    pub restore_tabs: bool,
    pub search_delay_ms: u32,
    pub hover_preview_ms: u32,
    /// "hover" or "always": when single-click mode underlines item names.
    pub underline_titles: String,
    pub show_recent: bool,
    pub show_frequent: bool,
    pub show_status: bool,
    pub preview_handlers: bool,
    pub item_checkboxes: bool,
    pub search_system: bool,
    pub search_archives: bool,
    /// Opt-in: once a day, ask GitHub for the latest release (see `update`).
    pub check_updates: bool,
    /// The folder tree opens the way to the current folder. None (the key absent): follow Explorer's own option.
    pub expand_to_folder: Option<bool>,
    pub bookmarks: Vec<(String, String)>,
    /// Keys this build does not read, written back as read.
    pub extra: Vec<(String, String)>,
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
            show_hidden: false,
            show_extensions: true,
            single_click: false,
            restore_tabs: false,
            search_delay_ms: 250,
            hover_preview_ms: 2000,
            underline_titles: "hover".into(),
            show_recent: true,
            show_frequent: true,
            show_status: true,
            preview_handlers: true,
            item_checkboxes: false,
            search_system: true,
            search_archives: false,
            check_updates: false,
            expand_to_folder: None,
            // The sidebar's Favorites come from Quick Access with the shell's names; bookmarks are the user's own.
            bookmarks: Vec::new(),
            extra: Vec::new(),
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
            ("show_hidden".to_string(), self.show_hidden.to_string()),
            ("show_extensions".to_string(), self.show_extensions.to_string()),
            ("single_click".to_string(), self.single_click.to_string()),
            ("restore_tabs".to_string(), self.restore_tabs.to_string()),
            ("search_delay_ms".to_string(), self.search_delay_ms.to_string()),
            ("hover_preview_ms".to_string(), self.hover_preview_ms.to_string()),
            ("underline_titles".to_string(), self.underline_titles.clone()),
            ("show_recent".to_string(), self.show_recent.to_string()),
            ("show_frequent".to_string(), self.show_frequent.to_string()),
            ("show_status".to_string(), self.show_status.to_string()),
            ("preview_handlers".to_string(), self.preview_handlers.to_string()),
            ("item_checkboxes".to_string(), self.item_checkboxes.to_string()),
            ("search_system".to_string(), self.search_system.to_string()),
            ("search_archives".to_string(), self.search_archives.to_string()),
            ("check_updates".to_string(), self.check_updates.to_string()),
        ];
        k.extend(self.expand_to_folder.map(|v| ("expand_to_folder".to_string(), v.to_string())));
        k.extend(self.bookmarks.iter().map(|(l, t)| (format!("bookmark.{l}"), t.clone())));
        k.extend(self.extra.iter().cloned());
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
                "show_hidden" => p.show_hidden = parse_bool(v).unwrap_or(p.show_hidden),
                "show_extensions" => p.show_extensions = parse_bool(v).unwrap_or(p.show_extensions),
                "single_click" => p.single_click = parse_bool(v).unwrap_or(p.single_click),
                "restore_tabs" => p.restore_tabs = parse_bool(v).unwrap_or(p.restore_tabs),
                "search_delay_ms" => p.search_delay_ms = v.parse().unwrap_or(p.search_delay_ms),
                "hover_preview_ms" => p.hover_preview_ms = v.parse().map_or(p.hover_preview_ms, |n: u32| n.clamp(200, 10000)),
                "underline_titles" => p.underline_titles = if v.trim().eq_ignore_ascii_case("always") { "always" } else { "hover" }.into(),
                "show_recent" => p.show_recent = parse_bool(v).unwrap_or(p.show_recent),
                "show_frequent" => p.show_frequent = parse_bool(v).unwrap_or(p.show_frequent),
                "show_status" => p.show_status = parse_bool(v).unwrap_or(p.show_status),
                "preview_handlers" => p.preview_handlers = parse_bool(v).unwrap_or(p.preview_handlers),
                "item_checkboxes" => p.item_checkboxes = parse_bool(v).unwrap_or(p.item_checkboxes),
                "search_system" => p.search_system = parse_bool(v).unwrap_or(p.search_system),
                "search_archives" => p.search_archives = parse_bool(v).unwrap_or(p.search_archives),
                "check_updates" => p.check_updates = parse_bool(v).unwrap_or(p.check_updates),
                "expand_to_folder" => p.expand_to_folder = parse_bool(v),
                _ => match k.strip_prefix("bookmark.") {
                    Some(label) => p.bookmarks.push((label.to_string(), v.clone())),
                    None => p.extra.push((k.clone(), v.clone())),
                },
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
    /// The `[keys]` section: `cmd.name=Ctrl+T` accelerator overrides, kept as written.
    pub keys: Vec<(String, String)>,
    /// `[general] language=`: `system`, or a code with a table (`en`, `es`, a `lang\<code>.ini`). Applies at the next start.
    pub language: String,
    /// settings.ini's; see [`loaded_format`].
    pub format: u32,
    /// What this build does not read, written back as read: unknown `[general]` keys under `general`, then other sections.
    pub extra: Vec<Section>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            active: "default".into(),
            profiles: vec![Profile::default_profile()],
            themes: vec![Theme::light(), Theme::dark()],
            keys: Vec::new(),
            language: "system".into(),
            format: FORMAT,
            extra: Vec::new(),
        }
    }
}

/// `%APPDATA%\rexplorer`, or `<temp>\rexplorer` when APPDATA is unset.
pub fn dir() -> PathBuf {
    std::env::var_os("APPDATA").map_or_else(std::env::temp_dir, PathBuf::from).join("rexplorer")
}

/// Writes a profile file whole or not at all: a temp file beside it, then a rename over it. A crash or a full disk
/// midway leaves the old file, where a plain write leaves it empty or cut short.
pub fn write_atomic(path: &Path, text: impl AsRef<[u8]>) -> std::io::Result<()> {
    // A link (a settings file kept in a dotfiles folder) is written through: a rename would replace the link.
    if path.is_symlink() {
        return std::fs::write(path, text);
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id())); // per process: two windows never share a temp file
    std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, path)).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn theme_path(dir: &Path, name: &str) -> PathBuf {
    dir.join("themes").join(format!("{name}.ini"))
}

/// The profile theme name that follows Windows' app mode.
pub const SYSTEM_THEME: &str = "system";

/// `system` follows Windows' app mode; any other name is a theme of its own.
pub fn resolve_theme_name(name: &str, apps_dark: bool) -> &str {
    if name.eq_ignore_ascii_case(SYSTEM_THEME) {
        if apps_dark { "dark" } else { "light" }
    } else {
        name
    }
}

impl Settings {
    pub fn load() -> Settings {
        Settings::load_from(&dir())
    }

    /// Missing or unreadable files yield defaults; builtin themes are always present and
    /// replaced by same-named theme files.
    pub fn load_from(dir: &Path) -> Settings {
        let mut s = Settings::from_ini(&std::fs::read_to_string(dir.join("settings.ini")).unwrap_or_default());
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

    /// settings.ini's text, with the built-in themes. `[keys]` is kept whole, so unknown `cmd.*` keys survive.
    fn from_ini(text: &str) -> Settings {
        let mut s = Settings::default();
        let sections = parse_ini(text);
        let profiles: Vec<Profile> = sections
            .iter()
            .filter_map(|(n, k)| n.strip_prefix("profile ").map(|name| Profile::from_section(name.trim(), k)))
            .collect();
        if !profiles.is_empty() {
            s.profiles = profiles;
        }
        let general: Vec<&(String, String)> = sections.iter().filter(|(n, _)| n == "general").flat_map(|(_, k)| k).collect();
        let get = |key: &str| general.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        s.active = get("profile").map_or_else(|| s.profiles[0].name.clone(), str::to_string);
        if let Some(v) = get("language") {
            s.language = v.to_string();
        }
        s.format = loaded_format(get("format"));
        s.keys = sections.iter().filter(|(n, _)| n == "keys").flat_map(|(_, k)| k.clone()).collect();

        let unknown: Vec<(String, String)> =
            general.iter().filter(|(k, _)| !matches!(k.as_str(), "profile" | "language" | "format")).map(|&kv| kv.clone()).collect();
        if !unknown.is_empty() {
            s.extra.push(("general".into(), unknown));
        }
        s.extra.extend(sections.iter().filter(|(n, _)| !matches!(n.as_str(), "general" | "keys") && !n.starts_with("profile ")).cloned());
        s
    }

    pub fn save(&self) {
        let _ = self.save_to(&dir());
    }

    /// A file from a newer format is left as it is: this build cannot write back a value it could not read.
    pub fn save_to(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir.join("themes"))?;
        if self.format <= FORMAT {
            write_atomic(&dir.join("settings.ini"), self.settings_text())?;
        }
        for t in self.themes.iter().filter(|t| t.format <= FORMAT) {
            write_atomic(&theme_path(dir, &t.name), t.to_ini())?;
        }
        Ok(())
    }

    fn settings_text(&self) -> String {
        let mut general = vec![
            ("format".to_string(), self.format.to_string()),
            ("profile".to_string(), self.active.clone()),
            ("language".to_string(), self.language.clone()),
        ];
        general.extend(self.extra.iter().filter(|(n, _)| n == "general").flat_map(|(_, k)| k.clone()));
        let mut sections = vec![("general".to_string(), general)];
        sections.extend(self.profiles.iter().map(Profile::to_section));
        if !self.keys.is_empty() {
            sections.push(("keys".to_string(), self.keys.clone()));
        }
        sections.extend(self.extra.iter().filter(|(n, _)| n != "general").cloned());
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
    /// holding an earlier version's unedited defaults gets the current defaults. settings.ini and theme files from
    /// before formats (0.1.x) are migrated in place: they get `format=` and keep everything else.
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
        // Only the folder: a copy of a built-in language here would shadow the text of every later version.
        let _ = std::fs::create_dir_all(dir.join("lang"));
        for t in &d.themes {
            let p = theme_path(dir, &t.name);
            if std::fs::read_to_string(&p).is_ok_and(|text| Theme::is_legacy_default(&text)) {
                let _ = write_atomic(&p, t.to_ini());
            }
            write_new(p, t.to_ini());
        }
        let themes = std::fs::read_dir(dir.join("themes")).into_iter().flatten().flatten().map(|e| e.path());
        for (p, section) in std::iter::once((dir.join("settings.ini"), "general"))
            .chain(themes.filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ini"))).map(|p| (p, "theme")))
        {
            if let Some(text) = std::fs::read_to_string(&p).ok().and_then(|t| stamp_format(&t, section)) {
                let _ = write_atomic(&p, text);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rexplorer-settings-{}", std::process::id())).join(tag);
        let _ = std::fs::remove_dir_all(&d);
        // Another test's cleanup can remove the shared parent between the two steps of create_dir_all: try again.
        assert!((0..5).any(|_| std::fs::create_dir_all(&d).is_ok()), "{}", d.display());
        d
    }

    fn cleanup(tag: &str) {
        let base = std::env::temp_dir().join(format!("rexplorer-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(base.join(tag));
        // Only succeeds once the last test's folder is gone, so the per-process folder does not pile up in %TEMP%.
        let _ = std::fs::remove_dir(base);
    }

    #[test]
    fn contrast_never_read_or_written() {
        assert_eq!(Theme::light().contrast, None);
        assert_eq!(Theme::from_ini("x", "[theme]
dark=false").contrast, None);
        let c = Contrast { highlight_text: 1, hot: 2, gray: 3, button_face: 4, button_text: 5 };
        let ini = Theme { contrast: Some(c), ..Theme::light() }.to_ini();
        assert!(!ini.contains("hot") && !ini.contains("contrast"), "{ini}");
        assert_eq!(Theme::from_ini("x", &ini).contrast, None);
    }

    #[test]
    fn system_theme_follows_the_app_mode() {
        assert_eq!(resolve_theme_name("system", true), "dark");
        assert_eq!(resolve_theme_name("System", false), "light");
        assert_eq!(resolve_theme_name("sepia", true), "sepia");
        assert_eq!(Settings::default().theme(resolve_theme_name("system", false)).name, "light");
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
            show_hidden: true,
            show_extensions: false,
            single_click: true,
            restore_tabs: true,
            search_delay_ms: 400,
            hover_preview_ms: 1500,
            underline_titles: "always".into(),
            show_recent: false,
            show_frequent: false,
            show_status: false,
            preview_handlers: false,
            item_checkboxes: true,
            search_system: false,
            search_archives: true,
            check_updates: true,
            expand_to_folder: Some(true),
            bookmarks: vec![("Repos".into(), r"C:\Users\x\My Repos".into())],
            ..Profile::default_profile()
        });
        s.active = "work".into();
        s.keys = vec![("cmd.new_tab".into(), "Ctrl+Y".into())];
        s.language = "es".into();
        s.save_to(&d).unwrap();
        let back = Settings::load_from(&d);
        assert_eq!(back, s);
        assert_eq!(back.active_profile().name, "work");
        assert_eq!(back.theme("nope").name, "dark");
        cleanup("save");
    }

    #[test]
    fn missing_new_keys_take_defaults() {
        let p = Profile::from_section("x", &[("show_hidden".into(), "true".into()), ("search_delay_ms".into(), "junk".into())]);
        assert!(p.show_hidden && p.show_extensions && !p.single_click);
        assert_eq!((p.search_delay_ms, p.hover_preview_ms), (250, 2000));
        assert_eq!(p.underline_titles, "hover");
        assert!(p.show_recent && p.show_frequent && p.show_status && p.preview_handlers && p.search_system);
        assert!(!p.item_checkboxes && !p.search_archives && !p.restore_tabs);
        assert!(!p.check_updates, "the update check is opt-in");
        assert!(!Profile::from_section("x", &[("check_updates".into(), "junk".into())]).check_updates);
        // Absent (or unreadable): the tree follows Explorer's "Expand to open folder"; set: the profile decides.
        assert_eq!(p.expand_to_folder, None);
        assert_eq!(Profile::from_section("x", &[("expand_to_folder".into(), "false".into())]).expand_to_folder, Some(false));
        assert!(!Profile::default_profile().to_section().1.iter().any(|(k, _)| k == "expand_to_folder"), "not written unless set");
        assert_eq!(Profile::from_section("x", &[("underline_titles".into(), "junk".into())]).underline_titles, "hover");
        let hover = |v: &str| Profile::from_section("x", &[("hover_preview_ms".into(), v.into())]).hover_preview_ms;
        assert_eq!((hover("5"), hover("1500"), hover("999999"), hover("junk")), (200, 1500, 10000, 2000));
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
        // The edited one is only stamped with the format.
        assert_eq!(std::fs::read_to_string(d.join("themes/light.ini")).unwrap(), edited.replace("[theme]\n", "[theme]\nformat=1\n"));
        cleanup("legacy");
    }

    /// settings.ini as 0.1.0 through 0.1.4 wrote it on a first run (their writers are identical).
    const SETTINGS_0_1: &str = "[general]\nprofile=default\n\n[profile default]\nstart=shell:::{F874310E-B6B7-47DC-BC84-B9E6B38F5903}\n\
        theme=dark\nview=details\nshow_tree=false\nshow_preview=false\ntree_width=220\npreview_width=360\nshow_hidden=false\n\
        show_extensions=true\nsingle_click=false\nsearch_delay_ms=250\nhover_preview_ms=2000\nunderline_titles=hover\nshow_recent=true\n\
        show_frequent=true\nshow_status=true\npreview_handlers=true\nitem_checkboxes=false\nsearch_system=true\nsearch_archives=false\n\n";
    /// A 0.1.x settings.ini after the user's own edits: a profile, a bookmark, a shortcut and a comment.
    const SETTINGS_0_1_EDITED: &str = "[profile work]\n; mine\ntheme=sepia\nview=icons\nshow_hidden=true\nbookmark.Repos=C:\\Users\\x\\My Repos\n\n\
        [keys]\ncmd.new_tab=Ctrl+Y\n";
    const DARK_0_1: &str = "[theme]\ndark=true\nmica=true\nbackground=#1E1E1E\nsurface=#262626\ntext=#F2F2F7\nsecondary_text=#98989D\n\
        accent=#0A84FF\nselection=#21374D\nborder=#2F2F2F\nfont=Segoe UI Variable Text\nfont_size=10\n\n";
    const LIGHT_0_1: &str = "[theme]\ndark=false\nmica=true\nbackground=#F5F5F7\nsurface=#FFFFFF\ntext=#1D1D1F\nsecondary_text=#6E6E73\n\
        accent=#0A84FF\nselection=#D3E9FF\nborder=#E4E4E6\nicon_theme=fluent\nfont=Segoe UI Variable Text\nfont_size=10\n\n";

    #[test]
    fn v0_1_files_migrate_and_round_trip() {
        assert_eq!(Settings::from_ini(SETTINGS_0_1), Settings::default());
        let s = Settings::from_ini(&format!("{SETTINGS_0_1}{SETTINGS_0_1_EDITED}").replace("profile=default", "profile=work"));
        let work = s.active_profile();
        assert_eq!((work.name.as_str(), work.theme.as_str(), work.view.as_str(), work.show_hidden), ("work", "sepia", "icons", true));
        assert_eq!(work.bookmarks, vec![("Repos".to_string(), r"C:\Users\x\My Repos".to_string())]);
        assert!(!work.restore_tabs && work.extra.is_empty(), "keys added after 0.1.x take their defaults");
        assert_eq!((s.format, s.language.as_str(), s.keys.len(), s.extra.len()), (FORMAT, "system", 1, 0));
        let text = s.settings_text();
        assert!(text.starts_with("[general]\nformat=1\n"), "{text}");
        assert_eq!(Settings::from_ini(&text), s);

        assert_eq!(Theme::from_ini("dark", DARK_0_1), Theme::dark());
        let light = Theme::from_ini("light", LIGHT_0_1);
        assert_eq!(light, Theme { icon_theme: Some("fluent".into()), ..Theme::light() });
        assert!(light.to_ini().starts_with("[theme]\nformat=1\n"));
        assert_eq!(Theme::from_ini("light", &light.to_ini()), light);
    }

    #[test]
    fn unknown_keys_and_sections_survive() {
        let text = "[general]\nformat=1\nprofile=default\nlanguage=es\nwindow=max\n\n[profile default]\nview=list\nzoom=3\n\n\
            [keys]\ncmd.future=Ctrl+J\n\n[plugins]\nx=1\n";
        let s = Settings::from_ini(text);
        assert_eq!(s.active_profile().extra, vec![("zoom".to_string(), "3".to_string())]);
        assert_eq!(s.keys, vec![("cmd.future".to_string(), "Ctrl+J".to_string())]);
        let out = s.settings_text();
        for line in ["window=max", "zoom=3", "cmd.future=Ctrl+J", "[plugins]\nx=1"] {
            assert!(out.contains(line), "{line} in {out}");
        }
        assert_eq!(Settings::from_ini(&out), s);

        let t = Theme::from_ini("x", "top=1\n[theme]\naccent=#010203\nglass=0.5\n[extra]\nk=v\n");
        let ini = t.to_ini();
        assert!(ini.starts_with("top=1\n") && ini.contains("glass=0.5") && ini.contains("[extra]\nk=v"), "{ini}");
        assert_eq!(Theme::from_ini("x", &ini), t);
    }

    #[test]
    fn a_theme_names_an_icon_folder_never_a_path_and_keeps_a_sane_font_size() {
        let icon = |v: &str| Theme::from_ini("x", &format!("[theme]\nicon_theme={v}\n")).icon_theme;
        assert_eq!(icon("fluent").as_deref(), Some("fluent"));
        for bad in ["", r"\\host\share", r"..\..\x", "..", r"C:\icons", "a/b", "C:x"] {
            assert_eq!(icon(bad), None, "{bad}");
        }
        let size = |v: &str| Theme::from_ini("x", &format!("[theme]\nfont_size={v}\n")).font_size;
        assert_eq!((size("2000000000"), size("-4"), size("12"), size("junk")), (72, 6, 12, Theme::from_ini("x", "").font_size));
    }

    #[test]
    fn a_profile_write_replaces_the_file_and_leaves_no_temp_file() {
        let d = tmp("atomic");
        let f = d.join("a.txt");
        write_atomic(&f, "one").unwrap();
        write_atomic(&f, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "two");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
        // A write that cannot land (the target is a folder) keeps what was there and removes its temp file.
        std::fs::create_dir(d.join("dir")).unwrap();
        assert!(write_atomic(&d.join("dir"), "x").is_err());
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 2);
        cleanup("atomic");
    }

    #[test]
    fn newer_formats_are_read_and_not_overwritten() {
        assert_eq!((loaded_format(None), loaded_format(Some("junk")), loaded_format(Some("0")), loaded_format(Some(" 7 "))), (1, 1, 1, 7));
        let d = tmp("newer");
        std::fs::create_dir_all(d.join("themes")).unwrap();
        let ini = "[general]\nformat=7\nprofile=default\n[profile default]\nview=list\nfont_size=10.5\n";
        let theme = "[theme]\nformat=7\naccent=#010203\nfont_size=10.5\n";
        std::fs::write(d.join("settings.ini"), ini).unwrap();
        std::fs::write(d.join("themes/sepia.ini"), theme).unwrap();
        let s = Settings::load_from(&d);
        assert_eq!((s.format, s.active_profile().view.as_str(), s.theme("sepia").format), (7, "list", 7));
        assert_eq!(s.theme("sepia").accent, 0x0003_0201);
        s.save_to(&d).unwrap();
        Settings::ensure_files(&d);
        assert_eq!(std::fs::read_to_string(d.join("settings.ini")).unwrap(), ini);
        assert_eq!(std::fs::read_to_string(d.join("themes/sepia.ini")).unwrap(), theme);
        cleanup("newer");
    }

    #[test]
    fn stamping_keeps_the_rest_of_the_file() {
        assert_eq!(stamp_format("\u{feff}[general]\r\n; note\r\nprofile=a\r\n", "general").unwrap(), "\u{feff}[general]\r\nformat=1\r\n; note\r\nprofile=a\r\n");
        assert_eq!(stamp_format("[profile a]\nview=list\n", "general").unwrap(), "[profile a]\nview=list\n[general]\nformat=1\n");
        assert_eq!(stamp_format("", "theme").unwrap(), "[theme]\nformat=1\n");
        assert_eq!(stamp_format("[theme]\nformat=2\n", "theme"), None);
    }

    #[test]
    fn ensure_files_stamps_old_files_once() {
        let d = tmp("stamp");
        std::fs::create_dir_all(d.join("themes")).unwrap();
        std::fs::write(d.join("settings.ini"), SETTINGS_0_1).unwrap();
        std::fs::write(d.join("themes/sepia.ini"), LIGHT_0_1).unwrap();
        Settings::ensure_files(&d);
        let once = std::fs::read_to_string(d.join("settings.ini")).unwrap();
        assert!(once.starts_with("[general]\nformat=1\nprofile=default\n"), "{once}");
        assert!(std::fs::read_to_string(d.join("themes/sepia.ini")).unwrap().starts_with("[theme]\nformat=1\n"));
        Settings::ensure_files(&d);
        assert_eq!(std::fs::read_to_string(d.join("settings.ini")).unwrap(), once);
        assert_eq!(Settings::load_from(&d).profiles, Settings::from_ini(SETTINGS_0_1).profiles);
        cleanup("stamp");
    }

    #[test]
    fn language_defaults_to_system() {
        let d = tmp("lang");
        assert_eq!(Settings::default().language, "system");
        assert_eq!(Settings::load_from(&d).language, "system");
        std::fs::write(d.join("settings.ini"), "[general]
language=es
").unwrap();
        assert_eq!(Settings::load_from(&d).language, "es");
        cleanup("lang");
    }

    #[test]
    fn ensure_files_leaves_languages_to_the_user() {
        let d = tmp("langfiles");
        Settings::ensure_files(&d);
        // A copy written here would shadow the built-in text of every later version.
        assert!(d.join("lang").is_dir());
        assert!(!d.join("lang/en.ini").exists() && !d.join("lang/es.ini").exists());
        std::fs::write(d.join("lang/es.ini"), "x").unwrap();
        Settings::ensure_files(&d);
        assert_eq!(std::fs::read_to_string(d.join("lang/es.ini")).unwrap(), "x");
        cleanup("langfiles");
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

    // Fuzz-style: seeded junk through the pure parsers. No panic, and the documented invariants hold.
    #[test]
    fn fuzz_parse_ini_trims_and_splits_on_the_first_equals() {
        for seed in 0..400 {
            fuzz_check(&crate::fuzz::Rng::new(seed).text(crate::fuzz::TOKENS, 80));
        }
    }

    #[test]
    fn fuzz_colors() {
        let mut r = crate::fuzz::Rng::new(7);
        for _ in 0..500 {
            let c = (r.step() & 0xFF_FFFF) as u32;
            assert_eq!(parse_color(&format_color(c)), Some(c));
            fuzz_check(&r.text(crate::fuzz::TOKENS, 4));
        }
    }
}
