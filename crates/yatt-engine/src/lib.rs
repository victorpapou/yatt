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
use yatt_store::{EntryRow, Store, TaskTime, TimeEntry};

/// A change to the time log, kept for undo beside the board's own history.
#[derive(Debug, Clone)]
enum TimeEdit {
    Set { id: String, start: Timestamp, end: Timestamp },
    Add { id: String, task: TaskId, start: Timestamp, end: Timestamp },
    Delete { id: String },
}

pub type Clock = Box<dyn Fn() -> Timestamp>;

pub struct Engine {
    pub board: Board,
    pub history: History,
    pub timer: Timer,
    pub filter: Filter,
    /// Tracked time per task, refreshed once a second while the timer runs.
    pub times: HashMap<TaskId, TaskTime>,
    store: Store,
    /// Messages for the user that nobody has shown yet.
    problems: Vec<String>,
    /// Saving is failing right now; the user has been told once already.
    save_failing: bool,
    clock: Clock,
    tz: TimeZone,
    last_refresh: i64,
    last_saved: i64,
    /// A clock jump longer than this while running means sleep or a closed app.
    pub max_gap_ms: i64,
    day: Option<jiff::civil::Date>,
    /// Told when a phase ends: the phase that finished and the one that follows.
    pub on_phase_finished: Option<Box<dyn FnMut(Phase, Phase)>>,
    /// Which history each undo step belongs to, oldest first: `true` for a time
    /// edit, `false` for a board command; and the same for redo.
    order_undo: Vec<bool>,
    order_redo: Vec<bool>,
    time_undo: Vec<TimeEdit>,
    time_redo: Vec<TimeEdit>,
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
            problems: Vec::new(),
            save_failing: false,
            clock,
            tz,
            last_refresh: 0,
            last_saved: 0,
            max_gap_ms: 15_000,
            day: None,
            on_phase_finished: None,
            order_undo: Vec::new(),
            order_redo: Vec::new(),
            time_undo: Vec::new(),
            time_redo: Vec::new(),
        };
        // Catch up on phases that ended while the app was closed.
        engine.purge_expired();
        engine.tick();
        engine.sync_active();
        engine.refresh_times();
        Ok(engine)
    }

    /// Hands over the oldest message the user has not seen yet (a failed save,
    /// say), or `None`. The app shows it in an alert.
    pub fn take_problem(&mut self) -> Option<String> {
        if self.problems.is_empty() {
            None
        } else {
            Some(self.problems.remove(0))
        }
    }

    /// Records a failed save. The user hears about it once, not on every edit
    /// until it works again.
    fn save_result(&mut self, what: &str, result: Result<(), impl std::fmt::Display>) {
        match result {
            Ok(()) => self.save_failing = false,
            Err(e) => {
                eprintln!("{what} failed: {e}");
                if !self.save_failing {
                    self.save_failing = true;
                    self.problems.push(format!(
                        "Не удалось записать изменения на диск ({what}): {e}\n\n\
                         Последние правки могут пропасть после перезапуска. \
                         Проверьте свободное место и не открыта ли база в другой программе."
                    ));
                }
            }
        }
    }

    pub fn now(&self) -> Timestamp {
        (self.clock)()
    }

    pub fn execute(&mut self, cmd: Command) {
        let now = self.now();
        match self.history.execute(&mut self.board, cmd, now) {
            Ok(applied) => {
                self.order_undo.push(false);
                self.order_redo.clear();
                self.time_redo.clear();
                self.after_change(&applied.changed)
            }
            Err(e) => eprintln!("command failed: {e}"),
        }
    }

    pub fn undo(&mut self) {
        while let Some(time) = self.order_undo.pop() {
            if time {
                if let Some(edit) = self.time_undo.pop() {
                    if let Some(inverse) = self.apply_time_edit(edit) {
                        self.time_redo.push(inverse);
                        self.order_redo.push(true);
                    }
                    return;
                }
                continue;
            }
            let now = self.now();
            match self.history.undo(&mut self.board, now) {
                Ok(Some(applied)) => {
                    self.order_redo.push(false);
                    self.after_change(&applied.changed);
                    return;
                }
                Ok(None) => continue,
                Err(e) => {
                    eprintln!("undo failed: {e}");
                    return;
                }
            }
        }
    }

    pub fn redo(&mut self) {
        while let Some(time) = self.order_redo.pop() {
            if time {
                if let Some(edit) = self.time_redo.pop() {
                    if let Some(inverse) = self.apply_time_edit(edit) {
                        self.time_undo.push(inverse);
                        self.order_undo.push(true);
                    }
                    return;
                }
                continue;
            }
            let now = self.now();
            match self.history.redo(&mut self.board, now) {
                Ok(Some(applied)) => {
                    self.order_undo.push(false);
                    self.after_change(&applied.changed);
                    return;
                }
                Ok(None) => continue,
                Err(e) => {
                    eprintln!("redo failed: {e}");
                    return;
                }
            }
        }
    }

    // ---- the time log ----

    /// Every entry of the task, newest first, with ids.
    pub fn entries(&self, task: TaskId) -> Vec<EntryRow> {
        self.store.entries(task).unwrap_or_else(|e| {
            eprintln!("reading entries failed: {e}");
            Vec::new()
        })
    }

    /// Applies one time edit and returns its inverse; `None` when it could not apply.
    fn apply_time_edit(&mut self, edit: TimeEdit) -> Option<TimeEdit> {
        let (inverse, task) = match edit {
            TimeEdit::Set { id, start, end } => {
                let old = self.store.entry(&id).ok().flatten()?;
                let old_end = old.ended_at?;
                self.store.set_entry(&id, start, Some(end)).ok()?;
                (TimeEdit::Set { id, start: old.started_at, end: old_end }, old.task)
            }
            TimeEdit::Add { id, task, start, end } => {
                self.store.add_entry(&id, task, start, end).ok()?;
                (TimeEdit::Delete { id }, task)
            }
            TimeEdit::Delete { id } => {
                let old = self.store.entry(&id).ok().flatten()?;
                let end = old.ended_at?;
                self.store.delete_entry(&id).ok()?;
                (
                    TimeEdit::Add { id, task: old.task, start: old.started_at, end },
                    old.task,
                )
            }
        };
        self.refresh_times_of(vec![task]);
        Some(inverse)
    }

    fn record_time_edit(&mut self, edit: TimeEdit) {
        if let Some(inverse) = self.apply_time_edit(edit) {
            self.time_undo.push(inverse);
            self.time_redo.clear();
            self.order_undo.push(true);
            self.order_redo.clear();
        }
    }

    /// Moves a finished entry's start and end. A running entry, or one that
    /// would end before it starts, is left alone.
    pub fn edit_entry(&mut self, id: &str, start: Timestamp, end: Timestamp) {
        if end > start {
            self.record_time_edit(TimeEdit::Set { id: id.to_string(), start, end });
        }
    }

    /// Adds a finished entry written by hand.
    pub fn add_entry(&mut self, task: TaskId, start: Timestamp, end: Timestamp) {
        if end > start {
            let id = uuid::Uuid::now_v7().to_string();
            self.record_time_edit(TimeEdit::Add { id, task, start, end });
        }
    }

    /// Removes a finished entry.
    pub fn delete_entry(&mut self, id: &str) {
        self.record_time_edit(TimeEdit::Delete { id: id.to_string() });
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

    /// A new task with its own colour, as one step to undo.
    pub fn create_task_coloured(&mut self, title: String, status: Status, color: u8) -> TaskId {
        let id = TaskId::new();
        self.execute(Command::Batch(vec![
            Command::Create {
                id,
                title,
                status,
                placement: Placement::Top,
            },
            Command::SetLook {
                id,
                color: Some(color),
                icon: None,
            },
        ]));
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

    /// Tucks every done task on the board into the archive, as one undo step.
    pub fn archive_done(&mut self) {
        let cmds: Vec<Command> = self
            .board
            .done_ids()
            .into_iter()
            .map(|id| Command::Archive { id })
            .collect();
        if !cmds.is_empty() {
            self.execute(Command::Batch(cmds));
        }
    }

    pub fn archive_task(&mut self, id: TaskId) {
        self.execute(Command::Archive { id });
    }

    /// Brings a task back from the archive or the trash onto the board.
    pub fn bring_back(&mut self, id: TaskId) {
        let Some(t) = self.board.get(id) else { return };
        let mut cmds = Vec::new();
        if t.is_deleted() {
            cmds.push(Command::Restore { id });
        }
        if t.is_archived() {
            cmds.push(Command::Unarchive { id });
        }
        if !cmds.is_empty() {
            self.execute(Command::Batch(cmds));
        }
    }

    /// Empties the trash for good. Not undoable.
    pub fn empty_trash(&mut self) {
        let ids: Vec<TaskId> = self.board.trashed().iter().map(|t| t.id).collect();
        self.purge(&ids);
    }

    /// Drops what has been in the trash longer than [`yatt_core::task::TRASH_DAYS`].
    pub fn purge_expired(&mut self) {
        let limit = self.now() - jiff::SignedDuration::from_hours(24 * yatt_core::task::TRASH_DAYS);
        let ids: Vec<TaskId> = self
            .board
            .trashed()
            .iter()
            .filter(|t| t.deleted_at.is_some_and(|d| d < limit))
            .map(|t| t.id)
            .collect();
        self.purge(&ids);
    }

    fn purge(&mut self, ids: &[TaskId]) {
        if ids.is_empty() {
            return;
        }
        match self.store.purge_tasks(ids) {
            Ok(()) => {
                self.board.purge(ids);
                self.history.clear();
            }
            Err(e) => eprintln!("emptying the trash failed: {e}"),
        }
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

    /// The pomodoro's work and break lengths, in minutes. Kept with the timer;
    /// a running interval takes the new length at once, its time so far kept.
    pub fn set_durations(&mut self, work_min: u32, break_min: u32) {
        self.timer.settings.work_secs = work_min.clamp(1, 240) * 60;
        self.timer.settings.short_break_secs = break_min.clamp(1, 120) * 60;
        self.apply_timer(Vec::new());
    }

    /// The Mac is going to sleep or the screen locks: stop counting right now.
    pub fn system_sleep(&mut self) {
        let fx = self.timer.interrupt(self.now());
        self.apply_timer(fx);
    }

    /// Long break, "every N" and the two auto-starts. Kept with the timer.
    pub fn set_cycle(&mut self, long_min: u32, every: u32, auto_breaks: bool, auto_work: bool) {
        let s = &mut self.timer.settings;
        s.long_break_secs = long_min.clamp(1, 120) * 60;
        s.long_break_every = every.clamp(2, 12);
        s.auto_start_breaks = auto_breaks;
        s.auto_start_work = auto_work;
        self.apply_timer(Vec::new());
    }

    pub fn skip(&mut self) {
        let fx = self.timer.skip(self.now());
        self.apply_timer(fx);
    }

    /// Advances the timer. Returns true when something worth redrawing beyond the clock changed.
    pub fn tick(&mut self) -> bool {
        let now = self.now();
        let mut fx = self.timer.heartbeat(now, self.max_gap_ms);
        fx.extend(self.timer.tick(now));
        let changed = !fx.is_empty();
        if changed {
            self.apply_timer(fx);
        }
        let day = now.to_zoned(self.tz.clone()).date();
        if self.day != Some(day) {
            // Midnight passed: "today" restarts for every task, running or not.
            self.day = Some(day);
            self.refresh_times();
        }
        let sec = now.as_second();
        // Keep the last-seen moment on disk so a crash or quit loses at most a few seconds.
        if !changed && self.timer.is_running() && sec - self.last_saved >= 5 {
            self.last_saved = sec;
            if let Err(e) = self.store.save_timer(&self.timer) {
                eprintln!("saving timer failed: {e}");
            }
        }
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
        // A deleted task drops out of bulk refreshes; take its final numbers once.
        self.refresh_times_of(changed.to_vec());
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
                if let Some(f) = self.on_phase_finished.as_mut() {
                    f(*phase, *next);
                }
            }
        }
        let saved = self.store.apply_timer(&fx, &self.timer);
        self.save_result("таймер", saved);
        self.refresh_times();
    }

    fn persist(&mut self, changed: &[TaskId]) {
        let tasks: Vec<_> = changed
            .iter()
            .filter_map(|id| self.board.get(*id))
            .collect();
        let saved = self.store.save_tasks(tasks);
        self.save_result("задачи", saved);
    }

    fn refresh_times(&mut self) {
        let ids: Vec<TaskId> = self
            .board
            .tasks()
            .filter(|t| t.deleted_at.is_none())
            .map(|t| t.id)
            .collect();
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
            let mut engine = engine;
            engine.max_gap_ms = i64::MAX / 2; // tests jump the clock freely
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
    fn review_writes_no_time() {
        let mut f = Fixture::new();
        let a = f.task("a", Status::InProgress);
        f.at(0).start_or_pause(Mode::Stopwatch);
        f.at(10).move_task(a, Status::Review, Placement::Top);
        assert!(!f.engine.timer.active_tasks.contains(&a));
        f.at(40).stop();
        assert_eq!(f.total(a), 10_000);
        assert_eq!(f.status(a), Status::Review);
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
    fn durations_change_the_running_interval_keeping_its_time() {
        let mut f = Fixture::new();
        f.task("a", Status::InProgress);
        f.at(0).start_or_pause(Mode::Pomodoro);
        f.at(600).set_durations(50, 15);
        let now = f.engine.now();
        assert_eq!(f.engine.timer.settings.work_secs, 50 * 60);
        assert_eq!(f.engine.timer.settings.short_break_secs, 15 * 60);
        assert_eq!(f.engine.timer.elapsed_ms(now), 600_000);
        assert_eq!(f.engine.timer.remaining_ms(now), Some(40 * 60_000));
    }

    #[test]
    fn sleep_gap_is_not_tracked() {
        let mut f = Fixture::new();
        let a = f.task("a", Status::InProgress);
        f.at(0).start_or_pause(Mode::Stopwatch);
        f.engine.max_gap_ms = 15_000;
        f.at(5).tick();
        f.at(36_000).tick(); // woke up ten hours later
        assert_eq!(f.total(a), 5_000);
        assert!(!f.engine.timer.is_running());
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

    #[test]
    fn archive_done_and_empty_trash() {
        let mut f = Fixture::new();
        let a = f.engine.create_task("a".into(), Status::Done);
        let b = f.engine.create_task("b".into(), Status::Backlog);
        f.engine.archive_done();
        assert!(f.engine.board.get(a).unwrap().is_archived());
        f.engine.undo();
        assert!(!f.engine.board.get(a).unwrap().is_archived());
        f.engine.delete_task(b);
        f.engine.empty_trash();
        assert!(f.engine.board.get(b).is_none());
    }
}
