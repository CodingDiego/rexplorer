//! Command-line parsing for the app, and the payload sent to an already running instance
//! (WM_COPYDATA). Pure Rust, no shell or Win32 calls (it only asks the filesystem whether a path is a file).
//!
//! Once installed, rexplorer is launched as `explorer.exe` too, so it takes Explorer's arguments: `/select,<path>`
//! (or `/select,` then the path), `/e,<path>`, `/root,<path>`, `/n`, `/separate` (switches may be chained with
//! commas), a file path (its folder opens with it selected), and paths with a trailing backslash or with the
//! trailing quote some callers leave (`"C:\"` arrives as `C:"`). Anything else Explorer-style (`/idlist,...`,
//! URLs such as `ms-settings:`, `shell:AppsFolder\<app>`) sets `forward`: the caller hands the command line to
//! the real Explorer.

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
    pub install: bool,
    pub uninstall: bool,
    /// Not ours to handle: pass the whole command line to Windows Explorer and exit.
    pub forward: bool,
}

/// Explorer's comma-chained switches in one argument (`/n,/e,/select,C:\x`). False when something in it is not
/// one we handle (`/idlist`, an unknown switch, a missing path).
fn explorer_switches(a: &str, it: &mut impl Iterator<Item = String>, out: &mut Args) -> bool {
    let mut rest = a;
    loop {
        let (switch, after) = match rest.split_once(',') {
            Some((s, r)) => (s, Some(r)),
            None => (rest, None),
        };
        match switch.to_ascii_lowercase().as_str() {
            // Tabs stand in for windows: a new window is a new tab, which every hand-off opens anyway.
            "/n" | "/e" | "/separate" => {}
            sw @ ("/root" | "/select") => {
                let path = match after.filter(|p| !p.is_empty()) {
                    Some(p) => p.to_string(),
                    None => match it.next() {
                        Some(p) => p,
                        None => return false,
                    },
                };
                if path.starts_with('/') || forwarded_name(&path) {
                    return false;
                }
                if sw == "/select" {
                    out.select = Some(clean(&path));
                } else {
                    out.paths.push(clean(&path));
                }
                return true;
            }
            _ => return false,
        }
        match after {
            None | Some("") => return true,
            Some(r) if r.starts_with('/') => rest = r,
            Some(r) => {
                out.paths.push(clean(r));
                return true;
            }
        }
    }
}

/// Names Explorer launches rather than browses: URLs (`ms-settings:`, `https://`) and apps (`shell:AppsFolder\..`).
fn forwarded_name(p: &str) -> bool {
    let lower = p.to_ascii_lowercase();
    if lower.starts_with("shell:appsfolder\\") {
        return true;
    }
    match lower.split_once(':') {
        Some((scheme, _)) if scheme.len() >= 2 => {
            let url = scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
            url && !matches!(scheme, "shell" | "ftp" | "file")
        }
        _ => false,
    }
}

/// Undoes callers' quoting artefacts: a trailing quote (`C:"` from `"C:\"`, with whatever spaces followed it on
/// the command line), a trailing backslash (kept on a drive root, which needs it).
fn clean(p: &str) -> String {
    let t = p.trim_end().trim_end_matches('"');
    let t = if t.len() > 3 { t.trim_end_matches('\\') } else { t };
    if t.len() == 2 && t.ends_with(':') { format!("{t}\\") } else { t.to_string() }
}

/// An existing file other than an archive the app browses: Explorer would show it selected in its folder.
fn is_plain_file(p: &str) -> bool {
    let lower = p.to_ascii_lowercase();
    !(lower.ends_with(".zip") || lower.ends_with(".rar")) && std::path::Path::new(p).is_file()
}

