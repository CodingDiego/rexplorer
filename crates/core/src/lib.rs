//! Headless shell-namespace layer. No window code lives here.
pub mod access;
pub mod archive;
pub mod browser;
pub mod cli;
pub mod com;
pub mod crash;
pub mod dragdrop;
pub mod desktop;
pub mod elevation;
pub mod fallback;
pub mod folder;
/// The parsers' invariant checks, for the seeded tests and the cargo-fuzz targets in fuzz/ (built with `--cfg fuzzing`).
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
pub mod fuzzy;
pub mod i18n;
pub mod icons;
pub mod import_explorer;
pub mod install;
pub mod images;
pub mod item;
pub mod jumplist;
pub mod menu;
pub mod notify;
pub mod packaged;
pub mod preview;
pub mod query;
pub mod register;
pub mod search;
pub mod settings;
pub mod tree;
pub mod undo;
pub mod update;
pub mod usn;
pub mod wsearch;

pub use folder::{Column, ShellFolder};
pub use item::ShellItem;
