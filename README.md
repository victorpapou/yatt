# yatt

yet another task tracker: a native macOS task manager in Rust + [GPUI](https://www.gpui.rs).

- Kanban board (backlog, in progress, done) with drag and drop and undo.
- Pomodoro and stopwatch tied to tasks: every second of work is a time entry on each active task.
- Focus profiles: app and site blocking, macOS Focus via Shortcuts (in progress).

## Layout

| Crate | What |
| --- | --- |
| `yatt-core` | Domain model: tasks, board ordering, commands + undo, timer state machine. No IO. |
| `yatt-store` | SQLite persistence (WAL file in `~/Library/Application Support/yatt`). |
| `yatt-platform` | OS integration traits (app/site blocking, Focus). macOS and Windows implement them. |
| `yatt-app` | The GPUI app. The current UI is a placeholder until the real design lands. |

## Run

```sh
cargo run -p yatt-app            # macOS 14+
YATT_DB=/tmp/yatt.sqlite cargo run -p yatt-app   # throwaway database
cargo test --workspace
```

Shortcuts: `space` start/pause, `cmd-.` stop, `cmd-]` skip phase, `cmd-n` new task, `cmd-z` / `cmd-shift-z` undo/redo. Click a card to add it to (or remove it from) the timer; several cards can be active at once.
