# 10: Roadmap

Each phase ends with a **completion check that BK or an agent can observe**, run through the CLI against BK's real account unless stated otherwise. "Tests pass" is required, but it doesn't count as done on its own (spotuify's rule).

## Phase 0: Spikes and setup (before any product code)

- BK registers the Entra app ([03](03-graph-provider.md)) and puts the client ID in 1Password.
- A throwaway spike binary (or `curl` plus `ms-todo`'s future token, or MSAL's device-code sample) runs every spike in [12-open-questions.md](12-open-questions.md) against a throwaway list.
- Write the findings into `12-open-questions.md` and update the blueprint documents they affect.

**Done when:** every spike has a recorded answer with evidence (the request, the response and a date), and the blueprint reflects those answers.

## Phase 1: Foundation: sign-in, Graph client, store

- Workspace skeleton, the boundary test, CI (fmt, clippy with `-D warnings`, nextest), release-please.
- `crates/graph`:
  - sign-in (device code, token store, refresh, `AuthRevoked`)
  - HTTP client (retry, rate limit, errors, pagination, batch)
  - endpoints for lists and tasks
- `crates/store`: migrations for lists and tasks, and upsert.
- CLI without a daemon yet, in direct mode: `auth login|status|logout|bearer`, `lists list`, `tasks list`, `raw`.

**Done when:** `ms-todo auth login` works through device code. `ms-todo lists list --format json` and `ms-todo tasks list --list Tasks --format json` return BK's real data, with **every** task in a list of more than 100 tasks (paginate a test list). `ms-todo raw GET /me` works. A deliberately revoked token makes commands exit with code 4 and a clear message.

## Phase 2: Daemon, sync, outbox

- The protocol and codec, the socket server, auto-start, instance separation, `daemon start|stop|status`.
- `crates/sync`: delta for lists and tasks, reconciliation after a lost token, tombstones.
- The outbox, with instant writes, rollback and events.
- The CLI switches to going through the daemon. Mutations for lists and tasks: add, edit, complete, reopen, delete.
- `doctor`, `outbox list|retry|discard`, `sync --wait`.

**Done when:**

- A task added on the phone shows up in `ms-todo tasks list` within about 30 seconds, with no manual sync.
- `ms-todo tasks add` with the network off returns immediately with the task marked `pending`. Once the network is back, it syncs, and the phone shows it.
- A deliberately invalid write (for example, one against a deleted list) is rolled back and reported.
- Deleting a task on the phone removes it from the cache.
- Forcing a delta token to be invalid triggers reconciliation, and a task deleted during that window disappears.

## Phase 3: Full API surface

- Steps, links, attachments (direct upload and upload session, download), extensions, categories, `$batch` use, `tasks move` (the copy that checks itself), and the whole set of task fields including recurrence.

**Done when:**

- Every command in [07](07-cli.md) is implemented, has wiremock tests, and has run live against a throwaway list. There's a feature-gated live smoke test for this.
- A 10 MB attachment uploads and downloads with identical bytes (compare the sha256).
- `tasks move` keeps the steps, links, attachments and extension. Check with `show` before and after.

## Phase 4: TUI

- The layout, smart views, folders in the sidebar, the detail pane, editing, multi-select, undo, the palette, the hint bar, sync markers and the diagnostics page.

**Done when:** BK uses it for a day and every change made in the TUI shows up on the phone. The measured latencies meet [08](08-tui.md)'s budget: under 16 ms per keypress and under 150 ms for a cold start, logged through tracing.

## Phase 5: Custom features and natural language

- My Day (category, extension date, rollover, suggestions, special view), folders, assignment.
- `crates/nlp`: the evaluation spike S8 with clockwords and interim, token grammar, recurrence grammar, live highlighting in the TUI, `tasks parse`, `--no-parse`, `--dry-run`.

**Done when:**

- `ms-todo tasks add "Pay rent every 1st #Home p1 !9am"` creates the right task, and the phone shows its recurrence, list, importance and reminder correctly.
- A task added to My Day in ms-todo shows the "My Day" category on the phone, and it's gone from My Day after midnight.
- Adding the category on the phone puts the task in ms-todo's My Day view.
- The phrase corpus passes.

## Phase 6: Ship

- Agent skill, README, `AGENTS.md` ("Use ms-todo to build ms-todo"), install script, Homebrew formula, launchd and systemd service files.
- Use the release-please chain to publish v0.1.0 (BK's "ship it" definition in AGENTS.md).

**Done when:** on a clean machine, `brew install` (or `install.sh`) followed by `ms-todo auth login` and `ms-todo tui` works. An agent session using only the skill can create, edit, complete and move a task with the right exit codes.

## Deferred (not in v1)

- An optional local-LLM parser behind `QuickAddParser` (D-016).
- Windows support.
- Location text on tasks (Q2).
