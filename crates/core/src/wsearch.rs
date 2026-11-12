//! The Windows Search index (the WSearch service): which folders it covers, and a file name query below a folder.
//! Call it on a worker thread initialized with `ComInit::mta()`. Every failure (service stopped or disabled, no
//! catalog) is an `Err` the caller falls back from.
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::E_FAIL;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, CoCreateInstance};
use windows::Win32::System::Search::{
    CSearchManager, DBACCESSOR_ROWDATA, DBBINDING, DBMEMOWNER_CLIENTOWNED, DBPARAMIO_NOTPARAM, DBPART_LENGTH,
    DBPART_STATUS, DBPART_VALUE, DBSTATUS_S_OK, DBTYPE_WSTR, HACCESSOR, IAccessor, ICommand, ICommandText,
    IDBCreateCommand, IDBCreateSession, IDBInitialize, IDataInitialize, IRowset, ISearchCrawlScopeManager,
    ISearchManager, MSDAINITIALIZE,
};
use windows::core::{Error, GUID, HSTRING, IUnknown, Interface, Result, w};

use crate::com::take_cotaskmem_string;
use crate::query::Query;

/// The OLE DB command dialect "provider default" (oledb.h); the windows crate does not carry it.
const DBGUID_DEFAULT: GUID = GUID::from_u128(0xc8b521fb_5cf3_11ce_ade5_00aa0044773d);
/// Longest path a row carries, in UTF-16 units; a longer one comes back truncated and is skipped.
const PATH_MAX: usize = 32768;

pub struct Index {
    scope: ISearchCrawlScopeManager,
    connection: String,
}

#[repr(C)]
struct Row {
    status: u32,
    len: usize,
    value: [u16; PATH_MAX],
}

impl Index {
    pub fn open() -> Result<Index> {
        unsafe {
            let manager: ISearchManager = CoCreateInstance(&CSearchManager, None, CLSCTX_LOCAL_SERVER)?;
            let catalog = manager.GetCatalog(w!("SystemIndex"))?;
            let connection = take_cotaskmem_string(catalog.GetQueryHelper()?.ConnectionString()?);
            Ok(Index { scope: catalog.GetCrawlScopeManager()?, connection })
        }
    }

    /// Whether the index covers folder `path` (a file system path), by the crawl scope rules.
    pub fn covers(&self, path: &str) -> bool {
        unsafe { self.scope.IncludedInCrawlScope(&HSTRING::from(folder_url(path))) }.is_ok_and(|b| b.as_bool())
    }

