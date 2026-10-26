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

/// What the timer is doing, for the menu bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MenuBarTimer {
    Idle,
    /// A work interval or a break: how much of it is left.
    Pomodoro {
        remaining_ms: i64,
        total_ms: i64,
        paused: bool,
    },
    Stopwatch {
        elapsed_ms: i64,
        paused: bool,
    },
}

/// The menu bar item: a pie `disc` of the way round (0..=1) clockwise from the
/// top, or none, maybe a short label, dimmed when nothing runs.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuBarLook {
    pub disc: Option<f64>,
    pub label: Option<String>,
    pub dimmed: bool,
    pub paused: bool,
}

/// What an idle timer shows: a still three-quarter pie, dimmed.
pub const IDLE_DISC: f64 = 0.75;

/// Pomodoro: the time left as a pie, no text. Stopwatch: no pie (it has no
/// end), only the minutes so far as text (no seconds, so it does not tick).
pub fn menu_bar_look(t: MenuBarTimer) -> MenuBarLook {
    match t {
        MenuBarTimer::Idle => MenuBarLook {
            disc: Some(IDLE_DISC),
            label: None,
            dimmed: true,
            paused: false,
        },
        MenuBarTimer::Pomodoro {
            remaining_ms,
            total_ms,
            paused,
        } => MenuBarLook {
            disc: Some(if total_ms > 0 {
                (remaining_ms as f64 / total_ms as f64).clamp(0.0, 1.0)
            } else {
                0.0
            }),
            label: None,
            dimmed: paused,
            paused,
        },
        MenuBarTimer::Stopwatch { elapsed_ms, paused } => MenuBarLook {
            disc: None,
            label: Some(minutes_label(elapsed_ms)),
            dimmed: paused,
            paused,
        },
    }
}

/// "<1 мин", "12 мин", "1 ч 05 мин": whole minutes, never seconds.
pub fn minutes_label(ms: i64) -> String {
    let min = ms.max(0) / 60_000;
    match min {
        0 => "<1 мин".to_string(),
        m if m < 60 => format!("{m} мин"),
        m => format!("{} ч {:02} мин", m / 60, m % 60),
    }
}

/// Shows the timer in the system's menu bar.
pub trait MenuBar {
    fn show(&self, look: &MenuBarLook);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_bar_shows_what_the_timer_does() {
        let idle = menu_bar_look(MenuBarTimer::Idle);
        assert!(idle.dimmed && idle.disc == Some(IDLE_DISC) && idle.label.is_none());
        let p = menu_bar_look(MenuBarTimer::Pomodoro {
            remaining_ms: 15 * 60_000,
            total_ms: 25 * 60_000,
            paused: false,
        });
        assert!((p.disc.unwrap() - 0.6).abs() < 1e-9 && p.label.is_none() && !p.dimmed);
        let paused = menu_bar_look(MenuBarTimer::Pomodoro {
            remaining_ms: 1,
            total_ms: 0,
            paused: true,
        });
        assert!(paused.paused && paused.dimmed && paused.disc == Some(0.0));
        let s = menu_bar_look(MenuBarTimer::Stopwatch {
            elapsed_ms: 90 * 60_000 + 59_000,
            paused: false,
        });
        assert_eq!(s.disc, None);
        assert_eq!(s.label.as_deref(), Some("1 ч 30 мин"));
    }

    #[test]
    fn minutes_never_tick_seconds() {
        assert_eq!(minutes_label(59_999), "<1 мин");
        assert_eq!(minutes_label(60_000), "1 мин");
        assert_eq!(minutes_label(12 * 60_000 + 59_000), "12 мин");
        assert_eq!(minutes_label(65 * 60_000), "1 ч 05 мин");
    }
}
