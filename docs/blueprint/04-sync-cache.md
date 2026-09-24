# 04: Sync and cache

Goal: the TUI and the CLI read the daemon's SQLite cache (over the protocol, D-031) and are never more than a few seconds out of date while the daemon is running.

## Delta sync

- **Lists:** `GET /me/todo/lists/delta`, keeping one `deltaLink` in `sync_state('lists')`.
- **Tasks:** `GET /me/todo/lists/{id}/tasks/delta`, one `deltaLink` per list (`sync_state('tasks:<id>')`).
- The first sync pages through everything. After that, the saved `@odata.deltaLink` returns only changes. Send the plain delta request with no query options, and `Prefer: odata.maxpagesize` on every page (see [03](03-graph-provider.md#query-options)).
- Delta returns the whole entity on every change, not a diff.
- Deleted items come back with `@removed`. Mark them with a tombstone (`deleted_at`) instead of hard-deleting, so an outbox operation still in flight can see the conflict.

**How often:**

- While a client is connected, or for 10 minutes after any client request, run a delta pass every 15–30 seconds (setting `sync.interval_secs`).
- Otherwise, every 5 minutes.
- Immediately after the outbox sends something, and on `ms-todo sync`.
- On app focus, if the TUI tells the daemon it has focus.

A delta pass on a list with no changes costs one request per list. With about 20 lists, that's well inside Graph's limits.

**No webhooks.** Subscriptions for `todoTask` exist in v1.0 and last up to about 3 days, but they need a public HTTPS endpoint. Delta polling gives the same freshness for a local tool (D-007).

### Children of a task

Settled by S1 and S2 (D-029). Every child change bumps the parent task's `lastModifiedDateTime` and `@odata.etag`, so the task appears in the next delta round. What delta carries differs by child:

| Child | In delta? | What to do |
|---|---|---|
| Checklist items | Inline in every task, no `$expand` needed | Nothing more. An absent `checklistItems` key means the task has none |
| Linked resources | Inline, same as checklist items | Nothing more |
| Attachments | Only `hasAttachments` flips | For a task delta reports with `hasAttachments` true, fetch `GET …/attachments` (metadata only) and make the cache match that set: add, update and remove. With `hasAttachments` false, remove the task's cached attachment metadata |
| Extensions | Change detected, content never returned | After the delta round, fetch the content with a filtered `$expand` (below) |

Fetching extension content after a delta round:

- **Lists:** if `lists/delta` reported any list, one `GET /me/todo/lists?$expand=extensions($filter=id eq 'com.planetaryescape.mstodo')` covers every list.
- **Tasks:** fetch exactly the task IDs delta reported, one GET per task with the same filtered `$expand`, grouped into sequential `$batch` calls of up to 20. When a fetched task has no extension, clear its cached extension fields (`my_day_date`, `assignee` and the rest).
- **Deleted entries:** `@removed` entries are tombstoned and never fetched. If an extension or attachment GET for a reported task returns 404, the task was deleted in between: tombstone it, and count that fetch as a success for the checkpoint. The next delta round confirms the removal.

No background sweep is needed.

**Checkpoint.** Save a scope's new `deltaLink` only after every fetch that round needs has succeeded: extension content and attachment metadata. If one fails, the next round replays the same delta, which is safe because applying delta is an upsert. This is simpler than keeping a table of task IDs still waiting to be filled in, so it's the one we use.

**Accepted limitation:** ms-todo's own extension writes read, merge and PATCH the whole document ([05](05-custom-features.md)), so another device writing the same extension between our GET and PATCH can lose its change. With one user this is rare, and it's documented rather than solved; whether If-Match helps is still open in [12](12-open-questions.md#s2-result-2026-09-24).

## Reconciliation after a lost delta token

A `deltaLink` can expire or be rejected. S4 observed exactly two ways, and both mean **reset this cursor**:

- **410** `SyncStateNotFound`, whatever the `innerError` code.
- **400** with the message "Badly formed token.".

**404 and 5xx on a delta request are not resets.** A valid deltaLink returned 404 once and 500 once, just after a mass delete, then 200 a minute later. Retry them with backoff and keep the cursor.

How long a deltaLink lives is still open (S4). One lists deltaLink died after about 6 minutes for no reason we could find, while others lasted 20 minutes or more. So expect resets in normal running, not only after days away. A lists reset costs one or two requests, and a tasks reset about one request per 50 tasks.

When a cursor resets:

1. Keep the cached data.
2. Do a full enumeration of that scope, recording every ID seen.
3. Upsert everything that came back, then fill in what enumeration doesn't carry: extension content and attachment metadata for every enumerated task that needs them, by the same rules as a delta round, including the 404 rule ([children of a task](#children-of-a-task)).
4. **Explicitly tombstone every cached row in that scope that wasn't seen**, except rows with no `graph_id` yet and rows with any outbox operation in `pending`, `inflight` or `unknown`. Those keep their optimistic fields until their operations resolve.
5. Save the new `deltaLink`, only once step 3's fetches have all succeeded (the same checkpoint rule as a delta round).

This is mxr's `plans/015-authoritative-gmail-resync.md` lesson: when a sync cursor is invalidated, deletions have to be reconciled explicitly. Restarting delta alone misses anything deleted while the token was dead.

**Deleted lists.** Only `lists/delta` (an `@removed` entry) or a 404 on `GET /me/todo/lists/{id}` shows that a list is gone. A deleted list's own tasks delta keeps returning 200 with nothing in it, so the per-list cursor never reports the deletion. Tombstone the list and its tasks, and drop its tasks cursor.

## Instant local writes

A client sends a mutation to the daemon, and the daemon does three things:

1. In one transaction: apply it to SQLite, save the previous state (`rollback_json`), and add the outbox row.
2. Send an `EntityChanged` event right away, so every client re-renders.
3. Wake the outbox worker.

**Every mutating request carries a client request ID** (vault: `Agent-Native Interfaces`). The CLI generates one, and `--idempotency-key K` lets an agent supply its own. The daemon keeps each ID with its outbox operation and a request fingerprint (the operation plus a hash of its payload) while the operation is unresolved (`pending`, `inflight` or `unknown`), and for 24 hours after it's `done`, `failed` or discarded. A repeat with the same fingerprint gets the original result; a repeat with a different fingerprint is rejected as invalid input (exit code 2). The IPC client never re-sends a mutation after a timeout; it asks the daemon about that request ID instead. (Idempotency keys arrive in rung 3a of [10](10-roadmap.md). In rung 2, before the store and outbox exist, the CLI never retries a mutation, and an uncertain create just returns `outcome_unknown` with its `opId`.)

**Moves are outbox jobs with saved steps** (vault: `A Detached Child Outlives Its Supervisor`). The target task is created with the move job's own `opId`; the source's `opId` is never copied. On start, the daemon resumes or rolls back an unfinished `tasks move`, and still checks the copy before deleting the source. If any copy step's outcome is `unknown` (a checklist item, linked resource or attachment create, or the final upload chunk), the job pauses as unresolved: it keeps the source and the partial target, deletes nothing, and is flagged in `ms-todo outbox list` for the user. Cleaning up a half-built target is allowed only when every step's outcome is known.

Once the source DELETE may have started, recovery never deletes the target. It GETs both:

- source gone, target present: `done`.
- both present: finish the delete.
- source present, target missing: roll back; nothing is lost.
- **both missing:** the job stays unresolved and keeps the full local copy (`raw_json` and children). `ms-todo outbox list` flags it, and the user resolves it with `outbox retry` (recreate it in the target list) or `outbox discard`. Nothing is recreated automatically.

See [05](05-custom-features.md#move-between-lists).

The worker sends the operation to Graph:

- **Success:** save the returned `graph_id`, `etag` and `raw_json`, and mark the operation `done`.
- **Temporary failure:** back off and retry, within the rules in [03](03-graph-provider.md#http-client) (no automatic retry of a non-idempotent request after a timeout or 5xx). Nothing changes in the UI apart from a small "pending sync" marker.
- **Permanent rejection:** roll back, send `WriteRejected`, and keep the operation as `failed` in `ms-todo outbox list`.
- **Unknown outcome:** the operation goes to `unknown`; see below.

Before sending, check that the target list isn't tombstoned. That isn't enough on its own: **a POST into a deleted list returns 201** for a while after the delete (S4), so the write "succeeds" into a list nobody can see. So after a task create returns 201, the operation isn't `done` until a `GET /me/todo/lists/{l}` sent after the 201 returns 200. A deleted list returns 404 there (S4: "After `DELETE /me/todo/lists/{id}` (204), `GET /lists/{id}` is 404"), which also catches a list created and deleted between two delta rounds, which `lists/delta` never reports. Send these GETs in a batch, one per distinct list per round. If one returns 404, send `WriteRejected` ("the list was deleted on another device") and keep the task's content as a `failed` outbox entry, so `ms-todo outbox list` shows it and the user can add it again. Never drop it silently. Whether it should move to "Tasks" automatically is Q12 in [12](12-open-questions.md#product-questions-for-bk). Pending operations for a list that gets an `@removed` fail the same way.

A DELETE that returns 404 means the entity is already gone. Treat it as success.

### Unknown outcome (D-028)

Graph To Do has no idempotency key. If a create POST times out, returns a 5xx, or is in flight when the daemon crashes, Graph may already have created the entity. Sending it again could make a duplicate. The same goes for a PATCH that completes a recurring task: a repeat would complete the next occurrence as well (inferred from S12, not tested). The HTTP client never retries these automatically ([03](03-graph-provider.md#http-client)); they go to the outbox state **`unknown`**, and on daemon start any of them still marked `inflight` go there too. Other operations still `inflight` at start go back to `pending`: a PATCH that sets absolute values is safe to repeat, and a DELETE that finds nothing counts as success.

**Invariant: an ambiguous outcome of a non-idempotent operation stays `unknown` until it can be attributed or the user resolves it. Not finding something never allows a replay or a delete.** How each operation is handled:

- **Task create.** Every task POST carries our extension inline, holding the outbox operation's ID: `"extensions": [{"@odata.type": "microsoft.graph.openTypeExtension", "extensionName": "com.planetaryescape.mstodo", "opId": "<op_id>", …}]` (S13). Each sync round, the worker looks it up: `GET /me/todo/lists/{l}/tasks?$filter=lastModifiedDateTime ge {sent_at − 5 min}&$expand=extensions($filter=id eq 'com.planetaryescape.mstodo')`, matching `opId` exactly, client-side (Graph can't filter on extensions). On a match, first confirm the target list is live: `GET /me/todo/lists/{l}` must return 200, the same gate as after a normal 201. If the list is gone, send `WriteRejected` and keep the content as a `failed` entry; an absent list never triggers a replay or a recreation. Inside a move, the source is never deleted until the target's list is confirmed live. Then adopt it and mark the operation `done`. Adoption is an **atomic merge** in one transaction, because delta may already have inserted the Graph task as a new local row: the original optimistic row keeps its `local_id` and takes the Graph data (`graph_id`, `etag`, `raw_json`); the row sync inserted is deleted; anything that pointed at it (outbox operations, children) is re-pointed to the original; `graph_id` stays unique. Clients get one `EntityChanged` for the original row and an `EntityRemoved` for the other. It keeps looking for up to 24 hours (`outbox.unknown_lookup_hours`). It never goes back to `pending` by itself.
- **Checklist-item and linked-resource creates.** These have no extensions, and they're never adopted by matching their content. They stay `unknown`.
- **Recurring completion.** Before sending, record the task's due date. Each round, GET that task and record whether its due date has moved. A moved due date doesn't prove *our* PATCH worked, because the phone may have edited or completed the task. So the operation stays `unknown`: it's never marked `done` and never re-sent automatically. `ms-todo outbox list` shows what was seen ("may already be completed: due moved from X to Y"), and the user resolves it.
- **Any other create** (category, extension, attachment, upload session) has no attribution rule. It stays `unknown`. For an attachment, that includes a lost response to an upload session's final PUT, which is the one that commits it ([03](03-graph-provider.md#endpoints-the-whole-surface)): the upload is never retried or recreated because nothing was found.
- **Handing over to the user.** An operation still `unknown` after 24 hours, and any operation with no attribution rule, is flagged in `ms-todo outbox list` and in the TUI. The user resolves it: `outbox retry` re-sends it (a re-send the user chose), and `outbox discard` drops it.
- **Duplicate check.** If sync ever sees two tasks with the same `opId`, it sends a `DuplicateDetected` event and lists both in `ms-todo outbox list` and `doctor` for the user. Nothing is deleted automatically.

This follows mxr's `plans/014-send-outcome-recovery.md` (keep "unknown" apart from "definitely unsent", and resolve it before sending again) and `plans/023-interrupted-mutation-jobs.md` (after a crash, finish interrupted work honestly and don't replay external changes whose outcome is uncertain).

### Completing a recurring task

Completing a recurring task doesn't complete that ID (S12). The PATCH returns 200 with the **same ID** still `notStarted` and its due date rolled to the next occurrence. Graph also creates **a new task with a new ID** holding the completed occurrence. Delta returns both.

- Count the completion as successful when the PATCH returns 200 and either `status` is `completed`, or `recurrence` is set and the due date moved forward.
- Apply the rolled state to the local row. It isn't a conflict.
- Expect the completed copy to arrive through delta as a new task.
- Undoing the completion isn't a PATCH back. It means deleting the completed copy and restoring the old due date. Undo never picks the copy automatically. It shows the candidates, which are new tasks with the original's title, `status: completed` and a `createdDateTime` at or after `sent_at` minus 5 minutes, with their title, `createdDateTime` and list. The user picks one, in the TUI or with `ms-todo undo --copy <id>`. `createdDateTime` is stamped at real time on the copy (S12: "created 14:20:50.55Z"); `completedDateTime` isn't usable because it's rebased to midnight. With no candidate yet, undo says "can't undo yet". An agent calling `ms-todo undo` on a recurring completion without `--copy` gets exit code 2, with the candidates in the error JSON.

## Conflicts

Policy: **last write wins at the field level, with a warning when an edit is overwritten.**

- Where PATCH is concerned, send only the fields that changed. That way a phone edit to `title` and a CLI edit to `dueDateTime` both stick. Two exceptions, both because Graph resets what you leave out: a checklist-item PATCH always includes `isChecked` ([02](02-data-model.md#outbox-semantics)), and an extension write always sends the whole document ([05](05-custom-features.md)).
- Where `If-Match` works (S6), send it: task PATCH, and checklist-item PATCH and DELETE, which take the parent task's etag. **Task DELETE and list PATCH ignore it** and apply the change anyway. For a delete, accept last write wins, or GET and compare the etag first and accept the small race. Any child change moves the parent task's etag, so a task PATCH gets a 412 after a concurrent step edit too; the re-fetch below handles that.
- If `If-Match` gets a 412, meaning the server copy changed since we last saw it, re-fetch the entity. If the field we're changing is untouched on the server, re-send. If the server changed the same field, apply ours (last write wins) and send a `ConflictOverwritten` event that the TUI shows briefly and `ms-todo outbox list` records.
- When delta brings in a server change to an entity that has pending local operations, keep the local values for the fields those operations touch. Apply the server values to everything else.

## Freshness guarantee for the CLI

A read through the daemon returns the cache. `--fresh` forces a delta pass for the relevant lists before answering, and `ms-todo sync --wait` blocks until a full pass finishes. Agents should normally just read. The daemon keeps the cache current.

Each scope has a **sync generation** that only goes up, plus `in_progress`, `last_success_at` and `last_changed_count` ([02](02-data-model.md#tables-first-draft-refine-in-rung-3a)). A pass that finds nothing still bumps the generation, and `sync --wait` waits for the generation to move, not for a change or a clock tick (vault: `Zero Change Can Be Success`, `Faster Code Breaks Coarse Clocks`).
