//! The command line and the hand-off payload. The input is split on U+001F: the first part is a raw command line
//! (and payload text), the next four are values for the payload round trip, the rest are argv entries.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let mut parts = text.split('\u{1f}').map(String::from);
    let line = parts.next().unwrap_or_default();
    let values: Vec<String> = parts.by_ref().take(4).collect();
    let args: Vec<String> = parts.collect();
    rexplorer_core::fuzz::command_line(&line, &args, &values, data.first().is_some_and(|b| b & 1 == 1));
});
