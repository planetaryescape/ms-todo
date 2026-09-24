-- Rung 3a's cache (docs/blueprint/02-data-model.md). Every row keeps Graph's
-- JSON in raw_json; the other columns are what reads filter and sort on.
-- Rows are never hard-deleted: deleted_at is a tombstone.
--
-- local_rev is the value of counters.local_rev when the daemon last wrote the
-- row itself (a create, edit or delete). A sync pass records the counter
-- before it fetches and leaves alone any row written after that, so a stale
-- page can't undo or tombstone a write made while it was in flight.

CREATE TABLE lists (
    local_id            TEXT PRIMARY KEY NOT NULL,
    graph_id            TEXT UNIQUE,
    display_name        TEXT NOT NULL,
    wellknown_list_name TEXT,
    is_owner            INTEGER NOT NULL DEFAULT 0,
    is_shared           INTEGER NOT NULL DEFAULT 0,
    -- Our open extension, com.planetaryescape.mstodo, as Graph returned it.
    extension_json      TEXT,
    raw_json            TEXT NOT NULL,
    etag                TEXT,
    local_rev           INTEGER NOT NULL DEFAULT 0,
    deleted_at          INTEGER
);

CREATE TABLE tasks (
    local_id          TEXT PRIMARY KEY NOT NULL,
    graph_id          TEXT UNIQUE,
    list_local_id     TEXT NOT NULL REFERENCES lists(local_id),
    title             TEXT NOT NULL,
    body_content      TEXT,
    body_content_type TEXT,
    status            TEXT NOT NULL,
    importance        TEXT NOT NULL,
    is_reminder_on    INTEGER NOT NULL DEFAULT 0,
    reminder_at_utc   TEXT,
    -- Local dates, YYYY-MM-DD (S11).
    due_date          TEXT,
    start_date        TEXT,
    completed_at_utc  TEXT,
    recurrence_json   TEXT,
    categories_json   TEXT NOT NULL DEFAULT '[]',
    has_attachments   INTEGER NOT NULL DEFAULT 0,
    created_at        TEXT,
    last_modified_at  TEXT,
    extension_json    TEXT,
    -- The etag the extension was last fetched at. A task whose etag has moved
    -- since needs its extension fetched again (04, children of a task).
    hydrated_etag     TEXT,
    raw_json          TEXT NOT NULL,
    etag              TEXT,
    local_rev         INTEGER NOT NULL DEFAULT 0,
    deleted_at        INTEGER
);

CREATE INDEX tasks_by_list ON tasks(list_local_id, status, deleted_at);
CREATE INDEX tasks_by_due ON tasks(due_date);
CREATE INDEX tasks_by_importance ON tasks(importance);

-- One row per scope: 'lists', or 'tasks:<list graph_id>'.
CREATE TABLE sync_state (
    scope              TEXT PRIMARY KEY NOT NULL,
    -- Only goes up, by one per finished pass, even one that changed nothing.
    generation         INTEGER NOT NULL DEFAULT 0,
    in_progress        INTEGER NOT NULL DEFAULT 0,
    last_success_at    INTEGER,
    last_changed_count INTEGER NOT NULL DEFAULT 0,
    last_error         TEXT,
    -- An ms_todo_core::ErrorKind string.
    last_error_kind    TEXT,
    last_error_at      INTEGER
);

-- `--idempotency-key` (04, instant local writes). result_json is null while
-- the operation runs; finished rows are kept for 24 hours.
CREATE TABLE idempotency_keys (
    key         TEXT PRIMARY KEY NOT NULL,
    fingerprint TEXT NOT NULL,
    op_id       TEXT NOT NULL,
    result_json TEXT,
    created_at  INTEGER NOT NULL,
    finished_at INTEGER
);

CREATE TABLE counters (
    name  TEXT PRIMARY KEY NOT NULL,
    value INTEGER NOT NULL
);

INSERT INTO counters (name, value) VALUES ('local_rev', 0);
