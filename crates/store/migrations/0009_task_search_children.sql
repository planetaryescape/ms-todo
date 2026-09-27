-- Search reaches a task's steps, categories and attachment names (D-068),
-- beside 0004's title and plain-text body. FTS5 can't add a column to a
-- table, so tasks_fts is made again with three more, filled through the
-- view tasks_fts_source: steps are raw_json's checklistItems (their
-- displayName), attachments raw_json's attachments (their name, which the
-- cache keeps there, 0007), and categories categories_json.
--
-- Each column holds its items joined by '; ', so a snippet from it reads
-- as a list. Semantic search (0008) embeds tasks.title and body_text, not
-- this table, so it's unchanged.

DROP TRIGGER tasks_fts_insert;
DROP TRIGGER tasks_fts_update;
DROP TRIGGER tasks_fts_delete;
DROP TABLE tasks_fts;

CREATE VIRTUAL TABLE tasks_fts USING fts5(
    title,
    body,
    steps,
    categories,
    attachments,
    tokenize = 'unicode61 remove_diacritics 2',
    prefix = '2 3'
);

-- What the index holds for each live task, in its columns' order: one
-- definition for the backfill and both triggers.
CREATE VIEW tasks_fts_source AS
SELECT rowid, title, body_text AS body,
    (SELECT group_concat(json_extract(value, '$.displayName'), '; ')
     FROM json_each(raw_json, '$.checklistItems')) AS steps,
    (SELECT group_concat(value, '; ') FROM json_each(categories_json)) AS categories,
    (SELECT group_concat(json_extract(value, '$.name'), '; ')
     FROM json_each(raw_json, '$.attachments')) AS attachments
FROM tasks WHERE deleted_at IS NULL;

INSERT INTO tasks_fts (rowid, title, body, steps, categories, attachments)
SELECT * FROM tasks_fts_source;

CREATE TRIGGER tasks_fts_insert AFTER INSERT ON tasks
WHEN new.deleted_at IS NULL
BEGIN
    INSERT INTO tasks_fts (rowid, title, body, steps, categories, attachments)
    SELECT * FROM tasks_fts_source WHERE rowid = new.rowid;
END;

-- Every write sets every column, and raw_json changes with any field, so
-- the WHEN compares only what the index holds: a sync that changed a
-- task's status or etag doesn't rewrite its entry. A tombstoned row isn't
-- in the view, so it leaves the index.
CREATE TRIGGER tasks_fts_update AFTER UPDATE OF title, body_text, deleted_at, raw_json,
    categories_json ON tasks
WHEN old.title IS NOT new.title
    OR old.body_text IS NOT new.body_text
    OR (old.deleted_at IS NULL) <> (new.deleted_at IS NULL)
    OR old.categories_json IS NOT new.categories_json
    OR json_extract(old.raw_json, '$.checklistItems')
        IS NOT json_extract(new.raw_json, '$.checklistItems')
    OR json_extract(old.raw_json, '$.attachments')
        IS NOT json_extract(new.raw_json, '$.attachments')
BEGIN
    DELETE FROM tasks_fts WHERE rowid = old.rowid;
    INSERT INTO tasks_fts (rowid, title, body, steps, categories, attachments)
    SELECT * FROM tasks_fts_source WHERE rowid = new.rowid;
END;

CREATE TRIGGER tasks_fts_delete AFTER DELETE ON tasks
BEGIN
    DELETE FROM tasks_fts WHERE rowid = old.rowid;
END;
