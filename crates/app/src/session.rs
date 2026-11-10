//! Persisted session (open tabs, active tab, view, pane widths) for startup,
//! crash recovery and the --recover flag. Plain `key=value` lines.

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

/// One folder per line, tab-separated, the name last (paths may hold anything but a tab or newline):
/// `view sort-col desc(0|1) group-col(or -) idx:width,idx:width,... name`. An FTP password never goes in.
pub fn format_folders(f: &[(String, FolderView)]) -> String {
    let mut out = String::new();
    let clean = |t: &str| !t.contains(['\t', '\r', '\n']);
    for (key, v) in f.iter().filter(|(k, v)| !k.is_empty() && clean(k) && clean(&v.view) && crate::connect::redact(k) == *k).take(MAX_FOLDERS) {
        let cols: Vec<String> = v.cols.iter().take(MAX_COLS).map(|(i, w)| format!("{i}:{w}")).collect();
        let group = v.group.map_or("-".to_string(), |g| g.to_string());
        out.push_str(&format!("{}\t{}\t{}\t{}\t{}\t{}\n", v.view, v.sort.0, v.sort.1 as u8, group, cols.join(","), key));
    }
    out
}

/// Reads `format_folders` text; a line that is too long, short or has a bad number is skipped.
pub fn parse_folders(text: &str) -> Vec<(String, FolderView)> {
    let mut out: Vec<(String, FolderView)> = Vec::new();
    for line in text.lines().filter(|l| l.len() <= MAX_LINE) {
        if out.len() >= MAX_FOLDERS {
            break;
        }
        let f: Vec<&str> = line.splitn(6, '\t').collect();
        let [view, col, desc, group, cols, key] = f[..] else { continue };
        let (Ok(col), true) = (col.parse::<u32>(), matches!(desc, "0" | "1")) else { continue };
        let group = match group {
            "-" => Some(None),
            g => g.parse().ok().map(Some),
        };
        let cols: Option<Vec<(u32, i32)>> = cols
            .split(',')
            .filter(|c| !c.is_empty())
            .map(|c| c.split_once(':').and_then(|(i, w)| Some((i.parse().ok()?, w.parse::<i32>().ok()?.clamp(0, 4000)))))
            .collect();
        let (Some(group), Some(mut cols)) = (group, cols) else { continue };
        if key.is_empty() || out.iter().any(|(k, _)| k.eq_ignore_ascii_case(key)) {
            continue;
        }
        cols.truncate(MAX_COLS);
        out.push((key.to_string(), FolderView { view: view.to_string(), sort: (col, desc == "1"), group, cols }));
    }
    out.truncate(MAX_FOLDERS);
    out
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
        }
    }
}

pub fn parse(text: &str) -> Session {
    let mut s = Session::default();
    let mut has_sidebar = false;
    for line in text.lines() {
        let Some((k, v)) = line.trim_end_matches('\r').split_once('=') else { continue };
        match k {
            "tab" if !v.is_empty() => s.tabs.push(v.to_string()),
            "active" => s.active = v.trim().parse().unwrap_or(0),
            "view" if !v.is_empty() => s.view = v.to_string(),
            "tree" => s.show_tree = v.trim() != "0",
            "sidebar" => {
                s.show_sidebar = v.trim() != "0";
                has_sidebar = true;
            }
            "preview" => s.show_preview = v.trim() != "0",
            "details" => s.show_details = v.trim() != "0",
            "tree_width" => s.tree_width = v.trim().parse().unwrap_or(s.tree_width),
            "preview_width" => s.preview_width = v.trim().parse().unwrap_or(s.preview_width),
            _ => {}
        }
    }
    // A session from before the sidebar had the tree on by default: it gets the sidebar instead.
    if !has_sidebar {
        s.show_tree = false;
    }
    s.show_details &= !s.show_preview;
    s.active = s.active.min(s.tabs.len().saturating_sub(1));
    s.tree_width = s.tree_width.clamp(120, 1200);
    s.preview_width = s.preview_width.clamp(120, 1200);
    s
}

