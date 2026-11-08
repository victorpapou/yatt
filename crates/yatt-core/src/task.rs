use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::ids::{ProjectId, TaskId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Status {
    Backlog,
    InProgress,
    /// Handed on for review: no time is written here.
    Review,
    Done,
}

impl Status {
    pub const ALL: [Status; 4] = [
        Status::Backlog,
        Status::InProgress,
        Status::Review,
        Status::Done,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Backlog => "backlog",
            Status::InProgress => "in_progress",
            Status::Review => "review",
            Status::Done => "done",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|st| st.as_str() == s)
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub enum Priority {
    #[default]
    None,
    Low,
    Medium,
    High,
}

impl Priority {
    pub fn as_i64(self) -> i64 {
        self as i64
    }

    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => Priority::Low,
            2 => Priority::Medium,
            3 => Priority::High,
            _ => Priority::None,
        }
    }
}

/// A step inside a task: a checkbox with a title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subtask {
    pub title: String,
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    pub notes: String,
    pub status: Status,
    /// Fractional index within the status column, see [`crate::position`].
    pub position: String,
    pub project_id: Option<ProjectId>,
    pub priority: Priority,
    pub due_at: Option<Timestamp>,
    pub estimate_pomodoros: Option<u32>,
    /// The card's colour (an index into the app's palette) and icon (an SF
    /// Symbol name); `None`: picked from the id.
    pub color: Option<u8>,
    pub icon: Option<String>,
    /// Tag names, in the order given: the first one can colour the card.
    pub tags: Vec<String>,
    /// The task's steps, in order.
    pub subtasks: Vec<Subtask>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub completed_at: Option<Timestamp>,
    /// Done tasks tucked away from the board; they keep their column.
    pub archived_at: Option<Timestamp>,
    /// In the trash since; emptied for good after [`TRASH_DAYS`].
    pub deleted_at: Option<Timestamp>,
}

/// How long a deleted task stays in the trash.
pub const TRASH_DAYS: i64 = 30;

impl Task {
    pub fn new(
        id: TaskId,
        title: impl Into<String>,
        status: Status,
        position: String,
        now: Timestamp,
    ) -> Self {
        Self {
            id,
            title: title.into(),
            notes: String::new(),
            status,
            position,
            project_id: None,
            priority: Priority::None,
            due_at: None,
            estimate_pomodoros: None,
            color: None,
            icon: None,
            tags: Vec::new(),
            subtasks: Vec::new(),
            created_at: now,
            updated_at: now,
            completed_at: (status == Status::Done).then_some(now),
            archived_at: None,
            deleted_at: None,
        }
    }

    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }

    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }

    /// On the board: neither in the trash nor archived.
    pub fn is_live(&self) -> bool {
        !self.is_deleted() && !self.is_archived()
    }
}
