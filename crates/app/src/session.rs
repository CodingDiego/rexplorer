//! Persisted session (open tabs, active tab, view, pane widths) for startup,
//! crash recovery and the --recover flag. Plain `key=value` lines.

use rexplorer_core::settings::{loaded_format, FORMAT};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub tabs: Vec<String>,
    pub active: usize,
    pub view: String,
    /// The folder tree, shown instead of the sidebar.
    pub show_tree: bool,
    /// The places sidebar (when the tree is not shown).
    pub show_sidebar: bool,
    pub show_preview: bool,
    /// The details pane (never with the preview).
    pub show_details: bool,
    pub tree_width: i32,
    pub preview_width: i32,
    /// What each folder remembers, most recent first; kept in folders.txt beside session.txt.
    pub folders: Vec<(String, FolderView)>,
    /// session.txt's; see `rexplorer_core::settings::loaded_format`.
    pub format: u32,
    /// `key=value` lines this build does not read, written back as read.
    pub extra: Vec<(String, String)>,
    /// The lines of a folders.txt from a newer format that this build does not read, its `format=` line first.
    pub folders_extra: Vec<String>,
}

/// One folder's remembered view, sort, grouping and columns.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderView {
    pub view: String,
    /// Shell column and descending.
    pub sort: (u32, bool),
    pub group: Option<u32>,
    /// The shown columns in display order: shell index and width in 96-DPI pixels (0 = the shell's default). Empty
    /// means the folder's default columns.
    pub cols: Vec<(u32, i32)>,
}

const MAX_FOLDERS: usize = 500;
const MAX_COLS: usize = 32;
const MAX_LINE: usize = 2048;

impl Session {
    /// Stores `fv` for the folder as the most recent, dropping the oldest past the cap.
    pub fn remember(&mut self, key: &str, fv: FolderView) {
        self.folders.retain(|(k, _)| !k.eq_ignore_ascii_case(key));
        self.folders.insert(0, (key.to_string(), fv));
        self.folders.truncate(MAX_FOLDERS);
    }
}