    /// Folders strictly below `root` the index leaves out, named by its exclusion rules (AppData in a user profile):
    /// the caller walks them. A folder inside another one listed is left to that one's walk.
    pub fn excluded_below(&self, root: &str) -> Result<Vec<String>> {
        let mut patterns = Vec::new();
        unsafe {
            let rules = self.scope.EnumerateScopeRules()?;
            loop {
                let (mut rule, mut got) = ([None], 0);
                rules.Next(&mut rule, &mut got)?;
                let Some(rule) = rule[0].take().filter(|_| got == 1) else { break };
                if !rule.IsIncluded()?.as_bool() {
                    patterns.push(take_cotaskmem_string(rule.PatternOrURL()?));
                }
            }
        }
        let subfolders = |dir: &str| -> Vec<String> {
            let Ok(entries) = std::fs::read_dir(format!("{dir}\\")) else { return Vec::new() };
            entries.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.file_name().to_string_lossy().into_owned()).collect()
        };
        // A rule can be overridden by another one: only what the index really leaves out counts.
        let dirs = patterns.iter().flat_map(|p| expand(p, root, &subfolders)).filter(|d| std::path::Path::new(d).is_dir() && !self.covers(d));
        Ok(outermost(dirs.collect()))
    }

    /// Paths below folder `root` that answer `query` (its words on the file name, its filters on the file), in
    /// batches. `cancel` is checked before every row.
    pub fn query(&self, root: &str, query: &Query, cancel: &AtomicBool, sink: &mut dyn FnMut(Vec<String>)) -> Result<()> {
        unsafe {
            let init: IDataInitialize = CoCreateInstance(&MSDAINITIALIZE, None, CLSCTX_INPROC_SERVER)?;
            let mut source: Option<IUnknown> = None;
            init.GetDataSource(None, CLSCTX_INPROC_SERVER.0, &HSTRING::from(&self.connection), &IDBInitialize::IID, &mut source)?;
            let source: IDBInitialize = source.ok_or(E_FAIL)?.cast()?;
            source.Initialize()?;
            let session: IDBCreateCommand = source.cast::<IDBCreateSession>()?.CreateSession(None, &IDBCreateCommand::IID)?.cast()?;
            let text: ICommandText = session.CreateCommand(None, &ICommandText::IID)?.cast()?;
            text.SetCommandText(&DBGUID_DEFAULT, &HSTRING::from(sql(root, &query.words)))?;
            let mut rowset: Option<IUnknown> = None;
            text.cast::<ICommand>()?.Execute(None, &IRowset::IID, None, None, Some(&mut rowset))?;
            let rowset: IRowset = rowset.ok_or(E_FAIL)?.cast()?;
            let accessor: IAccessor = rowset.cast()?;
            let binding = DBBINDING {
                iOrdinal: 1,
                obValue: std::mem::offset_of!(Row, value),
                obLength: std::mem::offset_of!(Row, len),
                obStatus: std::mem::offset_of!(Row, status),
                dwPart: (DBPART_VALUE.0 | DBPART_LENGTH.0 | DBPART_STATUS.0) as u32,
                dwMemOwner: DBMEMOWNER_CLIENTOWNED.0 as u32,
                eParamIO: DBPARAMIO_NOTPARAM.0 as u32,
                cbMaxLen: PATH_MAX * 2,
                wType: DBTYPE_WSTR.0 as u16,
                ..Default::default()
            };
            let mut handle = HACCESSOR::default();
            accessor.CreateAccessor(DBACCESSOR_ROWDATA.0 as u32, 1, &binding, size_of::<Row>(), &mut handle, None)?;
            let mut row = Box::new(Row { status: 0, len: 0, value: [0; PATH_MAX] });
            let mut batch = Vec::new();
            let mut result = Ok(());
            let mut strays = 0usize;
            while !cancel.load(Ordering::Relaxed) {
                let (mut hrow, mut got) = (0usize, 0usize);
                // The binding passes this slice's length as the row count and its first element as the HROW array.
                if let Err(e) = rowset.GetNextRows(0, 0, &mut got, &mut [&mut hrow as *mut usize]) {
                    result = Err(e);
                    break;
                }
                if got == 0 {
                    break;
                }
                let read = rowset.GetData(hrow, handle, &mut *row as *mut Row as *mut _);
                let _ = rowset.ReleaseRows(1, &hrow, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut());
                if read.is_ok() && row.status == DBSTATUS_S_OK.0 as u32 {
                    let path = String::from_utf16_lossy(&row.value[..(row.len / 2).min(PATH_MAX)]);
                    match classify(root, &path, query) {
                        None => strays += 1,
                        Some(true) => batch.push(path),
                        Some(false) => {}
                    }
                }
                if batch.len() >= 256 {
                    sink(std::mem::take(&mut batch));
                }
            }
            let _ = accessor.ReleaseAccessor(handle, None);
            if !batch.is_empty() {
                sink(batch);
            }
            // SCOPE can resolve through a known folder to another place (Documents moved to "Documentos", with the
            // old folder still indexed): rows outside the root mean the index does not answer for this folder.
            if result.is_ok() && strays > 0 {
                return Err(Error::new(E_FAIL, format!("{strays} index rows outside {root}")));
            }
            result
        }
    }
}

/// Crawl scope rules name folders with a trailing separator (`file:///C:\Users\*`); without it an excluded folder's
/// own URL would not match its rule.
fn folder_url(path: &str) -> String {
    format!("file:///{}\\", path.trim_end_matches('\\'))
}

