//! yatt's native AppKit views. They render [`Snapshot`]s built by the app and
//! report [`UiEvent`]s back; they know nothing about the engine or storage.

pub mod model;

#[cfg(target_os = "macos")]
mod classes;
#[cfg(target_os = "macos")]
mod editor;
#[cfg(target_os = "macos")]
mod modal;
#[cfg(target_os = "macos")]
pub mod motion;
#[cfg(target_os = "macos")]
mod window;

pub use model::{
    CardSnap, ColumnSnap, Snapshot, TimerSnap, UiEvent, format_clock, format_duration,
};
#[cfg(target_os = "macos")]
pub use objc2::MainThreadMarker;
#[cfg(target_os = "macos")]
pub use window::{MainWindow, Sink};
