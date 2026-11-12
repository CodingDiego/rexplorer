//! The search box's syntax, as Explorer's: plain words (all must be in the name) plus the filters `kind:`, `date:`,
//! `size:` and `ext:`, with their Spanish names (`tipo:`, `fecha:`, `tamaño:`) and values. A token that is not a
//! filter, or a filter with a value it does not know, is a plain word (so it matches nothing, as in Explorer).
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Folder,
    Picture,
    Document,
    Music,
    Video,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// Lowercase; each one must be in the name.
    pub words: Vec<String>,
    kind: Option<Kind>,
    /// Local days since 1970-01-01 the item was modified on, `[from, to)`.
    date: Option<(i64, i64)>,
    /// Bytes, `[min, max)`; files only.
    size: Option<(u64, u64)>,
    /// Lowercase extensions without the dot; any of them.
    ext: Vec<String>,
}

const PICTURE: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "ico", "tif", "tiff", "heic", "heif", "avif", "jxl", "svg", "raw", "dng", "cr2", "nef", "arw",
];
const DOCUMENT: &[&str] = &[
    "txt", "rtf", "doc", "docx", "docm", "odt", "pdf", "xps", "md", "csv", "xls", "xlsx", "xlsm", "ods", "ppt", "pptx", "odp", "epub", "one",
];
const MUSIC: &[&str] = &["mp3", "wav", "flac", "aac", "m4a", "ogg", "oga", "opus", "wma", "aif", "aiff", "mid", "midi"];
const VIDEO: &[&str] = &["mp4", "m4v", "mkv", "avi", "mov", "wmv", "webm", "mpg", "mpeg", "flv", "3gp", "ts", "mts", "m2ts"];

fn kind_of(value: &str) -> Option<Kind> {
    Some(match value {
        "folder" | "folders" | "carpeta" | "carpetas" => Kind::Folder,
        "picture" | "pictures" | "pic" | "image" | "images" | "photo" | "photos" | "imagen" | "imagenes" | "imágenes" | "foto" | "fotos" => Kind::Picture,
        "document" | "documents" | "doc" | "docs" | "documento" | "documentos" => Kind::Document,
        "music" | "song" | "songs" | "audio" | "música" | "musica" | "canción" | "cancion" | "canciones" => Kind::Music,
        "video" | "videos" | "movie" | "movies" | "vídeo" | "vídeos" | "película" | "pelicula" | "películas" | "peliculas" => Kind::Video,
        _ => return None,
    })
}

/// Explorer's size names (Windows 10 and 11 boundaries).
fn size_name(value: &str) -> Option<(u64, u64)> {
    const K: u64 = 1024;
    Some(match value {
        "empty" | "vacío" | "vacio" => (0, 1),
        "tiny" | "diminuto" => (1, 16 * K + 1),
        "small" | "pequeño" | "pequeno" => (16 * K + 1, K * K + 1),
        "medium" | "mediano" => (K * K + 1, 128 * K * K + 1),
        "large" | "grande" => (128 * K * K + 1, K * K * K + 1),
        "huge" | "enorme" => (K * K * K + 1, 4 * K * K * K + 1),
        "gigantic" | "gigantesco" => (4 * K * K * K + 1, u64::MAX),
        _ => return None,
    })
}

/// `10MB`, `1.5 GB` (one token), `500` bytes: the value and its unit's size (1024-based).
fn size_literal(value: &str) -> Option<(u64, u64)> {
    let split = value.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ',')).unwrap_or(value.len());
    let number: f64 = value[..split].replace(',', ".").parse().ok()?;
    let unit = match value[split..].trim() {
        "" | "b" => 1u64,
        "k" | "kb" => 1 << 10,
        "m" | "mb" => 1 << 20,
        "g" | "gb" => 1 << 30,
        "t" | "tb" => 1 << 40,
        _ => return None,
    };
    (number >= 0.0).then_some(((number * unit as f64) as u64, unit))
}

/// A size value as a byte interval: a name, or a literal as shown rounded down to its unit (`10MB` is 10 to 11 MB).
fn size_value(value: &str) -> Option<(u64, u64)> {
    size_name(value).or_else(|| size_literal(value).map(|(v, unit)| (v, v.saturating_add(unit))))
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's days_from_civil).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn month_start(y: i64, m: i64) -> i64 {
    days_from_civil(y + (m - 1).div_euclid(12), (m - 1).rem_euclid(12) + 1, 1)
}

