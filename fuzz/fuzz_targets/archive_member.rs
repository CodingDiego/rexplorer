//! An archive entry name: normalized, split and checked for escaping, it never lands outside the destination.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(init: rexplorer_fuzz::init!(), |data: &[u8]| rexplorer_core::fuzz::archive_member(&String::from_utf8_lossy(data)));
