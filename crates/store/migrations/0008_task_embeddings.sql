-- Rung 9c's semantic search (D-062): one vector per live task, from a local
-- embedding model, over the same text the FTS index holds (the title and
-- body_text). The daemon fills it in the background after a sync; a search
-- ranks the vectors by cosine similarity.
--
-- text_hash is the SHA-256 of the text the vector was made from, and model
-- the model that made it, so only a task whose text (or the model) changed
-- is embedded again. vector is little-endian f32s, unit length.
--
-- Rows follow the task: a hard delete removes them through the foreign key,
-- and the daemon prunes those of tombstoned tasks.

CREATE TABLE task_embeddings (
    task_local_id TEXT PRIMARY KEY NOT NULL REFERENCES tasks(local_id) ON DELETE CASCADE,
    model         TEXT NOT NULL,
    text_hash     BLOB NOT NULL,
    vector        BLOB NOT NULL
);