/// A string literal of the index's SQL: quotes doubled. With `like`, the LIKE wildcards `%`, `_` and `[` match
/// themselves (bracketed).
fn literal(text: &str, like: bool) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    for c in text.chars() {
        match c {
            '\'' => out.push_str("''"),
            '%' | '_' | '[' if like => {
                out.push('[');
                out.push(c);
                out.push(']');
            }
            c => out.push(c),
        }
    }
    out
}

/// The query: items below `root` whose file name contains every word (all items without words: the filters are
/// checked on each row). User text only ever goes in as an escaped literal.
fn sql(root: &str, words: &[String]) -> String {
    let mut scope = root.trim_end_matches('\\').to_string();
    if scope.ends_with(':') {
        scope.push('\\');
    }
    let mut sql = format!("SELECT System.ItemPathDisplay FROM SystemIndex WHERE SCOPE='file:{}'", literal(&scope, false));
    for w in words {
        sql.push_str(&format!(" AND System.FileName LIKE '%{}%'", literal(w, true)));
    }
    sql
}

/// An index row for a search of `root`: whether it answers `query` (its name and filters, as the walk would), or
/// None when it is not strictly below `root` (the index's SCOPE has its own rules).
fn classify(root: &str, path: &str, query: &Query) -> Option<bool> {
    below(path, root).then(|| query.path_matches(std::path::Path::new(path)))
}

/// Whether `path` is strictly below folder `root`, ignoring case.
fn below(path: &str, root: &str) -> bool {
    path.to_lowercase().starts_with(&format!("{}\\", root.trim_end_matches('\\').to_lowercase()))
}

/// Folders strictly below `root` that rule `pattern` (`file:///C:\Users\*\AppData\*`) names, listing folders with
/// `subfolders` only where the pattern has a `*`.
/// shortcut: `*` matches within one folder name here, the rules let it span several; such a rule names fewer folders,
/// so their files stay missing. Upgrade to walking the rule's whole literal prefix if a real rule needs it.
fn expand(pattern: &str, root: &str, subfolders: &dyn Fn(&str) -> Vec<String>) -> Vec<String> {
    let Some(path) = pattern.get(..8).filter(|s| s.eq_ignore_ascii_case("file:///")).and_then(|_| pattern.get(8..)) else { return Vec::new() };
    let mut segments = path.trim_end_matches('*').split('\\').filter(|s| !s.is_empty());
    let Some(drive) = segments.next().filter(|d| d.len() == 2 && d.ends_with(':') && d.as_bytes()[0].is_ascii_alphabetic()) else { return Vec::new() };
    // Only folders on the way to `root` or below it are listed.
    let near = |p: &str| below(root, p) || below(p, root) || p.eq_ignore_ascii_case(root.trim_end_matches('\\'));
    let mut found = vec![drive.to_string()];
    for seg in segments {
        found = found
            .iter()
            .flat_map(|base| {
                let names = if seg.contains('*') { subfolders(base).into_iter().filter(|n| glob(seg, n)).collect() } else { vec![seg.to_string()] };
                names.into_iter().map(move |n| format!("{base}\\{n}"))
            })
            .filter(|p| near(p))
            .collect();
    }
    found.retain(|p| below(p, root));
    found
}

/// `*` wildcard match, ignoring case.
fn glob(pattern: &str, name: &str) -> bool {
    let (pattern, name) = (pattern.to_lowercase(), name.to_lowercase());
    match pattern.split_once('*') {
        None => pattern == name,
        Some((head, tail)) => name.starts_with(head) && (head.len()..=name.len()).any(|i| name.is_char_boundary(i) && glob(tail, &name[i..])),
    }
}

