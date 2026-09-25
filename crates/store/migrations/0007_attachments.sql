-- Rung 8b's attachments (docs/blueprint/04-sync-cache.md#children-of-a-task,
-- D-056). A task's attachment metadata lives in its raw_json under
-- `attachments`, which Graph never sends inline (S1): a sync fetches it
-- for each task whose etag moved, or that has attachments and no list yet.
-- A task already cached doesn't show up in a delta round until it changes,
-- so every list holding one with attachments is read whole on the next
-- pass, which fetches their lists.

UPDATE sync_state SET delta_link = NULL
WHERE scope IN (
    SELECT 'tasks:' || l.graph_id FROM lists l
    JOIN tasks t ON t.list_local_id = l.local_id
    WHERE t.has_attachments = 1 AND t.deleted_at IS NULL AND l.graph_id IS NOT NULL
);
