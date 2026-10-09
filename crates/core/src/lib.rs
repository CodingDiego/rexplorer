//! Headless shell-namespace layer. No window code lives here.
pub mod com;
pub mod folder;
pub mod item;

pub use folder::ShellFolder;
pub use item::ShellItem;
