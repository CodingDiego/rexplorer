//! A language file (lang\*.ini) of any text: the English keys stay, placeholder substitution does not panic.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(init: rexplorer_fuzz::init!(), |data: &[u8]| rexplorer_core::fuzz::language_file(&String::from_utf8_lossy(data)));
