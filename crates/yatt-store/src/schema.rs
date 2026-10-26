use rusqlite_migration::{M, Migrations};

/// Timestamps are UTC milliseconds (INTEGER), ids are hyphenated UUIDv7 (TEXT).
pub fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(
            r#"
CREATE TABLE projects (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    color       TEXT,
    position    TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    deleted_at  INTEGER
);

CREATE TABLE tasks (
    id                  TEXT PRIMARY KEY,
    title               TEXT NOT NULL,
    notes               TEXT NOT NULL DEFAULT '',
    status              TEXT NOT NULL CHECK (status IN ('backlog', 'in_progress', 'done')),
    position            TEXT NOT NULL,
    project_id          TEXT REFERENCES projects(id),
    priority            INTEGER NOT NULL DEFAULT 0,
    due_at              INTEGER,
    estimate_pomodoros  INTEGER,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    completed_at        INTEGER,
    deleted_at          INTEGER
);
CREATE INDEX tasks_board ON tasks(status, position) WHERE deleted_at IS NULL;

CREATE TABLE subtasks (
    id          TEXT PRIMARY KEY,
    task_id     TEXT NOT NULL REFERENCES tasks(id),
    title       TEXT NOT NULL,
    done        INTEGER NOT NULL DEFAULT 0,
    position    TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    deleted_at  INTEGER
);
CREATE INDEX subtasks_task ON subtasks(task_id, position);

CREATE TABLE tags (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    color       TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    deleted_at  INTEGER
);

CREATE TABLE task_tags (
    task_id  TEXT NOT NULL REFERENCES tasks(id),
    tag_id   TEXT NOT NULL REFERENCES tags(id),
    PRIMARY KEY (task_id, tag_id)
);

CREATE TABLE focus_profiles (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    config      TEXT NOT NULL,  -- JSON: blocked apps/domains, focus shortcuts, strict, ...
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    deleted_at  INTEGER
);

CREATE TABLE focus_sessions (
    id            TEXT PRIMARY KEY,
    kind          TEXT NOT NULL CHECK (kind IN ('work', 'short_break', 'long_break', 'stopwatch')),
    planned_secs  INTEGER,
    started_at    INTEGER NOT NULL,
    ended_at      INTEGER,
    outcome       TEXT CHECK (outcome IN ('completed', 'skipped', 'abandoned')),
    profile_id    TEXT REFERENCES focus_profiles(id)
);
CREATE INDEX focus_sessions_started ON focus_sessions(started_at);

CREATE TABLE time_entries (
    id          TEXT PRIMARY KEY,
    task_id     TEXT REFERENCES tasks(id),
    session_id  TEXT REFERENCES focus_sessions(id),
    started_at  INTEGER NOT NULL,
    ended_at    INTEGER,
    source      TEXT NOT NULL CHECK (source IN ('pomodoro', 'stopwatch', 'manual'))
);
CREATE INDEX time_entries_task ON time_entries(task_id, started_at);
CREATE INDEX time_entries_started ON time_entries(started_at);

-- Settings and the persisted timer, as JSON values.
CREATE TABLE kv (
    key    TEXT PRIMARY KEY,
    value  TEXT NOT NULL
);
"#,
        ),
        // The card's own colour and icon.
        M::up(
            "ALTER TABLE tasks ADD COLUMN color INTEGER;
         ALTER TABLE tasks ADD COLUMN icon TEXT;",
        ),
    ])
}
