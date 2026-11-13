//! The search box syntax (kind:, size:, date:, ext: and words) of any text, against several dates.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| rexplorer_core::fuzz::search_query(&String::from_utf8_lossy(data)));
