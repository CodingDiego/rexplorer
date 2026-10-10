//! Browsing inside a .rar file as if it were a folder: a listing model the main window renders
//! in its ListView (columns Name, Date modified, Type, Size), plus extraction helpers.
#![allow(dead_code)]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use rexplorer_core::archive::{self, Entry};
use windows::core::Result;

pub struct ArchiveListing {
    pub archive: PathBuf,
    /// "" = root, backslashes.
    pub dir: String,
    entries: Vec<Entry>,
}

fn norm_dir(s: &str) -> String {
    s.replace('/', "\\").trim_matches('\\').to_string()
}

/// "C:\\a\\b.rar\\docs\\x" -> ("C:\\a\\b.rar", "docs\\x"). None if no component ends in ".rar".
pub fn split_archive_path(text: &str) -> Option<(PathBuf, String)> {
    let mut start = 0;
    for comp in text.split(['\\', '/']) {
        let end = start + comp.len();
        if comp.len() > 4 && comp.as_bytes()[comp.len() - 4..].eq_ignore_ascii_case(b".rar") {
            return Some((PathBuf::from(&text[..end]), norm_dir(&text[end..])));
        }
        start = end + 1;
    }
    None
}

pub fn open(archive: &Path, dir: &str) -> Result<ArchiveListing> {
    Ok(ArchiveListing { archive: archive.to_path_buf(), dir: norm_dir(dir), entries: archive::list(archive)? })
}

fn format_size(n: u64) -> String {
    const U: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64 / 1024.0;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", U[i])
}

fn format_date(dos: u32) -> String {
    if dos == 0 || archive::dos_time_to_systemtime(dos).is_none() {
        return String::new();
    }
    let (d, t) = (dos >> 16, dos & 0xFFFF);
    format!("{:04}-{:02}-{:02} {:02}:{:02}", 1980 + (d >> 9), (d >> 5) & 15, d & 31, t >> 11, (t >> 5) & 63)
}

impl ArchiveListing {
    /// Directories first, then names case-insensitive.
    pub fn rows(&self) -> Vec<&Entry> {
        let mut v = archive::children(&self.entries, &self.dir);
        v.sort_by_key(|e| (!e.is_dir, e.name.to_lowercase()));
        v
    }

    pub fn enter(&mut self, name: &str) -> bool {
        let next = self.rows().into_iter().find(|e| e.is_dir && e.name.eq_ignore_ascii_case(name)).map(|e| e.path.clone());
        match next {
            Some(p) => {
                self.dir = p;
                true
            }
            None => false,
        }
    }

    pub fn up(&mut self) -> bool {
        if self.dir.is_empty() {
            return false;
        }
        self.dir = self.dir.rfind('\\').map_or(String::new(), |i| self.dir[..i].to_string());
        true
    }

    pub fn editing_name(&self) -> String {
        let a = self.archive.to_string_lossy();
        if self.dir.is_empty() { a.into_owned() } else { format!("{a}\\{}", self.dir) }
    }

    pub fn title(&self) -> String {
        if self.dir.is_empty() {
            self.archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            self.dir.rsplit('\\').next().unwrap_or("").to_string()
        }
    }

    /// 0 Name, 1 Date modified, 2 Type, 3 Size.
    pub fn detail(e: &Entry, column: usize) -> String {
        match column {
            0 => e.name.clone(),
            1 => format_date(e.dos_time),
            2 if e.is_dir => "File folder".into(),
            2 => match Path::new(&e.name).extension() {
                Some(x) => format!("{} file", x.to_string_lossy().to_uppercase()),
                None => "File".into(),
            },
            3 if !e.is_dir => format_size(e.size),
            _ => String::new(),
        }
    }

    /// Extracts one file under extract_dir() (cached by archive path + mtime) and returns its path.
    pub fn extract_for_open(&self, e: &Entry) -> Result<PathBuf> {
        let mtime = std::fs::metadata(&self.archive).and_then(|m| m.modified()).ok();
        let mut h = DefaultHasher::new();
        self.archive.hash(&mut h);
        mtime.hash(&mut h);
        let dest = extract_dir().join(format!("{:016x}", h.finish())).join(&e.parent);
        let target = dest.join(&e.name);
        if std::fs::metadata(&target).is_ok_and(|m| m.len() == e.size) {
            return Ok(target);
        }
        archive::extract(&self.archive, &e.path, &dest)
    }

    pub fn extract_all_to(&self, dest: &Path) -> Result<()> {
        archive::extract_all(&self.archive, dest)
    }
}