/// A date value as a day interval. `today` in local days; `week_start` 0 = Monday .. 6 = Sunday.
fn date_value(value: &str, today: i64, week_start: i64) -> Option<(i64, i64)> {
    let (y, m, _) = civil_from_days(today);
    let monday_based = (today + 3).rem_euclid(7);
    let week = today - (monday_based - week_start).rem_euclid(7);
    Some(match value {
        "today" | "hoy" => (today, today + 1),
        "yesterday" | "ayer" => (today - 1, today),
        "this week" | "esta semana" => (week, week + 7),
        "last week" | "la semana pasada" | "semana pasada" => (week - 7, week),
        "this month" | "este mes" => (month_start(y, m), month_start(y, m + 1)),
        "last month" | "el mes pasado" | "mes pasado" => (month_start(y, m - 1), month_start(y, m)),
        "this year" | "este año" | "este ano" => (month_start(y, 1), month_start(y + 1, 1)),
        "last year" | "el año pasado" | "año pasado" | "el ano pasado" | "ano pasado" => (month_start(y - 1, 1), month_start(y, 1)),
        _ => {
            let parts: Vec<i64> = value.split(['-', '/', '.']).map(|p| p.parse().ok()).collect::<Option<_>>()?;
            match parts[..] {
                [y] if (1601..=9999).contains(&y) => (month_start(y, 1), month_start(y + 1, 1)),
                [y, m] if (1601..=9999).contains(&y) && (1..=12).contains(&m) => (month_start(y, m), month_start(y, m + 1)),
                [y, m, d] if (1601..=9999).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&d) => {
                    let day = days_from_civil(y, m, d);
                    (day, day + 1)
                }
                _ => return None,
            }
        }
    })
}

/// `>x`, `>=x`, `<x`, `<=x`, `a..b` or a bare value, over the interval `parse` gives a value; `unbounded` is the
/// interval's open end on each side.
fn compare<T: Copy>(raw: &str, parse: impl Fn(&str) -> Option<(T, T)>, unbounded: (T, T)) -> Option<(T, T)> {
    if let Some(v) = raw.strip_prefix(">=") {
        parse(v).map(|(lo, _)| (lo, unbounded.1))
    } else if let Some(v) = raw.strip_prefix("<=") {
        parse(v).map(|(_, hi)| (unbounded.0, hi))
    } else if let Some(v) = raw.strip_prefix('>') {
        parse(v).map(|(_, hi)| (hi, unbounded.1))
    } else if let Some(v) = raw.strip_prefix('<') {
        parse(v).map(|(lo, _)| (unbounded.0, lo))
    } else if let Some((a, b)) = raw.split_once("..") {
        Some((parse(a)?.0, parse(b)?.1))
    } else {
        parse(raw)
    }
}

/// Words split at spaces outside double quotes; the quotes go.
fn tokens(text: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => out.extend((!cur.is_empty()).then(|| std::mem::take(&mut cur))),
            c => cur.push(c),
        }
    }
    out.extend((!cur.is_empty()).then_some(cur));
    out
}

/// Words that start a date phrase of up to three words (`last week`, `la semana pasada`).
const PHRASE_START: &[&str] = &["this", "last", "esta", "este", "la", "el", "semana", "mes", "año", "ano"];

impl Query {
    /// Parses `text` against the local date and the user's first day of the week.
    pub fn parse(text: &str) -> Query {
        Query::parse_at(text, local_day(filetime(SystemTime::now())).unwrap_or(0), week_start())
    }

