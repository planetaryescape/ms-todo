-- Rung 3b's delta sync (docs/blueprint/04-sync-cache.md#delta-sync). Each
-- scope keeps the @odata.deltaLink its last checkpointed pass ended with. A
-- scope without one is in enumeration mode: its next pass pages a fresh
-- delta to the end and reconciles, the reset path. last_delta_at is when a
-- pass last checkpointed by replaying a saved link, for `doctor`.

ALTER TABLE sync_state ADD COLUMN delta_link TEXT;
ALTER TABLE sync_state ADD COLUMN last_delta_at INTEGER;
