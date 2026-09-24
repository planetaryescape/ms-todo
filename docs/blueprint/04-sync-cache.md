# 04: Sync and cache

Goal: the TUI and the CLI read from SQLite and are never more than a few seconds out of date while the daemon is running.

## Delta sync

- **Lists:** `GET /me/todo/lists/delta`, keeping one `deltaLink` in `sync_state('lists')`.
- **Tasks:** `GET /me/todo/lists/{id}/tasks/delta`, one `deltaLink` per list (`sync_state('tasks:<id>')`).
- The first sync pages through everything. After that, the saved `@odata.deltaLink` returns only changes. Delta encodes any query options you give it (`$select`, `$top`, `$expand`) into the link, so choose them on the first request.
- Deleted items come back with `@removed`. Mark them with a tombstone (`deleted_at`) instead of hard-deleting, so an outbox operation still in flight can see the conflict.

**Children of a task.** Checklist items, linked resources, attachments and extensions: whether task delta includes them, or reports changes to them, isn't documented. **Spike S1 and S2 decide this.** Plan for either answer:

- **(a)** Delta supports `$expand=checklistItems,extensions` and a change to a child bumps the parent task. Then use `$expand` on the first delta request and there's nothing more to do.
- **(b)** It doesn't. Then when delta reports a task changed, re-fetch its children with `$batch`. For changes to children that don't bump the task's `lastModifiedDateTime`, run a slow background sweep that re-fetches children for recently viewed and recently changed tasks, and re-fetch when the TUI opens a task's detail view.

**How often:**

- While a client is connected, run a delta pass every 15–30 seconds (setting `sync.interval_secs`).
- With no clients, every 5 minutes.
- Immediately after the outbox sends something, and on `ms-todo sync`.
- On app focus, if the TUI tells the daemon it has focus.

A delta pass on a list with no changes costs one request per list. With about 20 lists, that's well inside Graph's limits.

**No webhooks.** Subscriptions for `todoTask` exist in v1.0 and last up to about 3 days, but they need a public HTTPS endpoint. Delta polling gives the same freshness for a local tool (D-007).

## Reconciliation after a lost delta token

A `deltaLink` can expire or be rejected: HTTP 410 Gone, or `syncStateNotFound` / `resyncRequired`-style errors. How long they last for To Do is spike S4. When that happens:

1. Keep the cached data.
2. Do a full enumeration of that scope, recording every ID seen.
3. Upsert everything that came back.
4. **Explicitly tombstone every cached row in that scope that wasn't seen**, unless an outbox operation still in flight owns it.
5. Save the new `deltaLink`.

This is mxr's `plans/015-authoritative-gmail-resync.md` lesson: when a sync cursor is invalidated, deletions have to be reconciled explicitly. Restarting delta alone misses anything deleted while the token was dead.

## Instant local writes

A client sends a mutation to the daemon, and the daemon does three things:

1. In one transaction: apply it to SQLite, save the previous state (`rollback_json`), and add the outbox row.
2. Send an `EntityChanged` event right away, so every client re-renders.
3. Wake the outbox worker.

The worker sends the operation to Graph:

- **Success:** save the returned `graph_id`, `etag` and `raw_json`, and mark the operation `done`.
- **Temporary failure:** back off and retry. Nothing changes in the UI apart from a small "pending sync" marker.
- **Permanent rejection:** roll back, send `WriteRejected`, and keep the operation as `failed` in `ms-todo outbox list`.

## Conflicts

Policy: **last write wins at the field level, with a warning when an edit is overwritten.**

- Where PATCH is concerned, send only the fields that changed. That way a phone edit to `title` and a CLI edit to `dueDateTime` both stick.
- If `If-Match` gets a 412, meaning the server copy changed since we last saw it, re-fetch the entity. If the field we're changing is untouched on the server, re-send. If the server changed the same field, apply ours (last write wins) and send a `ConflictOverwritten` event that the TUI shows briefly and `ms-todo outbox list` records.
- When delta brings in a server change to an entity that has pending local operations, keep the local values for the fields those operations touch. Apply the server values to everything else.

## Freshness guarantee for the CLI

A read through the daemon returns the cache. `--fresh` forces a delta pass for the relevant lists before answering, and `ms-todo sync --wait` blocks until a full pass finishes. Agents should normally just read. The daemon keeps the cache current.
