//! The GPUI side: one entity holds the engine plus UI state, turns UI events into
//! engine calls and builds the snapshots the AppKit window renders.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use yatt_core::{Command, Mode, Phase, Placement, Status, TaskId};
use yatt_engine::Engine;
use yatt_macos::{CardSnap, ColumnSnap, Snapshot, TimerSnap, UiEvent, format_duration};

/// How many time entries the expanded card lists.
const ENTRIES_SHOWN: usize = 8;

pub struct Workspace {
    pub engine: Engine,
    pub expanded: Option<TaskId>,
    /// Last second the clock was redrawn for.
    pub shown_second: i64,
}

impl Workspace {
    pub fn new(engine: Engine) -> Self {
        Self {
            engine,
            expanded: None,
            shown_second: 0,
        }
    }

    /// Applies one UI event. Returns false for `Quit`, which the caller handles.
    pub fn handle(&mut self, event: UiEvent) -> bool {
        let e = &mut self.engine;
        match event {
            UiEvent::NewTask(title) => {
                e.create_task(title);
            }
            UiEvent::Move { id, status, index } => {
                e.move_task(id, status, Placement::Index(index));
            }
            UiEvent::ToggleExpand(id) => {
                self.expanded = if self.expanded == Some(id) {
                    None
                } else {
                    Some(id)
                };
            }
            UiEvent::Rename(id, title) => e.execute(Command::Rename { id, title }),
            UiEvent::SetNotes(id, notes) => e.execute(Command::SetNotes { id, notes }),
            UiEvent::Delete(id) => {
                e.delete_task(id);
                if self.expanded == Some(id) {
                    self.expanded = None;
                }
            }
            UiEvent::StartPause => e.start_or_pause(Mode::Pomodoro),
            UiEvent::StartStopwatch => e.start_or_pause(Mode::Stopwatch),
            UiEvent::Skip => e.skip(),
            UiEvent::Stop => e.stop(),
            UiEvent::Undo => e.undo(),
            UiEvent::Redo => e.redo(),
            UiEvent::Quit => return false,
        }
        true
    }

    /// Advances the timer. Returns true when the window needs a new snapshot.
    pub fn tick(&mut self) -> bool {
        let changed = self.engine.tick();
        let sec = self.engine.now().as_second();
        let clock_moved = self.engine.timer.is_running() && sec != self.shown_second;
        if clock_moved {
            self.shown_second = sec;
        }
        changed || clock_moved
    }

    pub fn snapshot(&self) -> Snapshot {
        let e = &self.engine;
        let now = e.now();
        let accruing = e.timer.is_running() && e.timer.is_tracking_phase();
        let columns = Status::ALL.map(|status| ColumnSnap {
            status,
            title: column_title(status).to_string(),
            cards: e
                .board
                .column(status, &e.filter)
                .into_iter()
                .map(|t| {
                    let time = e.times.get(&t.id).copied().unwrap_or_default();
                    let expanded = self.expanded == Some(t.id);
                    CardSnap {
                        id: t.id,
                        title: t.title.clone(),
                        notes: t.notes.clone(),
                        total_ms: time.total_ms,
                        today_ms: time.today_ms,
                        pomodoros: time.pomodoros,
                        tracking: accruing && e.is_active(t.id),
                        entries: if expanded {
                            e.recent_entries(t.id, ENTRIES_SHOWN)
                                .into_iter()
                                .map(|en| entry_label(en.started_at, en.ended_at, now))
                                .collect()
                        } else {
                            Vec::new()
                        },
                    }
                })
                .collect(),
        });
        Snapshot {
            columns,
            timer: self.timer_snap(now),
            expanded: self.expanded,
        }
    }

    fn timer_snap(&self, now: Timestamp) -> TimerSnap {
        let e = &self.engine;
        let t = &e.timer;
        let phase_label = match t.phase {
            Phase::Idle => "Готов",
            Phase::Work if t.mode == Mode::Stopwatch => "Секундомер",
            Phase::Work => "Работа",
            Phase::ShortBreak => "Перерыв",
            Phase::LongBreak => "Длинный перерыв",
        };
        let primary_label = if t.is_running() {
            "Пауза"
        } else if t.phase == Phase::Idle {
            "Старт"
        } else {
            "Продолжить"
        };
        let titles: Vec<&str> = t
            .active_tasks
            .iter()
            .filter_map(|id| e.board.get(*id))
            .map(|task| task.title.as_str())
            .collect();
        let tasks_label = if titles.is_empty() {
            "Нет задач «В работе»: время не пишется".to_string()
        } else {
            titles.join(", ")
        };
        TimerSnap {
            clock_ms: t.remaining_ms(now).unwrap_or_else(|| t.elapsed_ms(now)),
            phase_label: phase_label.to_string(),
            primary_label: primary_label.to_string(),
            tasks_label,
            can_start_stopwatch: t.phase == Phase::Idle,
            can_skip: t.phase != Phase::Idle && t.mode == Mode::Pomodoro,
            can_stop: t.phase != Phase::Idle,
            progress: t.progress(now),
            running: t.is_running(),
        }
    }
}

fn column_title(status: Status) -> &'static str {
    match status {
        Status::Backlog => "Бэклог",
        Status::InProgress => "В работе",
        Status::Done => "Готово",
    }
}

/// "24.10 20:08 – 20:31 · 0:23:00", or "… – сейчас" for a running entry.
fn entry_label(start: Timestamp, end: Option<Timestamp>, now: Timestamp) -> String {
    let tz = TimeZone::system();
    let s = start.to_zoned(tz.clone());
    let until = match end {
        Some(end) => end.to_zoned(tz).strftime("%H:%M").to_string(),
        None => "сейчас".to_string(),
    };
    let ms = end.unwrap_or(now).as_millisecond() - start.as_millisecond();
    format!(
        "{} – {}  ·  {}",
        s.strftime("%d.%m %H:%M"),
        until,
        format_duration(ms)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use yatt_store::Store;

    fn workspace() -> Workspace {
        Workspace::new(Engine::load(Store::open_in_memory().unwrap()).unwrap())
    }

    #[test]
    fn events_drive_engine_and_snapshot() {
        let mut ws = workspace();
        ws.handle(UiEvent::NewTask("Написать план".into()));
        let id = ws.snapshot().columns[0].cards[0].id;
        ws.handle(UiEvent::Move {
            id,
            status: Status::InProgress,
            index: 0,
        });
        ws.handle(UiEvent::ToggleExpand(id));
        ws.handle(UiEvent::StartStopwatch);
        let snap = ws.snapshot();
        assert!(snap.columns[0].cards.is_empty());
        let card = &snap.columns[1].cards[0];
        assert!(card.tracking);
        assert_eq!(card.entries.len(), 1);
        assert!(card.entries[0].contains("сейчас"));
        assert_eq!(snap.expanded, Some(id));
        assert_eq!(snap.timer.tasks_label, "Написать план");

        ws.handle(UiEvent::Rename(id, "План".into()));
        ws.handle(UiEvent::Delete(id));
        let snap = ws.snapshot();
        assert!(snap.columns.iter().all(|c| c.cards.is_empty()));
        assert_eq!(snap.expanded, None);
        ws.handle(UiEvent::Undo);
        assert_eq!(ws.snapshot().columns[1].cards[0].title, "План");
        assert!(!ws.handle(UiEvent::Quit));
    }
}
