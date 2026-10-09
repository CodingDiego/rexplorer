//! Headless shell-namespace layer. No window code lives here.
pub mod com;
pub mod fallback;
pub mod folder;
pub mod item;
pub mod menu;
pub mod notify;

pub use folder::{Column, ShellFolder};
pub use item::ShellItem;
