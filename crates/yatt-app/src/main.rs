//! yatt: AppKit draws the window, GPUI holds the state.
//!
//! The AppKit views report [`UiEvent`]s into a channel; a GPUI task drains it and
//! updates the [`Workspace`] entity; an observer on that entity builds a snapshot
//! and hands it to the window. Menus are GPUI menus mapped to the same events.

#[cfg(target_os = "macos")]
mod app;
#[cfg(target_os = "macos")]
mod demo;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod workspace;

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    app::run()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("yatt's window is macOS-only for now; the engine and its tests build anywhere.");
}
