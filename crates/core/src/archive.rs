//! RAR listing and extraction on top of the bundled unrar library (zip is handled by the shell).

use std::collections::HashSet;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use unrar_ng::{Archive, VolumeInfo};
use unrar_ng::error::{Code, UnrarError, When};
use windows::Win32::Foundation::{E_FAIL, ERROR_INVALID_PASSWORD, ERROR_NOT_AUTHENTICATED};
use windows::Win32::Globalization::{CP_ACP, WideCharToMultiByte};
use windows::core::{Error, HRESULT, Result};

/// The archive needs a password and none is known for it.
pub const E_PASSWORD_MISSING: HRESULT = ERROR_NOT_AUTHENTICATED.to_hresult();
/// The password known for the archive is not its password.
pub const E_PASSWORD_WRONG: HRESULT = ERROR_INVALID_PASSWORD.to_hresult();

/// A password typed for an archive. It lives only in memory and is overwritten with zeros when dropped.
pub struct Password(Vec<u16>);

impl Password {
    /// Takes the UTF-16 text (no terminator) as it is, without a copy.
    pub fn from_wide(text: Vec<u16>) -> Password {
        Password(text)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn wipe<T: Default>(v: &mut [T]) {
    for x in v.iter_mut() {
        // Volatile, so the compiler cannot drop the writes to memory about to be freed.
        unsafe { std::ptr::write_volatile(x, T::default()) };
    }
}

impl Drop for Password {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

/// Passwords typed for archives in this session, by archive path. Never written anywhere.
static PASSWORDS: Mutex<Vec<(PathBuf, Password)>> = Mutex::new(Vec::new());

fn same(a: &Path, b: &Path) -> bool {
    a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
}

/// Remembers `password` for `archive` until `forget_password` (or the end of the process).
pub fn set_password(archive: &Path, password: Password) {
    let mut all = PASSWORDS.lock().unwrap_or_else(|e| e.into_inner());
    all.retain(|(a, _)| !same(a, archive));
    all.push((archive.to_path_buf(), password));
}

/// Drops (and zeroes) the password known for `archive`.
pub fn forget_password(archive: &Path) {
    PASSWORDS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(a, _)| !same(a, archive));
}

/// Drops every password; on exit.
pub fn forget_passwords() {
    PASSWORDS.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Some(wrong) when `e` means `archive` needs a password: none was known, or (`wrong`) the known one was refused.
pub fn password_needed(e: &Error) -> Option<bool> {
    match e.code() {
        c if c == E_PASSWORD_MISSING => Some(false),
        c if c == E_PASSWORD_WRONG => Some(true),
        _ => None,
    }
}

/// `f` gets the archive, opened with the password known for it if any. unrar takes the password in the ANSI code
/// page (its RARSetPassword) and copies it; the bytes made here are zeroed when `f` returns.
/// shortcut: a character outside the ANSI code page cannot be passed this way (it becomes '?'); unrar's
/// UCM_NEEDPASSWORDW callback takes UTF-16 if that is ever needed.
fn with_archive<T>(archive: &Path, f: impl FnOnce(Archive) -> T) -> T {
    let all = PASSWORDS.lock().unwrap_or_else(|e| e.into_inner());
    let Some((_, pw)) = all.iter().find(|(a, _)| same(a, archive)) else {
        drop(all);
        return f(Archive::new(archive));
    };
    let mut bytes = vec![0u8; pw.0.len() * 4 + 1];
    let n = unsafe { WideCharToMultiByte(CP_ACP, 0, &pw.0, Some(&mut bytes), None, None) }.max(0) as usize;
    drop(all);
    let out = f(Archive::with_password(archive, &bytes[..n]));
    wipe(&mut bytes);
    out
}

/// unrar's error, with its password codes kept apart. A refused password is forgotten, so the next try asks again.
fn rar_err(archive: &Path, e: UnrarError) -> Error {
    match e.code {
        Code::MissingPassword => Error::new(E_PASSWORD_MISSING, crate::i18n::tr("err.password_missing")),
        Code::BadPassword => {
            forget_password(archive);
            Error::new(E_PASSWORD_WRONG, crate::i18n::tr("err.password_wrong"))
        }
        Code::EOpen if !matches!(e.when, When::Open) => err(crate::i18n::tr("err.rar_volume_missing")),
        _ => err(e),
    }
}

/// A set is read from its first volume: a middle one would list and extract a partial run of members.
fn first_volume(v: VolumeInfo) -> Result<()> {
    if matches!(v, VolumeInfo::Subsequent) { Err(err(crate::i18n::tr("err.rar_middle_volume"))) } else { Ok(()) }
}

/// A member whose data stops at a missing volume is left half written by unrar: it must not stay.
fn drop_partial(dest_dir: &Path, path: &str) {
    for p in [dest_dir.join(path), dest_dir.join(usable(path))] {
        if p.symlink_metadata().is_ok_and(|m| m.is_file()) {
            let _ = std::fs::remove_file(p);
        }
    }
}

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

/// What any member name keeps through `norm`, `split` and `escapes` (the seeded test and fuzz/'s target): no
/// slash or edge backslash, a split that joins back, and a non-escaping name that stays under the destination.
#[cfg(any(test, fuzzing))]
pub fn fuzz_check(raw: &str) {
    let dest = Path::new("C:\\dest");
    let n = norm(Path::new(raw));
    assert!(!n.contains('/') && !n.starts_with('\\') && !n.ends_with('\\'), "{raw:?} -> {n:?}");
    let (dir, name) = split(&n);
    assert!(!name.contains('\\'), "{raw:?}");
    assert_eq!(if dir.is_empty() { name.to_string() } else { format!("{dir}\\{name}") }, n, "{raw:?}");
    if !escapes(&n) {
        let p = dest.join(&n);
        assert!(p.starts_with(dest), "{raw:?}: {p:?}");
        assert!(!p.components().any(|c| matches!(c, std::path::Component::ParentDir)), "{raw:?}: {p:?}");
    }
}

fn norm(p: &Path) -> String {
    p.to_string_lossy().replace('/', "\\").trim_matches('\\').to_string()
}

/// An entry that could land outside the extraction directory or on a device: a drive or stream colon (Path::join
/// replaces the base with "C:x", and "a:s" writes a hidden stream); a step Windows reads as "." or ".." once it
/// strips trailing dots and spaces (".. " is ".."); a name ending in a dot or a space (Windows writes another
/// name); a DOS device name, with or without an extension (CON, com1.jpg, CONIN$).
fn escapes(path: &str) -> bool {
    path.contains(':') || path.split('\\').any(|c| c.trim_end_matches(['.', ' ']).is_empty() || c.ends_with(['.', ' ']) || is_device_name(c))
}

/// A DOS device name, with or without an extension (CON, com1.jpg, CONIN$): Windows opens the device, not a file.
pub(crate) fn is_device_name(c: &str) -> bool {
    const DEVICES: [&str; 8] = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "CLOCK$", "KEYBD$"];
    let base = c.split('.').next().unwrap_or("").trim_end_matches(' ').to_uppercase();
    let numbered = (base.starts_with("COM") || base.starts_with("LPT"))
        && base[3..].chars().count() == 1
        && base[3..].chars().all(|d| d.is_ascii_digit() || matches!(d, '¹' | '²' | '³'));
    DEVICES.contains(&base.as_str()) || numbered
}

/// Whether `p` is a link (symbolic link, junction, any reparse point), without following it.
fn is_link(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.file_attributes() & 0x400 != 0)
}

/// Whether `p` is a plain file and its only name: not a link, and not a hard link sharing a file with another name.
fn single_file(p: &Path) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
    if is_link(p) || !p.is_file() {
        return false;
    }
    let Ok(f) = std::fs::File::open(p) else { return false };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(f.as_raw_handle()), &mut info) }.is_ok() && info.nNumberOfLinks == 1
}

