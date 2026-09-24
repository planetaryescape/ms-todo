-- Rung 4's outbox (docs/blueprint/02-data-model.md#outbox-semantics,
-- docs/blueprint/04-sync-cache.md#instant-local-writes). Each task write is
-- applied to `tasks` and added here in one transaction; a worker sends the
-- rows to Graph in order.
--
-- One row changes one task. A command that changes several tasks shares
-- its op_id as command_id: its first row's op_id is the command's, the
-- others are `<command_id>.<n>`. A create's op_id is the `opId` in our
-- extension on the task Graph makes (S13), which attributes an `unknown`
-- create.
--
-- state: pending | inflight | unknown | failed | done. A discarded row is
-- deleted.

CREATE TABLE outbox (
    op_id             TEXT PRIMARY KEY NOT NULL,
    -- Queue order, which is also the order of each task's operations.
    seq               INTEGER NOT NULL UNIQUE,
    command_id        TEXT NOT NULL,
    created_at        INTEGER NOT NULL,
    entity_kind       TEXT NOT NULL DEFAULT 'task',
    entity_local_id   TEXT NOT NULL,
    list_local_id     TEXT NOT NULL,
    -- What is sent: create (POST), update (PATCH) or delete (DELETE).
    op                TEXT NOT NULL,
    -- What the user asked for: add, edit, complete, reopen or delete.
    action            TEXT NOT NULL,
    -- {"body": the Graph JSON sent, …}; an update of a recurring task's
    -- status also has "recurring" and "due_before".
    payload_json      TEXT NOT NULL,
    -- The operation on the same task queued before this one, unresolved
    -- when this was queued. This one waits until that one is resolved.
    depends_on_op_id  TEXT,
    -- For an undo: the command_id it undoes.
    undoes_command_id TEXT,
    attempts          INTEGER NOT NULL DEFAULT 0,
    next_attempt_at   INTEGER NOT NULL DEFAULT 0,
    state             TEXT NOT NULL,
    last_error_kind   TEXT,
    last_error        TEXT,
    -- The task's Graph JSON before this operation's local change, or null
    -- for a create: what a rejection restores and what undo inverts.
    rollback_json     TEXT,
    -- Unix seconds of the last send, and since when it has been unknown.
    sent_at           INTEGER,
    unknown_since     INTEGER,
    -- What was seen while unknown, for `outbox list`.
    note              TEXT,
    finished_at       INTEGER
);

CREATE INDEX outbox_by_entity ON outbox(entity_local_id, state);
CREATE INDEX outbox_by_state ON outbox(state, next_attempt_at);
CREATE INDEX outbox_by_command ON outbox(command_id);
-- Undo looks up what undid a command; the worker what waits for an operation.
CREATE INDEX outbox_by_undone ON outbox(undoes_command_id) WHERE undoes_command_id IS NOT NULL;
CREATE INDEX outbox_by_dependency ON outbox(depends_on_op_id) WHERE depends_on_op_id IS NOT NULL;