/// The command line after argv[0], as Windows passed it (for forwarding verbatim).
pub fn tail(cmdline: &str) -> &str {
    let rest = match cmdline.strip_prefix('"') {
        Some(r) => r.split_once('"').map_or("", |(_, after)| after),
        None => cmdline.split_once([' ', '\t']).map_or("", |(_, after)| after),
    };
    rest.trim_start_matches([' ', '\t'])
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
            "--install" => out.install = true,
            "--uninstall" => out.uninstall = true,
            "-h" | "--help" => out.help = true,
            _ if a.starts_with('-') => return Err(format!("unknown option: {a}")),
            _ if a.starts_with('/') && !explorer_switches(&a, &mut it, &mut out) => {
                return Ok(Args { forward: true, ..Default::default() });
            }
            _ if a.starts_with('/') => {}
            _ if forwarded_name(&a) => return Ok(Args { forward: true, ..Default::default() }),
            _ => out.paths.push(clean(&a)),
        }
    }
    if out.select.is_none() {
        if let Some(i) = out.paths.iter().position(|p| is_plain_file(p)) {
            out.select = Some(out.paths.remove(i));
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
     \x20 --install           copy rexplorer to %LOCALAPPDATA%\\Programs and make it the default file manager\n\
     \x20 --uninstall         undo --install\n\
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
        assert_eq!(p(&["/SELECT,C:\\a b\\f.txt"]).unwrap().select.as_deref(), Some("C:\\a b\\f.txt"));
        let a = p(&["/e", "/n", "/root,C:\\r"]).unwrap();
        assert_eq!(a.paths, ["C:\\r"]);
        assert_eq!(a.select, None);
        assert_eq!(p(&["/e,C:\\x"]).unwrap().paths, ["C:\\x"]);
        assert_eq!(p(&["/n,/e,C:\\a,b"]).unwrap().paths, ["C:\\a,b"]);
        assert_eq!(p(&["/n,/e,/select,C:\\f.txt"]).unwrap().select.as_deref(), Some("C:\\f.txt"));
        assert_eq!(p(&["/separate,C:\\x"]).unwrap().paths, ["C:\\x"]);
        assert_eq!(p(&["/e,"]).unwrap(), Args::default());
        assert!(!p(&["/n"]).unwrap().forward);
    }

    #[test]
    fn explorer_names_and_artefacts() {
        // "C:\" through the C runtime's quoting arrives as C:" ; a trailing backslash goes except on a root.
        assert_eq!(p(&["C:\""]).unwrap().paths, ["C:\\"]);
        assert_eq!(p(&["C:\\"]).unwrap().paths, ["C:\\"]);
        assert_eq!(p(&["C:"]).unwrap().paths, ["C:\\"]);
        assert_eq!(p(&["C:\\Users\\"]).unwrap().paths, ["C:\\Users"]);
        assert_eq!(p(&["C:\\Users\\x\""]).unwrap().paths, ["C:\\Users\\x"]);
        assert_eq!(p(&["C:\\Users\\x\" "]).unwrap().paths, ["C:\\Users\\x"]);
        assert_eq!(p(&["/e,C:\""]).unwrap().paths, ["C:\\"]);
        for n in ["shell:Downloads", "shell:::{20D04FE0-3AEA-1069-A2D8-08002B30309D}", "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}", "\\\\server\\share", "ftp://h/x"] {
            assert_eq!(p(&[n]).unwrap().paths, [n], "{n}");
        }
        assert_eq!(p(&["\\\\server\\share\\"]).unwrap().paths, ["\\\\server\\share"]);
        assert_eq!(p(&[]).unwrap(), Args::default());
    }

    #[test]
    fn a_file_opens_its_folder_selected() {
        let dir = std::env::temp_dir().join(format!("rexplorer-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, "x").unwrap();
        let fs = f.display().to_string();
        let a = p(&[fs.as_str()]).unwrap();
        assert_eq!((a.paths.len(), a.select.as_deref()), (0, Some(fs.as_str())));
        let ds = dir.display().to_string();
        assert_eq!(p(&[ds.as_str()]).unwrap().paths, [ds.clone()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn forwarded_to_explorer() {
        for a in [&["/idlist,:123:456"][..], &["/root,/idlist,:1:2"], &["/zzz"], &["/select,"], &["ms-settings:display"], &["https://example.com"], &["shell:AppsFolder\\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"], &["C:\\x", "/idlist,1"]] {
            assert_eq!(p(a).unwrap(), Args { forward: true, ..Default::default() }, "{a:?}");
        }
    }

    #[test]
    fn command_line_tail() {
        assert_eq!(tail(r#""C:\a b\rexplorer.exe" /idlist,:1:2 x"#), "/idlist,:1:2 x");
        assert_eq!(tail(r#"rexplorer.exe   "/select,C:\x""#), r#""/select,C:\x""#);
        assert_eq!(tail("rexplorer.exe"), "");
        assert_eq!(tail(r#""C:\rexplorer.exe""#), "");
    }

    #[test]
    fn errors() {
        assert_eq!(p(&["--bogus"]).unwrap_err(), "unknown option: --bogus");
        assert!(p(&["--select"]).is_err());
        assert!(p(&["--profile"]).is_err());
        let a = p(&["--install", "--uninstall"]).unwrap();
        assert!(a.install && a.uninstall && !a.forward);
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
        for o in ["--select", "/select,", "--profile", "--new-tab", "-t", "--register", "--unregister", "--install", "--uninstall", "--recover", "--search", "-h", "--help"] {
            assert!(u.contains(o), "{o}");
        }
    }
}
