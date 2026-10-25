//! The app's live state: board, timer and store, wired together.
//!
//! Views never touch the store directly. They call methods here, which update
//! memory first (so the UI can animate right away) and then write to SQLite.
//! No UI code lives here, so the rules between board and timer are tested on any OS.
//!
//! Time goes to the tasks in "In progress", and only to them: the timer's active
//! set always equals the live tasks of that column. Moving a task in starts its
//! entry (if a work phase runs), moving it out or deleting it closes the entry.
//! This holds after every board change, undo and redo included.

use std::collections::{BTreeSet, HashMap};

use jiff::Timestamp;
use jiff::tz::TimeZone;
use yatt_core::{
    Board, Command, Effect, Filter, History, Mode, Phase, Placement, Status, TaskId, Timer,
};
use yatt_store::{Store, TaskTime, TimeEntry};

pub type Clock = Box<dyn Fn() -> Timestamp>;

pub struct Engine {
    pub board: Board,
    pub history: History,
    pub timer: Timer,
    pub filter: Filter,
    /// Tracked time per task, refreshed once a second while the timer runs.
    pub times: HashMap<TaskId, TaskTime>,
    store: Store,
    clock: Clock,
    tz: TimeZone,
    last_refresh: i64,
}

impl Engine {
    pub fn load(store: Store) -> anyhow::Result<Self> {
        Self::with_clock(store, Box::new(Timestamp::now), TimeZone::system())
    }

    pub fn with_clock(store: Store, clock: Clock, tz: TimeZone) -> anyhow::Result<Self> {
        let board = Board::from_tasks(store.load_tasks()?);
        let timer = store.load_timer()?.unwrap_or_default();
        let mut engine = Self {
            board,
            history: History::default(),
            timer,
            filter: Filter::default(),
            times: HashMap::new(),
            store,
            clock,
            tz,
            last_refresh: 0,
        };
        // Catch up on phases that ended while the app was closed.
        engine.tick();
        engine.sync_active();
        engine.refresh_times();
        Ok(engine)
    }

    pub fn now(&self) -> Timestamp {
        (self.clock)()
    }

    pub fn execute(&mut self, cmd: Command) {
        let now = self.now();
        match self.history.execute(&mut self.board, cmd, now) {
            Ok(applied) => self.after_change(&applied.changed),
            Err(e) => eprintln!("command failed: {e}"),
        }
    }

    pub fn undo(&mut self) {
        let now = self.now();
        match self.history.undo(&mut self.board, now) {
            Ok(Some(applied)) => self.after_change(&applied.changed),
            Ok(None) => {}
            Err(e) => eprintln!("undo failed: {e}"),
        }
    }

    pub fn redo(&mut self) {
        let now = self.now();
        match self.history.redo(&mut self.board, now) {
            Ok(Some(applied)) => self.after_change(&applied.changed),
            Ok(None) => {}
            Err(e) => eprintln!("redo failed: {e}"),
        }
    }

    /// A new task on top of `status`.
    pub fn create_task(&mut self, title: String, status: Status) -> TaskId {
        let id = TaskId::new();
        self.execute(Command::Create {
            id,
            title,
            status,
            placement: Placement::Top,
        });
        id
    }

    pub fn move_task(&mut self, id: TaskId, status: Status, placement: Placement) {
        self.execute(Command::Move {
            id,
            status,
            placement,
        });
    }

    pub fn delete_task(&mut self, id: TaskId) {
        self.execute(Command::Delete { id });
    }

    pub fn is_active(&self, id: TaskId) -> bool {
        self.timer.active_tasks.contains(&id)
    }

    /// Start, pause or resume, whichever fits the current state.
    pub fn start_or_pause(&mut self, mode: Mode) {
        let now = self.now();
        let fx = if self.timer.is_running() {
            self.timer.pause(now)
        } else if self.timer.phase == Phase::Idle {
            self.timer.start(mode, now)
        } else {
            self.timer.resume(now)
        };
        self.apply_timer(fx);
    }