    /// `today`: local days since 1970-01-01; `week_start`: 0 = Monday .. 6 = Sunday.
    pub fn parse_at(text: &str, today: i64, week_start: i64) -> Query {
        let mut q = Query::default();
        let toks: Vec<String> = tokens(&text.to_lowercase());
        let mut i = 0;
        while i < toks.len() {
            let tok = &toks[i];
            i += 1;
            let Some((key, value)) = tok.split_once(':').filter(|(_, v)| !v.is_empty()) else {
                q.words.push(tok.clone());
                continue;
            };
            let known = match key {
                "kind" | "tipo" | "clase" => kind_of(value).map(|k| q.kind = Some(k)),
                "ext" | "extension" | "extensión" | "fileext" => {
                    let exts: Vec<String> = value.split([';', ',']).map(|e| e.trim_start_matches(['.', '*']).to_string()).filter(|e| !e.is_empty()).collect();
                    (!exts.is_empty()).then(|| q.ext.extend(exts))
                }
                "size" | "tamaño" | "tamano" => {
                    // Compared, a literal is exact (`>10MB` is over 10 MB); alone it is rounded (`size_value`).
                    let point = |v: &str| size_name(v).or_else(|| size_literal(v).map(|(v, _)| (v, v.saturating_add(1))));
                    let r = if value.starts_with(['<', '>']) || value.contains("..") { compare(value, point, (0, u64::MAX)) } else { size_value(value) };
                    r.map(|r| q.size = Some(r))
                }
                "date" | "datemodified" | "modified" | "fecha" | "fechamodificación" | "modificado" => {
                    let bare = value.trim_start_matches(['>', '<', '=']);
                    // The longest phrase the next words complete: `date:last week`.
                    let mut found = None;
                    if PHRASE_START.contains(&bare) {
                        for take in (1..=2).rev() {
                            if i + take <= toks.len() {
                                let phrase = format!("{value} {}", toks[i..i + take].join(" "));
                                if let Some(r) = compare(&phrase, |v| date_value(v, today, week_start), (i64::MIN, i64::MAX)) {
                                    found = Some(r);
                                    i += take;
                                    break;
                                }
                            }
                        }
                    }
                    found.or_else(|| compare(value, |v| date_value(v, today, week_start), (i64::MIN, i64::MAX))).map(|r| q.date = Some(r))
                }
                _ => None,
            };
            if known.is_none() {
                q.words.push(tok.clone());
            }
        }
        q
    }

    /// Nothing to search for.
    pub fn is_empty(&self) -> bool {
        self == &Query::default()
    }

    /// Whether every word is in `name`, ignoring case.
    pub fn name_matches(&self, name: &str) -> bool {
        let name = name.to_lowercase();
        self.words.iter().all(|w| name.contains(w.as_str()))
    }

    /// Whether a filter needs more than the name.
    pub fn needs_meta(&self) -> bool {
        self.kind.is_some() || self.date.is_some() || self.size.is_some() || !self.ext.is_empty()
    }

    /// The filters, for an item named `file_name` (extension included). `size` and `modified_day` (local days) are
    /// only asked for when a filter needs them.
    pub fn meta_matches(&self, file_name: &str, is_folder: bool, size: impl FnOnce() -> Option<u64>, modified_day: impl FnOnce() -> Option<i64>) -> bool {
        let ext = Path::new(file_name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let ext_in = |list: &[&str]| !is_folder && list.contains(&ext.as_str());
        let kind = match self.kind {
            None => true,
            Some(Kind::Folder) => is_folder,
            Some(Kind::Picture) => ext_in(PICTURE),
            Some(Kind::Document) => ext_in(DOCUMENT),
            Some(Kind::Music) => ext_in(MUSIC),
            Some(Kind::Video) => ext_in(VIDEO),
        };
        let exts = self.ext.is_empty() || !is_folder && self.ext.contains(&ext);
        let sized = self.size.is_none_or(|(lo, hi)| !is_folder && size().is_some_and(|s| (lo..hi).contains(&s)));
        sized && kind && exts && self.date.is_none_or(|(from, to)| modified_day().is_some_and(|d| (from..to).contains(&d)))
    }

    /// The name and the filters for a file system path, its metadata read from disk only when a filter needs it.
    pub fn path_matches(&self, path: &Path) -> bool {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !self.name_matches(&name) {
            return false;
        }
        if !self.needs_meta() {
            return true;
        }
        let Ok(md) = std::fs::metadata(path) else { return false };
        self.meta_matches(&name, md.is_dir(), || Some(md.len()), || md.modified().ok().and_then(|t| local_day(filetime(t))))
    }
}

/// A `SystemTime` as a FILETIME (100 ns since 1601, UTC).
pub fn filetime(t: SystemTime) -> FILETIME {
    let ticks = match t.duration_since(UNIX_EPOCH) {
        Ok(d) => 116_444_736_000_000_000u64.saturating_add((d.as_nanos() / 100) as u64),
        Err(e) => 116_444_736_000_000_000u64.saturating_sub((e.duration().as_nanos() / 100) as u64),
    };
    FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 }
}

/// The local calendar day (days since 1970-01-01) of a UTC FILETIME, with that date's daylight saving rule.
pub fn local_day(ft: FILETIME) -> Option<i64> {
    unsafe {
        let mut utc = Default::default();
        FileTimeToSystemTime(&ft, &mut utc).ok()?;
        let mut local = Default::default();
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
        Some(days_from_civil(local.wYear as i64, local.wMonth as i64, local.wDay as i64))
    }
}

