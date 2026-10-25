//! SQLite persistence for yatt: one WAL-mode file in Application Support.
//!
//! The in-memory [`yatt_core::Board`] and [`yatt_core::Timer`] are the live state;
//! the store writes the rows they change and the [`Effect`]s the timer emits.

mod schema;

use std::path::Path;

use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension, Row, params};
use uuid::Uuid;
use yatt_core::{Effect, Priority, ProjectId, Status, Task, TaskId, Timer};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Migration(#[from] rusqlite_migration::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("corrupt row: {0}")]
    Corrupt(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

const TIMER_KEY: &str = "timer";

/// Time tracked on one task.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TaskTime {
    pub total_ms: i64,
    pub today_ms: i64,
    /// Completed work sessions in which this task had time.
    pub pomodoros: u32,
}

/// One tracked interval on a task. `ended_at` is `None` while it is still running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeEntry {
    pub started_at: Timestamp,
    pub ended_at: Option<Timestamp>,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| StoreError::Corrupt(e.to_string()))?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        schema::migrations().to_latest(&mut conn)?;
        Ok(Self { conn })
    }

    // ---- tasks ----

    pub fn load_tasks(&self) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, notes, status, position, project_id, priority, due_at, estimate_pomodoros,
                    created_at, updated_at, completed_at, deleted_at
             FROM tasks",
        )?;
        let rows = stmt.query_map([], task_from_row)?;
        rows.map(|r| r?).collect()
    }

    pub fn save_task(&self, t: &Task) -> Result<()> {
        save_task(&self.conn, t)
    }

    pub fn save_tasks<'a>(&mut self, tasks: impl IntoIterator<Item = &'a Task>) -> Result<()> {
        let tx = self.conn.transaction()?;
        tasks.into_iter().try_for_each(|t| save_task(&tx, t))?;
        tx.commit()?;
        Ok(())
    }

    // ---- timer ----

    pub fn load_timer(&self) -> Result<Option<Timer>> {
        let json: Option<String> = self
            .conn
            .query_row("SELECT value FROM kv WHERE key = ?1", [TIMER_KEY], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(json.map(|j| serde_json::from_str(&j)).transpose()?)
    }

    pub fn save_timer(&self, timer: &Timer) -> Result<()> {
        self.conn.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![TIMER_KEY, serde_json::to_string(timer)?],
        )?;
        Ok(())
    }

    /// Writes timer effects and the timer state in one transaction, so a crash
    /// never leaves entries and the timer disagreeing.
    pub fn apply_timer(&mut self, effects: &[Effect], timer: &Timer) -> Result<()> {
        let tx = self.conn.transaction()?;
        for e in effects {
            match *e {
                Effect::SessionStarted {
                    id,
                    kind,
                    planned_secs,
                    at,
                } => {
                    tx.execute(
                        "INSERT INTO focus_sessions (id, kind, planned_secs, started_at) VALUES (?1, ?2, ?3, ?4)",
                        params![id.0.to_string(), kind.as_str(), planned_secs, ms(at)],
                    )?;
                }
                Effect::SessionEnded { id, outcome, at } => {
                    tx.execute(
                        "UPDATE focus_sessions SET ended_at = ?2, outcome = ?3 WHERE id = ?1",
                        params![id.0.to_string(), ms(at), outcome.as_str()],
                    )?;
                }
                Effect::EntryOpened { task, session, at } => {
                    tx.execute(
                        "INSERT INTO time_entries (id, task_id, session_id, started_at, source)
                         SELECT ?1, ?2, ?3, ?4,
                                CASE kind WHEN 'stopwatch' THEN 'stopwatch' ELSE 'pomodoro' END
                         FROM focus_sessions WHERE id = ?3",
                        params![
                            Uuid::now_v7().to_string(),
                            task.0.to_string(),
                            session.0.to_string(),
                            ms(at)
                        ],
                    )?;
                }
                Effect::EntryClosed { task, session, at } => {
                    tx.execute(
                        "UPDATE time_entries SET ended_at = ?3
                         WHERE task_id = ?1 AND session_id = ?2 AND ended_at IS NULL",
                        params![task.0.to_string(), session.0.to_string(), ms(at)],
                    )?;
                }
                Effect::PhaseFinished { .. } => {}
            }
        }
        tx.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![TIMER_KEY, serde_json::to_string(timer)?],
        )?;
        tx.commit()?;
        Ok(())
    }

    // ---- stats ----

    /// Total, today's time and pomodoro count for a task. Open entries count up to `now`.
    pub fn task_time(
        &self,
        task: TaskId,
        day_start: Timestamp,
        now: Timestamp,
    ) -> Result<TaskTime> {
        let id = task.0.to_string();
        let (now, day) = (ms(now), ms(day_start));
        let (total_ms, today_ms): (i64, i64) = self.conn.query_row(
            "SELECT
                COALESCE(SUM(COALESCE(ended_at, ?2) - started_at), 0),
                COALESCE(SUM(MAX(0, COALESCE(ended_at, ?2) - MAX(started_at, ?3))), 0)
             FROM time_entries WHERE task_id = ?1",
            params![id, now, day],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let pomodoros: u32 = self.conn.query_row(
            "SELECT COUNT(DISTINCT s.id) FROM focus_sessions s
             JOIN time_entries e ON e.session_id = s.id
             WHERE e.task_id = ?1 AND s.kind = 'work' AND s.outcome = 'completed'",
            [&id],
            |r| r.get(0),
        )?;
        Ok(TaskTime {
            total_ms,
            today_ms,
            pomodoros,
        })
    }

    /// The task's latest time entries, newest first.
    pub fn recent_entries(&self, task: TaskId, limit: usize) -> Result<Vec<TimeEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT started_at, ended_at FROM time_entries WHERE task_id = ?1
             ORDER BY started_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![task.0.to_string(), limit as i64], |r| {
            Ok(TimeEntry {
                started_at: ts(r.get(0)?)?,
                ended_at: r.get::<_, Option<i64>>(1)?.map(ts).transpose()?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Wall-clock time worked in `[from, to)`. Parallel entries on several tasks
    /// are merged, so an hour on two tasks counts as one hour here.
    pub fn worked_ms(&self, from: Timestamp, to: Timestamp, now: Timestamp) -> Result<i64> {
        let (from, to, now) = (ms(from), ms(to), ms(now));
        let mut stmt = self.conn.prepare(
            "SELECT MAX(started_at, ?1), MIN(COALESCE(ended_at, ?3), ?2) FROM time_entries
             WHERE started_at < ?2 AND COALESCE(ended_at, ?3) > ?1
             ORDER BY 1",
        )?;
        let spans = stmt
            .query_map(params![from, to, now], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(merged_len(spans))
    }
}

fn merged_len(sorted: Vec<(i64, i64)>) -> i64 {
    let mut total = 0;
    let mut cur: Option<(i64, i64)> = None;
    for (s, e) in sorted {
        match cur {
            Some((cs, ce)) if s <= ce => cur = Some((cs, ce.max(e))),
            Some((cs, ce)) => {
                total += ce - cs;
                cur = Some((s, e));
            }
            None => cur = Some((s, e)),
        }
    }
    total + cur.map_or(0, |(s, e)| e - s)
}

fn save_task(conn: &Connection, t: &Task) -> Result<()> {
    conn.execute(
        "INSERT INTO tasks (id, title, notes, status, position, project_id, priority, due_at, estimate_pomodoros,
                            created_at, updated_at, completed_at, deleted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(id) DO UPDATE SET
            title = excluded.title, notes = excluded.notes, status = excluded.status,
            position = excluded.position, project_id = excluded.project_id, priority = excluded.priority,
            due_at = excluded.due_at, estimate_pomodoros = excluded.estimate_pomodoros,
            updated_at = excluded.updated_at, completed_at = excluded.completed_at,
            deleted_at = excluded.deleted_at",
        params![
            t.id.0.to_string(),
            t.title,
            t.notes,
            t.status.as_str(),
            t.position,
            t.project_id.map(|p| p.0.to_string()),
            t.priority.as_i64(),
            t.due_at.map(ms),
            t.estimate_pomodoros,
            ms(t.created_at),
            ms(t.updated_at),
            t.completed_at.map(ms),
            t.deleted_at.map(ms),
        ],
    )?;
    Ok(())
}

fn ms(t: Timestamp) -> i64 {
    t.as_millisecond()
}

fn ts(v: i64) -> rusqlite::Result<Timestamp> {
    Timestamp::from_millisecond(v).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

fn uuid(s: String) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(&s).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

fn task_from_row(r: &Row<'_>) -> rusqlite::Result<Result<Task>> {
    let status: String = r.get(3)?;
    let Some(status) = Status::parse(&status) else {
        return Ok(Err(StoreError::Corrupt(format!("task status {status:?}"))));
    };
    Ok(Ok(Task {
        id: TaskId(uuid(r.get(0)?)?),
        title: r.get(1)?,
        notes: r.get(2)?,
        status,
        position: r.get(4)?,
        project_id: r
            .get::<_, Option<String>>(5)?
            .map(uuid)
            .transpose()?
            .map(ProjectId),
        priority: Priority::from_i64(r.get(6)?),
        due_at: r.get::<_, Option<i64>>(7)?.map(ts).transpose()?,
        estimate_pomodoros: r.get(8)?,
        created_at: ts(r.get(9)?)?,
        updated_at: ts(r.get(10)?)?,
        completed_at: r.get::<_, Option<i64>>(11)?.map(ts).transpose()?,
        deleted_at: r.get::<_, Option<i64>>(12)?.map(ts).transpose()?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use yatt_core::{Board, Command, History, Mode, Placement};

    fn t(secs: i64) -> Timestamp {
        Timestamp::from_second(1_800_000_000 + secs).unwrap()
    }

    #[test]
    fn tasks_roundtrip() {
        let mut store = Store::open_in_memory().unwrap();
        let (mut board, mut h) = (Board::default(), History::default());
        let id = TaskId::new();
        let a = h
            .execute(
                &mut board,
                Command::Create {
                    id,
                    title: "Plan".into(),
                    status: Status::Backlog,
                    placement: Placement::Bottom,
                },
                t(0),
            )
            .unwrap();
        store
            .save_tasks(a.changed.iter().filter_map(|id| board.get(*id)))
            .unwrap();
        h.execute(
            &mut board,
            Command::Move {
                id,
                status: Status::Done,
                placement: Placement::Top,
            },
            t(10),
        )
        .unwrap();
        store.save_task(board.get(id).unwrap()).unwrap();

        let loaded = store.load_tasks().unwrap();
        assert_eq!(loaded, vec![board.get(id).unwrap().clone()]);
    }

    #[test]
    fn parallel_tasks_get_full_time_but_day_total_is_merged() {
        let mut store = Store::open_in_memory().unwrap();
        let mut board = Board::default();
        let (a, b) = (TaskId::new(), TaskId::new());
        for (id, title) in [(a, "a"), (b, "b")] {
            board
                .apply(
                    Command::Create {
                        id,
                        title: title.into(),
                        status: Status::Backlog,
                        placement: Placement::Bottom,
                    },
                    t(0),
                )
                .unwrap();
            store.save_task(board.get(id).unwrap()).unwrap();
        }

        let mut timer = Timer::default();
        let mut fx = timer.add_task(a, t(0));
        fx.extend(timer.add_task(b, t(0)));
        fx.extend(timer.start(Mode::Pomodoro, t(0)));
        store.apply_timer(&fx, &timer).unwrap();
        let fx = timer.tick(t(1500));
        store.apply_timer(&fx, &timer).unwrap();

        for id in [a, b] {
            let time = store.task_time(id, t(0), t(2000)).unwrap();
            assert_eq!(
                time,
                TaskTime {
                    total_ms: 1_500_000,
                    today_ms: 1_500_000,
                    pomodoros: 1
                }
            );
        }
        assert_eq!(
            store.worked_ms(t(0), t(86_400), t(2000)).unwrap(),
            1_500_000
        );

        // Timer state survives a reopen.
        assert_eq!(store.load_timer().unwrap(), Some(timer));
    }

    #[test]
    fn open_entry_counts_up_to_now() {
        let mut store = Store::open_in_memory().unwrap();
        let mut board = Board::default();
        let a = TaskId::new();
        board
            .apply(
                Command::Create {
                    id: a,
                    title: "a".into(),
                    status: Status::Backlog,
                    placement: Placement::Bottom,
                },
                t(0),
            )
            .unwrap();
        store.save_task(board.get(a).unwrap()).unwrap();
        let mut timer = Timer::default();
        let mut fx = timer.add_task(a, t(0));
        fx.extend(timer.start(Mode::Stopwatch, t(0)));
        store.apply_timer(&fx, &timer).unwrap();
        assert_eq!(store.task_time(a, t(0), t(90)).unwrap().total_ms, 90_000);
        assert_eq!(
            store.recent_entries(a, 5).unwrap(),
            [TimeEntry {
                started_at: t(0),
                ended_at: None
            }]
        );
    }

    #[test]
    fn merge_overlapping_spans() {
        assert_eq!(merged_len(vec![(0, 10), (5, 15), (20, 30)]), 25);
        assert_eq!(merged_len(vec![]), 0);
    }
}
