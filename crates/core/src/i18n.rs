//! Localization: one lookup (`tr` / `trf` / `trn`), English built in, Spanish shipped, `lang\<code>.ini` overrides.
//! Pure Rust, no Win32.

use std::collections::HashMap;
use std::fmt::Display;
use std::path::Path;
use std::sync::OnceLock;

pub const EN: &str = include_str!("lang/en.ini");
pub const ES: &str = include_str!("lang/es.ini");
pub const BUILTIN: [(&str, &str); 2] = [("en", EN), ("es", ES)];
/// First segment of every key.
pub const AREAS: [&str; 19] =
    ["menu", "tb", "status", "group", "col", "home", "side", "fo", "conn", "pal", "preview", "tabs", "desk", "dlg", "err", "num", "cli", "inst", "reg"];

pub struct Table {
    map: HashMap<String, String>,
}

fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut it = v.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

impl Table {
    /// All sections flattened, last duplicate wins.
    pub fn parse(text: &str) -> HashMap<String, String> {
        let mut m = HashMap::new();
        for (_, keys) in crate::settings::parse_ini(text) {
            for (k, v) in keys {
                m.insert(k, unescape(&v));
            }
        }
        m
    }

    /// English, then `lang` (ini text) over it.
    pub fn new(lang: Option<&str>) -> Table {
        let mut map = Table::parse(EN);
        if let Some(t) = lang {
            map.extend(Table::parse(t));
        }
        Table { map }
    }

    /// English, the built-in table for `code`, then `<dir>\lang\<code>.ini` when it exists, is UTF-8 and under 1 MiB.
    /// An emptied value in that file keeps the text under it.
    pub fn load(dir: &Path, code: &str) -> Table {
        let code = code.to_ascii_lowercase();
        let mut t = Table::new(BUILTIN.iter().find(|(c, _)| *c == code).map(|(_, s)| *s));
        let p = dir.join("lang").join(format!("{code}.ini"));
        if std::fs::metadata(&p).is_ok_and(|m| m.len() <= 1 << 20)
            && let Ok(text) = std::fs::read_to_string(&p)
        {
            t.map.extend(Table::parse(&text).into_iter().filter(|(_, v)| !v.is_empty()));
        }
        t
    }

    pub fn get<'a>(&'a self, key: &'a str) -> &'a str {
        self.map.get(key).map_or(key, String::as_str)
    }

    /// One pass over the value, so an argument's own text (a file named `{error}`) is never substituted again.
    pub fn fmt(&self, key: &str, args: &[(&str, &dyn Display)]) -> String {
        let (mut out, mut rest) = (String::new(), self.get(key));
        while let Some(i) = rest.find('{') {
            out.push_str(&rest[..i]);
            rest = &rest[i..];
            match rest.find('}').and_then(|j| args.iter().find(|(n, _)| *n == &rest[1..j]).map(|(_, v)| (j, v))) {
                Some((j, v)) => {
                    out.push_str(&v.to_string());
                    rest = &rest[j + 1..];
                }
                None => {
                    out.push('{');
                    rest = &rest[1..];
                }
            }
        }
        out + rest
    }

    pub fn plural(&self, base: &str, n: u64) -> String {
        let key = format!("{base}.{}", if n == 1 { "one" } else { "many" });
        self.fmt(&key, &[("n", &n)])
    }
}

/// `en`, `es` and the lowercase stems of `<dir>\lang\*.ini`.
pub fn available(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = BUILTIN.iter().map(|(c, _)| c.to_string()).collect();
    for e in std::fs::read_dir(dir.join("lang")).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ini"))
            && let Some(s) = p.file_stem().and_then(|s| s.to_str())
        {
            let s = s.to_ascii_lowercase();
            if !v.contains(&s) {
                v.push(s);
            }
        }
    }
    v
}