    pub fn stop(&mut self) {
        let fx = self.timer.stop(self.now());
        self.apply_timer(fx);
    }

    pub fn skip(&mut self) {
        let fx = self.timer.skip(self.now());
        self.apply_timer(fx);
    }

    /// Advances the timer. Returns true when something worth redrawing beyond the clock changed.
    pub fn tick(&mut self) -> bool {
        let now = self.now();
        let fx = self.timer.tick(now);
        let changed = !fx.is_empty();
        if changed {
            self.apply_timer(fx);
        }
        let sec = now.as_second();
        if self.timer.is_running() && sec != self.last_refresh {
            self.last_refresh = sec;
            let active: Vec<TaskId> = self.timer.active_tasks.iter().copied().collect();
            self.refresh_times_of(active);
        }
        changed
    }

    /// The task's latest time entries, newest first.
    pub fn recent_entries(&self, id: TaskId, limit: usize) -> Vec<TimeEntry> {
        self.store.recent_entries(id, limit).unwrap_or_else(|e| {
            eprintln!("reading entries failed: {e}");
            Vec::new()
        })
    }

    /// Persists changed tasks and brings the timer in line with "In progress".
    fn after_change(&mut self, changed: &[TaskId]) {
        self.persist(changed);
        self.sync_active();
    }

    /// Makes the timer's active set equal the live tasks in "In progress".
    fn sync_active(&mut self) {
        let want: BTreeSet<TaskId> = self
            .board
            .column(Status::InProgress, &Filter::default())
            .iter()
            .map(|t| t.id)
            .collect();
        if want == self.timer.active_tasks {
            return;
        }
        let now = self.now();
        let mut fx = Vec::new();
        let gone: Vec<TaskId> = self.timer.active_tasks.difference(&want).copied().collect();
        for id in gone {
            fx.extend(self.timer.remove_task(id, now));
        }
        for &id in want.difference(&self.timer.active_tasks.clone()) {
            fx.extend(self.timer.add_task(id, now));
        }
        self.apply_timer(fx);
    }

    fn apply_timer(&mut self, fx: Vec<Effect>) {
        for e in &fx {
            if let Effect::PhaseFinished { phase, next } = e {
                // Notifications and sounds hook in here (stage 3).
                eprintln!("phase {phase:?} finished, next {next:?}");
            }
        }
        if let Err(e) = self.store.apply_timer(&fx, &self.timer) {
            eprintln!("saving timer failed: {e}");
        }
        self.refresh_times();
    }

    fn persist(&mut self, changed: &[TaskId]) {
        let tasks: Vec<_> = changed
            .iter()
            .filter_map(|id| self.board.get(*id))
            .collect();
        if let Err(e) = self.store.save_tasks(tasks) {
            eprintln!("saving tasks failed: {e}");
        }
    }

    fn refresh_times(&mut self) {
        let ids: Vec<TaskId> = self.board.tasks().map(|t| t.id).collect();
        self.refresh_times_of(ids);
    }

