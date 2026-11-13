//! The update check's answer (a GitHub "latest release" JSON body) and version strings, as raw bytes.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| rexplorer_core::fuzz::update_response(data));