/// An explicit unknown setting gives English, never the system language.
pub fn resolve(setting: &str, system: &str, available: &[String]) -> String {
    let setting = setting.trim();
    let want = if setting.is_empty() || setting.eq_ignore_ascii_case("system") { system } else { setting }.to_ascii_lowercase();
    let want = want.replace('_', "-");
    let primary = want.split('-').next().unwrap_or("").to_string();
    [want, primary].into_iter().find(|c| available.contains(c)).unwrap_or_else(|| "en".into())
}

static ACTIVE: OnceLock<Table> = OnceLock::new();

/// Picks the language and freezes the table; a second call is ignored. Nothing may call `tr` before this.
pub fn init(dir: &Path, setting: &str, system: &str) -> String {
    let code = resolve(setting, system, &available(dir));
    let _ = ACTIVE.set(Table::load(dir, &code));
    code
}

fn active() -> &'static Table {
    ACTIVE.get_or_init(|| Table::new(None))
}

pub fn tr(key: &'static str) -> &'static str {
    active().get(key)
}

pub fn trf(key: &'static str, args: &[(&str, &dyn Display)]) -> String {
    active().fmt(key, args)
}

pub fn trn(base: &'static str, n: u64) -> String {
    active().plural(base, n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rexplorer-i18n-{}", std::process::id())).join(tag);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn cleanup(tag: &str) {
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("rexplorer-i18n-{}", std::process::id())).join(tag));
    }

    fn key_shape(k: &str) -> bool {
        k.contains('.')
            && k.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
            && AREAS.contains(&k.split('.').next().unwrap())
    }

    fn placeholders(v: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = v;
        while let Some(i) = rest.find('{') {
            let Some(j) = rest[i..].find('}') else { break };
            out.push(rest[i + 1..i + j].to_string());
            rest = &rest[i + j + 1..];
        }
        out.sort();
        out
    }

    fn accels(v: &str) -> usize {
        let b = v.as_bytes();
        let (mut n, mut i) = (0, 0);
        while i < b.len() {
            if b[i] == b'&' {
                if b.get(i + 1) == Some(&b'&') {
                    i += 2;
                    continue;
                }
                n += 1;
            }
            i += 1;
        }
        n
    }

    #[test]
    fn english_table_is_well_formed() {
        let en = Table::parse(EN);
        assert!(!en.is_empty());
        let mut bad = Vec::new();
        for (k, v) in &en {
            if !key_shape(k) {
                bad.push(format!("key {k}"));
            }
            if v.is_empty() || v.contains('\t') || v.starts_with(' ') || v.ends_with(' ') || v.contains("{}") {
                bad.push(format!("value of {k}: {v:?}"));
            }
        }
        assert!(bad.is_empty(), "{bad:?}");
    }

    #[test]
    fn missing_keys_fall_back_to_english() {
        let t = Table::new(Some("status.items.many={n} cosas"));
        assert_eq!(t.get("status.items.many"), "{n} cosas");
        assert_eq!(t.get("status.items.one"), Table::new(None).get("status.items.one"));
        assert_eq!(t.get("nope.key"), "nope.key");
    }

    #[test]
    fn escapes() {
        let t = Table::new(Some(r"err.open=a\nb\\c"));
        assert_eq!(t.get("err.open"), "a\nb\\c");
    }

    #[test]
    fn named_placeholders_any_order() {
        let t = Table::new(Some("dlg.about=x {b} then {a}"));
        assert_eq!(t.fmt("dlg.about", &[("a", &1), ("b", &2)]), "x 2 then 1");
        assert_eq!(t.fmt("dlg.about", &[]), "x {b} then {a}");
        // An argument's own text (a file named "{a}") is never substituted again.
        assert_eq!(t.fmt("dlg.about", &[("b", &"{a}"), ("a", &"é")]), "x {a} then é");
        assert_eq!(Table::new(Some("dlg.about={a")).fmt("dlg.about", &[("a", &1)]), "{a");
    }

    #[test]
    fn menu_accelerators_are_unique() {
        // One group per menu key prefix (menu.view, menu.list, tabs, desk...): no two entries share an `&` letter.
        for (lang, text) in BUILTIN {
            let mut seen: HashMap<(String, char), String> = HashMap::new();
            let mut dup = Vec::new();
            for (k, v) in Table::parse(text) {
                let group = k.rsplit_once('.').map_or("", |(g, _)| g).to_string();
                let b: Vec<char> = v.chars().collect();
                let at = (0..b.len()).find(|&i| b[i] == '&' && b.get(i + 1).is_some_and(|c| *c != '&') && (i == 0 || b[i - 1] != '&'));
                if let Some(i) = at.filter(|_| k.starts_with("menu.") || k.starts_with("tabs.") || k.starts_with("desk."))
                    && let Some(other) = seen.insert((group, b[i + 1].to_lowercase().next().unwrap()), k.clone())
                {
                    dup.push(format!("{lang}: {k} / {other}"));
                }
            }
            assert!(dup.is_empty(), "{dup:?}");
        }
    }

    #[test]
    fn plurals() {
        let en = Table::new(None);
        assert_eq!(en.plural("status.items", 0), "0 items");
        assert_eq!(en.plural("status.items", 1), "1 item");
        assert_eq!(en.plural("status.items", 2), "2 items");
        assert_eq!(en.plural("status.selected", 1), "1 selected");
        let es = Table::new(Some(ES));
        assert_eq!(es.plural("status.items", 1), "1 elemento");
        assert_eq!(es.plural("status.items", 5), "5 elementos");
        assert_eq!(es.plural("status.selected", 1), "1 seleccionado");
        assert_eq!(es.plural("status.selected", 5), "5 seleccionados");
    }

    #[test]
    fn spanish_is_complete() {
        let (en, es) = (Table::parse(EN), Table::parse(ES));
        let ek: BTreeSet<_> = en.keys().collect();
        let sk: BTreeSet<_> = es.keys().collect();
        let missing: Vec<_> = ek.difference(&sk).collect();
        let extra: Vec<_> = sk.difference(&ek).collect();
        assert!(missing.is_empty() && extra.is_empty(), "missing in es: {missing:?}; extra in es: {extra:?}");
        let mut bad = Vec::new();
        for (k, ev) in &en {
            let sv = &es[k];
            if sv.is_empty() {
                bad.push(format!("{k}: empty"));
            }
            if placeholders(ev) != placeholders(sv) {
                bad.push(format!("{k}: placeholders {ev:?} vs {sv:?}"));
            }
            if accels(ev) != accels(sv) {
                bad.push(format!("{k}: & count {ev:?} vs {sv:?}"));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
        assert_eq!((en["num.decimal"].as_str(), es["num.decimal"].as_str()), (".", ","));
    }

    #[test]
    fn resolve_language() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(resolve("system", "es-ES", &a(&["en", "es"])), "es");
        assert_eq!(resolve("System", "fr-FR", &a(&["en", "es"])), "en");
        assert_eq!(resolve("system", "es-MX", &a(&["en", "es", "es-mx"])), "es-mx");
        assert_eq!(resolve("ES", "en-US", &a(&["en", "es"])), "es");
        assert_eq!(resolve("de", "es-ES", &a(&["en", "es"])), "en");
        assert_eq!(resolve("", "es-ES", &a(&["en", "es"])), "es");
    }

    #[test]
    fn load_overlays_file_on_builtin() {
        let d = tmp("load");
        std::fs::create_dir_all(d.join("lang")).unwrap();
        std::fs::write(d.join("lang/es.ini"), "num.decimal=X\n").unwrap();
        std::fs::write(d.join("lang/xx.ini"), "status.items.one=uno {n}\n").unwrap();
        std::fs::write(d.join("lang/bad.ini"), [0xffu8, 0xfe, 0xfd, b'=', 0xc3]).unwrap();
        let es = Table::load(&d, "es");
        assert_eq!(es.get("num.decimal"), "X");
        assert_eq!(es.get("status.items.many"), Table::new(Some(ES)).get("status.items.many"));
        let xx = Table::load(&d, "xx");
        assert_eq!(xx.get("status.items.one"), "uno {n}");
        assert_eq!(xx.get("status.items.many"), Table::new(None).get("status.items.many"));
        assert_eq!(Table::load(&d, "bad").get("num.decimal"), ".");
        // A user's file: BOM, CRLF, garbage lines and an emptied value (which keeps the built-in text).
        std::fs::write(d.join("lang/yy.ini"), "\u{feff}garbage\r\n[x\r\nstatus.items.one=un {n}\r\nnum.decimal=\r\n=\r\n").unwrap();
        let yy = Table::load(&d, "yy");
        assert_eq!((yy.get("status.items.one"), yy.get("num.decimal")), ("un {n}", "."));
        // A runaway file is not read at all.
        std::fs::write(d.join("lang/zz.ini"), format!("num.decimal=Z\n;{}\n", "x".repeat(2 << 20))).unwrap();
        assert_eq!(Table::load(&d, "zz").get("num.decimal"), ".");
        let av = available(&d);
        assert!(["en", "es", "xx"].iter().all(|c| av.iter().any(|a| a == c)), "{av:?}");
        cleanup("load");
    }

    /// Every `"<area>.…"` literal in non-test code: (file:line, literal).
    fn key_literals() -> Vec<(String, String)> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut out = Vec::new();
        for dir in [root.join("src"), root.join("../app/src")] {
            let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
            files.sort();
            for p in files.into_iter().filter(|p| p.extension().is_some_and(|x| x == "rs")) {
                let text = std::fs::read_to_string(&p).unwrap();
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                let all: Vec<&str> = text.lines().collect();
                for (i, line) in all.iter().enumerate() {
                    // The test module (`mod x {`), not a `#[cfg(test)] mod x;` declaration at a file's top.
                    if line.trim() == "#[cfg(test)]" && all.get(i + 1).is_some_and(|n| n.trim().starts_with("mod ") && n.trim_end().ends_with('{')) {
                        break;
                    }
                    for area in AREAS {
                        let pat = format!("\"{area}.");
                        let mut rest: &str = line;
                        while let Some(at) = rest.find(&pat) {
                            let s = &rest[at + 1..];
                            let lit = &s[..s.find('"').unwrap_or(s.len())];
                            if key_shape(lit) {
                                out.push((format!("{name}:{}", i + 1), lit.to_string()));
                            }
                            rest = &rest[at + 1..];
                        }
                    }
                }
            }
        }
        out
    }

    #[test]
    fn every_key_in_code_is_english() {
        let en = Table::parse(EN);
        let miss: Vec<_> = key_literals()
            .into_iter()
            .filter(|(_, k)| !(en.contains_key(k) || (en.contains_key(&format!("{k}.one")) && en.contains_key(&format!("{k}.many")))))
            .map(|(w, k)| format!("{w} {k}"))
            .collect();
        assert!(miss.is_empty(), "{miss:#?}");
    }

    #[test]
    fn every_english_key_is_used() {
        let used: BTreeSet<String> = key_literals().into_iter().map(|(_, k)| k).collect();
        let unused: Vec<_> = Table::parse(EN)
            .keys()
            .filter(|k| {
                let base = k.strip_suffix(".one").or_else(|| k.strip_suffix(".many"));
                !(used.contains(*k) || base.is_some_and(|b| used.contains(b)))
            })
            .cloned()
            .collect();
        assert!(unused.is_empty(), "{unused:#?}");
    }

    #[test]
    fn fuzz_language_files_never_drop_the_english_keys() {
        let en = Table::parse(EN);
        for seed in 0..300 {
            let text = crate::fuzz::Rng::new(seed).text(crate::fuzz::TOKENS, 80);
            let t = Table::new(Some(&text));
            assert!(en.keys().all(|k| t.map.contains_key(k)), "seed {seed}");
            // Placeholder substitution on junk templates must not panic either.
            let mut junk = Table::new(None);
            junk.map.insert("k".into(), text.clone());
            let _ = junk.fmt("k", &[("a", &1), ("b", &"x")]);
        }
    }
}
