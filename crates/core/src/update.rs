//! The opt-in update notice (Folder options, `check_updates=true`; off by default). At most once a day it makes one
//! HTTPS GET for the latest GitHub release: User-Agent `rexplorer/<version>`, no cookies, no credentials, no ids. It
//! follows no redirect, reads at most `MAX_BODY` bytes, downloads and runs nothing, and every failure is silent.

use std::ffi::c_void;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// What the notice opens: a fixed page, never a URL taken from the response.
pub const RELEASE_PAGE: &str = "https://github.com/CodingDiego/rexplorer/releases/latest";
const HOST: &str = "api.github.com";
const PATH: &str = "/repos/CodingDiego/rexplorer/releases/latest";
const MAX_BODY: usize = 1 << 20;
const DAY: Duration = Duration::from_secs(24 * 60 * 60);
/// In the settings folder: the last check's time (its mtime) and the version it found.
const STAMP: &str = "update-check.txt";

/// `1.2.3`, `v1.2.3` or `1.2` (missing parts are 0). Pre-releases, build metadata and anything else are None.
pub fn parse_version(s: &str) -> Option<[u32; 3]> {
    let parts: Vec<&str> = s.strip_prefix('v').unwrap_or(s).split('.').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut v = [0; 3];
    for (slot, p) in v.iter_mut().zip(&parts) {
        // u32's parser takes a leading '+'; a tag does not.
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = p.parse().ok()?;
    }
    Some(v)
}

fn format(v: [u32; 3]) -> String {
    format!("{}.{}.{}", v[0], v[1], v[2])
}

