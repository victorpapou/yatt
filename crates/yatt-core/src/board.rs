//! The task board: three status columns, commands that change it, and undo/redo.
//!
//! Every change goes through [`Command`]. Applying one updates the in-memory board
//! immediately (the UI animates from it) and returns the ids to persist plus the
//! inverse command, which is what undo replays.

use std::collections::HashMap;

use jiff::Timestamp;

use crate::ids::{ProjectId, TaskId};
use crate::position::{self, PositionError};
use crate::task::{Priority, Status, Task};

#[derive(Debug, Clone, PartialEq)]
pub enum Placement {
    Top,
    Bottom,
    /// Index in the target column as the user sees it, with the moved card removed.
    Index(usize),
    /// An exact position key. Used by undo to put a card back where it was.
    Key(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Create {
        id: TaskId,
        title: String,
        status: Status,
        placement: Placement,
    },
    Rename {
        id: TaskId,
        title: String,
    },
    SetNotes {
        id: TaskId,
        notes: String,
    },
    SetPriority {
        id: TaskId,
        priority: Priority,
    },
    SetProject {
        id: TaskId,
        project_id: Option<ProjectId>,
    },
    SetEstimate {
        id: TaskId,
        pomodoros: Option<u32>,
    },
    Move {
        id: TaskId,
        status: Status,
        placement: Placement,
    },
    Delete {
        id: TaskId,
    },
    Restore {
        id: TaskId,
    },
    /// Several commands as one: applied in order, undone as one step. If one
    /// fails, the ones before it are rolled back and nothing changes.
    Batch(Vec<Command>),
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BoardError {
    #[error("task {0} not found")]
    NotFound(TaskId),
    #[error("task {0} already exists")]
    AlreadyExists(TaskId),
    #[error("title must not be empty")]
    EmptyTitle,
    #[error(transparent)]
    Position(#[from] PositionError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    /// Tasks whose stored row must be written.
    pub changed: Vec<TaskId>,
    pub inverse: Command,
}

#[derive(Debug, Clone, Default)]
pub struct Board {
    tasks: HashMap<TaskId, Task>,
}

/// Which slice of the board to show. One board, narrowed by filters.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter {
    pub project: Option<ProjectId>,
    pub text: Option<String>,
}

impl Filter {
    pub fn matches(&self, task: &Task) -> bool {
        if self.project.is_some() && task.project_id != self.project {
            return false;
        }
        match &self.text {
            Some(q) if !q.is_empty() => {
                let q = q.to_lowercase();
                task.title.to_lowercase().contains(&q) || task.notes.to_lowercase().contains(&q)
            }
            _ => true,
        }
    }
}

impl Board {
    pub fn from_tasks(tasks: impl IntoIterator<Item = Task>) -> Self {
        Self {
            tasks: tasks.into_iter().map(|t| (t.id, t)).collect(),
        }
    }

    pub fn get(&self, id: TaskId) -> Option<&Task> {
        self.tasks.get(&id)
    }

    pub fn tasks(&self) -> impl Iterator<Item = &Task> {
        self.tasks.values()
    }

    /// Live tasks of one column in display order.
    pub fn column(&self, status: Status, filter: &Filter) -> Vec<&Task> {
        let mut col: Vec<&Task> = self
            .tasks
            .values()
            .filter(|t| t.status == status && !t.is_deleted() && filter.matches(t))
            .collect();
        col.sort_by(|a, b| a.position.cmp(&b.position).then(a.id.cmp(&b.id)));
        col
    }

    pub fn apply(&mut self, cmd: Command, now: Timestamp) -> Result<Applied, BoardError> {
        match cmd {
            Command::Create {
                id,
                title,
                status,
                placement,
            } => {
                if self.tasks.contains_key(&id) {
                    return Err(BoardError::AlreadyExists(id));
                }
                let title = clean_title(&title)?;
                let position = self.position_for(status, None, &placement)?;
                self.tasks
                    .insert(id, Task::new(id, title, status, position, now));
                Ok(Applied {
                    changed: vec![id],
                    inverse: Command::Delete { id },
                })
            }
            Command::Rename { id, title } => {
                let title = clean_title(&title)?;
                let task = self.task_mut(id)?;
                let old = std::mem::replace(&mut task.title, title);
                task.updated_at = now;
                Ok(Applied {
                    changed: vec![id],
                    inverse: Command::Rename { id, title: old },
                })
            }
            Command::SetNotes { id, notes } => {
                let task = self.task_mut(id)?;
                let old = std::mem::replace(&mut task.notes, notes);
                task.updated_at = now;
                Ok(Applied {
                    changed: vec![id],
                    inverse: Command::SetNotes { id, notes: old },
                })
            }
            Command::SetPriority { id, priority } => {
                let task = self.task_mut(id)?;
                let old = std::mem::replace(&mut task.priority, priority);
                task.updated_at = now;
                Ok(Applied {
                    changed: vec![id],
                    inverse: Command::SetPriority { id, priority: old },
                })
            }
            Command::SetProject { id, project_id } => {
                let task = self.task_mut(id)?;
                let old = std::mem::replace(&mut task.project_id, project_id);
                task.updated_at = now;
                Ok(Applied {
                    changed: vec![id],
                    inverse: Command::SetProject {
                        id,
                        project_id: old,
                    },
                })
            }
            Command::SetEstimate { id, pomodoros } => {
                let task = self.task_mut(id)?;
                let old = std::mem::replace(&mut task.estimate_pomodoros, pomodoros);
                task.updated_at = now;
                Ok(Applied {
                    changed: vec![id],
                    inverse: Command::SetEstimate { id, pomodoros: old },
                })
            }
            Command::Move {
                id,
                status,
                placement,
            } => {
                let (old_status, old_position) = {
                    let t = self.task_mut(id)?;
                    (t.status, t.position.clone())
                };
                let position = self.position_for(status, Some(id), &placement)?;
                let task = self.task_mut(id)?;
                task.status = status;
                task.position = position;
                task.updated_at = now;
                if status != old_status {
                    task.completed_at = (status == Status::Done).then_some(now);
                }
                Ok(Applied {
                    changed: vec![id],
                    inverse: Command::Move {
                        id,
                        status: old_status,
                        placement: Placement::Key(old_position),
                    },
                })
            }
            Command::Delete { id } => {
                let task = self.task_mut(id)?;
                task.deleted_at = Some(now);
                task.updated_at = now;
                Ok(Applied {
                    changed: vec![id],
                    inverse: Command::Restore { id },
                })
            }
            Command::Restore { id } => {
                let task = self.task_mut(id)?;
                task.deleted_at = None;
                task.updated_at = now;
                Ok(Applied {
                    changed: vec![id],
                    inverse: Command::Delete { id },
                })
            }
            Command::Batch(cmds) => {
                let mut changed = Vec::new();
                let mut inverses = Vec::new();
                for cmd in cmds {
                    match self.apply(cmd, now) {
                        Ok(applied) => {
                            for id in applied.changed {
                                if !changed.contains(&id) {
                                    changed.push(id);
                                }
                            }
                            inverses.push(applied.inverse);
                        }
                        Err(e) => {
                            // Back to where the batch started.
                            for inverse in inverses.into_iter().rev() {
                                let _ = self.apply(inverse, now);
                            }
                            return Err(e);
                        }
                    }
                }
                inverses.reverse();
                Ok(Applied {
                    changed,
                    inverse: Command::Batch(inverses),
                })
            }
        }
    }

    fn task_mut(&mut self, id: TaskId) -> Result<&mut Task, BoardError> {
        self.tasks.get_mut(&id).ok_or(BoardError::NotFound(id))
    }

    fn position_for(
        &self,
        status: Status,
        moving: Option<TaskId>,
        placement: &Placement,
    ) -> Result<String, BoardError> {
        if let Placement::Key(k) = placement {
            return Ok(k.clone());
        }
        let col: Vec<&str> = self
            .column(status, &Filter::default())
            .into_iter()
            .filter(|t| Some(t.id) != moving)
            .map(|t| t.position.as_str())
            .collect();
        let i = match placement {
            Placement::Top => 0,
            Placement::Bottom => col.len(),
            Placement::Index(i) => (*i).min(col.len()),
            Placement::Key(_) => unreachable!(),
        };
        let before = i.checked_sub(1).map(|j| col[j]);
        let after = col.get(i).copied();
        Ok(position::key_between(before, after)?)
    }
}

fn clean_title(title: &str) -> Result<String, BoardError> {
    let t = title.trim();
    if t.is_empty() {
        Err(BoardError::EmptyTitle)
    } else {
        Ok(t.to_string())
    }
}

/// Undo/redo stacks of inverse commands.
#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Command>,
    redo: Vec<Command>,
}

impl History {
    const LIMIT: usize = 200;

