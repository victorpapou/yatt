//! Pomodoro / stopwatch state machine.
//!
//! The timer never counts ticks. It keeps when the current run started and how
//! much time was banked before that, so the remaining time is computed from the
//! clock at any moment: the UI can redraw every frame, and a restart or a sleeping
//! Mac loses nothing. Methods take `now` and return [`Effect`]s; the caller writes
//! them to storage and turns them into notifications.
//!
//! Several tasks can be active at once. Every active task gets its own time entry
//! for the same interval, so an hour on two tasks is an hour on each.

use std::collections::BTreeSet;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::ids::{SessionId, TaskId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimerSettings {
    pub work_secs: u32,
    pub short_break_secs: u32,
    pub long_break_secs: u32,
    /// A long break follows every N-th completed work phase.
    pub long_break_every: u32,
    pub auto_start_breaks: bool,
    pub auto_start_work: bool,
}

impl Default for TimerSettings {
    fn default() -> Self {
        Self {
            work_secs: 25 * 60,
            short_break_secs: 5 * 60,
            long_break_secs: 15 * 60,
            long_break_every: 4,
            auto_start_breaks: true,
            auto_start_work: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    Pomodoro,
    Stopwatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Idle,
    Work,
    ShortBreak,
    LongBreak,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionKind {
    Work,
    ShortBreak,
    LongBreak,
    Stopwatch,
}

impl SessionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionKind::Work => "work",
            SessionKind::ShortBreak => "short_break",
            SessionKind::LongBreak => "long_break",
            SessionKind::Stopwatch => "stopwatch",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Completed,
    Skipped,
    Abandoned,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Completed => "completed",
            Outcome::Skipped => "skipped",
            Outcome::Abandoned => "abandoned",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    SessionStarted {
        id: SessionId,
        kind: SessionKind,
        planned_secs: Option<u32>,
        at: Timestamp,
    },
    SessionEnded {
        id: SessionId,
        outcome: Outcome,
        at: Timestamp,
    },
    EntryOpened {
        task: TaskId,
        session: SessionId,
        at: Timestamp,
    },
    EntryClosed {
        task: TaskId,
        session: SessionId,
        at: Timestamp,
    },
    /// A pomodoro phase ran to its end: time for a sound and a notification.
    PhaseFinished { phase: Phase, next: Phase },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timer {
    pub settings: TimerSettings,
    pub mode: Mode,
    pub phase: Phase,
    /// `None` while the phase is armed but not started yet (e.g. a break waiting for a click).
    pub session: Option<SessionId>,
    /// Set while running: start of the current run segment.
    pub running_since: Option<Timestamp>,
    /// Milliseconds counted in this phase before `running_since`.
    pub banked_ms: i64,
    /// Work phases completed since the last long break.
    pub completed_work: u32,
    pub active_tasks: BTreeSet<TaskId>,
}

impl Default for Timer {
    fn default() -> Self {
        Self::new(TimerSettings::default())
    }
}

impl Timer {
    pub fn new(settings: TimerSettings) -> Self {
        Self {
            settings,
            mode: Mode::Pomodoro,
            phase: Phase::Idle,
            session: None,
            running_since: None,
            banked_ms: 0,
            completed_work: 0,
            active_tasks: BTreeSet::new(),
        }
    }

    pub fn is_running(&self) -> bool {
        self.running_since.is_some()
    }

    /// True for phases whose time is written to tasks.
    pub fn is_tracking_phase(&self) -> bool {
        self.phase == Phase::Work
    }

    pub fn elapsed_ms(&self, now: Timestamp) -> i64 {
        self.banked_ms + self.running_since.map_or(0, |s| (ms(now) - ms(s)).max(0))
    }

    /// Planned length of the current phase; `None` for the stopwatch and idle.
    pub fn planned_secs(&self) -> Option<u32> {
        if self.mode == Mode::Stopwatch {
            return None;
        }
        match self.phase {
            Phase::Idle => None,
            Phase::Work => Some(self.settings.work_secs),
            Phase::ShortBreak => Some(self.settings.short_break_secs),
            Phase::LongBreak => Some(self.settings.long_break_secs),
        }
    }

    pub fn remaining_ms(&self, now: Timestamp) -> Option<i64> {
        self.planned_secs()
            .map(|p| (p as i64 * 1000 - self.elapsed_ms(now)).max(0))
    }

    /// When the running phase ends, if it has an end.
    pub fn deadline(&self) -> Option<Timestamp> {
        let since = self.running_since?;
        let planned = self.planned_secs()? as i64 * 1000;
        Some(from_ms(ms(since) + planned - self.banked_ms))
    }

    /// Phase progress in 0.0..=1.0 for rings and bars.
    pub fn progress(&self, now: Timestamp) -> f32 {
        match self.planned_secs() {
            Some(p) if p > 0 => (self.elapsed_ms(now) as f32 / (p as f32 * 1000.0)).clamp(0.0, 1.0),
            _ => 0.0,
        }
    }

    /// Starts a work phase (or the stopwatch) from idle. Ignored when not idle.
    pub fn start(&mut self, mode: Mode, now: Timestamp) -> Vec<Effect> {
        if self.phase != Phase::Idle {
            return Vec::new();
        }
        self.mode = mode;
        self.phase = Phase::Work;
        self.banked_ms = 0;
        let mut fx = Vec::new();
        self.begin_session(now, &mut fx);
        fx
    }

    pub fn pause(&mut self, now: Timestamp) -> Vec<Effect> {
        let Some(since) = self.running_since.take() else {
            return Vec::new();
        };
        self.banked_ms += (ms(now) - ms(since)).max(0);
        let mut fx = Vec::new();
        self.close_entries(now, &mut fx);
        fx
    }

    /// Resumes a paused phase or starts an armed one.
    pub fn resume(&mut self, now: Timestamp) -> Vec<Effect> {
        if self.phase == Phase::Idle || self.is_running() {
            return Vec::new();
        }
        let mut fx = Vec::new();
        if self.session.is_none() {
            self.begin_session(now, &mut fx);
        } else {
            self.running_since = Some(now);
            self.open_entries(now, &mut fx);
        }
        fx
    }

    /// Stops everything and returns to idle. A stopwatch run counts as completed.
    pub fn stop(&mut self, now: Timestamp) -> Vec<Effect> {
        let mut fx = Vec::new();
        if self.phase == Phase::Idle {
            return fx;
        }
        if self.is_running() {
            self.close_entries(now, &mut fx);
        }
        if let Some(id) = self.session.take() {
            let outcome = if self.mode == Mode::Stopwatch {
                Outcome::Completed
            } else {
                Outcome::Abandoned
            };
            fx.push(Effect::SessionEnded {
                id,
                outcome,
                at: now,
            });
        }
        self.phase = Phase::Idle;
        self.running_since = None;
        self.banked_ms = 0;
        fx
    }

    /// Ends the current pomodoro phase early and moves on to the next one.
    pub fn skip(&mut self, now: Timestamp) -> Vec<Effect> {
        if self.mode != Mode::Pomodoro || self.phase == Phase::Idle {
            return Vec::new();
        }
        let mut fx = Vec::new();
        self.finish_phase(now, Outcome::Skipped, &mut fx);
        fx
    }

    pub fn add_task(&mut self, task: TaskId, now: Timestamp) -> Vec<Effect> {
        let mut fx = Vec::new();
        if self.active_tasks.insert(task)
            && self.writes_entries()
            && let Some(session) = self.session
        {
            fx.push(Effect::EntryOpened {
                task,
                session,
                at: now,
            });
        }
        fx
    }

    pub fn remove_task(&mut self, task: TaskId, now: Timestamp) -> Vec<Effect> {
        let mut fx = Vec::new();
        if self.active_tasks.remove(&task)
            && self.writes_entries()
            && let Some(session) = self.session
        {
            fx.push(Effect::EntryClosed {
                task,
                session,
                at: now,
            });
        }
        fx
    }

    /// Advances phases whose deadline has passed. Call it from a UI frame or a
    /// once-a-second timer, and once at launch to catch up after a restart.
    pub fn tick(&mut self, now: Timestamp) -> Vec<Effect> {
        let mut fx = Vec::new();
        // A loop, because after a long sleep with auto-start several phases may have ended.
        for _ in 0..64 {
            match self.deadline() {
                Some(deadline) if deadline <= now => {
                    self.finish_phase(deadline, Outcome::Completed, &mut fx)
                }
                _ => break,
            }
        }
        fx
    }

    fn writes_entries(&self) -> bool {
        self.is_running() && self.is_tracking_phase()
    }

    fn begin_session(&mut self, at: Timestamp, fx: &mut Vec<Effect>) {
        let id = SessionId::new();
        let kind = match (self.mode, self.phase) {
            (Mode::Stopwatch, _) => SessionKind::Stopwatch,
            (_, Phase::ShortBreak) => SessionKind::ShortBreak,
            (_, Phase::LongBreak) => SessionKind::LongBreak,
            _ => SessionKind::Work,
        };
        self.session = Some(id);
        self.running_since = Some(at);
        fx.push(Effect::SessionStarted {
            id,
            kind,
            planned_secs: self.planned_secs(),
            at,
        });
        self.open_entries(at, fx);
    }

    fn open_entries(&self, at: Timestamp, fx: &mut Vec<Effect>) {
        if let (true, Some(session)) = (self.is_tracking_phase(), self.session) {
            fx.extend(self.active_tasks.iter().map(|&task| Effect::EntryOpened {
                task,
                session,
                at,
            }));
        }
    }

    fn close_entries(&self, at: Timestamp, fx: &mut Vec<Effect>) {
        if let (true, Some(session)) = (self.is_tracking_phase(), self.session) {
            fx.extend(self.active_tasks.iter().map(|&task| Effect::EntryClosed {
                task,
                session,
                at,
            }));
        }
    }

    fn finish_phase(&mut self, at: Timestamp, outcome: Outcome, fx: &mut Vec<Effect>) {
        let was_running = self.is_running();
        if was_running {
            self.close_entries(at, fx);
        }
        if let Some(id) = self.session.take() {
            fx.push(Effect::SessionEnded { id, outcome, at });
        }
        let finished = self.phase;
        let next = match finished {
            Phase::Work => {
                if outcome == Outcome::Completed {
                    self.completed_work += 1;
                }
                let every = self.settings.long_break_every.max(1);
                if self.completed_work > 0 && self.completed_work.is_multiple_of(every) {
                    self.completed_work = 0;
                    Phase::LongBreak
                } else {
                    Phase::ShortBreak
                }
            }
            Phase::ShortBreak | Phase::LongBreak | Phase::Idle => Phase::Work,
        };
        if outcome == Outcome::Completed {
            fx.push(Effect::PhaseFinished {
                phase: finished,
                next,
            });
        }
        self.phase = next;
        self.banked_ms = 0;
        self.running_since = None;
        let auto = match next {
            Phase::Work => self.settings.auto_start_work,
            _ => self.settings.auto_start_breaks,
        };
        // Auto-start only continues an unattended run; a skipped phase starts the next one right away.
        if (auto && was_running) || outcome == Outcome::Skipped {
            self.begin_session(at, fx);
        }
    }
}

fn ms(t: Timestamp) -> i64 {
    t.as_millisecond()
}

fn from_ms(v: i64) -> Timestamp {
    Timestamp::from_millisecond(v).expect("timestamp in range")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(secs: i64) -> Timestamp {
        Timestamp::from_second(1_800_000_000 + secs).unwrap()
    }

    fn opened(fx: &[Effect]) -> Vec<TaskId> {
        fx.iter()
            .filter_map(|e| {
                if let Effect::EntryOpened { task, .. } = e {
                    Some(*task)
                } else {
                    None
                }
            })
            .collect()
    }

    fn closed(fx: &[Effect]) -> Vec<(TaskId, Timestamp)> {
        fx.iter()
            .filter_map(|e| {
                if let Effect::EntryClosed { task, at, .. } = e {
                    Some((*task, *at))
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn work_phase_opens_entries_for_every_active_task() {
        let (a, b) = (TaskId::new(), TaskId::new());
        let mut timer = Timer::default();
        timer.add_task(a, t(0));
        timer.add_task(b, t(0));
        let fx = timer.start(Mode::Pomodoro, t(0));
        assert_eq!(opened(&fx).len(), 2);
        assert_eq!(timer.remaining_ms(t(60)), Some(24 * 60 * 1000));
    }

    #[test]
    fn pause_banks_time_and_closes_entries() {
        let a = TaskId::new();
        let mut timer = Timer::default();
        timer.add_task(a, t(0));
        timer.start(Mode::Pomodoro, t(0));
        let fx = timer.pause(t(100));
        assert_eq!(closed(&fx), [(a, t(100))]);
        assert_eq!(timer.elapsed_ms(t(500)), 100_000);
        let fx = timer.resume(t(500));
        assert_eq!(opened(&fx), [a]);
        assert_eq!(timer.elapsed_ms(t(600)), 200_000);
        assert_eq!(timer.deadline(), Some(t(500 + 1500 - 100)));
    }

    #[test]
    fn tick_finishes_work_at_exact_deadline_and_starts_break() {
        let a = TaskId::new();
        let mut timer = Timer::default();
        timer.add_task(a, t(0));
        timer.start(Mode::Pomodoro, t(0));
        // App was closed and comes back 3 minutes after the deadline.
        let fx = timer.tick(t(1500 + 180));
        assert_eq!(closed(&fx), [(a, t(1500))]);
        assert!(fx.contains(&Effect::PhaseFinished {
            phase: Phase::Work,
            next: Phase::ShortBreak
        }));
        assert_eq!(timer.phase, Phase::ShortBreak);
        assert_eq!(timer.remaining_ms(t(1500 + 180)), Some(120_000));
        // The break does not write time to tasks.
        assert!(opened(&fx).is_empty());
    }

    #[test]
    fn work_waits_for_click_after_break_by_default() {
        let mut timer = Timer::default();
        timer.start(Mode::Pomodoro, t(0));
        timer.tick(t(1500 + 300));
        assert_eq!(timer.phase, Phase::Work);
        assert!(!timer.is_running());
        assert!(timer.session.is_none());
        let fx = timer.resume(t(2000));
        assert!(matches!(
            fx[0],
            Effect::SessionStarted {
                kind: SessionKind::Work,
                ..
            }
        ));
    }

    #[test]
    fn every_fourth_break_is_long() {
        let mut timer = Timer::new(TimerSettings {
            auto_start_work: true,
            ..Default::default()
        });
        timer.start(Mode::Pomodoro, t(0));
        let mut phases = Vec::new();
        let mut now = 0;
        for _ in 0..8 {
            now = timer
                .deadline()
                .map(|d| d.as_second() - 1_800_000_000)
                .unwrap();
            timer.tick(t(now));
            phases.push(timer.phase);
        }
        assert_eq!(
            phases,
            [
                Phase::ShortBreak,
                Phase::Work,
                Phase::ShortBreak,
                Phase::Work,
                Phase::ShortBreak,
                Phase::Work,
                Phase::LongBreak,
                Phase::Work
            ]
        );
        assert_eq!(now, 4 * 1500 + 3 * 300 + 900);
    }

    #[test]
    fn adding_and_removing_tasks_mid_phase() {
        let (a, b) = (TaskId::new(), TaskId::new());
        let mut timer = Timer::default();
        timer.add_task(a, t(0));
        timer.start(Mode::Pomodoro, t(0));
        assert_eq!(opened(&timer.add_task(b, t(60))), [b]);
        assert_eq!(closed(&timer.remove_task(a, t(120))), [(a, t(120))]);
        // Adding an already active task is a no-op.
        assert!(timer.add_task(b, t(130)).is_empty());
    }

    #[test]
    fn stopwatch_has_no_deadline_and_completes_on_stop() {
        let a = TaskId::new();
        let mut timer = Timer::default();
        timer.add_task(a, t(0));
        timer.start(Mode::Stopwatch, t(0));
        assert_eq!(timer.deadline(), None);
        assert!(timer.tick(t(100_000)).is_empty());
        let fx = timer.stop(t(7200));
        assert_eq!(closed(&fx), [(a, t(7200))]);
        assert!(fx.iter().any(|e| matches!(
            e,
            Effect::SessionEnded {
                outcome: Outcome::Completed,
                ..
            }
        )));
        assert_eq!(timer.phase, Phase::Idle);
    }

    #[test]
    fn skip_work_starts_break_without_counting_pomodoro() {
        let mut timer = Timer::default();
        timer.start(Mode::Pomodoro, t(0));
        let fx = timer.skip(t(60));
        assert!(fx.iter().any(|e| matches!(
            e,
            Effect::SessionEnded {
                outcome: Outcome::Skipped,
                ..
            }
        )));
        assert_eq!(timer.phase, Phase::ShortBreak);
        assert!(timer.is_running());
        assert_eq!(timer.completed_work, 0);
    }

    #[test]
    fn survives_serialization() {
        let mut timer = Timer::default();
        timer.add_task(TaskId::new(), t(0));
        timer.start(Mode::Pomodoro, t(0));
        let json = serde_json::to_string(&timer).unwrap();
        assert_eq!(serde_json::from_str::<Timer>(&json).unwrap(), timer);
    }
}
