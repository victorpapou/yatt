//! What the views show and what they report back. Plain data, no AppKit, so the
//! app builds snapshots and handles events the same way on any OS.

use yatt_core::{Status, TaskId};

/// Everything the window shows, rebuilt by the app whenever state changes.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub columns: [ColumnSnap; 3],
    pub timer: TimerSnap,
    pub expanded: Option<TaskId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnSnap {
    pub status: Status,
    pub title: String,
    pub cards: Vec<CardSnap>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CardSnap {
    pub id: TaskId,
    pub title: String,
    pub notes: String,
    pub total_ms: i64,
    pub today_ms: i64,
    pub pomodoros: u32,
    /// Accruing time right now.
    pub tracking: bool,
    /// Latest time entries, formatted ("20:08 – 20:31 · 0:23:00"). Filled only for the expanded card.
    pub entries: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimerSnap {
    /// Remaining time for a countdown, elapsed time for the stopwatch.
    pub clock_ms: i64,
    pub phase_label: String,
    /// What the main button does now: start, pause or resume.
    pub primary_label: String,
    /// Tasks that collect time, joined for display.
    pub tasks_label: String,
    pub can_start_stopwatch: bool,
    pub can_skip: bool,
    pub can_stop: bool,
    /// Phase progress in 0.0..=1.0.
    pub progress: f32,
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    NewTask(String),
    /// `index` counts cards of the target column with the moved card left out.
    Move {
        id: TaskId,
        status: Status,
        index: usize,
    },
    ToggleExpand(TaskId),
    Rename(TaskId, String),
    SetNotes(TaskId, String),
    Delete(TaskId),
    StartPause,
    StartStopwatch,
    Skip,
    Stop,
    Undo,
    Redo,
    /// The main window was closed.
    Quit,
}

/// `h:mm:ss`, so a few seconds of work show right away.
pub fn format_duration(ms: i64) -> String {
    let secs = ms.max(0) / 1000;
    format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
}

/// The big timer: `mm:ss`, or `h:mm:ss` past an hour.
pub fn format_clock(ms: i64) -> String {
    // Round up so a countdown shows 25:00 at the start and hits 00:00 at the end.
    let secs = (ms.max(0) + 999) / 1000;
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
    } else {
        format!("{:02}:{:02}", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_formats() {
        assert_eq!(format_clock(25 * 60 * 1000), "25:00");
        assert_eq!(format_clock(1), "00:01");
        assert_eq!(format_clock(0), "00:00");
        assert_eq!(format_clock(3_600_000 + 61_000), "1:01:01");
        assert_eq!(format_duration(42_500), "0:00:42");
        assert_eq!(format_duration(3_723_000), "1:02:03");
    }
}
