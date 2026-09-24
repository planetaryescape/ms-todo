-- Rung 4b's search (docs/blueprint/02-data-model.md#indexes, D-041): an FTS5
-- index over each live task's title and its body as plain text.
--
-- body_text is the body without markup: the content itself for a text
-- body, and HTML rendered to text by the store for an html one. SQL can't
-- render HTML, so this migration fills only text bodies; the store fills
-- html ones when it opens the database (Store::open), and the update
-- trigger below indexes them then.
--
-- The index keeps its own copy of the text, keyed by tasks.rowid, so
-- snippet() works and a tombstoned row simply isn't in it. tasks has no
-- INTEGER PRIMARY KEY, so a VACUUM could renumber its rowids: rebuild
-- tasks_fts after one.

ALTER TABLE tasks ADD COLUMN body_text TEXT;

UPDATE tasks SET body_text = body_content
WHERE body_content IS NOT NULL AND lower(coalesce(body_content_type, 'text')) <> 'html';

-- What the store still has to render: empty once it has, so checking at
-- every open costs nothing.
CREATE INDEX tasks_body_text_missing ON tasks(local_id)
WHERE body_text IS NULL AND body_content IS NOT NULL;

-- remove_diacritics 2: "cafe" finds "café". prefix: `ab*` and `abc*` read a
-- ready-made index instead of scanning every term.
CREATE VIRTUAL TABLE tasks_fts USING fts5(
    title,
    body,
    tokenize = 'unicode61 remove_diacritics 2',
    prefix = '2 3'
);

INSERT INTO tasks_fts (rowid, title, body)
SELECT rowid, title, body_text FROM tasks WHERE deleted_at IS NULL;

CREATE TRIGGER tasks_fts_insert AFTER INSERT ON tasks
WHEN new.deleted_at IS NULL
BEGIN
    INSERT INTO tasks_fts (rowid, title, body) VALUES (new.rowid, new.title, new.body_text);
END;

-- Every write sets every column, so the WHEN keeps a sync that changed only
-- a task's status or etag from rewriting its index entry. A tombstone
-- removes the entry; clearing one puts it back.
CREATE TRIGGER tasks_fts_update AFTER UPDATE OF title, body_text, deleted_at ON tasks
WHEN old.title IS NOT new.title
    OR old.body_text IS NOT new.body_text
    OR (old.deleted_at IS NULL) <> (new.deleted_at IS NULL)
BEGIN
    DELETE FROM tasks_fts WHERE rowid = old.rowid;
    INSERT INTO tasks_fts (rowid, title, body)
    SELECT new.rowid, new.title, new.body_text WHERE new.deleted_at IS NULL;
END;

CREATE TRIGGER tasks_fts_delete AFTER DELETE ON tasks
BEGIN
    DELETE FROM tasks_fts WHERE rowid = old.rowid;
END;
