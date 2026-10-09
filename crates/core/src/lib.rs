//! Headless shell-namespace layer. No window code lives here.
pub mod archive;
pub mod browser;
pub mod cli;
pub mod com;
pub mod crash;
pub mod dragdrop;
pub mod fallback;
pub mod folder;
pub mod fuzzy;
pub mod icons;
pub mod images;
pub mod item;
pub mod menu;
pub mod notify;
pub mod preview;
pub mod register;
pub mod search;
pub mod settings;
pub mod tree;
pub mod usn;

pub use folder::{Column, ShellFolder};
pub use item::ShellItem;
