# 02: Data model

SQLite (sqlx, migrations in `crates/store/migrations/`) is the local source of truth for reads. Graph is the source of truth for what the data is. The schema mirrors Graph closely so nothing is lost when data goes to the cache and back.

## Principles

- **Store the raw Graph JSON with each row** (`raw_json` column), next to the columns we query on. New Graph fields survive without a migration, and full-fidelity copies (see move in [05](05-custom-features.md)) have everything they need.
- **Local IDs are stable; Graph IDs can change.** Every entity has a `local_id` (UUID) that clients use, and a nullable `graph_id`. A task created locally has no `graph_id` until the outbox sends it. A task's Graph ID also changes when it's moved between lists ("By default, this value changes when the item is moved from one list to another", todoTask docs), and our move operation creates a new ID anyway. `local_id` is what the CLI's `--json` output and IPC use. Graph IDs are shown too, for debugging and raw API calls.
- **Store times as Graph sends them.** Graph returns `dateTimeTimeZone` (a local time plus a zone name) for due, start, reminder and completed dates. The raw pair stays in `raw_json`.
- **Due and start are dates, not instants** (S11). Graph keeps only the date, as midnight in whatever zone the writer used, so BK's data has due values at 23:00, 00:00 and 20:00 UTC. Store them as local dates (`YYYY-MM-DD`). To read one, convert the returned instant to the user's zone and **round to the nearest midnight**; truncating gives the wrong day for dates written in another zone. Write them as `T00:00:00` in the user's IANA zone.
  - **Known limitation:** rounding is exact only when the writer's zone is within 12 hours of the reader's. For example, a date written as midnight in Pacific/Kiritimati (UTC+14) reads as the previous day in London. The raw pair stays in `raw_json`, so nothing is lost.
- **Reminder and completed times** are instants. Store a UTC value computed from the raw pair, for sorting and filtering.

## Tables (first draft; refine during phase 1)

```
lists(local_id PK, graph_id UNIQUE NULL, display_name, wellknown_list_name,
      is_owner, is_shared, folder TEXT NULL,          -- from extension, see 05
      raw_json, etag, updated_at, deleted_at NULL)

tasks(local_id PK, graph_id UNIQUE NULL, list_local_id FK, title,
      body_content, body_content_type, status, importance,
      is_reminder_on, reminder_at_utc NULL, due_date NULL, start_date NULL,  -- local YYYY-MM-DD
      completed_at_utc NULL, recurrence_json NULL, categories_json,
      has_attachments, my_day_date TEXT NULL,          -- see 05
      assignee TEXT NULL,                              -- see 05
      created_at, last_modified_at, raw_json, etag, deleted_at NULL)

checklist_items(local_id PK, graph_id NULL, task_local_id FK, display_name,
      is_checked, checked_at NULL, created_at, raw_json)

linked_resources(local_id PK, graph_id NULL, task_local_id FK, web_url,
      application_name, display_name, external_id, raw_json)

attachments(local_id PK, graph_id NULL, task_local_id FK, name, content_type,
      size, last_modified_at, cached_path NULL)       -- file contents fetched on demand

extensions(owner_kind 'list'|'task', owner_local_id, extension_name, data_json,
      PRIMARY KEY(owner_kind, owner_local_id, extension_name))

categories(graph_id PK, display_name UNIQUE, color)

sync_state(scope TEXT PK,        -- 'lists' or 'tasks:<list graph_id>'
      delta_link TEXT NULL, last_full_sync_at, last_delta_at, last_error NULL,
      generation INTEGER,          -- only goes up; bumped on every finished pass, even with 0 changes
      in_progress, last_success_at NULL, last_changed_count)

outbox(op_id PK, created_at, client_request_id NULL,  -- kept 24 h for IPC idempotency, see 04
      entity_kind, entity_local_id, op TEXT, payload_json,
      depends_on_op_id NULL, attempts, next_attempt_at, state
      'pending'|'inflight'|'unknown'|'failed'|'done', last_error NULL,
      rollback_json NULL)       -- the previous state, for undo when rejected

settings(key PK, value)          -- e.g. my_day_category_name, last_rollover_date
```

## Indexes

Index what the TUI views need to be instant: `tasks(list_local_id, status, deleted_at)`, `tasks(due_date)`, `tasks(my_day_date)`, `tasks(importance)`, plus an FTS5 table over `tasks(title, body_content)` for search. FTS5 is enough at To Do's scale; we're not using Tantivy (D-011).

## Outbox semantics

- Each local write applies to SQLite **and** adds a row to the outbox in one transaction.
- A create that other operations depend on (for example, creating a task and then adding checklist items) records `depends_on_op_id`, so the children wait until the parent has a `graph_id`.
- The worker sends pending operations in order for each entity, with retries and backoff (see [03](03-graph-provider.md)).
- A 4xx error that won't go away on retry (other than 401, 408 and 429) marks the operation `failed`, applies `rollback_json`, and sends a `WriteRejected` event with Graph's error code and message. The CLI and TUI show it. It's never swallowed.
- A create whose result we never saw (a timeout, a 5xx, or a crash while it was `inflight`) goes to `unknown`, not back to `pending`, because Graph To Do has no idempotency key and a resend can duplicate it. So does a recurring-task completion, which is never marked `done` automatically, because a moved due date could be the phone's doing. An `unknown` operation is adopted only when its result can be attributed to it: a task create carries its `op_id` as `opId` in our extension, so a lookup can find it exactly. Adoption merges atomically into the original row, which keeps its `local_id`; a duplicate row that sync inserted first is deleted and its references are re-pointed ([04](04-sync-cache.md#unknown-outcome-d-028)). Anything else stays `unknown` until the user resolves it with `outbox retry` or `outbox discard`. Not finding something never allows a replay or a delete. Details: [04](04-sync-cache.md#unknown-outcome-d-028) (D-028).
- **A checklist-item PATCH always includes `isChecked`.** Leaving it out resets the step to unchecked (S1).
- An entity's `sync_state` in client responses comes from its outbox operations: `synced`, `pending`, `unknown` (an operation's outcome is ambiguous) or `failed` ([07](07-cli.md#output-contract)).
- `ms-todo outbox list|retry|discard` shows the queue and lets you act on it.