/// A `format=` line, then one folder per line, tab-separated, the name last (paths may hold anything but a tab or
/// newline): `view sort-col desc(0|1) group-col(or -) idx:width,idx:width,... name`. An FTP password never goes in.
/// `extra` (from [`parse_folders`]) goes last, and its own `format=` line replaces this build's.
pub fn format_folders(f: &[(String, FolderView)], extra: &[String]) -> String {
    let mut out = if extra.is_empty() { format!("format={FORMAT}\n") } else { String::new() };
    let clean = |t: &str| !t.contains(['\t', '\r', '\n']);
    for (key, v) in f.iter().filter(|(k, v)| !k.is_empty() && clean(k) && clean(&v.view) && crate::connect::redact(k) == *k).take(MAX_FOLDERS) {
        let cols: Vec<String> = v.cols.iter().take(MAX_COLS).map(|(i, w)| format!("{i}:{w}")).collect();
        let group = v.group.map_or("-".to_string(), |g| g.to_string());
        out.push_str(&format!("{}\t{}\t{}\t{}\t{}\t{}\n", v.view, v.sort.0, v.sort.1 as u8, group, cols.join(","), key));
    }
    for line in extra {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Reads `format_folders` text: the folders, and the lines to keep. A line that is too long, short or has a bad number
/// is skipped, unless the file is from a newer format: then it is kept, with the `format=` line, for `format_folders`.
pub fn parse_folders(text: &str) -> (Vec<(String, FolderView)>, Vec<String>) {
    let mut out: Vec<(String, FolderView)> = Vec::new();
    let (mut version, mut unread) = (None, Vec::new());
    for line in text.lines().filter(|l| l.len() <= MAX_LINE) {
        if let Some(v) = line.strip_prefix("format=") {
            version = Some(v);
            continue;
        }
        if out.len() >= MAX_FOLDERS {
            break;
        }
        let f: Vec<&str> = line.splitn(6, '\t').collect();
        let [view, col, desc, group, cols, key] = f[..] else {
            unread.push(line);
            continue;
        };
        let (Ok(col), true) = (col.parse::<u32>(), matches!(desc, "0" | "1")) else {
            unread.push(line);
            continue;
        };
        let group = match group {
            "-" => Some(None),
            g => g.parse().ok().map(Some),
        };
        let cols: Option<Vec<(u32, i32)>> = cols
            .split(',')
            .filter(|c| !c.is_empty())
            .map(|c| c.split_once(':').and_then(|(i, w)| Some((i.parse().ok()?, w.parse::<i32>().ok()?.clamp(0, 4000)))))
            .collect();
        let (Some(group), Some(mut cols)) = (group, cols) else {
            unread.push(line);
            continue;
        };
        if key.is_empty() || out.iter().any(|(k, _)| k.eq_ignore_ascii_case(key)) {
            continue;
        }
        cols.truncate(MAX_COLS);
        out.push((key.to_string(), FolderView { view: view.to_string(), sort: (col, desc == "1"), group, cols }));
    }
    let format = loaded_format(version);
    let keep = std::iter::once(format!("format={format}")).chain(unread.into_iter().take(MAX_FOLDERS).map(str::to_string));
    (out, if format > FORMAT { keep.collect() } else { Vec::new() })
}

pub fn folders_path() -> PathBuf {
    path().with_file_name("folders.txt")
}

impl Default for Session {
    fn default() -> Self {
        Session {
            tabs: Vec::new(),
            active: 0,
            view: "details".to_string(),
            show_tree: false,
            show_sidebar: true,
            show_preview: false,
            show_details: false,
            tree_width: 220,
            preview_width: 360,
            folders: Vec::new(),
            format: FORMAT,
            extra: Vec::new(),
            folders_extra: Vec::new(),
        }
    }
}

pub fn parse(text: &str) -> Session {
    let mut s = Session::default();
    let (mut version, mut has_sidebar) = (None, false);
    for line in text.lines() {
        let Some((k, v)) = line.trim_end_matches('\r').split_once('=') else { continue };
        match k {
            "format" => version = Some(v),
            "tab" => {
                if !v.is_empty() {
                    s.tabs.push(v.to_string())
                }
            }
            "active" => s.active = v.trim().parse().unwrap_or(0),
            "view" => {
                if !v.is_empty() {
                    s.view = v.to_string()
                }
            }
            "tree" => s.show_tree = v.trim() != "0",
            "sidebar" => {
                s.show_sidebar = v.trim() != "0";
                has_sidebar = true;
            }
            "preview" => s.show_preview = v.trim() != "0",
            "details" => s.show_details = v.trim() != "0",
            "tree_width" => s.tree_width = v.trim().parse().unwrap_or(s.tree_width),
            "preview_width" => s.preview_width = v.trim().parse().unwrap_or(s.preview_width),
            _ => s.extra.push((k.to_string(), v.to_string())),
        }
    }
    // Format 0 (no `format=`): one from before the sidebar had the tree on by default; it gets the sidebar instead.
    if version.is_none() && !has_sidebar {
        s.show_tree = false;
    }
    s.format = loaded_format(version);
    s.show_details &= !s.show_preview;
    s.active = s.active.min(s.tabs.len().saturating_sub(1));
    s.tree_width = s.tree_width.clamp(120, 1200);
    s.preview_width = s.preview_width.clamp(120, 1200);
    s
}

pub fn format(s: &Session) -> String {
    let mut out = format!("format={}\n", s.format);
    // A name from an FTP server can hold a line break: written as is it would add lines (another tab=, opened at
    // the next start) to this file.
    let kept: Vec<usize> = (0..s.tabs.len()).filter(|&i| !s.tabs[i].contains(['\r', '\n'])).collect();
    for &i in &kept {
        out.push_str(&format!("tab={}\n", crate::connect::redact(&s.tabs[i])));
    }
    // The active tab among those written; when it is the one skipped, the nearest one.
    let active = kept.iter().enumerate().min_by_key(|(_, i)| i.abs_diff(s.active)).map_or(0, |(n, _)| n);
    out.push_str(&format!(
        "active={}\nview={}\ntree={}\nsidebar={}\npreview={}\ndetails={}\ntree_width={}\npreview_width={}\n",
        active, s.view, s.show_tree as u8, s.show_sidebar as u8, s.show_preview as u8, s.show_details as u8, s.tree_width, s.preview_width
    ));
    for (k, v) in &s.extra {
        out.push_str(&format!("{k}={v}\n"));
    }
    out
}

/// Whether startup reopens the saved tabs: always after a crash; with restore_tabs only for a plain launch of the first window.
pub fn restores(recover: bool, restore_tabs: bool, new_window: bool, names_something: bool) -> bool {
    recover || (restore_tabs && !new_window && !names_something)
}

/// The saved tabs for a recovery start: one whose parsing name (`parsing`) is among the places that were loading
/// (`loading`, one per line) opens `home` instead.
pub fn recover_tabs(saved: Vec<String>, loading: &str, parsing: impl Fn(&str) -> String, home: &str) -> Vec<String> {
    saved
        .into_iter()
        .map(|n| {
            let p = parsing(&n);
            if loading.lines().any(|l| !l.is_empty() && l.eq_ignore_ascii_case(&p)) { home.to_string() } else { n }
        })
        .collect()
}

pub fn path() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("rexplorer").join("session.txt")
}

pub fn load() -> Session {
    let mut s = load_from(&path());
    (s.folders, s.folders_extra) = std::fs::read_to_string(folders_path()).map(|t| parse_folders(&t)).unwrap_or_default();
    s
}

pub fn save(s: &Session) {
    let p = path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = save_to(&p, s);
    let _ = rexplorer_core::settings::write_atomic(&folders_path(), format_folders(&s.folders, &s.folders_extra));
}

pub fn load_from(p: &Path) -> Session {
    std::fs::read_to_string(p).map(|t| parse(&t)).unwrap_or_default()
}

pub fn save_to(p: &Path, s: &Session) -> std::io::Result<()> {
    rexplorer_core::settings::write_atomic(p, format(s))
}

pub struct Debounce {
    min: Duration,
    last: Option<Instant>,
}

impl Debounce {
    pub fn new(min_interval: Duration) -> Debounce {
        Debounce { min: min_interval, last: None }
    }

    pub fn should_save(&mut self, now: Instant) -> bool {
        if self.last.is_none_or(|l| now.saturating_duration_since(l) >= self.min) {
            self.last = Some(now);
            true
        } else {
            false
        }
    }

    /// How long until `should_save` says yes again.
    pub fn remaining(&self, now: Instant) -> Duration {
        self.last.map_or(Duration::ZERO, |l| self.min.saturating_sub(now.saturating_duration_since(l)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Session {
        Session {
            tabs: vec!["C:/a".into(), "::{GUID}/x=y".into()],
            active: 1,
            view: "icons".into(),
            show_tree: true,
            show_sidebar: false,
            show_preview: true,
            show_details: false,
            tree_width: 300,
            preview_width: 500,
            ..Session::default()
        }
    }

    #[test]
    fn restores_rules() {
        assert!(restores(true, false, false, false));
        assert!(restores(true, false, false, true), "recover is unchanged");
        assert!(restores(false, true, false, false));
        assert!(!restores(false, true, true, false), "new window");
        assert!(!restores(false, true, false, true), "paths given");
        assert!(!restores(false, false, false, false));
    }

    #[test]
    fn recovery_opens_home_for_the_tabs_that_were_loading() {
        let tabs = |v: &[&str]| v.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        let parsing = |n: &str| if n == "shell:Downloads" { r"C:\D".to_string() } else { n.to_string() };
        let saved = tabs(&[r"C:\a", r"c:\CRASH", "shell:Downloads", "ftp://u@h/x"]);
        let got = recover_tabs(saved.clone(), "C:\\crash\nC:\\D\nftp://u@h/x\n", parsing, "HOME");
        assert_eq!(got, tabs(&[r"C:\a", "HOME", "HOME", "HOME"]), "by parsing name, any case");
        assert_eq!(recover_tabs(saved.clone(), "", parsing, "HOME"), saved, "nothing was loading");
        assert_eq!(recover_tabs(tabs(&[""]), "\n\n", parsing, "HOME"), tabs(&[""]), "an empty line matches nothing");
    }

    #[test]
    fn round_trip_with_equals_in_tab() {
        let s = sample();
        assert_eq!(parse(&format(&s)), s);
    }

    #[test]
    fn details_flag_round_trip_and_exclusion() {
        let s = Session { show_details: true, show_preview: false, ..sample() };
        assert_eq!(parse(&format(&s)), s);
        let both = parse("preview=1\ndetails=1\n");
        assert!(both.show_preview && !both.show_details);
        assert!(parse("details=1\n").show_details);
        assert!(!parse("preview=1\n").show_details, "an old file without the key");
    }

    #[test]
    fn ftp_passwords_are_not_persisted() {
        let s = Session { tabs: vec!["ftp://u:pw@h/x".into()], ..Session::default() };
        assert!(!format(&s).contains("pw"));
        assert_eq!(parse(&format(&s)).tabs, vec!["ftp://u@h/x".to_string()]);
    }

    #[test]
    fn a_tab_with_a_line_break_adds_no_lines() {
        let s = Session { tabs: vec!["ftp://h/a\ntab=\\\\attacker\\share\nview=x".into(), "C:\\b".into()], ..Session::default() };
        let back = parse(&format(&s));
        assert_eq!((back.tabs, back.view.as_str()), (vec!["C:\\b".to_string()], "details"));
        // The active tab keeps its place among the tabs written, and a skipped active one gives way to the nearest.
        let tabs = |v: &[&str]| v.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        let s = Session { tabs: tabs(&["C:\\a", "ftp://h/x\ny", "C:\\c", "C:\\d"]), active: 2, ..Session::default() };
        let back = parse(&format(&s));
        assert_eq!((back.tabs[back.active].as_str(), back.tabs.len()), ("C:\\c", 3));
        let s = Session { active: 1, ..s };
        assert_eq!(parse(&format(&s)).active, 0, "C:\\a, before the skipped one");
        let s = Session { tabs: tabs(&["ftp://h/x\ny", "C:\\b"]), active: 0, ..Session::default() };
        assert_eq!(parse(&format(&s)).active, 0, "C:\\b, after it");
    }

    #[test]
    fn clamping_and_unknown_lines() {
        let s = parse("junk\nfoo=bar\ntab=a\ntab=b\nactive=9\ntree_width=5\npreview_width=99999\n");
        assert_eq!(s.active, 1);
        assert_eq!(s.tree_width, 120);
        assert_eq!(s.preview_width, 1200);
        assert_eq!(parse("active=4").active, 0);
        assert_eq!(s.extra, vec![("foo".to_string(), "bar".to_string())]);
        assert!(format(&s).ends_with("foo=bar\n"));
        assert_eq!(parse(&format(&s)), s);
    }

    /// session.txt as 0.1.0 through 0.1.4 wrote it (their writers are identical).
    const SESSION_0_1: &str = "tab=C:\\Users\\x\ntab=::{20D04FE0-3AEA-1069-A2D8-08002B30309D}\nactive=1\nview=icons\ntree=1\nsidebar=0\npreview=1\n\
        tree_width=300\npreview_width=500\n";

    #[test]
    fn v0_1_session_migrates_and_round_trips() {
        let s = parse(SESSION_0_1);
        assert_eq!((s.tabs.len(), s.active, s.view.as_str(), s.show_tree, s.show_sidebar), (2, 1, "icons", true, false));
        assert_eq!((s.show_preview, s.show_details, s.tree_width, s.preview_width, s.format), (true, false, 300, 500, FORMAT));
        assert!(s.extra.is_empty());
        let text = format(&s);
        assert!(text.starts_with("format=1\ntab="), "{text}");
        assert_eq!(parse(&text), s);
        // Only a format 0 file gets the sidebar migration.
        assert!(parse("format=1\ntree=1\n").show_tree);
    }

    #[test]
    fn newer_session_keeps_its_format_and_unknown_keys() {
        let s = parse("format=4\ntab=C:\\a\ntree=1\nsidebar=0\nribbon=1\nlayout=a=b\n");
        assert_eq!((s.format, s.show_tree, s.tabs.len()), (4, true, 1));
        let text = format(&s);
        assert!(text.starts_with("format=4\n") && text.contains("ribbon=1\n") && text.contains("layout=a=b\n"), "{text}");
        assert_eq!(parse(&text), s);
    }

    #[test]
    fn newer_folders_lines_are_kept() {
        let f = vec![("C:\\a".to_string(), fv("icons", (0, false), None, &[]))];
        let text = format!("format=3\n{}new-record\n", format_folders(&f, &[]).trim_start_matches("format=1\n"));
        let (back, extra) = parse_folders(&text);
        assert_eq!(back, f);
        assert_eq!(extra, vec!["format=3".to_string(), "new-record".to_string()]);
        let again = format_folders(&back, &extra);
        assert!(!again.contains("format=1") && again.ends_with("format=3\nnew-record\n"), "{again}");
        assert_eq!(parse_folders(&again), (back, extra));
        // Junk in this build's format is dropped, not kept.
        assert_eq!(parse_folders("format=1\njunk\n").1, Vec::<String>::new());
        assert_eq!(parse_folders("junk\n").1, Vec::<String>::new());
    }

    #[test]
    fn sessions_before_the_sidebar_get_it() {
        let old = parse("tab=C:\\Windows\nactive=0\nview=details\ntree=1\npreview=0\ntree_width=240\n");
        assert!(!old.show_tree && old.show_sidebar);
        assert_eq!(old.tree_width, 240);
        let new = parse("tree=1\nsidebar=0\n");
        assert!(new.show_tree && !new.show_sidebar);
    }

    #[test]
    fn missing_file_is_default_and_save_load_equal() {
        let dir = std::env::temp_dir().join(format!("rexplorer-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("session.txt");
        assert_eq!(load_from(&p), Session::default());
        save_to(&p, &sample()).unwrap();
        assert_eq!(load_from(&p), sample());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn fv(view: &str, sort: (u32, bool), group: Option<u32>, cols: &[(u32, i32)]) -> FolderView {
        FolderView { view: view.into(), sort, group, cols: cols.to_vec() }
    }

    #[test]
    fn folders_round_trip() {
        let f = vec![
            ("C:\\a=b".to_string(), fv("icons", (3, true), Some(3), &[(0, 300), (3, 120)])),
            ("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}\\x".to_string(), fv("details", (0, false), None, &[])),
        ];
        assert!(format_folders(&f, &[]).starts_with("format=1\n"));
        assert_eq!(parse_folders(&format_folders(&f, &[])), (f, Vec::new()));
    }

    #[test]
    fn folders_keep_new_view_names() {
        let f = vec![
            ("C:\\a".to_string(), fv("list", (0, false), None, &[])),
            ("C:\\b".to_string(), fv("tiles", (2, true), Some(2), &[(0, 280)])),
            ("C:\\c".to_string(), fv("content", (0, false), None, &[])),
        ];
        assert_eq!(parse_folders(&format_folders(&f, &[])).0, f);
    }

    #[test]
    fn folders_bounded_and_bad_lines_skipped() {
        let mut s = Session::default();
        for i in 0..600 {
            s.remember(&format!("C:\\f{i}"), fv("details", (0, false), None, &[]));
        }
        assert_eq!((s.folders.len(), s.folders[0].0.as_str()), (500, "C:\\f599"));
        s.remember("C:\\A", fv("icons", (0, false), None, &[]));
        s.remember("c:\\a", fv("details", (0, false), None, &[]));
        assert_eq!(s.folders.iter().filter(|(k, _)| k.eq_ignore_ascii_case("C:\\a")).count(), 1);
        assert_eq!(s.folders[0].1.view, "details");
        assert!(parse_folders("junk\nicons\tx\t1\t-\t\tC:\\a\n").0.is_empty());
        let cols = (0..40).map(|i| format!("{i}:99999")).collect::<Vec<_>>().join(",");
        let p = parse_folders(&format!("icons\t0\t0\t-\t{cols}\tC:\\a\n")).0;
        assert_eq!((p[0].1.cols.len(), p[0].1.cols[0].1), (32, 4000));
        assert!(parse_folders(&format!("icons\t0\t0\t-\t\tC:\\{}\n", "x".repeat(3000))).0.is_empty());
        let ftp = [("ftp://u:pw@h/x".to_string(), fv("icons", (0, false), None, &[]))];
        assert!(!format_folders(&ftp, &[]).contains("pw"));
    }

    #[test]
    fn folders_file_round_trip() {
        let dir = std::env::temp_dir().join(format!("rexplorer-folders-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("folders.txt");
        let read = || std::fs::read_to_string(&p).map(|t| parse_folders(&t).0).unwrap_or_default();
        assert!(read().is_empty());
        let f = vec![("C:\\a".to_string(), fv("small", (2, true), Some(2), &[(0, 280), (2, 90)]))];
        std::fs::write(&p, format_folders(&f, &[])).unwrap();
        assert_eq!(read(), f);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn debounce_timing() {
        let t0 = Instant::now();
        let mut d = Debounce::new(Duration::from_secs(2));
        assert!(d.should_save(t0));
        assert!(!d.should_save(t0 + Duration::from_secs(1)));
        assert!(d.should_save(t0 + Duration::from_secs(2)));
        assert!(!d.should_save(t0 + Duration::from_secs(3)));
        assert_eq!(d.remaining(t0 + Duration::from_secs(3)), Duration::from_secs(1));
        assert_eq!(d.remaining(t0 + Duration::from_secs(9)), Duration::ZERO);
    }

    // Fuzz-style: seeded junk through the session and folders parsers (a tiny xorshift, no dependency).
    fn junk(seed: u64, max: u64) -> String {
        const T: &[&str] = &[
            "tab=", "active=", "view=", "tree_width=", "preview_width=", "preview=", "details=", "format=", "=", "\n", "\r\n", "\t", "-", "0", "1", "9",
            "99999999999999999999", "-5", "details", "C:\\a", "0:100,", "1:99999", ":", ",", "x", "\u{e9}",
        ];
        let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let n = next() % (max + 1);
        (0..n).map(|_| T[(next() % T.len() as u64) as usize]).collect()
    }

    #[test]
    fn fuzz_session_file_keeps_its_clamps() {
        for seed in 0..500 {
            let s = parse(&junk(seed, 120));
            assert!(s.active < s.tabs.len().max(1), "seed {seed}");
            assert!((120..=1200).contains(&s.tree_width) && (120..=1200).contains(&s.preview_width), "seed {seed}");
            assert!(!(s.show_details && s.show_preview), "seed {seed}");
            assert_eq!(format(&parse(&format(&s))), format(&s), "seed {seed}");
        }
    }

    #[test]
    fn hostile_session_and_folders_files() {
        let big = "9".repeat(40);
        let lone = String::from_utf16_lossy(&[0xD800, 0x61]);
        for t in [
            "a".repeat(1 << 20),
            "=".repeat(100_000),
            format!("tab={}\nactive={big}\ntree_width={big}\npreview_width=-{big}\nformat={big}", "t".repeat(1 << 20)),
            format!("\0=\0\ntab=\0\n{lone}={lone}"),
            format!("icons\t{big}\t1\t-\t0:100\tC:\\a\nicons\t3\t1\t{big}\t{big}:{big}\tC:\\b\nformat={big}"),
            format!("icons\t3\t1\t-\t{}\tC:\\a", "1:5,".repeat(100_000)),
            "\t".repeat(100_000),
        ] {
            let s = parse(&t);
            assert!(s.active < s.tabs.len().max(1) && (120..=1200).contains(&s.tree_width) && (120..=1200).contains(&s.preview_width));
            assert_eq!(format(&parse(&format(&s))), format(&s));
            let (f, extra) = parse_folders(&t);
            assert!(f.len() <= MAX_FOLDERS && f.iter().all(|(_, v)| v.cols.len() <= MAX_COLS));
            let out = format_folders(&f, &extra);
            assert_eq!(format_folders(&parse_folders(&out).0, &extra), out);
        }
    }

    #[test]
    fn fuzz_folders_file_keeps_its_caps() {
        let mut parsed = 0;
        for seed in 0..500 {
            // Pure junk, plus valid lines with one field swapped for junk so the number checks and caps are reached.
            let mut lines = vec![junk(seed, 120)];
            for l in 0..8 {
                let k = seed * 8 + l;
                let mut f = ["icons".to_string(), "3".into(), "1".into(), "-".into(), "0:100,1:99999".into(), ["C:\\a", "c:\\A", "C:\\b"][(k % 3) as usize].into()];
                if let Some(x) = f.get_mut((k % 7) as usize) {
                    *x = junk(k, 3);
                }
                lines.push(f.join("\t"));
            }
            let (f, _) = parse_folders(&lines.join("\n"));
            assert!(f.len() <= MAX_FOLDERS, "seed {seed}");
            for (i, (k, v)) in f.iter().enumerate() {
                assert!(!k.is_empty() && v.cols.len() <= MAX_COLS && v.cols.iter().all(|c| (0..=4000).contains(&c.1)), "seed {seed}");
                assert!(f[..i].iter().all(|(o, _)| !o.eq_ignore_ascii_case(k)), "seed {seed}: duplicate {k:?}");
            }
            // format_folders drops keys it cannot write (a tab in the key), so compare the written form.
            let out = format_folders(&f, &[]);
            assert_eq!(format_folders(&parse_folders(&out).0, &[]), out, "seed {seed}");
            parsed += f.len();
        }
        assert!(parsed > 500, "the valid lines must reach the caps: {parsed}");
    }
}