pub fn extract_dir() -> PathBuf {
    std::env::temp_dir().join("rexplorer-archive")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crc32(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { (c >> 1) ^ 0xEDB88320 } else { c >> 1 };
            }
        }
        !c
    }

    fn vint(mut v: u64) -> Vec<u8> {
        let mut o = Vec::new();
        loop {
            let b = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                o.push(b);
                return o;
            }
            o.push(b | 0x80);
        }
    }

    fn header(body: &[u8]) -> Vec<u8> {
        let mut after = vint(body.len() as u64);
        after.extend_from_slice(body);
        let mut h = crc32(&after).to_le_bytes().to_vec();
        h.extend(after);
        h
    }

    fn fixture(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let name = b"docs/a.txt";
        let mut f = vec![2, 0x02];
        for v in [5, 0, 5, 0x20, 0, 0, name.len() as u64] {
            f.extend(vint(v));
        }
        f.extend_from_slice(name);
        let mut rar = vec![0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x01, 0x00];
        rar.extend(header(&[1, 0, 0]));
        rar.extend(header(&f));
        rar.extend_from_slice(b"hello");
        rar.extend(header(&[5, 0, 0]));
        let p = dir.join("t.rar");
        std::fs::write(&p, rar).unwrap();
        p
    }

    fn entry(name: &str, is_dir: bool, size: u64, dos_time: u32) -> Entry {
        Entry { path: name.into(), name: name.into(), parent: String::new(), is_dir, size, packed: 0, dos_time }
    }

    #[test]
    fn split_table() {
        let s = |t: &str| split_archive_path(t).map(|(a, d)| (a.to_string_lossy().into_owned(), d));
        assert_eq!(s("C:\\a\\b.rar\\docs\\x"), Some(("C:\\a\\b.rar".into(), "docs\\x".into())));
        assert_eq!(s("C:/a/B.RAR/docs/x/"), Some(("C:/a/B.RAR".into(), "docs\\x".into())));
        assert_eq!(s("C:\\a\\b.rar"), Some(("C:\\a\\b.rar".into(), String::new())));
        assert_eq!(s("C:\\a\\b.txt"), None);
        assert_eq!(s("C:\\a.rarx\\b"), None);
    }

    #[test]
    fn split_non_ascii_components() {
        let s = |t: &str| split_archive_path(t).map(|(a, d)| (a.to_string_lossy().into_owned(), d));
        assert_eq!(s("E:\\写真\\x.rar\\a"), Some(("E:\\写真\\x.rar".into(), "a".into())));
        assert_eq!(s("E:\\写真\\b"), None);
    }

    #[test]
    fn detail_formatting() {
        let dos = ((2024 - 1980) << 25) | (3 << 21) | (5 << 16) | (14 << 11) | (30 << 5);
        let f = entry("Photo.jpg", false, 1536 * 1024, dos);
        assert_eq!(ArchiveListing::detail(&f, 0), "Photo.jpg");
        assert_eq!(ArchiveListing::detail(&f, 1), "2024-03-05 14:30");
        assert_eq!(ArchiveListing::detail(&f, 2), "JPG file");
        assert_eq!(ArchiveListing::detail(&f, 3), "1.5 MB");
        let d = entry("docs", true, 0, 0);
        assert_eq!(ArchiveListing::detail(&d, 1), "");
        assert_eq!(ArchiveListing::detail(&d, 2), "File folder");
        assert_eq!(ArchiveListing::detail(&d, 3), "");
        assert_eq!(ArchiveListing::detail(&entry("README", false, 10, 0), 2), "File");
        assert_eq!(ArchiveListing::detail(&entry("README", false, 10, 0), 3), "10 B");
    }

    #[test]
    fn browse_and_extract() {
        let dir = std::env::temp_dir().join(format!("rexplorer-archive_view-{}", std::process::id()));
        let rar = fixture(&dir);
        let mut l = open(&rar, "").unwrap();
        assert_eq!(l.title(), "t.rar");
        assert_eq!(l.rows().len(), 1);
        assert!(!l.enter("nope"));
        assert!(l.enter("DOCS"));
        assert_eq!(l.title(), "docs");
        assert!(l.editing_name().ends_with("t.rar\\docs"));
        let e = l.rows()[0].clone();
        assert_eq!(e.name, "a.txt");
        let p1 = l.extract_for_open(&e).unwrap();
        let text = std::fs::read_to_string(&p1).unwrap();
        let p2 = l.extract_for_open(&e).unwrap();
        let all = dir.join("all");
        let all_ok = l.extract_all_to(&all).is_ok() && all.join("docs").join("a.txt").exists();
        assert!(l.up());
        assert!(!l.up());
        let _ = std::fs::remove_dir_all(&dir);
        if let Some(p) = p1.parent().and_then(|p| p.parent()) {
            let _ = std::fs::remove_dir_all(p);
        }
        assert_eq!(text, "hello");
        assert_eq!(p1, p2);
        assert!(all_ok);
    }
}
