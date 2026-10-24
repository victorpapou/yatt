//! The app's live state: board, timer and store, wired together.
//!
//! Views never touch the store directly. They call methods here, which update
//! memory first (so the UI can animate right away) and then write to SQLite.

use std::collections::HashMap;

use jiff::{Timestamp, Zoned};
use yatt_core::{Board, Command, Effect, Filter, History, Mode, Placement, Status, TaskId, Timer};
use yatt_store::{Store, TaskTime};

pub struct AppState {
    pub board: Board,
    pub history: History,
    pub timer: Timer,
    pub filter: Filter,
    /// Tracked time per task, refreshed once a second while the timer runs.
    pub times: HashMap<TaskId, TaskTime>,
    store: Store,
    last_refresh: i64,
}

impl AppState {
    pub fn load(store: Store) -> anyhow::Result<Self> {
        let board = Board::from_tasks(store.load_tasks()?);
        let timer = store.load_timer()?.unwrap_or_default();
        let mut state = Self {
            board,
            history: History::default(),
            timer,
            filter: Filter::default(),
            times: HashMap::new(),
            store,
            last_refresh: 0,
        };
        // Catch up on phases that ended while the app was closed.
        state.tick();
        state.refresh_times();
        Ok(state)
    }

    pub fn execute(&mut self, cmd: Command) {
        match self.history.execute(&mut self.board, cmd, Timestamp::now()) {
            Ok(applied) => self.persist(&applied.changed),
            Err(e) => eprintln!("command failed: {e}"),
        }
    }

    pub fn undo(&mut self) {
        match self.history.undo(&mut self.board, Timestamp::now()) {
            Ok(Some(applied)) => self.persist(&applied.changed),
            Ok(None) => {}
            Err(e) => eprintln!("undo failed: {e}"),
        }
    }

    pub fn redo(&mut self) {
        match self.history.redo(&mut self.board, Timestamp::now()) {
            Ok(Some(applied)) => self.persist(&applied.changed),
            Ok(None) => {}
            Err(e) => eprintln!("redo failed: {e}"),
        }
    }

    pub fn create_task(&mut self, title: String) {
        self.execute(Command::Create {
            id: TaskId::new(),
            title,
            status: Status::Backlog,
            placement: Placement::Top,
        });
    }

    pub fn move_task(&mut self, id: TaskId, status: Status, placement: Placement) {
        self.execute(Command::Move {
            id,
            status,
            placement,
        });
    }

    /// Adds the task to the timer, or removes it if it is already active.
    pub fn toggle_active(&mut self, id: TaskId) {
        let now = Timestamp::now();
        let fx = if self.timer.active_tasks.contains(&id) {
            self.timer.remove_task(id, now)
        } else {
            self.timer.add_task(id, now)
        };
        self.apply_timer(fx);
    }

    /// Start, pause or resume, whichever fits the current state.
    pub fn start_or_pause(&mut self, mode: Mode) {
        let now = Timestamp::now();
        let fx = if self.timer.is_running() {
            self.timer.pause(now)
        } else if self.timer.phase == yatt_core::Phase::Idle {
            self.start_work(mode, now)
        } else {
            self.timer.resume(now)
        };
        self.apply_timer(fx);
    }

    pub fn stop(&mut self) {
        let fx = self.timer.stop(Timestamp::now());
        self.apply_timer(fx);
    }

    pub fn skip(&mut self) {
        let fx = self.timer.skip(Timestamp::now());
        self.apply_timer(fx);
    }

    /// Advances the timer. Returns true when something worth redrawing beyond the clock changed.
    pub fn tick(&mut self) -> bool {
        let fx = self.timer.tick(Timestamp::now());
        let changed = !fx.is_empty();
        if changed {
            self.apply_timer(fx);
        }
        let sec = Timestamp::now().as_second();
        if self.timer.is_running() && sec != self.last_refresh {
            self.last_refresh = sec;
            let active: Vec<TaskId> = self.timer.active_tasks.iter().copied().collect();
            self.refresh_times_of(active);
        }
        changed
    }

    /// Starting work pulls the active backlog tasks into "In progress".
    fn start_work(&mut self, mode: Mode, now: Timestamp) -> Vec<Effect> {
        let to_move: Vec<TaskId> = self
            .timer
            .active_tasks
            .iter()
            .copied()
            .filter(|id| {
                self.board
                    .get(*id)
                    .is_some_and(|t| t.status == Status::Backlog)
            })
            .collect();
        for id in to_move {
            self.move_task(id, Status::InProgress, Placement::Bottom);
        }
        self.timer.start(mode, now)
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
        let now = Timestamp::now();
        let day_start = Zoned::now()
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