    /// Applies a user command and records it for undo.
    pub fn execute(
        &mut self,
        board: &mut Board,
        cmd: Command,
        now: Timestamp,
    ) -> Result<Applied, BoardError> {
        let applied = board.apply(cmd, now)?;
        self.undo.push(applied.inverse.clone());
        if self.undo.len() > Self::LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        Ok(applied)
    }

    pub fn undo(
        &mut self,
        board: &mut Board,
        now: Timestamp,
    ) -> Result<Option<Applied>, BoardError> {
        let Some(cmd) = self.undo.pop() else {
            return Ok(None);
        };
        let applied = board.apply(cmd, now)?;
        self.redo.push(applied.inverse.clone());
        Ok(Some(applied))
    }

    pub fn redo(
        &mut self,
        board: &mut Board,
        now: Timestamp,
    ) -> Result<Option<Applied>, BoardError> {
        let Some(cmd) = self.redo.pop() else {
            return Ok(None);
        };
        let applied = board.apply(cmd, now)?;
        self.undo.push(applied.inverse.clone());
        Ok(Some(applied))
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Timestamp {
        Timestamp::from_second(1_800_000_000).unwrap()
    }

    fn titles(board: &Board, status: Status) -> Vec<String> {
        board
            .column(status, &Filter::default())
            .iter()
            .map(|t| t.title.clone())
            .collect()
    }

    fn create(board: &mut Board, h: &mut History, title: &str) -> TaskId {
        let id = TaskId::new();
        h.execute(
            board,
            Command::Create {
                id,
                title: title.into(),
                status: Status::Backlog,
                placement: Placement::Bottom,
            },
            now(),
        )
        .unwrap();
        id
    }

    #[test]
    fn create_and_reorder() {
        let (mut b, mut h) = (Board::default(), History::default());
        let a = create(&mut b, &mut h, "a");
        create(&mut b, &mut h, "b");
        create(&mut b, &mut h, "c");
        assert_eq!(titles(&b, Status::Backlog), ["a", "b", "c"]);

        h.execute(
            &mut b,
            Command::Move {
                id: a,
                status: Status::Backlog,
                placement: Placement::Index(2),
            },
            now(),
        )
        .unwrap();
        assert_eq!(titles(&b, Status::Backlog), ["b", "c", "a"]);

        h.execute(
            &mut b,
            Command::Move {
                id: a,
                status: Status::Backlog,
                placement: Placement::Index(1),
            },
            now(),
        )
        .unwrap();
        assert_eq!(titles(&b, Status::Backlog), ["b", "a", "c"]);
    }

    #[test]
    fn move_between_columns_tracks_completion_and_undoes() {
        let (mut b, mut h) = (Board::default(), History::default());
        let a = create(&mut b, &mut h, "a");
        create(&mut b, &mut h, "b");

        h.execute(
            &mut b,
            Command::Move {
                id: a,
                status: Status::Done,
                placement: Placement::Top,
            },
            now(),
        )
        .unwrap();
        assert_eq!(titles(&b, Status::Backlog), ["b"]);
        assert_eq!(titles(&b, Status::Done), ["a"]);
        assert!(b.get(a).unwrap().completed_at.is_some());

        h.undo(&mut b, now()).unwrap();
        assert_eq!(titles(&b, Status::Backlog), ["a", "b"]);
        assert!(b.get(a).unwrap().completed_at.is_none());

        h.redo(&mut b, now()).unwrap();
        assert_eq!(titles(&b, Status::Done), ["a"]);
    }

    #[test]
    fn delete_is_soft_and_undoable() {
        let (mut b, mut h) = (Board::default(), History::default());
        let a = create(&mut b, &mut h, "a");
        h.execute(&mut b, Command::Delete { id: a }, now()).unwrap();
        assert!(titles(&b, Status::Backlog).is_empty());
        h.undo(&mut b, now()).unwrap();
        assert_eq!(titles(&b, Status::Backlog), ["a"]);
    }

    #[test]
    fn rejects_empty_title() {
        let mut b = Board::default();
        let err = b
            .apply(
                Command::Create {
                    id: TaskId::new(),
                    title: "  ".into(),
                    status: Status::Backlog,
                    placement: Placement::Top,
                },
                now(),
            )
            .unwrap_err();
        assert_eq!(err, BoardError::EmptyTitle);
    }

    #[test]
    fn filter_by_project_and_text() {
        let (mut b, mut h) = (Board::default(), History::default());
        let p = ProjectId::new();
        let a = create(&mut b, &mut h, "Write plan");
        create(&mut b, &mut h, "Buy milk");
        h.execute(
            &mut b,
            Command::SetProject {
                id: a,
                project_id: Some(p),
            },
            now(),
        )
        .unwrap();

        let by_project = Filter {
            project: Some(p),
            text: None,
        };
        assert_eq!(b.column(Status::Backlog, &by_project).len(), 1);
        let by_text = Filter {
            project: None,
            text: Some("milk".into()),
        };
        assert_eq!(b.column(Status::Backlog, &by_text)[0].title, "Buy milk");
    }

    #[test]
    fn a_batch_is_one_undo_step_and_rolls_back_on_failure() {
        let mut board = Board::default();
        let mut history = History::default();
        let ids: Vec<TaskId> = (0..20).map(|_| TaskId::new()).collect();
        for (i, id) in ids.iter().enumerate() {
            history
                .execute(
                    &mut board,
                    Command::Create {
                        id: *id,
                        title: format!("t{i}"),
                        status: Status::Backlog,
                        placement: Placement::Bottom,
                    },
                    now(),
                )
                .unwrap();
        }
        let before: Vec<TaskId> = board
            .column(Status::Backlog, &Filter::default())
            .iter()
            .map(|t| t.id)
            .collect();
        // Twenty cards into "In progress" as one move.
        let moves = ids
            .iter()
            .map(|id| Command::Move {
                id: *id,
                status: Status::InProgress,
                placement: Placement::Bottom,
            })
            .collect();
        history
            .execute(&mut board, Command::Batch(moves), now())
            .unwrap();
        assert_eq!(
            board.column(Status::InProgress, &Filter::default()).len(),
            20
        );
        // One undo puts all twenty back, in their order.
        history.undo(&mut board, now()).unwrap();
        let after: Vec<TaskId> = board
            .column(Status::Backlog, &Filter::default())
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(after, before);
        assert!(
            board
                .column(Status::InProgress, &Filter::default())
                .is_empty()
        );
        // One redo moves them all again.
        history.redo(&mut board, now()).unwrap();
        assert_eq!(
            board.column(Status::InProgress, &Filter::default()).len(),
            20
        );
        // A batch that fails half way changes nothing.
        let bad = Command::Batch(vec![
            Command::Delete { id: ids[0] },
            Command::Delete { id: TaskId::new() },
        ]);
        assert!(history.execute(&mut board, bad, now()).is_err());
        assert!(board.get(ids[0]).unwrap().deleted_at.is_none());
    }
}