pub fn format(s: &Session) -> String {
    let mut out = String::new();
    for t in &s.tabs {
        out.push_str(&format!("tab={}\n", crate::connect::redact(t)));
    }
    out.push_str(&format!(
        "active={}\nview={}\ntree={}\nsidebar={}\npreview={}\ndetails={}\ntree_width={}\npreview_width={}\n",
        s.active, s.view, s.show_tree as u8, s.show_sidebar as u8, s.show_preview as u8, s.show_details as u8, s.tree_width, s.preview_width
    ));
    out
}

/// Whether startup reopens the saved tabs: always after a crash; with restore_tabs only for a plain launch of the first window.
pub fn restores(recover: bool, restore_tabs: bool, new_window: bool, names_something: bool) -> bool {
    recover || (restore_tabs && !new_window && !names_something)
}

pub fn path() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("rexplorer").join("session.txt")
}

pub fn load() -> Session {
    let mut s = load_from(&path());
    s.folders = std::fs::read_to_string(folders_path()).map(|t| parse_folders(&t)).unwrap_or_default();
    s
}

pub fn save(s: &Session) {
    let p = path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = save_to(&p, s);
    let _ = std::fs::write(folders_path(), format_folders(&s.folders));
}

pub fn load_from(p: &Path) -> Session {
    std::fs::read_to_string(p).map(|t| parse(&t)).unwrap_or_default()
}

pub fn save_to(p: &Path, s: &Session) -> std::io::Result<()> {
    std::fs::write(p, format(s))
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
        if self.last.map_or(true, |l| now.saturating_duration_since(l) >= self.min) {
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
            folders: Vec::new(),
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
    fn clamping_and_unknown_lines() {
        let s = parse("junk\nfoo=bar\ntab=a\ntab=b\nactive=9\ntree_width=5\npreview_width=99999\n");
        assert_eq!(s.active, 1);
        assert_eq!(s.tree_width, 120);
        assert_eq!(s.preview_width, 1200);
        assert_eq!(parse("active=4").active, 0);
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
        assert_eq!(parse_folders(&format_folders(&f)), f);
    }

    #[test]
    fn folders_keep_new_view_names() {
        let f = vec![
            ("C:\\a".to_string(), fv("list", (0, false), None, &[])),
            ("C:\\b".to_string(), fv("tiles", (2, true), Some(2), &[(0, 280)])),
            ("C:\\c".to_string(), fv("content", (0, false), None, &[])),
        ];
        assert_eq!(parse_folders(&format_folders(&f)), f);
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
        assert!(parse_folders("junk\nicons\tx\t1\t-\t\tC:\\a\n").is_empty());
        let cols = (0..40).map(|i| format!("{i}:99999")).collect::<Vec<_>>().join(",");
        let p = parse_folders(&format!("icons\t0\t0\t-\t{cols}\tC:\\a\n"));
        assert_eq!((p[0].1.cols.len(), p[0].1.cols[0].1), (32, 4000));
        assert!(parse_folders(&format!("icons\t0\t0\t-\t\tC:\\{}\n", "x".repeat(3000))).is_empty());
        let ftp = [("ftp://u:pw@h/x".to_string(), fv("icons", (0, false), None, &[]))];
        assert!(!format_folders(&ftp).contains("pw"));
    }

    #[test]
    fn folders_file_round_trip() {
        let dir = std::env::temp_dir().join(format!("rexplorer-folders-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("folders.txt");
        let read = || std::fs::read_to_string(&p).map(|t| parse_folders(&t)).unwrap_or_default();
        assert!(read().is_empty());
        let f = vec![("C:\\a".to_string(), fv("small", (2, true), Some(2), &[(0, 280), (2, 90)]))];
        std::fs::write(&p, format_folders(&f)).unwrap();
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
}
