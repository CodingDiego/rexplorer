//! RAR listing and extraction on top of the bundled unrar library (zip is handled by the shell).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use unrar_ng::Archive;
use windows::Win32::Foundation::E_FAIL;
use windows::core::{Error, Result};

/// One archive member. `path` uses backslashes, without leading or trailing separator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub name: String,
    /// Directory part of `path`; "" at the root.
    pub parent: String,
    pub is_dir: bool,
    pub size: u64,
    /// unrar's FileHeader does not expose the packed size, so this is always 0.
    pub packed: u64,
    pub dos_time: u32,
}

fn err(e: impl std::fmt::Display) -> Error {
    Error::new(E_FAIL, format!("{e}"))
}

fn norm(p: &Path) -> String {
    p.to_string_lossy().replace('/', "\\").trim_matches('\\').to_string()
}

/// An entry that climbs out of the extraction directory: a ".." step, or a drive or stream colon (Path::join
/// replaces the base with "C:x", and "a:s" writes a hidden stream).
fn escapes(path: &str) -> bool {
    path.contains(':') || path.split('\\').any(|c| c == "..")
}

/// The name unrar falls back to when Windows refuses the entry's own (its MakeNameUsable): without this check
/// `report?.txt` would overwrite an existing `report_.txt`.
fn usable(path: &str) -> String {
    path.chars().map(|c| if matches!(c, '?' | '*' | '<' | '>' | '|' | '"') || c < ' ' { '_' } else { c }).collect()
}

/// Best-effort Mark-of-the-Web: the archive carries a Zone.Identifier stream only when it was downloaded.
fn inherit_motw(archive: &Path, target: &Path) {
    let stream = |p: &Path| format!("{}:Zone.Identifier", p.display());
    if let Ok(z) = std::fs::read(stream(archive)) {
        let _ = std::fs::write(stream(target), z);
    }
}

fn split(path: &str) -> (&str, &str) {
    match path.rfind('\\') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    }
}

pub fn is_rar(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("rar"))
}

pub fn list(archive: &Path) -> Result<Vec<Entry>> {
    let mut out: Vec<Entry> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for h in Archive::new(archive).open_for_listing().map_err(err)? {
        let h = h.map_err(err)?;
        let path = norm(&h.filename);
        if path.is_empty() || escapes(&path) || !seen.insert(path.to_lowercase()) {
            continue;
        }
        let (parent, name) = split(&path);
        out.push(Entry {
            parent: parent.to_string(),
            name: name.to_string(),
            path,
            is_dir: h.is_directory(),
            size: h.unpacked_size,
            packed: 0,
            dos_time: h.file_time,
        });
    }
    // Synthesize intermediate directories that have no header of their own.
    let mut extra = Vec::new();
    for e in &out {
        let mut parent = e.parent.as_str();
        while !parent.is_empty() {
            let (pp, name) = split(parent);
            if seen.insert(parent.to_lowercase()) {
                extra.push(Entry {
                    path: parent.to_string(),
                    name: name.to_string(),
                    parent: pp.to_string(),
                    is_dir: true,
                    size: 0,
                    packed: 0,
                    dos_time: 0,
                });
            }
            parent = pp;
        }
    }
    out.extend(extra);
    Ok(out)
}

pub fn children<'a>(entries: &'a [Entry], parent: &str) -> Vec<&'a Entry> {
    entries.iter().filter(|e| e.parent.eq_ignore_ascii_case(parent)).collect()
}

pub fn extract(archive: &Path, entry_path: &str, dest_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dest_dir).map_err(err)?;
    let want = entry_path.replace('/', "\\");
    let want = want.trim_matches('\\');
    let mut cur = Archive::new(archive).open_for_processing().map_err(err)?;
    while let Some(h) = cur.read_header().map_err(err)? {
        let path = norm(&h.entry().filename);
        if !escapes(&path) && path.eq_ignore_ascii_case(want) && !h.entry().is_directory() {
            let target = dest_dir.join(split(&path).1);
            h.extract_to(&target).map_err(err)?;
            inherit_motw(archive, &target);
            return Ok(target);
        }
        cur = h.skip().map_err(err)?;
    }
    Err(err(format!("entry not found in archive: {entry_path}")))
}

