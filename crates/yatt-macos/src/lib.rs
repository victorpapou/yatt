//! yatt's native AppKit views. They render [`Snapshot`]s built by the app and
//! report [`UiEvent`]s back; they know nothing about the engine or storage.

pub mod eventlog;
pub mod model;
pub mod physics;

#[cfg(target_os = "macos")]
mod alert;
#[cfg(target_os = "macos")]
mod board;
#[cfg(target_os = "macos")]
mod classes;
#[cfg(target_os = "macos")]
mod durations;
#[cfg(target_os = "macos")]
mod editor;
#[cfg(target_os = "macos")]
mod mdspans;
#[cfg(target_os = "macos")]
mod menubar;
#[cfg(target_os = "macos")]
mod modal;
#[cfg(target_os = "macos")]
pub mod motion;
mod notes;
#[cfg(target_os = "macos")]
mod notify;
#[cfg(target_os = "macos")]
mod palette;
mod picker;
mod settings;
mod taskwindow;
mod tags;
#[cfg(target_os = "macos")]
mod style;
#[cfg(target_os = "macos")]
mod window;

#[cfg(target_os = "macos")]
pub use alert::alert;
pub use model::{
    CardSnap, ColumnSnap, Snapshot, TimerSnap, UiEvent, format_clock, format_duration, moves_into,
};
#[cfg(target_os = "macos")]
pub use notify::MacNotifier;
pub use objc2::MainThreadMarker;
#[cfg(target_os = "macos")]
pub use window::{DemoMouse, DemoPointer, MainWindow, Sink};
