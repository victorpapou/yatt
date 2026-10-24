use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::ids::{ProjectId, TaskId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Status {
    Backlog,
    InProgress,
    Done,
}

impl Status {
    pub const ALL: [Status; 3] = [Status::Backlog, Status::InProgress, Status::Done];

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Backlog => "backlog",
            Status::InProgress => "in_progress",
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
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub completed_at: Option<Timestamp>,
    pub deleted_at: Option<Timestamp>,
}

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
            created_at: now,
            updated_at: now,
            completed_at: (status == Status::Done).then_some(now),
            deleted_at: None,
        }
    }

    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }
}
