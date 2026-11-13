//! session.txt and folders.txt: the app crate's session.rs compiled in as is (the app is a binary, so it cannot be a
//! dependency). Its one outside call, the FTP password redaction, is stubbed.
#![no_main]
#![allow(dead_code)]
use libfuzzer_sys::fuzz_target;

mod connect {
    pub fn redact(name: &str) -> String {
        name.to_string()
    }
}
#[path = "../../crates/app/src/session.rs"]
mod session;

fuzz_target!(init: rexplorer_fuzz::init!(), |data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let s = session::parse(&text);
    assert!(s.active < s.tabs.len().max(1) && (120..=1200).contains(&s.tree_width) && (120..=1200).contains(&s.preview_width));
    assert_eq!(session::format(&session::parse(&session::format(&s))), session::format(&s));
    let (f, extra) = session::parse_folders(&text);
    let out = session::format_folders(&f, &extra);
    assert_eq!(session::format_folders(&session::parse_folders(&out).0, &extra), out);
});
