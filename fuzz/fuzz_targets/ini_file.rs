//! settings.ini, theme files and `#RRGGBB` colors: any text through parse_ini, Settings, Theme and parse_color.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(init: rexplorer_fuzz::init!(), |data: &[u8]| rexplorer_core::fuzz::ini_file(&String::from_utf8_lossy(data)));
