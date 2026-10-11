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
    pub tree_width: i32,
    pub preview_width: i32,
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
            tree_width: 220,
            preview_width: 360,
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
            "tree_width" => s.tree_width = v.trim().parse().unwrap_or(s.tree_width),
            "preview_width" => s.preview_width = v.trim().parse().unwrap_or(s.preview_width),
            _ => {}
        }
    }
    // A session from before the sidebar had the tree on by default: it gets the sidebar instead.
    if !has_sidebar {
        s.show_tree = false;
    }
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
        "active={}\nview={}\ntree={}\nsidebar={}\npreview={}\ntree_width={}\npreview_width={}\n",
        s.active, s.view, s.show_tree as u8, s.show_sidebar as u8, s.show_preview as u8, s.tree_width, s.preview_width
    ));
    out
}

pub fn path() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("rexplorer").join("session.txt")
}

pub fn load() -> Session {
    load_from(&path())
}

pub fn save(s: &Session) {
    let p = path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = save_to(&p, s);
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
            tree_width: 300,
            preview_width: 500,
        }
    }

    #[test]
    fn round_trip_with_equals_in_tab() {
        let s = sample();
        assert_eq!(parse(&format(&s)), s);
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
