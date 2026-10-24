# yatt

yet another task tracker: a native macOS task manager in Rust + [GPUI](https://www.gpui.rs).

- Kanban board (backlog, in progress, done) with drag and drop and undo.
- Pomodoro and stopwatch tied to tasks: every second of work is a time entry on each task in "In progress".
- Focus profiles: app and site blocking, macOS Focus via Shortcuts (in progress).

## Layout

| Crate | What |
| --- | --- |
| `yatt-core` | Domain model: tasks, board ordering, commands + undo, timer state machine. No IO. |
| `yatt-store` | SQLite persistence (WAL file in `~/Library/Application Support/yatt`). |
| `yatt-engine` | Live app state: board + timer + store, and the rules between them. No UI, testable anywhere. |
| `yatt-platform` | OS integration traits (app/site blocking, Focus). macOS and Windows implement them. |
| `yatt-macos` | Native AppKit window (objc2): renders `Snapshot`s, reports `UiEvent`s. Knows nothing about storage. |
| `yatt-app` | The app: GPUI holds the state (`Workspace` entity over the engine) and menus; AppKit draws. |

## Run

```sh
cargo run -p yatt-app            # macOS 14+
YATT_DB=/tmp/yatt.sqlite cargo run -p yatt-app   # throwaway database
cargo test --workspace
YATT_DEMO=/tmp/yatt-demo cargo run -p yatt-app   # scripted session, saves window PNGs there
```

Shortcuts: `space` start/pause (outside text fields), `cmd-.` stop, `cmd-]` skip phase, `cmd-n` new task, `cmd-z` / `cmd-shift-z` undo/redo, `esc` collapses the open card. Click a card to open it (title, notes, time entries); drag cards between columns. Time goes to every task in "In progress" while a work phase runs (two tasks there get the full time each). Drag a card out of the column, or delete it, and its time stops.