    fn refresh_times_of(&mut self, ids: Vec<TaskId>) {
        let now = self.now();
        let day_start = now
            .to_zoned(self.tz.clone())
            .start_of_day()
            .map(|z| z.timestamp())
            .unwrap_or(now);
        for id in ids {
            match self.store.task_time(id, day_start, now) {
                Ok(t) => {
                    self.times.insert(id, t);
                }
                Err(e) => eprintln!("reading time failed: {e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    struct Fixture {
        engine: Engine,
        clock: Rc<Cell<i64>>,
    }

    impl Fixture {
        fn new() -> Self {
            let clock = Rc::new(Cell::new(0));
            let c = clock.clone();
            let engine = Engine::with_clock(
                Store::open_in_memory().unwrap(),
                Box::new(move || Timestamp::from_second(1_800_000_000 + c.get()).unwrap()),
                TimeZone::UTC,
            )
            .unwrap();
            Self { engine, clock }
        }

        fn at(&mut self, secs: i64) -> &mut Engine {
            self.clock.set(secs);
            &mut self.engine
        }

        fn task(&mut self, title: &str, status: Status) -> TaskId {
            let id = self.engine.create_task(title.into(), Status::Backlog);
            if status != Status::Backlog {
                self.engine.move_task(id, status, Placement::Bottom);
            }
            id
        }

        fn total(&mut self, id: TaskId) -> i64 {
            self.engine.refresh_times();
            self.engine.times[&id].total_ms
        }

        fn status(&self, id: TaskId) -> Status {
            self.engine.board.get(id).unwrap().status
        }
    }

    #[test]
    fn in_progress_tasks_accrue_full_time_and_backlog_none() {
        let mut f = Fixture::new();
        let a = f.task("a", Status::InProgress);
        let b = f.task("b", Status::InProgress);
        let c = f.task("c", Status::Backlog);
        assert_eq!(f.engine.timer.active_tasks, BTreeSet::from([a, b]));
        f.at(0).start_or_pause(Mode::Pomodoro);
        f.at(42).tick();
        assert_eq!(f.engine.times[&a].total_ms, 42_000);
        f.at(60).start_or_pause(Mode::Pomodoro); // pause
        f.at(500);
        assert_eq!((f.total(a), f.total(b), f.total(c)), (60_000, 60_000, 0));
        // Start does not pull backlog tasks anywhere.
        assert_eq!(f.status(c), Status::Backlog);
    }

    #[test]
    fn done_closes_entry_and_undo_resumes_it() {
        let mut f = Fixture::new();
        let a = f.task("a", Status::InProgress);
        let b = f.task("b", Status::InProgress);
        f.at(0).start_or_pause(Mode::Stopwatch);
        f.at(10).move_task(a, Status::Done, Placement::Top);
        assert!(!f.engine.timer.active_tasks.contains(&a));
        let entries = f.engine.recent_entries(a, 10);
        assert_eq!(entries[0].ended_at, Some(f.engine.now()));
        f.at(20).undo();
        assert_eq!(f.status(a), Status::InProgress);
        f.at(30).redo();
        f.at(35).undo();
        f.at(50).stop();
        // a: 0-10, 20-30, 35-50.
        assert_eq!((f.total(a), f.total(b)), (35_000, 50_000));
    }

    #[test]
    fn moving_into_and_out_of_progress_mid_phase() {
        let mut f = Fixture::new();
        let a = f.task("a", Status::Backlog);
        f.at(0).start_or_pause(Mode::Pomodoro);
        f.at(10).move_task(a, Status::InProgress, Placement::Bottom);
        f.at(25).move_task(a, Status::Backlog, Placement::Top);
        f.at(40).move_task(a, Status::InProgress, Placement::Top);
        f.at(45).delete_task(a);
        f.at(100);
        assert_eq!(f.total(a), 20_000);
        assert!(f.engine.timer.active_tasks.is_empty());
    }

    #[test]
    fn breaks_write_nothing() {
        let mut f = Fixture::new();
        let a = f.task("a", Status::InProgress);
        f.at(0).start_or_pause(Mode::Pomodoro);
        // Work ends at 1500, the break auto-starts and runs to 1800.
        f.at(1700).tick();
        assert_eq!(f.engine.timer.phase, Phase::ShortBreak);
        // A task moved in during the break gets no time either.
        let b = f.task("b", Status::InProgress);
        f.at(1800).tick();
        f.at(1900);
        assert_eq!((f.total(a), f.total(b)), (1_500_000, 0));
        assert_eq!(f.engine.times[&a].pomodoros, 1);
    }

    #[test]
    fn load_syncs_active_set_with_column() {
        let mut f = Fixture::new();
        let a = f.task("a", Status::InProgress);
        // A stale timer from an older version that tracked a backlog task.
        f.engine.timer.active_tasks.insert(TaskId::new());
        f.engine.sync_active();
        assert_eq!(f.engine.timer.active_tasks, BTreeSet::from([a]));
    }
}