/// The `tag_name` of a GitHub "latest release" JSON body, as a version.
pub fn parse_latest(body: &[u8]) -> Option<[u32; 3]> {
    let text = String::from_utf8_lossy(body);
    // A quote inside a JSON string is escaped, so this only matches a key.
    let rest = &text[text.find("\"tag_name\"")? + "\"tag_name\"".len()..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    parse_version(&rest[..rest.find('"')?])
}

/// Any bytes as a release answer or a version: no panic, and what parses reads back through its own text.
#[cfg(any(test, fuzzing))]
pub fn fuzz_check(body: &[u8]) {
    if let Some(v) = parse_latest(body) {
        assert_eq!(parse_version(&format(v)), Some(v), "{body:?}");
    }
    let text = String::from_utf8_lossy(body);
    assert!(parse_version(&text).is_none_or(|v| parse_version(&format(v)) == Some(v)), "{body:?}");
    let _ = newer([u32::MAX; 3], &text);
}

/// One dot-separated part of a pre-release (`rc`, `1`): numbers before words, numbers by value (SemVer).
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Pre {
    Num(u64),
    Word(String),
}

/// This build's version, a pre-release too, in SemVer order: 0.2.0 < 1.0.0-rc.1 < 1.0.0-rc.2 < 1.0.0. Field order is
/// the comparison order.
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Current {
    core: [u32; 3],
    release: bool,
    pre: Vec<Pre>,
}

fn parse_current(s: &str) -> Option<Current> {
    let s = s.split_once('+').map_or(s, |(v, _)| v);
    let (core, pre) = s.split_once('-').map_or((s, None), |(c, p)| (c, Some(p)));
    let pre = match pre {
        None => Vec::new(),
        Some(p) => p
            .split('.')
            .map(|id| match id.parse::<u64>() {
                Ok(n) if id.bytes().all(|b| b.is_ascii_digit()) => Some(Pre::Num(n)),
                _ if !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') => Some(Pre::Word(id.to_string())),
                _ => None,
            })
            .collect::<Option<_>>()?,
    };
    Some(Current { core: parse_version(core)?, release: pre.is_empty(), pre })
}

/// `latest` (a release, never a pre-release) as text when it is newer than `current`.
fn newer(latest: [u32; 3], current: &str) -> Option<String> {
    (Current { core: latest, release: true, pre: Vec::new() } > parse_current(current)?).then(|| format(latest))
}

/// Checked under a day ago; a stamp from the future (the clock moved back) is stale.
fn fresh(checked: SystemTime, now: SystemTime) -> bool {
    now.duration_since(checked).is_ok_and(|d| d < DAY)
}

/// The release newer than `current`, if any: the last check's answer while it is under a day old, else asked now.
/// Blocks on the network: call it on a worker thread.
pub fn check(dir: &Path, current: &str) -> Option<String> {
    let stamp = dir.join(STAMP);
    let checked = std::fs::metadata(&stamp).and_then(|m| m.modified());
    let latest = if checked.is_ok_and(|t| fresh(t, SystemTime::now())) {
        parse_version(std::fs::read_to_string(&stamp).ok()?.trim())?
    } else {
        // Stamped before asking, so a failed check also waits a day; no stamp, no request.
        std::fs::write(&stamp, "").ok()?;
        let v = parse_latest(&fetch()?)?;
        let _ = std::fs::write(&stamp, format(v));
        v
    };
    newer(latest, current)
}

/// The GET itself: the body of a 200 answer, at most `MAX_BODY` bytes.
fn fetch() -> Option<Vec<u8>> {
    use windows::Win32::Networking::WinHttp::*;
    use windows::core::{HSTRING, PCWSTR, w};

    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
    let handle = |h: *mut c_void| (!h.is_null()).then_some(Handle(h));
    unsafe {
        let agent = HSTRING::from(format!("rexplorer/{}", env!("CARGO_PKG_VERSION")));
        let session = handle(WinHttpOpen(&agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0))?;
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 10_000, 10_000).ok()?;
        let connect = handle(WinHttpConnect(session.0, &HSTRING::from(HOST), INTERNET_DEFAULT_HTTPS_PORT, 0))?;
        let request =
            handle(WinHttpOpenRequest(connect.0, w!("GET"), &HSTRING::from(PATH), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE))?;
        let r = Some(request.0.cast_const());
        WinHttpSetOption(r, WINHTTP_OPTION_REDIRECT_POLICY, Some(&WINHTTP_OPTION_REDIRECT_POLICY_NEVER.to_ne_bytes())).ok()?;
        WinHttpSetOption(r, WINHTTP_OPTION_DISABLE_FEATURE, Some(&(WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_AUTHENTICATION).to_ne_bytes())).ok()?;
        WinHttpSendRequest(request.0, None, None, 0, 0, 0).ok()?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).ok()?;
        let (mut status, mut len) = (0u32, 4u32);
        let flags = WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER;
        WinHttpQueryHeaders(request.0, flags, PCWSTR::null(), Some(&mut status as *mut u32 as *mut c_void), &mut len, std::ptr::null_mut()).ok()?;
        if status != 200 {
            return None;
        }
        let (mut body, mut buf) = (Vec::new(), [0u8; 8192]);
        loop {
            let mut n = 0u32;
            WinHttpReadData(request.0, buf.as_mut_ptr().cast(), buf.len() as u32, &mut n).ok()?;
            if n == 0 {
                return Some(body);
            }
            if body.len() + n as usize > MAX_BODY {
                return None;
            }
            body.extend_from_slice(&buf[..n as usize]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("1.2.3"), Some([1, 2, 3]));
        assert_eq!(parse_version("v0.10.0"), Some([0, 10, 0]));
        assert_eq!(parse_version("2"), Some([2, 0, 0]));
        for bad in ["", "v", ".", "1..2", "1.2.3.4", "1.2.3-rc1", "1.2.3+b", "+1.2.3", "-1.0.0", " 1.2.3", "1.2.x", "V1.2.3", "99999999999.0.0", "1.\u{661}.0"] {
            assert_eq!(parse_version(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn only_a_newer_version_counts() {
        assert_eq!(newer([0, 1, 5], "0.1.4"), Some("0.1.5".into()));
        assert_eq!(newer([1, 0, 0], "0.9.9"), Some("1.0.0".into()));
        assert_eq!(newer([0, 2, 0], "0.10.0"), None, "numeric, not text, order");
        assert_eq!(newer([0, 1, 4], "0.1.4"), None);
        assert_eq!(newer([0, 1, 3], "0.1.4"), None);
        assert_eq!(newer([9, 9, 9], "junk"), None);
        // A release candidate is told about its release, not about older ones.
        assert_eq!(newer([1, 0, 0], "1.0.0-rc.1"), Some("1.0.0".into()));
        assert_eq!(newer([0, 2, 0], "1.0.0-rc.1"), None);
        assert_eq!(newer([1, 0, 0], "1.0.0"), None);
    }

    #[test]
    fn pre_releases_order_as_semver() {
        let v = |s| parse_current(s).unwrap_or_else(|| panic!("{s}"));
        let order = ["0.2.0", "1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-rc.1", "1.0.0-rc.2", "1.0.0-rc.10", "1.0.0", "1.0.1-rc.1", "1.0.1"];
        for w in order.windows(2) {
            assert!(v(w[0]) < v(w[1]), "{} < {}", w[0], w[1]);
        }
        assert_eq!(v("1.0.0+build.5"), v("1.0.0"), "build metadata does not count");
        assert!(parse_current(env!("CARGO_PKG_VERSION")).is_some(), "this build's own version parses");
        for bad in ["1.0.0-", "1.0.0-rc..1", "1.0.0-rc!", "-rc.1", "1.0.x-rc.1"] {
            assert_eq!(parse_current(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn latest_release_bodies() {
        assert_eq!(parse_latest(br#"{"url":"x","tag_name": "v1.2.3","name":"1.2.3"}"#), Some([1, 2, 3]));
        assert_eq!(parse_latest(b"{\"tag_name\"\n:\t\"0.2.0\"}"), Some([0, 2, 0]));
        // Hostile or broken bodies.
        let bad: [&[u8]; 12] = [
            b"",
            b"\"tag_name\"",
            b"{\"tag_name\": 123}",
            b"{\"tag_name\": \"v1.2.3",
            b"{\"tag_name\": \"v1.2.3-rc1\"}",
            b"{\"tag_name\": \"../../evil\"}",
            b"{\"tag_name\": \"https://evil.example/1.2.3\"}",
            b"{\"tag_name\": \"\"}",
            br#"{"body":"say \"tag_name\": \"9.9.9\" here"}"#,
            b"{\"tag_name\": \"\xff\xfe\"}",
            b"<html>rate limited</html>",
            b"{\"tag_name\" \"1.0.0\"}",
        ];
        for body in bad {
            assert_eq!(parse_latest(body), None, "{:?}", String::from_utf8_lossy(body));
        }
        // Invalid UTF-8 elsewhere in the body does not hide the tag.
        assert_eq!(parse_latest(b"{\"body\":\"\xff\",\"tag_name\":\"1.0.0\"}"), Some([1, 0, 0]));
    }

    #[test]
    fn a_check_lasts_a_day() {
        let now = SystemTime::now();
        assert!(fresh(now - Duration::from_secs(3600), now));
        assert!(!fresh(now - DAY, now));
        assert!(!fresh(now + Duration::from_secs(60), now), "a stamp from the future is stale");
    }

    #[test]
    fn a_fresh_stamp_answers_without_asking() {
        let d = std::env::temp_dir().join(format!("rexplorer-update-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        // Just written, so fresh: none of these reach the network.
        std::fs::write(d.join(STAMP), "9.9.9\n").unwrap();
        assert_eq!(check(&d, "0.1.4"), Some("9.9.9".into()));
        assert_eq!(check(&d, "9.9.9"), None);
        for junk in ["", "https://evil.example", "1.0.0-rc1"] {
            std::fs::write(d.join(STAMP), junk).unwrap();
            assert_eq!(check(&d, "0.1.4"), None, "{junk:?}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
