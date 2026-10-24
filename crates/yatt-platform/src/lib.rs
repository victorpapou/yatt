//! Everything yatt needs from the operating system, as traits.
//!
//! Core, storage, the timer and the GPUI views only talk to these traits. The
//! macOS implementation lives in `yatt-macos`; a Windows port adds `yatt-windows`
//! next to it without touching the rest of the app.

use jiff::Timestamp;

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("not supported on this platform")]
    Unsupported,
    #[error("permission needed: {0}")]
    PermissionNeeded(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, PlatformError>;

/// An installed application the user can pick for blocking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    /// Bundle id on macOS, executable name on Windows.
    pub id: String,
    pub name: String,
}

/// Keeps blocked applications out of the way while a focus profile is on.
pub trait AppBlocker {
    fn installed_apps(&self) -> Vec<AppInfo>;
    /// Replaces the blocked set; an empty slice lifts the block.
    fn set_blocked(&self, app_ids: &[String]) -> Result<()>;
}

/// System-wide website blocking. Implementations must lift the block by
/// themselves at `until`, even if the app has crashed meanwhile.
pub trait SiteBlocker {
    fn is_installed(&self) -> bool;
    fn block(&self, domains: &[String], until: Timestamp) -> Result<()>;
    fn unblock(&self) -> Result<()>;
}

/// The OS "Do Not Disturb" / Focus feature.
pub trait FocusMode {
    /// Names the user can choose from in a profile (Shortcuts on macOS).
    fn options(&self) -> Vec<String>;
    fn activate(&self, option: &str) -> Result<()>;
    fn deactivate(&self, option: &str) -> Result<()>;
}

/// The whole platform layer handed to the app at startup.
pub struct Platform {
    pub apps: Box<dyn AppBlocker>,
    pub sites: Box<dyn SiteBlocker>,
    pub focus: Box<dyn FocusMode>,
}

impl Platform {
    /// Does nothing; used in tests and until a real implementation lands.
    pub fn noop() -> Self {
        Self {
            apps: Box::new(Noop),
            sites: Box::new(Noop),
            focus: Box::new(Noop),
        }
    }
}

pub struct Noop;

impl AppBlocker for Noop {
    fn installed_apps(&self) -> Vec<AppInfo> {
        Vec::new()
    }
    fn set_blocked(&self, _: &[String]) -> Result<()> {
        Ok(())
    }
}

impl SiteBlocker for Noop {
    fn is_installed(&self) -> bool {
        false
    }
    fn block(&self, _: &[String], _: Timestamp) -> Result<()> {
        Err(PlatformError::Unsupported)
    }
    fn unblock(&self) -> Result<()> {
        Ok(())
    }
}

impl FocusMode for Noop {
    fn options(&self) -> Vec<String> {
        Vec::new()
    }
    fn activate(&self, _: &str) -> Result<()> {
        Err(PlatformError::Unsupported)
    }
    fn deactivate(&self, _: &str) -> Result<()> {
        Ok(())
    }
}