pub fn extract_all(archive: &Path, dest_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dest_dir).map_err(err)?;
    let mut cur = Archive::new(archive).open_for_processing().map_err(err)?;
    let (mut written, mut kept) = (Vec::new(), 0usize);
    while let Some(h) = cur.read_header().map_err(err)? {
        let path = norm(&h.entry().filename);
        let refused = h.entry().is_file() && escapes(&path);
        let file = h.entry().is_file() && !refused;
        // unrar overwrites without asking; Explorer asks, and leaving the file there is the answer that loses nothing.
        let taken = file && (dest_dir.join(&path).exists() || dest_dir.join(usable(&path)).exists());
        kept += usize::from(taken || refused);
        cur = if file && !taken {
            written.push(path);
            h.extract_with_base(dest_dir).map_err(err)?
        } else {
            h.skip().map_err(err)?
        };
    }
    // Only what this call wrote: "Extract here" lands next to files that were already there.
    for p in written {
        inherit_motw(archive, &dest_dir.join(p));
    }
    if kept > 0 {
        return Err(err(crate::i18n::trf("err.extract_kept", &[("count", &kept)])));
    }
    Ok(())
}

/// DOS date (high 16 bits) / time (low 16 bits) to SystemTime. DOS times are local time;
/// this helper interprets them as UTC. None for out-of-range fields.
pub fn dos_time_to_systemtime(dos: u32) -> Option<SystemTime> {
    let (date, time) = (dos >> 16, dos & 0xFFFF);
    let (y, m, d) = (1980 + (date >> 9) as i64, ((date >> 5) & 15) as i64, (date & 31) as i64);
    let (h, mi, s) = ((time >> 11) as u64, ((time >> 5) & 63) as u64, ((time & 31) * 2) as u64);
    if !(1..=12).contains(&m) || d == 0 || h > 23 || mi > 59 || s > 59 {
        return None;
    }
    // Days from civil (Howard Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let secs = u64::try_from(days).ok()? * 86400 + h * 3600 + mi * 60 + s;
    Some(UNIX_EPOCH + Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usable_matches_unrars_fallback_name() {
        assert_eq!(usable("dir?\\re*port<>|\"\u{1}.txt"), "dir_\\re_port_____.txt");
        assert_eq!(usable("plain name.txt"), "plain name.txt");
    }

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
        fixture_named(dir, "docs/a.txt")
    }

    /// A one-file RAR5 archive whose entry is `name` holding "hello".
    fn fixture_named(dir: &Path, name: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let name = name.as_bytes();
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

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rexplorer-archive-{}-{tag}", std::process::id()))
    }

    #[test]
    fn list_and_children() {
        let dir = tmp("list");
        let entries = list(&fixture(&dir)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(entries.iter().any(|e| e.path == "docs" && e.is_dir));
        assert!(entries.iter().any(|e| e.path == "docs\\a.txt" && e.size == 5));
        let root = children(&entries, "");
        assert_eq!(root.len(), 1);
        assert_eq!(root[0].name, "docs");
        let docs = children(&entries, "DOCS");
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].name, "a.txt");
    }

    #[test]
    fn extract_single_file() {
        let dir = tmp("extract");
        let rar = fixture(&dir);
        let out = extract(&rar, "docs\\a.txt", &dir.join("out")).unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        let missing = extract(&rar, "nope", &dir.join("out")).is_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(text, "hello");
        assert!(missing);
    }

    #[test]
    fn escaping_entries() {
        assert!(escapes("..\\..\\x") && escapes("a\\..\\b") && escapes(".."));
        assert!(escapes("C:x") && escapes("C:\\Windows\\x") && escapes("a.txt:stream"));
        assert!(!escapes("a\\..b\\c") && !escapes("docs\\a.txt"));
    }

    #[test]
    fn extract_all_never_replaces_or_escapes() {
        let dir = tmp("conflict");
        let out = dir.join("out");
        // A file already at the target keeps its content, and the call says so.
        std::fs::create_dir_all(out.join("docs")).unwrap();
        std::fs::write(out.join("docs").join("a.txt"), "mine").unwrap();
        let kept = extract_all(&fixture(&dir), &out);
        let mine = std::fs::read_to_string(out.join("docs").join("a.txt"));
        // An entry that climbs out of the target is not written anywhere, and the call says so.
        let evil = fixture_named(&dir.join("evil"), "../escaped.txt");
        let escaped = extract_all(&evil, &out.join("deep"));
        let outside = out.join("escaped.txt").exists() || dir.join("evil").join("escaped.txt").exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(kept.is_err());
        assert_eq!(mine.unwrap(), "mine");
        assert!(escaped.is_err() && !outside);
    }

    #[test]
    fn extract_one_never_escapes() {
        // The thumbnail and preview path extracts single members: an entry that climbs out is never found, so never written.
        let dir = tmp("one-evil");
        let evil = fixture_named(&dir.join("evil"), "../escaped.txt");
        let out = dir.join("evil").join("out");
        let refused = extract(&evil, "../escaped.txt", &out).is_err() && extract(&evil, "..\\escaped.txt", &out).is_err();
        let outside = dir.join("evil").join("escaped.txt").exists() || out.join("escaped.txt").exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(refused && !outside);
    }

    #[test]
    fn extracted_files_inherit_the_zone_stream() {
        let dir = tmp("motw");
        let rar = fixture(&dir);
        let stream = |p: &Path| format!("{}:Zone.Identifier", p.display());
        std::fs::write(stream(&rar), "[ZoneTransfer]\r\nZoneId=3\r\n").unwrap();
        let one = extract(&rar, "docs\\a.txt", &dir.join("one")).unwrap();
        extract_all(&rar, &dir.join("all")).unwrap();
        let z1 = std::fs::read_to_string(stream(&one));
        let z2 = std::fs::read_to_string(stream(&dir.join("all").join("docs").join("a.txt")));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(z1.unwrap().contains("ZoneId=3"));
        assert!(z2.unwrap().contains("ZoneId=3"));
    }

    #[test]
    fn dos_time_round_trip() {
        let dos = ((2024 - 1980) << 25) | (3 << 21) | (5 << 16) | (14 << 11) | (30 << 5);
        let t = dos_time_to_systemtime(dos).unwrap();
        assert_eq!(t.duration_since(UNIX_EPOCH).unwrap().as_secs(), 1709649000);
        assert!(dos_time_to_systemtime(0).is_none());
    }

    #[test]
    fn is_rar_case_insensitive() {
        assert!(is_rar(Path::new("a.RAR")));
        assert!(!is_rar(Path::new("a.zip")));
    }

    #[test]
    fn fuzz_member_paths_never_escape_the_destination() {
        let dest = Path::new("C:\\dest");
        for seed in 0..500 {
            let raw = crate::fuzz::Rng::new(seed).text(crate::fuzz::TOKENS, 20);
            let n = norm(Path::new(&raw));
            assert!(!n.contains('/') && !n.starts_with('\\') && !n.ends_with('\\'), "seed {seed}: {n:?}");
            let (dir, name) = split(&n);
            assert!(!name.contains('\\'), "seed {seed}");
            assert_eq!(if dir.is_empty() { name.to_string() } else { format!("{dir}\\{name}") }, n, "seed {seed}");
            if !escapes(&n) {
                let p = dest.join(&n);
                assert!(p.starts_with(dest), "seed {seed}: {p:?}");
                assert!(!p.components().any(|c| matches!(c, std::path::Component::ParentDir)), "seed {seed}: {p:?}");
            }
        }
    }
}
