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
    /// The card's own colour and icon, when it chose them.
    pub color: Option<u8>,
    pub icon: Option<String>,
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
    /// For the menu bar.
    pub menu_bar: yatt_platform::MenuBarTimer,
}

/// The timer-only window mode (⇧⌘M): off for now; the code stays.
pub const MINIMAL_MODE: bool = false;

/// A status's name, as its column is called.
pub fn status_label(status: Status) -> &'static str {
    match status {
        Status::Backlog => "Бэклог",
        Status::InProgress => "В работе",
        Status::Done => "Готово",
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    /// A new task on top of `status`.
    NewTask {
        title: String,
        status: Status,
    },
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
    /// The card's colour and icon (`None`: from its id).
    SetLook {
        id: TaskId,
        color: Option<u8>,
        icon: Option<String>,
    },
    /// Several moves or deletes made together: undone and redone as one.
    Group(Vec<UiEvent>),
    /// Copies of tasks, `(title, notes)` in order, on top of `status`.
    Paste {
        items: Vec<(String, String)>,
        status: Status,
    },
    StartPause,
    StartStopwatch,
    Skip,
    Stop,
    Undo,
    Redo,
    /// The main window was closed.
    Quit,
}

/// The moves that put `ids` together, in order, at index `at` of a column whose
/// cards are `column` now; `at` counts the column's cards with `ids` left out.
/// Each move's index counts the column at that moment with the moved card left out.
pub fn moves_into(column: &[TaskId], ids: &[TaskId], at: usize, status: Status) -> Vec<UiEvent> {
    let rest: Vec<TaskId> = column
        .iter()
        .copied()
        .filter(|id| !ids.contains(id))
        .collect();
    let anchor = rest.get(at).copied();
    let mut now: Vec<TaskId> = column.to_vec();
    let mut events = Vec::new();
    for (j, id) in ids.iter().enumerate() {
        now.retain(|x| x != id);
        let index = match (j, anchor) {
            (0, Some(a)) => now.iter().position(|x| *x == a).unwrap_or(now.len()),
            (0, None) => now.len(),
            _ => now
                .iter()
                .position(|x| *x == ids[j - 1])
                .map_or(now.len(), |p| p + 1),
        };
        now.insert(index, *id);
        events.push(UiEvent::Move {
            id: *id,
            status,
            index,
        });
    }
    events
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
    fn moves_into_keeps_the_group_together() {
        let ids: Vec<TaskId> = (0..5).map(|_| TaskId::new()).collect();
        let [a, b, c, d, e] = [ids[0], ids[1], ids[2], ids[3], ids[4]];
        // Column a b c d; move d and b (in that order) to sit before c.
        let column = [a, b, c, d];
        let moves = moves_into(&column, &[d, b], 1, Status::Backlog);
        let mut sim = column.to_vec();
        for m in &moves {
            let UiEvent::Move { id, index, .. } = m else {
                unreachable!()
            };
            sim.retain(|x| x != id);
            sim.insert(*index, *id);
        }
        assert_eq!(sim, vec![a, d, b, c]);
        // From another column, to the end.
        let moves = moves_into(&[a], &[e], 1, Status::Done);
        assert_eq!(
            moves,
            vec![UiEvent::Move {
                id: e,
                status: Status::Done,
                index: 1
            }]
        );
    }

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