/// Removes the link at `p` itself, never what it points to.
fn remove_link(p: &Path) {
    if std::fs::remove_dir(p).is_err() {
        let _ = std::fs::remove_file(p);
    }
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
    let opened = with_archive(archive, |a| a.open_for_listing()).map_err(|e| rar_err(archive, e))?;
    first_volume(opened.volume_info())?;
    for h in opened {
        let h = h.map_err(|e| rar_err(archive, e))?;
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

/// Extracts member `entry_path` (a file) under `dest_dir`, which must be new or empty, at the member's own path, and
/// returns where it landed. A member whose name `escapes`, or that unrar makes into a link (symbolic link, junction,
/// hard link), is refused: nothing of it stays.
pub fn extract(archive: &Path, entry_path: &str, dest_dir: &Path) -> Result<PathBuf> {
    let want = entry_path.replace('/', "\\");
    let want = want.trim_matches('\\');
    if escapes(want) {
        return Err(err(format!("entry not found in archive: {entry_path}")));
    }
    std::fs::create_dir_all(dest_dir).map_err(err)?;
    // unrar resolves a hard-link or file-copy member's target inside dest_dir: empty, it holds nothing of the user's.
    if std::fs::read_dir(dest_dir).map_err(err)?.next().is_some() {
        return Err(err(format!("extraction folder is not empty: {}", dest_dir.display())));
    }
    let fail = |e| rar_err(archive, e);
    let mut cur = with_archive(archive, |a| a.open_for_processing()).map_err(fail)?;
    first_volume(cur.volume_info())?;
    while let Some(h) = cur.read_header().map_err(fail)? {
        let path = norm(&h.entry().filename);
        if !escapes(&path) && path.eq_ignore_ascii_case(want) && !h.entry().is_directory() {
            // The base-path form: with a whole destination name unrar has no base to keep a link's target inside
            // (dll.cpp DllDestName) and resolves it against the working directory.
            h.extract_with_base(dest_dir).map_err(|e| {
                drop_partial(dest_dir, &path);
                fail(e)
            })?;
            let at = [dest_dir.join(&path), dest_dir.join(usable(&path))];
            return match at.into_iter().find(|p| p.symlink_metadata().is_ok()) {
                Some(target) if single_file(&target) => {
                    inherit_motw(archive, &target);
                    Ok(target)
                }
                made => {
                    made.iter().for_each(|p| remove_link(p));
                    Err(err(crate::i18n::tr("err.archive_link")))
                }
            };
        }
        cur = h.skip().map_err(fail)?;
    }
    Err(err(format!("entry not found in archive: {entry_path}")))
}

/// The password error, if any, before anything is written: the first encrypted member is test-decrypted (nothing
/// lands on disk), so a missing or wrong password stops "Extract all" before the plain members before it are out.
fn check_password(archive: &Path) -> Result<()> {
    let fail = |e| rar_err(archive, e);
    let mut cur = with_archive(archive, |a| a.open_for_processing()).map_err(fail)?;
    first_volume(cur.volume_info())?;
    while let Some(h) = cur.read_header().map_err(fail)? {
        if h.entry().is_encrypted() && h.entry().is_file() {
            h.test().map_err(fail)?;
            return Ok(());
        }
        cur = h.skip().map_err(fail)?;
    }
    Ok(())
}

/// Extracts every member under `dest_dir`, at its own path. Never replaces a file already there, never writes a
/// member whose name `escapes`, and removes any link (symbolic link, junction, hard link) unrar creates as soon as it
/// is made, so no later member goes through it. Each one left out is counted in the error.
pub fn extract_all(archive: &Path, dest_dir: &Path) -> Result<()> {
    check_password(archive)?;
    std::fs::create_dir_all(dest_dir).map_err(err)?;
    let fail = |e| rar_err(archive, e);
    let mut cur = with_archive(archive, |a| a.open_for_processing()).map_err(fail)?;
    let (mut written, mut kept) = (Vec::new(), 0usize);
    while let Some(h) = cur.read_header().map_err(fail)? {
        let path = norm(&h.entry().filename);
        let refused = escapes(&path);
        let file = h.entry().is_file() && !refused;
        // unrar overwrites without asking; Explorer asks, and leaving the file there is the answer that loses nothing.
        let taken = file && (dest_dir.join(&path).symlink_metadata().is_ok() || dest_dir.join(usable(&path)).symlink_metadata().is_ok());
        kept += usize::from(taken || refused);
        cur = if file && !taken {
            let next = h.extract_with_base(dest_dir).map_err(|e| {
                drop_partial(dest_dir, &path);
                fail(e)
            })?;
            let at = [dest_dir.join(&path), dest_dir.join(usable(&path))];
            let made: Vec<&PathBuf> = at.iter().filter(|p| p.symlink_metadata().is_ok()).collect();
            if made.is_empty() {
                kept += 1; // unrar skipped it without an error (a link it judged unsafe)
            } else if !made.iter().all(|p| single_file(p)) {
                // A hard link can be a second name for a file that was already in dest_dir.
                made.iter().for_each(|p| remove_link(p));
                kept += 1;
            } else {
                written.push(path);
            }
            next
        } else {
            h.skip().map_err(fail)?
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

    /// A RAR5 member: a stored file, or a link entry (redirection record: 1 Unix symlink, 2 Windows symlink,
    /// 3 junction) to `target`, `dir` saying the target is a directory.
    enum M<'a> {
        File(&'a str, &'a [u8]),
        Link(&'a str, u64, &'a str, bool),
    }

    fn rar_of(dir: &Path, members: &[M]) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let mut rar = vec![0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x01, 0x00];
        rar.extend(header(&[1, 0, 0]));
        for m in members {
            let (name, data, extra, attr) = match m {
                M::File(name, data) => (*name, *data, Vec::new(), 0x20),
                M::Link(name, kind, target, is_dir) => {
                    let mut rec = vec![5];
                    for v in [*kind, u64::from(*is_dir), target.len() as u64] {
                        rec.extend(vint(v));
                    }
                    rec.extend_from_slice(target.as_bytes());
                    let mut extra = vint(rec.len() as u64);
                    extra.extend(rec);
                    (*name, &[][..], extra, if *is_dir { 0x10 } else { 0x20 })
                }
            };
            let mut f = vec![2, if extra.is_empty() { 0x02 } else { 0x01 }];
            if extra.is_empty() {
                f.extend(vint(data.len() as u64));
            } else {
                f.extend(vint(extra.len() as u64));
            }
            for v in [0, data.len() as u64, attr, 0, 0, name.len() as u64] {
                f.extend(vint(v));
            }
            f.extend_from_slice(name.as_bytes());
            f.extend(extra);
            rar.extend(header(&f));
            rar.extend_from_slice(data);
        }
        rar.extend(header(&[5, 0, 0]));
        let p = dir.join("t.rar");
        std::fs::write(&p, rar).unwrap();
        p
    }

    /// Every path below `root`, links included (not followed).
    fn tree(root: &Path) -> Vec<(String, bool)> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                let link = is_link(&p);
                out.push((p.strip_prefix(root).unwrap().display().to_string(), link));
                if !link && p.is_dir() {
                    stack.push(p);
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn dos_names_and_dotted_steps_never_land_anywhere() {
        let dir = tmp("dosnames");
        let rar = rar_of(
            &dir.join("src"),
            &[
                M::File("a/.. /.. /evil1.txt", b"x"),
                M::File(".. /evil2.txt", b"x"),
                M::File("a/... /evil3.txt", b"x"),
                M::File("CON", b"x"),
                M::File("con.txt", b"x"),
                M::File("COM1.jpg", b"x"),
                M::File("d/lpt3/x.txt", b"x"),
                M::File("CONIN$", b"x"),
                M::File("notes.txt.", b"x"),
                M::File("trail ", b"x"),
                M::File("ok.txt", b"fine"),
            ],
        );
        let names: Vec<String> = list(&rar).unwrap().into_iter().map(|e| e.path).collect();
        let out = dir.join("out").join("deep");
        let all = extract_all(&rar, &out);
        let one = ["a/.. /.. /evil1.txt", "CON", "COM1.jpg", "notes.txt."].map(|n| extract(&rar, n, &out.join("one")).is_err());
        let written = tree(&dir.join("out"));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(names, ["ok.txt"], "listed: only the safe member");
        assert_eq!(all.unwrap_err().message(), crate::i18n::trf("err.extract_kept", &[("count", &10)]));
        assert_eq!(one, [true; 4]);
        assert_eq!(written, [("deep".to_string(), false), (r"deep\ok.txt".to_string(), false)], "nothing else, anywhere");
    }

    #[test]
    fn link_entries_are_never_left_behind() {
        let dir = tmp("links");
        let rar = rar_of(
            &dir.join("src"),
            &[
                M::File("ok.txt", b"fine"),
                M::File("sub/inner.txt", b"in"),
                M::Link("jabs", 3, r"\??\C:\Windows", true),
                M::Link("x/jup", 3, r"..\..\..", true),
                M::Link("srel", 2, r"..\outside.txt", false),
                M::Link("usafe", 1, "ok.txt", false),
                M::Link("jsafe", 3, "sub", true),
                M::File("x/jup/evil.txt", b"x"),
                M::File("jsafe/through.txt", b"t"),
            ],
        );
        std::fs::write(dir.join("outside.txt"), b"mine").unwrap();
        let out = dir.join("out").join("deep");
        let all = extract_all(&rar, &out);
        let one = extract(&rar, "usafe", &out.join("one"));
        let written = tree(&dir.join("out"));
        let outside = std::fs::read(dir.join("outside.txt")).unwrap();
        let evil = ["evil.txt", r"out\evil.txt", r"src\evil.txt"].map(|p| dir.join(p).exists());
        let _ = std::fs::remove_dir_all(&dir);
        // All five links: the ones unrar refuses quietly and the ones it makes and we remove.
        assert_eq!(all.unwrap_err().message(), crate::i18n::trf("err.extract_kept", &[("count", &5)]));
        assert!(one.is_err());
        assert!(written.iter().all(|(_, link)| !link), "no link stays: {written:?}");
        assert!(written.iter().all(|(p, _)| p.starts_with("deep")), "{written:?}");
        assert!(written.iter().any(|(p, _)| p == r"deep\ok.txt") && written.iter().any(|(p, _)| p == r"deep\sub\inner.txt"));
        assert_eq!(outside, b"mine");
        assert_eq!(evil, [false; 3]);
    }

    fn fixture_copy(name: &str, tag: &str) -> PathBuf {
        let dir = tmp(tag);
        std::fs::create_dir_all(&dir).unwrap();
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join(name);
        let p = dir.join(name);
        std::fs::copy(src, &p).unwrap();
        p
    }

    fn pw(s: &str) -> Password {
        Password::from_wide(s.encode_utf16().collect())
    }

    #[test]
    fn encrypted_members_ask_for_the_password_and_take_only_the_right_one() {
        let rar = fixture_copy("enc-files.rar", "encfiles");
        let dir = rar.parent().unwrap().to_path_buf();
        let names: Vec<String> = list(&rar).unwrap().into_iter().map(|e| e.path).collect();
        let plain = extract(&rar, "readme.txt", &dir.join("a")).map(|p| std::fs::read(p).unwrap());
        let missing = extract(&rar, "secret.txt", &dir.join("b")).map_err(|e| password_needed(&e));
        let all_missing = extract_all(&rar, &dir.join("none")).map_err(|e| password_needed(&e));
        set_password(&rar, pw("not it"));
        let all_wrong = extract_all(&rar, &dir.join("none")).map_err(|e| password_needed(&e));
        let none_written = !dir.join("none").join("readme.txt").exists();
        set_password(&rar, pw("not it"));
        let wrong = extract(&rar, "secret.txt", &dir.join("c")).map_err(|e| password_needed(&e));
        let after_wrong = extract(&rar, "secret.txt", &dir.join("d")).map_err(|e| password_needed(&e));
        set_password(&rar, pw("rexplorer-pw"));
        let right = extract(&rar, "secret.txt", &dir.join("e")).map(|p| std::fs::read(p).unwrap());
        let all = extract_all(&rar, &dir.join("all"));
        let all_secret = std::fs::read(dir.join("all").join("secret.txt"));
        forget_password(&rar);
        let forgotten = extract(&rar, "secret.txt", &dir.join("f")).map_err(|e| password_needed(&e));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(names, ["readme.txt", "secret.txt"], "the names are not encrypted");
        assert_eq!(plain.unwrap(), b"plain");
        assert_eq!(missing.unwrap_err(), Some(false));
        assert_eq!((all_missing.unwrap_err(), all_wrong.unwrap_err()), (Some(false), Some(true)));
        assert!(none_written, "Extract all asks before writing the plain member");
        assert_eq!(wrong.unwrap_err(), Some(true));
        assert_eq!(after_wrong.unwrap_err(), Some(false), "a refused password is forgotten");
        assert_eq!(right.unwrap(), b"top secret\r\n");
        assert!(all.is_ok(), "{all:?}");
        assert_eq!(all_secret.unwrap(), b"top secret\r\n");
        assert_eq!(forgotten.unwrap_err(), Some(false));
    }

    #[test]
    fn encrypted_headers_ask_before_listing() {
        let rar = fixture_copy("enc-headers.rar", "encheaders");
        let dir = rar.parent().unwrap().to_path_buf();
        let missing = list(&rar).map_err(|e| password_needed(&e));
        set_password(&rar, pw("contrasena"));
        let wrong = list(&rar).map_err(|e| password_needed(&e));
        set_password(&rar, pw("contraseña"));
        let names = list(&rar).map(|l| l.into_iter().map(|e| e.path).collect::<Vec<_>>());
        let data = extract(&rar, r"docs\hidden.txt", &dir.join("x")).map(|p| std::fs::read(p).unwrap());
        // Not forget_passwords: tests run side by side and share the store.
        forget_password(&rar);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(missing.unwrap_err(), Some(false));
        assert_eq!(wrong.unwrap_err(), Some(true));
        // "ñ" goes through the ANSI code page: on one without it the password cannot be typed (see with_archive).
        let mut acp = [0u8; 8];
        let mut lost = windows::core::BOOL(0);
        unsafe { WideCharToMultiByte(CP_ACP, 0, &[0xF1], Some(&mut acp), None, Some(&mut lost)) };
        if lost.as_bool() {
            return;
        }
        assert_eq!(names.unwrap(), [r"docs\hidden.txt", "docs"], "the member, then its synthesized folder");
        assert_eq!(data.unwrap(), b"hidden name, hidden data");
    }

    #[test]
    fn passwords_are_wiped() {
        let mut v = vec![1u16, 2, 3];
        wipe(&mut v);
        assert_eq!(v, [0, 0, 0]);
    }

    #[test]
    fn only_the_first_volume_opens() {
        assert!(first_volume(VolumeInfo::None).is_ok() && first_volume(VolumeInfo::First).is_ok());
        assert_eq!(first_volume(VolumeInfo::Subsequent).unwrap_err().message(), crate::i18n::tr("err.rar_middle_volume"));
    }

    /// The set under tests/fixtures/multivol, made by WinRAR 7.23's `rar a -v1k -m0 -ep set.rar first.bin note.txt
    /// last.bin` (1500 and 900 seeded random bytes and one line of text): `first.bin` runs from part 1 into part 2,
    /// `note.txt` sits in part 2 and `last.bin` runs from part 2 into part 3. unrar cannot write archives.
    fn multivol(tag: &str, skip: Option<&str>) -> PathBuf {
        let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multivol");
        let dir = tmp(tag);
        std::fs::create_dir_all(&dir).unwrap();
        for part in ["set.part1.rar", "set.part2.rar", "set.part3.rar"] {
            if Some(part) != skip {
                std::fs::copy(from.join(part), dir.join(part)).unwrap();
            }
        }
        dir
    }

    #[test]
    fn multi_volume_set_lists_and_extracts_across_volumes() {
        let dir = multivol("mv-ok", None);
        let rar = dir.join("set.part1.rar");
        let mut files: Vec<(String, u64)> = list(&rar).unwrap().into_iter().filter(|e| !e.is_dir).map(|e| (e.path, e.size)).collect();
        files.sort();
        let x = extract_all(&rar, &dir.join("out"));
        let written: Vec<Option<u64>> = files.iter().map(|(p, _)| std::fs::metadata(dir.join("out").join(p)).ok().map(|m| m.len())).collect();
        let _ = std::fs::remove_dir_all(&dir);
        x.unwrap();
        let names: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(names, ["first.bin", "last.bin", "note.txt"]);
        assert_eq!(files[0].1, 1500, "first.bin's size, read across parts 1 and 2");
        assert_eq!(written, files.iter().map(|f| Some(f.1)).collect::<Vec<_>>(), "every member is whole, first.bin and last.bin cross a volume");
    }

    #[test]
    fn multi_volume_middle_or_missing_volume_fails_clearly() {
        let dir = multivol("mv-bad", None);
        let middle = dir.join("set.part2.rar");
        let (l, x) = (list(&middle), extract_all(&middle, &dir.join("o1")));
        let wrote = dir.join("o1").exists();
        let _ = std::fs::remove_dir_all(&dir);
        let want = crate::i18n::tr("err.rar_middle_volume");
        assert_eq!(l.unwrap_err().message(), want);
        assert_eq!(x.unwrap_err().message(), want);
        assert!(!wrote, "nothing written");

        // Sizes come from the whole set: listing the set with a gap fails too.
        let whole = list(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multivol/set.part1.rar")).unwrap();
        let dir = multivol("mv-gap", Some("set.part2.rar"));
        let rar = dir.join("set.part1.rar");
        let (l, x) = (list(&rar), extract_all(&rar, &dir.join("out")));
        let left: Vec<(String, u64)> = tree(&dir.join("out")).into_iter().filter_map(|(p, _)| std::fs::metadata(dir.join("out").join(&p)).ok().filter(|m| m.is_file()).map(|m| (p, m.len()))).collect();
        let _ = std::fs::remove_dir_all(&dir);
        let missing = crate::i18n::tr("err.rar_volume_missing");
        assert_eq!(l.unwrap_err().message(), missing);
        assert_eq!(x.unwrap_err().message(), missing);
        for (p, n) in left {
            assert!(whole.iter().any(|e| e.path.eq_ignore_ascii_case(&p) && e.size == n), "no partial file: {p}");
        }
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
        let missing = extract(&rar, "nope", &dir.join("out2")).is_err();
        let not_empty = extract(&rar, "docs\\a.txt", &dir.join("out")).is_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(text, "hello");
        assert_eq!(out, dir.join("out").join("docs").join("a.txt"), "at the member's own path");
        assert!(missing && not_empty);
    }

    #[test]
    fn hard_link_and_copy_members_reach_no_other_file() {
        let dir = tmp("hardlinks");
        // Relative targets: before the base-path form, unrar resolved them against the working directory (the crate).
        let one = rar_of(&dir.join("one"), &[M::Link("hl", 4, "Cargo.toml", false), M::Link("cp", 5, "Cargo.toml", false)]);
        let refused = ["hl", "cp"].map(|n| extract(&one, n, &dir.join("x").join(n)).is_err());
        let left = tree(&dir.join("x"));
        let all = rar_of(&dir.join("all"), &[M::File("ok.txt", b"fine"), M::Link("hl", 4, "mine.txt", false), M::Link("hl2", 4, "ok.txt", false)]);
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("mine.txt"), b"mine").unwrap();
        let kept = extract_all(&all, &out);
        let written = tree(&out);
        let singles = ["mine.txt", "ok.txt"].map(|n| single_file(&out.join(n)));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(refused, [true; 2]);
        assert!(left.iter().all(|(p, _)| !p.contains('\\')), "nothing inside the folders: {left:?}");
        assert!(single_file(Path::new("Cargo.toml")));
        assert_eq!(kept.unwrap_err().message(), crate::i18n::trf("err.extract_kept", &[("count", &2)]));
        assert_eq!(written, [("mine.txt".to_string(), false), ("ok.txt".to_string(), false)]);
        assert_eq!(singles, [true; 2]);
    }

    #[test]
    fn escaping_entries() {
        assert!(escapes("..\\..\\x") && escapes("a\\..\\b") && escapes(".."));
        assert!(escapes("C:x") && escapes("C:\\Windows\\x") && escapes("a.txt:stream"));
        assert!(!escapes("a\\..b\\c") && !escapes("docs\\a.txt"));
        for bad in [".. ", "a\\.. \\b", "...", "a\\.\\b", "a\\\\b", "a.txt.", "a ", "CON", "con.txt", "Aux .log", "COM1.jpg", "x\\lpt9", "COM¹", "CONIN$", "nul"] {
            assert!(escapes(bad), "{bad:?}");
        }
        for good in ["CONSOLE.txt", "COM10.txt", "COMX", "lpt", "..b", "a..b", "a. b", ".gitignore", "café\\ñ.txt"] {
            assert!(!escapes(good), "{good:?}");
        }
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
        for seed in 0..500 {
            fuzz_check(&crate::fuzz::Rng::new(seed).text(crate::fuzz::TOKENS, 20));
        }
    }
}
