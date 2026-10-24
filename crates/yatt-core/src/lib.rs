//! Domain model of yatt. Pure Rust, no IO and no UI, so it builds and tests on any OS.

pub mod board;
pub mod ids;
pub mod position;
pub mod task;
pub mod timer;

pub use board::{Applied, Board, BoardError, Command, Filter, History, Placement};
pub use ids::{ProjectId, SessionId, TaskId};
pub use task::{Priority, Status, Task};
pub use timer::{Effect, Mode, Outcome, Phase, SessionKind, Timer, TimerSettings};
