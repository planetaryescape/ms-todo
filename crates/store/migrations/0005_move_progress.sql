-- Rung 5e's move between lists (docs/blueprint/05-custom-features.md#move-between-lists,
-- docs/blueprint/04-sync-cache.md#instant-local-writes). A move is one
-- outbox operation (op 'move') that runs as a job of several steps:
-- create the copy, add its attachments, check it, delete the source. Each
-- step's outcome is saved here as it happens, so a daemon that stops
-- half-way resumes or rolls back the move honestly. Null for every other
-- operation.

ALTER TABLE outbox ADD COLUMN progress_json TEXT;
