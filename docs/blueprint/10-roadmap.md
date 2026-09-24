# 10: Roadmap

Each phase ends with a **completion check that BK or an agent can observe**, run through the CLI against BK's real account unless stated otherwise. "Tests pass" is required, but it doesn't count as done on its own (spotuify's rule).

## Phase 0: Spikes and setup (before any product code)

- **Done (2026-09-24):** BK registered the `ms-todo` Entra app in the personal Default Directory for organizational and personal Microsoft accounts. Public-client flows are enabled; the delegated permissions are `Tasks.ReadWrite`, `MailboxSettings.ReadWrite`, `offline_access` and `User.Read`. The client ID is recorded in [docs/setup/entra-app-registration.md](../setup/entra-app-registration.md) and in the registration machine's local, Git-ignored `.env`.
- **Verified (2026-09-24):** device-code sign-in against `/common` returned an access token and refresh token for BK's personal Microsoft account. Graph returned HTTP 200 for `/me/todo/lists` and `/me/outlook/masterCategories`; see [S5 evidence](12-open-questions.md#s5-result-2026-09-24). The client ID is also in the `ms-todo Entra app` item in 1Password's `Environment Variables` vault.
- **Recorded (2026-09-24):** spikes S1–S4, S6–S12, and the added P1 (pagination) and S13 (a marker on task create) are done. The Graph spikes ran against a throwaway list and throwaway categories; S8 ran offline. Each has a result in [12-open-questions.md](12-open-questions.md) and its evidence in `docs/research/spikes/`, and the blueprint documents they affect are updated (D-026 to D-030). Still open:
  - the phone halves of S7 (category display, filter and My Day colour) and S11 (how the phone shows due dates and reminders). BK does these on the phone; the steps are in the evidence files. The spike list and categories stay until then.
  - deltaLink lifetime (S4). The saved links are due to be replayed in a few days.
  - product questions Q6–Q12 in [12](12-open-questions.md#product-questions-for-bk).

**Done when:** every spike has a recorded answer with evidence (the request, the response and a date), and the blueprint reflects those answers. **Met on 2026-09-24, except** the S7 and S11 phone checks and the S4 deltaLink replay listed above. None of them blocks Phase 1.

## Phase 1: Foundation: sign-in, Graph client, store, minimal daemon

- Workspace skeleton, the boundary test, CI (fmt, clippy with `-D warnings`, nextest), release-please.
- `crates/graph`:
  - sign-in (device code, token store, refresh, `AuthRevoked`)
  - HTTP client (retry, rate limit, errors, pagination, batch)
  - endpoints for lists and tasks
- `crates/store`: migrations for lists and tasks, and upsert.
- A minimal daemon (D-032): the protocol and codec, the socket server, auto-start, instance separation, `daemon start|stop|status`. The daemon owns sign-in state and is the only token refresher.
- CLI over IPC from the start: `auth login|status|logout|bearer`, `lists list`, `tasks list`, `raw` (GET only), `sync`. No direct mode.
- The cache (D-032): on start and on `ms-todo sync`, the daemon runs a full enumeration of lists and tasks. It pages to the end, upserts into the store, fetches extension content and attachment metadata for the tasks that need them, and tombstones anything it didn't see, by [04](04-sync-cache.md#reconciliation-after-a-lost-delta-token)'s reconciliation rules. Reads are served from the store. Phase 2 adds delta on top, and this code becomes delta's reset path.

**Done when:** `ms-todo auth login` works through device code. `ms-todo lists list --format json` and `ms-todo tasks list --list Tasks --format json` return BK's real data, with **every** task in a list of more than 100 tasks (paginate a test list). The default page is 50 tasks (P1), so that list spans at least three pages. `ms-todo raw GET /me` works. A deliberately revoked token makes commands exit with code 4 and a clear message.

## Phase 2: Sync, outbox

- `crates/sync`: delta for lists and tasks on top of Phase 1's full enumeration, which becomes the reset path after a lost token. Tombstones.
- The outbox, with instant writes, rollback and events.
- Mutations for lists and tasks: add, edit, complete, reopen, delete, with `--dry-run`, `--idempotency-key` and `ms-todo undo`. Until Phase 5, `tasks add` takes its text literally, as `--no-parse` would.
- `doctor`, `outbox list|retry|discard`, `sync --wait`.
- `raw` POST, PATCH and DELETE, as synchronous debug passthroughs outside the outbox ([07](07-cli.md#output-contract)).

**Done when:**

- A task added on the phone shows up in `ms-todo tasks list` within about 30 seconds, with no manual sync.
- `ms-todo tasks add` with the network off returns immediately with the task marked `pending`. Once the network is back, it syncs, and the phone shows it.
- A write against a deleted list is reported as `WriteRejected`, and the task's content is kept as a `failed` outbox entry that shows in `ms-todo outbox list`. It's never silently dropped ([04](04-sync-cache.md#instant-local-writes)).
- Deleting a task on the phone removes it from the cache.
- Forcing a delta token to be invalid triggers reconciliation, and a task deleted during that window disappears.

## Phase 3: Full API surface

- Steps, links, attachments (direct upload and upload session, download), extensions, categories, `$batch` use, `tasks move` (the copy that checks itself), and the whole set of task fields including recurrence.

**Done when:**

- Every command in [07](07-cli.md) is implemented, except the ones Phase 5 builds: `myday`, `folders`, `lists move|order`, `tasks parse`, quick-add parsing (`--no-parse` only matters then), and the `--my-day` and `--assignee` flags.
- List and task operations (lists, tasks, steps, links, attachments, extensions, categories, move) have wiremock tests and have run live against a throwaway list. There's a feature-gated live smoke test for this.
- The commands that don't touch a list each have their own check:
  - `daemon start|stop|status`: a stop counts only when the socket is gone and the PID has exited.
  - `auth logout`, then `auth status` shows signed out.
  - `schema` output matches its insta snapshots.
  - `doctor` reports every subsystem.
  - `outbox list|retry|discard` works on an operation forced to `unknown` against wiremock.
- A 10 MB attachment uploads and downloads with identical bytes (compare the sha256).
- `tasks move` keeps the steps, links, attachments and extension. Check with `show` before and after.

## Phase 4: TUI

- The layout, smart views, the detail pane, editing, multi-select, undo, the palette, the hint bar, sync markers and the diagnostics page. Quick add takes text literally for now.
- Not in this phase, because Phase 5 builds what they rest on: the My Day view, the Assigned view, folders in the sidebar, and live highlighting while you type.

**Done when:** BK uses it for a day and every change made in the TUI shows up on the phone. The measured latencies meet [08](08-tui.md)'s budget: under 16 ms per keypress and under 150 ms for a cold start, logged through tracing.

## Phase 5: Custom features and natural language

- My Day (category, extension date, rollover, suggestions, special view), folders, assignment, and their TUI parts: the My Day and Assigned views and folders in the sidebar.
- `crates/nlp`: the rule-table date scanner (D-026, from spike S8), token grammar, recurrence grammar, live highlighting in the TUI, `tasks parse`, `--no-parse`, `--dry-run`.

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
