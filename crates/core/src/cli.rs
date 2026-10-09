//! Command-line parsing for the app, and the payload sent to an already running instance
//! (WM_COPYDATA). Pure Rust, no shell or Win32 calls.

#[derive(Debug, Default, PartialEq, Clone)]
pub struct Args {
    pub paths: Vec<String>,
    pub select: Option<String>,
    pub profile: Option<String>,
    pub new_tab: bool,
    pub register: bool,
    pub unregister: bool,
    pub recover: bool,
    pub search: Option<String>,
    pub help: bool,
}

/// Value of `--name value` or `--name=value`; None when `a` is not that option.
fn long_value(
    a: &str,
    name: &str,
    it: &mut impl Iterator<Item = String>,
) -> Option<std::result::Result<String, String>> {
    if a == name {
        return Some(it.next().ok_or_else(|| format!("{name} needs a value")));
    }
    a.strip_prefix(name)?.strip_prefix('=').map(|v| Ok(v.to_string()))
}

/// Parse arguments (without argv[0]).
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> std::result::Result<Args, String> {
    let mut out = Args::default();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if let Some(v) = long_value(&a, "--select", &mut it) {
            out.select = Some(v?);
            continue;
        }
        if let Some(v) = long_value(&a, "--profile", &mut it) {
            out.profile = Some(v?);
            continue;
        }
        if let Some(v) = long_value(&a, "--search", &mut it) {
            out.search = Some(v?);
            continue;
        }
        match a.as_str() {
            "--new-tab" | "-t" => out.new_tab = true,
            "--register" => out.register = true,
            "--unregister" => out.unregister = true,
            "--recover" => out.recover = true,
            "-h" | "--help" => out.help = true,
            _ if a.starts_with('-') => return Err(format!("unknown option: {a}")),
            _ if a.starts_with('/') => {
                let lower = a.to_ascii_lowercase();
                if lower.starts_with("/select,") {
                    let rest = &a["/select,".len()..];
                    out.select = Some(if rest.is_empty() {
                        it.next().ok_or("/select, needs a path")?
                    } else {
                        rest.to_string()
                    });
                } else if lower.starts_with("/root,") || lower.starts_with("/e,") || lower.starts_with("/n,") {
                    let rest = &a[a.find(',').unwrap() + 1..];
                    if !rest.is_empty() {
                        out.paths.push(rest.to_string());
                    }
                } else if lower != "/e" && lower != "/n" {
                    return Err(format!("unknown option: {a}"));
                }
            }
            _ => out.paths.push(a),
        }
    }
    Ok(out)
}

pub fn usage() -> String {
    "rexplorer [options] [folder...]\n\
     \x20 --select <path>     open the parent folder with <path> selected (also /select,<path>)\n\
     \x20 --profile <name>    use the named settings profile\n\
     \x20 --new-tab, -t       open in a new tab of the running window\n\
     \x20 --search <text>     start with a search for <text>\n\
     \x20 --register          register as a shell handler\n\
     \x20 --unregister        remove the shell handler registration\n\
     \x20 --recover           restore the previous session after a crash\n\
     \x20 -h, --help          show this help\n"
        .to_string()
}

/// Serialise for WM_COPYDATA: one `key=value` per line.
pub fn payload(args: &Args) -> String {
    let mut s = String::new();
    for p in &args.paths {
        s.push_str(&format!("path={p}\n"));
    }
    if let Some(p) = &args.select {
        s.push_str(&format!("select={p}\n"));
    }
    if let Some(q) = &args.search {
        s.push_str(&format!("search={q}\n"));
    }
    if args.new_tab {
        s.push_str("newtab=1\n");
    }
    s
}

pub fn parse_payload(text: &str) -> Args {
    let mut a = Args::default();
    for line in text.lines() {
        match line.split_once('=') {
            Some(("path", v)) => a.paths.push(v.to_string()),
            Some(("select", v)) => a.select = Some(v.to_string()),
            Some(("search", v)) => a.search = Some(v.to_string()),
            Some(("newtab", "1")) => a.new_tab = true,
            _ => {}
        }
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> std::result::Result<Args, String> {
        parse(a.iter().map(|s| s.to_string()))
    }

    #[test]
    fn options_and_paths() {
        let a = p(&["C:\\a", "--profile", "work", "-t", "--register", "--unregister", "--recover", "-h", "D:\\b"]).unwrap();
        assert_eq!(a.paths, ["C:\\a", "D:\\b"]);
        assert_eq!(a.profile.as_deref(), Some("work"));
        assert!(a.new_tab && a.register && a.unregister && a.recover && a.help);
        assert!(p(&["--new-tab"]).unwrap().new_tab);
        assert!(p(&["--help"]).unwrap().help);
    }

    #[test]
    fn equals_forms() {
        let a = p(&["--select=C:\\x\\y.txt", "--profile=p", "--search=a b"]).unwrap();
        assert_eq!(a.select.as_deref(), Some("C:\\x\\y.txt"));
        assert_eq!(a.profile.as_deref(), Some("p"));
        assert_eq!(a.search.as_deref(), Some("a b"));
        assert_eq!(p(&["--search", "q"]).unwrap().search.as_deref(), Some("q"));
        assert_eq!(p(&["--select", "C:\\f"]).unwrap().select.as_deref(), Some("C:\\f"));
    }

    #[test]
    fn explorer_style() {
        assert_eq!(p(&["/select,C:\\f.txt"]).unwrap().select.as_deref(), Some("C:\\f.txt"));
        assert_eq!(p(&["/select,", "C:\\f.txt"]).unwrap().select.as_deref(), Some("C:\\f.txt"));
        let a = p(&["/e", "/n", "/root,C:\\r"]).unwrap();
        assert_eq!(a.paths, ["C:\\r"]);
        assert_eq!(a.select, None);
    }

    #[test]
    fn errors() {
        assert_eq!(p(&["--bogus"]).unwrap_err(), "unknown option: --bogus");
        assert!(p(&["/zzz"]).unwrap_err().starts_with("unknown option"));
        assert!(p(&["--select"]).is_err());
        assert!(p(&["/select,"]).is_err());
        assert!(p(&["--profile"]).is_err());
    }

    #[test]
    fn payload_round_trip() {
        let a = Args {
            paths: vec!["C:\\a".into(), "D:\\b c".into()],
            select: Some("C:\\a\\f.txt".into()),
            search: Some("x=y".into()),
            new_tab: true,
            ..Default::default()
        };
        assert_eq!(parse_payload(&payload(&a)), a);
        assert_eq!(parse_payload("junk\nfoo=bar\n"), Args::default());
    }

    #[test]
    fn usage_mentions_every_option() {
        let u = usage();
        assert!(u.starts_with("rexplorer [options] [folder...]\n"));
        for o in ["--select", "/select,", "--profile", "--new-tab", "-t", "--register", "--unregister", "--recover", "--search", "-h", "--help"] {
            assert!(u.contains(o), "{o}");
        }
    }
}