/// `dirs` without repeats and without folders inside another one, so no folder is walked twice.
fn outermost(mut dirs: Vec<String>) -> Vec<String> {
    dirs.sort_by_key(|d| d.to_lowercase());
    let mut out: Vec<String> = Vec::new();
    for d in dirs {
        if !out.iter().any(|o| o.to_lowercase() == d.to_lowercase() || below(&d, o)) {
            out.push(d);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsearch_sql_escapes_user_text() {
        assert_eq!(
            sql(r"C:\Users\o'neil\", &["50%_[x]';".into(), "drop".into()]),
            r"SELECT System.ItemPathDisplay FROM SystemIndex WHERE SCOPE='file:C:\Users\o''neil' AND System.FileName LIKE '%50[%][_][[]x]'';%' AND System.FileName LIKE '%drop%'"
        );
        assert!(sql(r"C:\", &["a".into()]).contains(r"SCOPE='file:C:\'"));
        assert!(sql(r"C:\x", &[]).ends_with(r"SCOPE='file:C:\x'"), "filters only: every item, checked row by row");
        assert_eq!(folder_url(r"C:\"), r"file:///C:\");
        assert_eq!(folder_url(r"C:\Users\me\AppData"), r"file:///C:\Users\me\AppData\");
    }

    #[test]
    fn wsearch_keeps_matches_strictly_below_root() {
        let q = Query::parse("report");
        assert_eq!(classify(r"C:\Users\me", r"C:\Users\me\Docs\Report.txt", &q), Some(true));
        assert_eq!(classify(r"C:\", r"C:\a\report.txt", &q), Some(true));
        assert_eq!(classify(r"C:\Users\me", r"C:\Users\meow\report.txt", &q), None, "a sibling with the same prefix");
        assert_eq!(classify(r"C:\Users\me\Documentos", r"C:\Users\me\Documents\report.txt", &q), None, "SCOPE resolved elsewhere");
        assert_eq!(classify(r"C:\Users\me", r"C:\Users\me\report-dir\other.txt", &q), Some(false), "only the name matches");
        assert_eq!(classify(r"C:\Users\me", r"C:\Users\me", &Query::parse("me")), None);
        assert_eq!(classify(r"C:\Users\me", r"C:\Users\me\Docs\Report.txt", &Query::parse("report kind:picture")), Some(false), "the filters too");
    }

    #[test]
    fn wsearch_names_the_folders_exclusion_rules_leave_out() {
        let fs = |dir: &str| -> Vec<String> {
            match dir.to_lowercase().as_str() {
                r"c:\users" => vec!["me".into(), "Public".into()],
                _ => Vec::new(),
            }
        };
        let appdata = r"file:///C:\Users\*\AppData\*";
        assert_eq!(expand(appdata, r"C:\Users\me", &fs), [r"C:\Users\me\AppData"]);
        assert_eq!(expand(appdata, r"C:\Users\", &fs), [r"C:\Users\me\AppData", r"C:\Users\Public\AppData"]);
        assert!(expand(appdata, r"C:\Users\me\AppData", &fs).is_empty(), "the root itself is not below it");
        assert!(expand(r"file:///C:\Windows\*", r"C:\Users\me", &fs).is_empty(), "elsewhere");
        assert!(expand(r"iehistory://{S-1-5}\", r"C:\Users\me", &fs).is_empty(), "not a file rule");
        assert_eq!(expand(r"FILE:///c:\users\M*\appdata\*", r"C:\Users\me", &fs), [r"c:\users\me\appdata"]);
        assert!(glob("*.tmp", "A.TMP") && glob("a*b*c", "aXbYc") && !glob("a*b", "ac") && glob("*", ""));
        let dirs = vec![r"C:\a\b".to_string(), r"C:\A".into(), r"C:\a b".into(), r"c:\a".into()];
        assert_eq!(outermost(dirs), [r"C:\A", r"C:\a b"]);
    }

    #[test]
    fn wsearch_query_finds_nothing_outside_root() {
        // Needs COM and the WSearch service; with the service off, open() fails and the caller walks instead.
        let _c = crate::com::ComInit::mta();
        let Ok(index) = Index::open() else { return };
        let root = std::env::temp_dir().join(format!("rexplorer-wsearch-{}", std::process::id()));
        let mut found = Vec::new();
        let _ = index.query(&root.display().to_string(), &Query::parse("needle"), &AtomicBool::new(false), &mut |p| found.extend(p));
        assert!(found.is_empty(), "{found:?}");
    }
}