/// The user's first day of the week (Region settings), 0 = Monday .. 6 = Sunday; Monday when unset.
fn week_start() -> i64 {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::w;
    let mut buf = [0u16; 8];
    let mut len = std::mem::size_of_val(&buf) as u32;
    let read = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, w!(r"Control Panel\International"), w!("iFirstDayOfWeek"), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut len))
    };
    let text = String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)]);
    read.ok().ok().and_then(|()| text.trim().parse().ok()).filter(|d| (0..7).contains(d)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wednesday 2026-11-11.
    fn today() -> i64 {
        days_from_civil(2026, 11, 11)
    }

    fn p(text: &str) -> Query {
        Query::parse_at(text, today(), 0)
    }

    fn day(y: i64, m: i64, d: i64) -> i64 {
        days_from_civil(y, m, d)
    }

    #[test]
    fn civil_round_trip() {
        for z in [-800_000, -1, 0, 1, 19_000, day(2024, 2, 29), day(2026, 11, 11), 2_000_000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z, "{z}");
        }
        assert_eq!(day(1970, 1, 1), 0);
        assert_eq!((today() + 3).rem_euclid(7), 2, "2026-11-11 is a Wednesday");
    }

    #[test]
    fn plain_words_and_quotes() {
        let q = p(r#"Report  "two words" C:x"#);
        assert_eq!(q.words, ["report", "two words", "c:x"]);
        assert!(!q.needs_meta());
        assert!(q.name_matches("My Report with two words in C:x"));
        assert!(!q.name_matches("report"));
        assert!(p("").is_empty() && p("   ").is_empty());
        assert!(!p("kind:folder").is_empty());
        assert!(p("kind:folder").name_matches("anything"), "no words: every name");
    }

    #[test]
    fn kinds_including_spanish() {
        let m = |q: &str, name: &str, folder: bool| p(q).meta_matches(name, folder, || None, || None);
        assert!(m("kind:folder", "Docs", true) && !m("kind:folder", "a.txt", false));
        assert!(m("tipo:carpeta", "Docs", true));
        assert!(m("kind:picture", "a.JPG", false) && m("kind:pic", "a.heic", false) && m("tipo:imagen", "a.png", false));
        assert!(!m("kind:picture", "a.txt", false) && !m("kind:picture", "pics.jpg", true));
        assert!(m("kind:document", "a.docx", false) && m("tipo:documento", "a.pdf", false));
        assert!(m("kind:music", "a.flac", false) && m("tipo:música", "a.mp3", false));
        assert!(m("kind:video", "a.mkv", false) && m("tipo:vídeo", "a.mp4", false) && !m("kind:video", "a.mp3", false));
        assert_eq!(p("kind:banana").words, ["kind:banana"], "an unknown kind is a plain word");
    }

    #[test]
    fn dates_relative_and_absolute() {
        let d = |q: &str| p(q).date;
        assert_eq!(d("date:today"), Some((today(), today() + 1)));
        assert_eq!(d("fecha:ayer"), Some((today() - 1, today())));
        // Week from Monday 2026-11-09.
        assert_eq!(d("date:this week"), Some((day(2026, 11, 9), day(2026, 11, 16))));
        assert_eq!(d("date:last week"), Some((day(2026, 11, 2), day(2026, 11, 9))));
        assert_eq!(d("fecha:la semana pasada"), Some((day(2026, 11, 2), day(2026, 11, 9))));
        assert_eq!(Query::parse_at("date:this week", today(), 6).date, Some((day(2026, 11, 8), day(2026, 11, 15))), "weeks from Sunday");
        assert_eq!(d("date:this month"), Some((day(2026, 11, 1), day(2026, 12, 1))));
        assert_eq!(d("date:last month"), Some((day(2026, 10, 1), day(2026, 11, 1))));
        assert_eq!(Query::parse_at("date:last month", day(2026, 1, 15), 0).date, Some((day(2025, 12, 1), day(2026, 1, 1))), "across a year");
        assert_eq!(d("date:this year"), Some((day(2026, 1, 1), day(2027, 1, 1))));
        assert_eq!(d("fecha:el año pasado"), Some((day(2025, 1, 1), day(2026, 1, 1))));
        assert_eq!(d("date:2024"), Some((day(2024, 1, 1), day(2025, 1, 1))));
        assert_eq!(d("date:2024-02"), Some((day(2024, 2, 1), day(2024, 3, 1))));
        assert_eq!(d("date:>2024-01-01"), Some((day(2024, 1, 2), i64::MAX)));
        assert_eq!(d("date:>=2024-01-01"), Some((day(2024, 1, 1), i64::MAX)));
        assert_eq!(d("date:<2024"), Some((i64::MIN, day(2024, 1, 1))));
        assert_eq!(d("date:<=2024-01-01"), Some((i64::MIN, day(2024, 1, 2))));
        assert_eq!(d("date:2023..2024-06"), Some((day(2023, 1, 1), day(2024, 7, 1))));
        let q = p("report date:last week notes");
        assert_eq!(q.words, ["report", "notes"], "the phrase takes only its own words");
        assert_eq!(p("date:last").words, ["date:last"]);
        assert_eq!(p("date:2024-13-01").words, ["date:2024-13-01"]);
        let m = |q: &str, d: i64| p(q).meta_matches("a.txt", false, || None, || Some(d));
        assert!(m("date:today", today()) && !m("date:today", today() - 1));
        assert!(!p("date:today").meta_matches("a.txt", false, || None, || None), "no date: no match");
    }

    #[test]
    fn sizes_named_and_compared() {
        const K: u64 = 1024;
        let s = |q: &str| p(q).size;
        assert_eq!(s("size:empty"), Some((0, 1)));
        assert_eq!(s("tamaño:diminuto"), Some((1, 16 * K + 1)));
        assert_eq!(s("size:gigantic"), Some((4 * K * K * K + 1, u64::MAX)));
        assert_eq!(s("size:>10MB"), Some((10 * K * K + 1, u64::MAX)));
        assert_eq!(s("size:>medium"), Some((128 * K * K + 1, u64::MAX)));
        assert_eq!(s("size:>=10mb"), Some((10 * K * K, u64::MAX)));
        assert_eq!(s("size:<1kb"), Some((0, K)));
        assert_eq!(s("size:1.5GB"), Some((3 * K * K * K / 2, 5 * K * K * K / 2)));
        assert_eq!(s("size:100..200"), Some((100, 201)));
        assert_eq!(p("size:banana").words, ["size:banana"]);
        let m = |q: &str, n: u64, folder: bool| p(q).meta_matches("a.bin", folder, || Some(n), || None);
        assert!(m("size:empty", 0, false) && !m("size:empty", 1, false) && !m("size:empty", 0, true), "folders have no size");
        assert!(m("size:medium", 5 * K * K, false) && !m("size:medium", 200 * K * K, false));
        assert!(m("size:>10MB", 12 * K * K, false) && !m("size:>10MB", 10 * K * K, false));
    }

    #[test]
    fn extensions_and_combinations() {
        let q = p("ext:.TXT ext:md;csv");
        assert_eq!(q.ext, ["txt", "md", "csv"]);
        assert!(q.meta_matches("a.txt", false, || None, || None) && q.meta_matches("b.csv", false, || None, || None));
        assert!(!q.meta_matches("a.txt.bak", false, || None, || None) && !q.meta_matches("x.txt", true, || None, || None));
        let q = p("budget kind:document size:>1kb date:this year");
        assert_eq!(q.words, ["budget"]);
        let ok = |name: &str, size: u64, d: i64| q.name_matches(name) && q.meta_matches(name, false, || Some(size), || Some(d));
        assert!(ok("Budget 2026.xlsx", 4096, today()));
        assert!(!ok("Budget 2026.xlsx", 10, today()), "too small");
        assert!(!ok("Budget 2026.png", 4096, today()), "not a document");
        assert!(!ok("Budget 2026.xlsx", 4096, day(2025, 12, 31)), "last year");
        assert!(!ok("Plan.xlsx", 4096, today()), "the word");
        // Only what a filter needs is asked for.
        let q = p("kind:folder");
        assert!(q.meta_matches("Docs", true, || panic!("size asked"), || panic!("date asked")));
    }

    #[test]
    fn paths_read_metadata_from_disk() {
        let dir = std::env::temp_dir().join(format!("rexplorer-query-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("photo.jpg"), vec![0u8; 2048]).unwrap();
        let real = local_day(filetime(SystemTime::now())).unwrap();
        let q = |text: &str| Query::parse_at(text, real, 0);
        let checks = [
            q("photo kind:picture size:>1kb date:today").path_matches(&dir.join("photo.jpg")),
            !q("kind:picture date:yesterday").path_matches(&dir.join("photo.jpg")),
            q("kind:folder").path_matches(&dir.join("sub")),
            !q("kind:folder").path_matches(&dir.join("photo.jpg")),
            !q("kind:picture").path_matches(&dir.join("missing.jpg")),
            Query::parse("date:today").date == Some((real, real + 1)),
        ];
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(checks, [true; 6]);
    }
}
