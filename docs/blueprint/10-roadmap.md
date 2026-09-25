# 10: Roadmap

## Build philosophy

Skateboard → scooter → bicycle → car (vault: `Skateboard MVP`). The roadmap is a ladder of **usable releases**, not a list of components. Each rung is a working, installable product that lets BK do something new from start to finish, and it's sized to fit one working session (vault: `Shippable Increment Per Session`). lazydap's roadmap follows the same rule (`~/code/planetaryescape/lazydap/docs/blueprint/14-roadmap.md`, "Build philosophy").

- **Every rung is released and installable.** It goes out through release-please as a GitHub release, and installs with `install.sh`. From rung 5b it installs with Homebrew as well.
- **Ceremony is named and pulled into the nearest useful turn.** The workspace, the boundary test, CI and the release chain produce nothing to demo on their own.
- **A few named foundation turns are allowed** (up to about three) when a project can't be usable after its first turn. Each is named as a foundation turn, says what it makes possible, and ends with something runnable or checkable. The plan says which turn delivers the first usable rung; after that it's one usable release per turn. Here there's one: **F1**, and rung 1 (the first usable rung) arrives in turn 2.
- **At most two new unknowns per rung.** Each rung names its unknowns. If a rung needs a third, split it.
- **Stop after any rung and there's a working tool.** Each rung says what it deliberately leaves out.
- **Accepted rework:** rung 1 reads straight from Graph and rung 3a replaces that with the cache (D-034). That's the trade `Skateboard MVP` describes: "more re-work in exchange for de-risking".

Each rung ends with a **completion check that BK or an agent can observe**, run through the installed `ms-todo` binary against BK's real account unless stated otherwise. "Tests pass" is required, but it doesn't count as done on its own (spotuify's rule). Each rung also ends with its demo: run the thing and show what's new.

## Phase 0: Spikes and setup (before any product code)

- **Done (2026-09-24):** BK registered the `ms-todo` Entra app in the personal Default Directory for organizational and personal Microsoft accounts. Public-client flows are enabled; the delegated permissions are `Tasks.ReadWrite`, `MailboxSettings.ReadWrite`, `offline_access` and `User.Read`. The client ID is recorded in [docs/setup/entra-app-registration.md](../setup/entra-app-registration.md) and in the registration machine's local, Git-ignored `.env`.
- **Verified (2026-09-24):** device-code sign-in against `/common` returned an access token and refresh token for BK's personal Microsoft account. Graph returned HTTP 200 for `/me/todo/lists` and `/me/outlook/masterCategories`; see [S5 evidence](12-open-questions.md#s5-result-2026-09-24). The client ID is also in the `ms-todo Entra app` item in 1Password's `Environment Variables` vault.
- **Recorded (2026-09-24):** spikes S1–S4, S6–S12, and the added P1 (pagination) and S13 (a marker on task create) are done. The Graph spikes ran against a throwaway list and throwaway categories; S8 ran offline. Each has a result in [12-open-questions.md](12-open-questions.md) and its evidence in `docs/research/spikes/`, and the blueprint documents they affect are updated (D-026 to D-030). Still open:
  - the phone halves of S7 (category display, filter and My Day colour) and S11 (how the phone shows due dates and reminders). BK does these on the phone; the steps are in the evidence files. The spike list and categories stay until then.
  - deltaLink lifetime (S4). The saved links are due to be replayed in a few days.
  - product questions Q6–Q12 in [12](12-open-questions.md#product-questions-for-bk).

**Done when:** every spike has a recorded answer with evidence (the request, the response and a date), and the blueprint reflects those answers. **Met on 2026-09-24, except** the S7 and S11 phone checks and the S4 deltaLink replay listed above. None of them blocks rung 1.

## Foundation F1: Install and sign in (turn 1)

A named foundation turn: there's nothing to use yet, but it's released and checkable.

**Makes possible:** every later rung ships and authenticates the same way.

**Build:**

- The workspace skeleton, the boundary test ([01](01-architecture.md#crates)), CI (fmt, clippy with `-D warnings`, nextest), and the release chain (release-please → GitHub release → `install.sh`).
- `crates/graph` sign-in: device code, the token store, and the compare-and-swap refresh ([03](03-graph-provider.md#sign-in)).
- `ms-todo auth login|status|logout`. These work without a daemon anyway (D-033 item 4), so F1 needs no daemon.

**Done when:** ms-todo installs from a GitHub release with `install.sh`, `ms-todo auth login` signs in through device code, and `ms-todo auth status` shows the account and the token's expiry. `auth logout`, then `auth status`, shows signed out.

## Rung 1: Skateboard: see my tasks (turn 2, the first usable rung)

**Previously:** F1: an installed `ms-todo` that signs in. **Now:** the same, plus lists and tasks from BK's real account, through a minimal daemon.

**Promise:** "I can install ms-todo, sign in, and see every task in any list from my terminal or an agent."

**Build:**

- `crates/graph`, on top of F1's sign-in: `AuthRevoked`, the HTTP client with timeouts, retry and `decide_retry`, pagination, and typed errors. Endpoints for lists and tasks.
- A minimal daemon (D-032): the protocol and codec with an explicit frame cap, the socket server (0600 socket in a 0700 directory), auto-start, readiness ([01](01-architecture.md#daemon-lifecycle)), instance separation, and `daemon start|stop|status`. It owns the token and is the only process that refreshes it. **It reads straight from Graph, with no cache yet** (D-034).
- CLI over IPC: `auth bearer --reveal-secret`, `lists list`, `tasks list [--list L]`, `raw GET`, `--format table|json|jsonl|ids` with `schema_version`, and the exit codes in [07](07-cli.md#exit-codes). Entities carry an opaque `id`, which holds the Graph ID until rung 3a ([07](07-cli.md#output-contract)).

**New unknowns:** daemon auto-start and readiness; the IPC protocol end to end. The Graph client is adapted from mxr and spotuify ([09](09-reuse-map.md)) and were checked by the spikes.

**Demo:** `curl … install.sh | sh`, then `ms-todo auth login`, then `ms-todo tasks list --list Tasks`.

**Done when:**

- The new release installs with `install.sh`, and F1's sign-in still works.
- `ms-todo lists list --format json` returns BK's lists. `ms-todo tasks list --list Tasks --format json` returns **every** task of a list with more than 100 tasks (paginate a test list; the default page is 50 tasks (P1), so it spans at least three pages).
- `ms-todo raw GET /me` works.
- A deliberately revoked token makes commands exit with code 4 and a clear message.
- `daemon stop` counts only when the socket is gone and the PID has exited.

**Left out:** any writes, the cache, sync, the TUI.

## Rung 2: Scooter: capture and finish tasks

**Previously:** read-only lists and tasks. **Now:** read-only lists and tasks, plus adding, editing, completing, reopening and deleting tasks from the CLI or an agent.

**Promise:** "I can capture a task and tick it off from the terminal, and an agent can do the same safely."

**Build:**

- `tasks add "text"` (the text is taken literally) with `--due YYYY-MM-DD`, `--list`, `--importance` and `--reminder`; `tasks complete`, `reopen`, `edit` (title, due, importance, reminder) and `delete`. Each is sent synchronously through the daemon to Graph; there's no outbox yet. Due dates are dates only, and a time goes to the reminder (D-027).
- Task creates carry their `opId` in our extension (S13). The HTTP client never retries a non-idempotent request automatically ([03](03-graph-provider.md#http-client)): an uncertain create or recurring completion returns `outcome_unknown` with its `opId`, and nothing is re-sent. Completing a recurring task follows [04](04-sync-cache.md#completing-a-recurring-task)'s success rule.
- `--dry-run` from the same typed plan, and `--yes` for destructive commands (exit 2 off a terminal without it).
- `raw` POST, PATCH and DELETE, as synchronous debug passthroughs ([07](07-cli.md#output-contract)).
- The `--help` snapshots with a CI drift check. (`ms-todo schema` moved to rung 3a, which changes the output contract anyway.)
- **Known rung 2 limits.** An uncertain create returns error kind `outcome_unknown` with its `opId`; the daemon doesn't retry it, and there's no stored `unknown` state or automatic lookup until rung 4. The CLI never retries a mutation by itself. `--idempotency-key`, and the guarantee that a repeat returns the original result, arrive in rung 3a, when keys persist in the store. Without `--list`, a task the daemon hasn't read or written since it started is found by asking each list for it (Graph has no task path without the list). A 412 on a field someone else also changed is reported as a conflict (exit 5) and nothing is overwritten; [04](04-sync-cache.md#conflicts)'s last-write-wins with a `ConflictOverwritten` event needs the event stream and outbox, so it arrives with them.
- Agent skill v0, `skills/ms-todo/SKILL.md` ([07](07-cli.md#agent-skill)): literal `tasks add` and Graph IDs, resolved with `lists list` or `tasks list`. And the "Use ms-todo to build ms-todo" section in `AGENTS.md`.

**New unknowns:** the write path's error mapping; the agent skill's first contact with a real agent.

**Demo:** add a task in the CLI and watch it appear on the phone; complete it and watch it tick off. Then let an agent do the same using only the skill.

**Done when:**

- A task added with `ms-todo tasks add` shows on the phone, and one completed with `ms-todo tasks complete` shows as completed on the phone.
- An agent session using only the skill adds and completes a task, with the right exit codes.
- `--help` output matches its insta snapshots.

**Left out:** the cache (reads still go to Graph), offline writes, undo, natural-language parsing.

## Rung 3a: Bicycle: instant reads

**Previously:** reads and writes that each go to Graph. **Now:** the same writes, plus instant reads from a local cache the daemon refreshes by full enumeration.

**Promise:** "ms-todo answers instantly, and `sync` brings in what changed on my phone."

**Build:**

- `crates/store`: migrations for lists and tasks, upserts, local IDs ([02](02-data-model.md)). This replaces rung 1's Graph-direct read handler (D-034). Entities' `id` becomes the local ID, with `graph_id` alongside, and `schema_version` goes up ([07](07-cli.md#output-contract)).
- `ms-todo schema [CMD]`: the input and output JSON schemas at the new `schema_version`, snapshotted with insta and checked for drift in CI ([07](07-cli.md#output-contract)). Moved here from rung 2.
- Full enumeration with hydration and the checkpoint rule ([04](04-sync-cache.md#reconciliation-after-a-lost-delta-token), [04](04-sync-cache.md#children-of-a-task)), run on start, every 5 minutes and on `ms-todo sync`, with tombstones for what it didn't see.
- Reads served from the cache, with the local filters in [07](07-cli.md) (`--status`, `--due`, `--importance`, `--search` through FTS5, `--sort`, `--limit`), `EntityChanged` events, and the 500-ID cap with `ResyncNeeded`.
- The sync generation counter, `sync --wait` with progress events, `doctor`, and the `initial` state in the response envelope. `--idempotency-key` with its fingerprints, stored in the store ([04](04-sync-cache.md#instant-local-writes)). The agent skill moves to local IDs.
- Optional launchd and systemd service files.

**New unknowns:** hydration and the checkpoint against the real account; the store's shape under the ID change.

**Demo:** `time ms-todo tasks list`; change a task on the phone, run `ms-todo sync --wait`, and see it.

**Done when:**

- `ms-todo tasks list` answers from the cache. Measure it and record the number.
- After a change on the phone, `ms-todo sync --wait` returns and `tasks list` shows the change.
- `doctor` reports every subsystem.
- `ms-todo schema` output matches its insta snapshots, at the new `schema_version`.

**Left out:** delta and background freshness faster than 5 minutes (rung 3b), offline writes, undo, the TUI.

**Built (2026-09-24):** the store, full enumeration with the checkpoint rule, reads from the cache with the `initial` state, local IDs at `schema_version` 2, `--idempotency-key`, `schema`, `doctor`, and the stall deadline (issue 002). Measured on BK's account: first sync about 17 seconds, a later one about 2.3 seconds, `tasks list --list Tasks` about 10 ms. The local filters, `EntityChanged` events and the service files weren't built; [D-036](11-decision-log.md) lists them.

## Rung 3b: Bicycle with gears: live sync

**Previously:** instant reads from a cache refreshed every 5 minutes or on `sync`. **Now:** the same, plus live sync: a change on the phone shows up by itself within about 30 seconds.

**Promise:** "A change on my phone shows up by itself."

**Build:**

- Delta sync for lists and tasks ([04](04-sync-cache.md#delta-sync)), with rung 3a's enumeration becoming the reset path after a lost cursor, tombstones for `@removed`, and polling every 15–30 seconds while a client is connected or for 10 minutes after any client request, and every 5 minutes otherwise ([04](04-sync-cache.md#delta-sync)).

**New unknowns:** delta against the real account over hours, not minutes (S4's lifetime is still open); the reset path under real 410s.

**Demo:** run `ms-todo tasks list`, add a task on the phone, run `ms-todo tasks list` again within about 30 seconds, and it's there.

**Done when:**

- A task added on the phone shows up in `ms-todo tasks list` within about 30 seconds, with no manual sync.
- Deleting a task on the phone removes it from the cache.
- Forcing a delta token to be invalid triggers reconciliation, and a task deleted during that window disappears.

**Left out:** offline writes, undo, the TUI.

## Rung 4: E-bike: offline, and never lose a write

**Previously:** instant, live-synced reads, with writes that need the network. **Now:** instant reads, plus instant writes that work offline and are never lost, and undo.

**Promise:** "I can add and change tasks with no network, and nothing I write is ever silently dropped. I can undo."

**Build:**

- The outbox ([02](02-data-model.md#outbox-semantics), [04](04-sync-cache.md#instant-local-writes)): instant local writes, background sending, rollback, the `unknown` state with `opId` attribution, the ghost-write check, and `outbox list|retry|discard`. Rung 2's writes move onto it.
- `ms-todo undo [OP_ID]` ([07](07-cli.md#output-contract)).

**New unknowns:** the `unknown` state's recovery against real failures; undo of a recurring completion.

**Demo:** turn the network off, add a task, see it marked `pending` instantly; turn the network on and watch it reach the phone. Undo it.

**Done when:**

- `ms-todo tasks add` with the network off returns immediately with the task marked `pending`. Once the network is back, it syncs, and the phone shows it.
- A write against a deleted list is reported as `WriteRejected`, and the task's content is kept as a `failed` outbox entry that shows in `ms-todo outbox list`. It's never silently dropped ([04](04-sync-cache.md#instant-local-writes)).
- `ms-todo undo` reverses an add, a complete and a delete.
- `outbox list|retry|discard` works on an operation forced to `unknown` against wiremock.

**Left out:** the TUI, natural-language parsing.

## Rung 4b: search

Added at BK's request (2026-09-24), between rung 4 and rung 5.

**Previously:** instant, offline-safe reads and writes, one list at a time. **Now:** the same, plus finding any task in any list by the words in its title or notes.

**Promise:** "I can find any task by the words in it, instantly."

**Build:**

- Migration `0004`: an FTS5 table over each live task's title and its notes as plain text (html notes rendered to text), with the `unicode61 remove_diacritics 2` tokenizer and `prefix='2 3'`. Triggers on `tasks` keep it current; a tombstoned task isn't in it. The migration indexes the tasks already cached ([02](02-data-model.md#indexes), D-041).
- `ms-todo search QUERY [--list L] [--status open|completed|all] [--limit N]`: open tasks in every list by default, at most 50, ranked by bm25 with the title weighted above the notes. Each result is the task with its list's name (`list`) and the passage that matched (`snippet`, matches between `**`). The query is FTS5's syntax (words, `"phrases"`, `prefix*`, `AND`, `OR`, `NOT`, parentheses) with every word quoted first, so punctuation is text; a malformed query exits 2.
- `tasks list --search Q`: the same engine, within one list ([07](07-cli.md)).
- Every output format; CSV columns `id,title,list,status,due,snippet`. The `schema` snapshot and the agent skill's "Finding tasks" section.

**New unknowns:** FTS5's ranking and tokenizer against BK's real tasks; keeping a second structure in step with every write path.

**Demo:** `mst search "insurance"` returns the matching tasks across all lists, best match first, in about a millisecond. `mst search "insur*" --list Tasks --format csv` works as well.

**Done when:**

- `ms-todo search WORD --status all` returns the same tasks as a `grep` for the word over the titles and notes from `tasks list --format json`, for a few common words on BK's account.
- The query itself takes about a millisecond on BK's account. Measure it and record the number.
- A malformed query exits 2 with a clear message.

**Left out:** semantic search (D-042), search in the TUI (rung 5), searching steps, categories or attachments.

**Built (2026-09-24):** as above. On BK's account (30 lists, 655 tasks, all with text notes), in a separate `livetest` instance: for 12 common words, `search --status all` matched exactly the tasks a word-boundary `grep` over titles and notes found (for example 159 for "the", 26 for "check"). The query itself takes 0.3–0.8 ms in SQLite; `ms-todo search` end to end, including process start and IPC, takes 6–7 ms, and `search "insur*" --list Tasks --format csv` 4–6 ms.

## Rung 5: Motorbike: a fast TUI for daily use

Split in two during the build, because one session couldn't hold it (D-043). 5a is a TUI you can browse and act in; 5b makes it the one BK lives in.

### Rung 5a: Motorbike, part 1: browse and act

**Previously:** a CLI with instant, offline-safe reads and writes, and search. **Now:** the same, plus a keyboard TUI over the same daemon.

**Promise:** "I can open `mst tui`, see all my lists and tasks instantly, and add, complete, reopen, delete and undo from the keyboard, with every change reaching the phone."

**Build:**

- Protocol 5: `Seed` (the lists, the smart views' and lists' counts, and one scope's tasks, from the cache) and `Subscribe` (`EntityChanged` with up to 500 IDs, else `ResyncNeeded`; `WriteRejected`; `SyncState`).
- `crates/tui` over the protocol only ([08](08-tui.md)): the sidebar with Important, Planned (grouped), All, Completed and the lists; the task list with sync markers; the detail pane; the status line; the hint bar; help. Literal add, complete and reopen, delete with a confirmation, undo with the recurring-completion picker, `/` filtering through search (D-041), `r` to sync.
- The latency budget, measured: tracing spans and `--bench-startup`.

**Demo:** `mst tui`, move through the sidebar and the smart views, add a task, complete it, undo, and watch its sync marker go from pending to synced.

**Done when:**

- Driven against the real account, every step shows up in Graph (`raw GET`), and the marker goes from pending to synced.
- The measured latencies meet [08](08-tui.md)'s budget: under 16 ms per keypress and view switch, and under 150 ms for a cold start from the cache.

**Left out (5b):** editing fields, multi-select, the palette, the diagnostics page and Homebrew.

### Rung 5b: Motorbike, part 2: a TUI to live in

**Previously:** a TUI to browse and act in. **Now:** the TUI BK runs his day from, installable with Homebrew.

**Promise:** "I can run my day from `ms-todo tui`."

**Build:**

- Editing the title, due date, importance, reminder and notes in place.
- Multi-select (`v`) with the bulk actions, the command palette (`:`), and the diagnostics page (`ms-todo doctor` in the TUI).
- The Homebrew formula, linking `mst` too (D-039).

**New unknowns:** the ClientSeed-plus-events model under a day of real use.

**Demo:** `brew install`, then `ms-todo tui`, then a morning's worth of tasks.

**Done when:**

- On a clean machine, `brew install` (or `install.sh`) followed by `ms-todo auth login` and `ms-todo tui` works.
- BK uses it for a day and every change made in the TUI shows up on the phone.
- The latency budget still holds, measured the same way as in 5a.

**As built (2026-09-25, D-044):** editing, multi-select, the palette and the diagnostics page are driven live on the `livetest` instance: a title, due date, reminder, importance and notes edited in the TUI read back from Graph with `raw GET`, two tasks completed together as one command and then deleted together. `mst` alone opens the TUI in a terminal. The latency budget holds: cold start 5.7 ms, keypress median 1.1 ms (p95 2.3 ms), view switch p95 5.9 ms with `--bench-startup`; in the live session, 383 keypresses p95 0.9 ms and writes drawn p95 4.1 ms. **Still to check:** `brew install` on a clean machine, which needs the first release with the formula (the release workflow pushes it to the tap), and BK's day of use with the phone.

**Left out:** quick-add parsing and live highlighting, the My Day and Assigned views, folders in the sidebar, a multi-line notes editor (existing line breaks are kept, new ones can't be typed yet), and the steps editor.

### Rung 5c: Motorbike with panniers: folders

Pulled forward from 8d at BK's request (D-047): his To Do app groups lists PARA-style (Projects, Areas, Someday / Maybe…), and ms-todo showed them ungrouped.

**Promise:** "My lists are grouped into folders in ms-todo, like in the To Do app, and it syncs across my ms-todo machines."

**Build:**

- Folders and ordering in the list extension (`folder`, `order`, `folderOrder`), written through the outbox as a new operation kind: GET, merge, write the whole document; offline-safe and undoable ([05](05-custom-features.md#folders-list-groups)).
- `lists move L... --folder F | --no-folder`, `lists order`, `folders list|rename|delete|order` ([07](07-cli.md)).
- The TUI sidebar's collapsible folders and "Move list to folder…" ([08](08-tui.md)).
- Folder changes from another machine arrive with lists delta and the lists enumeration, which already carries the extension (D-036).

**Demo:** `mst lists move Finances --folder Areas`, `mst folders list`, then `mst` shows a collapsible **Areas** folder. Rename and delete it; delete never deletes a list.

**Done when:**

- A folder set in the CLI shows in the TUI sidebar, collapses and expands, and shows on another ms-todo machine after its next sync.
- Rename and delete patch every list in the folder, and no list is ever deleted; `undo` reverses each.
- The latency budget still holds.

**As built (2026-09-25, D-047):** driven live on the `livetest` instance on the spike list only: moved into `ms-todo-test` (a POST when the list had no extension, a PATCH that kept an unrelated field when it had one, both read back with `raw GET`), renamed, undone, deleted (the list stayed), a folder set by a raw write arrived with `sync --wait`, and `--no-folder` on a document left empty deleted the extension. The spike list is left with no folder. Latency with `--bench-startup`: keypress p95 about 1 ms and view switch p95 5–7 ms, as in 0.1.12; with the spike list in a folder the scripted path steps onto that uncached list and its seed takes p95 14–19 ms, the uncached-list round trip D-043 measured at 5–30 ms.

**Left out:** `lists create --folder` (with `lists create`, 8e), moving a list by keys in the TUI (only by typing a folder name), and ordering in the TUI (the CLI's `lists order` and `folders order`).

### Rung 5d: Motorbike with a logbook: what I finished, and clearing what's overdue

BK's research pick, trimmed to what's worth building (D-048): a logbook of what was finished (Things' Logbook; the standup question on Hacker News), and a way out of the overdue pile that doesn't take one edit per task. Plain code, no AI.

**Promise:** "I can see what I finished yesterday for my standup, and clear a pile of overdue tasks in one command, then undo it."

**Build:**

- `done [--since W] [--until W] [--list L | --folder F] [--limit N]`: completed tasks from the cache, by local day, newest first; every format, with `completed_on` in JSON and CSV. `--since` and `--until` read phrases looking back (`mon` is the latest Monday), a new mode of `crates/nlp`.
- `reschedule [--overdue | --due-before W | TASK... | -] --to W [--list L | --folder F] [--dry-run] [--yes]`, and `tasks edit` over the same selection or several IDs, for `--due`, `--importance` and `--reminder`: one command of one outbox operation per task.
- Undo per task for a change to several: tasks changed since are left alone and reported.
- TUI: the Completed view grouped by day; "Set due date…" (`S`) for the selection and "Reschedule overdue to…" (`R`), both in the palette.

**Unknowns:** whether `completedDateTime` is always midnight UTC of the day (S12 saw 418 of 420); how Graph takes back a due date it gave (it keeps the date part of what it's sent, S11).

**Demo:** `mst done --since yesterday`, `mst done --since mon --list Work --format csv`, `mst reschedule --overdue --to today`, `mst undo`.

**Done when:**

- `done` shows a task completed today under Today, on the day Graph keeps, in every format.
- `reschedule --overdue` previews, asks (or needs `--yes` off a terminal), moves every overdue task, and one `undo` puts every one back, as `raw GET` shows; a task changed since is left alone and named.
- The latency budget still holds.

**As built (2026-09-25, D-048):** driven live on the `livetest` instance on the spike list only: three tasks due yesterday; `reschedule --overdue --list ms-todo-spike-2026-09-24 --to tomorrow` planned exactly those 3, refused without `--yes` off a terminal (exit 2), and with it moved all three (`raw GET`: `2026-09-25T23:00Z`, midnight London on the 26th). The first `undo` found a bug that predates 5d: undo sent back the UTC instant Graph had given (`2026-09-23T23:00Z`), Graph kept only its date part, and every task landed a day early (the 23rd). Fixed: a date is put back as its local day's midnight in the user's zone, unless it's already midnight in its own. After the fix, `undo` restored all three exactly (`2026-09-23T23:00Z`); a second batch with one task moved again undid two and reported the third in `refused`. A bulk `tasks edit - --due yesterday --yes` from stdin moved all three. Completing one showed it in `done --since today --list …` at once as "Not synced yet" (`completed_on` null), then, once sent, under Today with `completed_on` 2026-09-25; Graph held `completedDateTime` `2026-09-25T00:00:00Z` (completed at 03:29 BST). The test tasks were deleted; the spike list is back to 31 tasks. Latency with `--bench-startup`: cold start 5.4 ms, keypress p95 0.2 ms, view switch p95 3.7 ms.

**Left out:** a `--where` expression language (the selection is `--overdue` or `--due-before`, as `tasks list`'s planned `--due` filter reads); clearing several due dates at once in the TUI; the time of day a task was completed, which Graph doesn't keep; per-day totals and "done this week" summaries beyond what `done --format json` gives an agent.

### Rung 5e: Motorbike with a trailer: move tasks between lists

BK pulled `tasks move` forward from 8c (D-051). It's the one operation that can lose data, so it's built as 05 and 04 describe it and checked against the real API first (S14).

**Promise:** "I can move a task, or several, to another list without losing anything, and undo it."

**Build:**

- The resumable move job ([05](05-custom-features.md#move-between-lists), [04](04-sync-cache.md#instant-local-writes)): one outbox operation per task with saved steps (migration `0005`): read the source and its attachments, create the copy with its children and our extension in one POST, add the attachments, check the copy and the target list, delete the source. Roll back before the delete, pause on an unknown outcome, and 04's four cases after a crash in the delete.
- `tasks move T… --to L [--list L] [--dry-run] [--yes]`, undo through the same job, `outbox retry` and `outbox discard` for paused moves.
- TUI: `m` "Move to list…" for the cursor's task or the selection, with a picker that shows folders.

**Unknowns:** what a copy keeps (S14: everything but `createdDateTime`), and how Graph takes a recurring task's dates back (S14: in the recurrence's zone).

**Demo:** `mst tasks move <ID> --to Groceries`, then `mst tasks list --list Groceries` and `mst undo`; in the TUI, `v` two tasks, `m`, type `groc`, Enter.

**Done when:**

- A task with steps, a link, attachments and our extension moves with all of them, byte for byte (sha256), the source is gone, and the task keeps its local ID; `undo` moves it back.
- A failure at any step before the delete leaves the source untouched; a crash at any step resumes or rolls back; nothing is deleted on a guess.

**As built (2026-09-25, D-051):** S14 first, on the spike list and a throwaway target list: the copy's fields, checked and unchecked steps, link and extension all come back from one POST; attachments' bytes match; `createdDateTime` doesn't survive (kept as `originalCreatedAt`); a task takes one link; upload-session bytes go to `<uploadUrl>/content`; a recurring task's dates must be written in its recurrence's zone. Then driven live on the `livetest` instance: a weekly recurring task with notes, due and start dates, a reminder, a category, three steps (one checked), a link, our extension and two files (88 bytes, and 4.2 MB through an upload session) moved to the target list in one attempt; `raw GET` of the source was 404, the copy held every field and child with the move's `opId` and `originalCreatedAt`, and both files' sha256 matched; the cache held it once, same local ID. `undo` moved it back the same way. A bulk move of two tasks exited 2 without `--yes` and moved both with it, one command. The test tasks and the target list were deleted; the spike list is back to 31 tasks. Fake-Graph tests cover every step's failure, every crash point and the four restart cases.

**Left out:** moving a whole list, moving tasks picked by `--overdue` or `--due-before`, and attributing a paused attachment upload (Graph has no marker for one).

## Rung 6: Car: quick add

Split into **6a**, the deterministic quick add below, and **6b**, where Jev suggests the list for a task captured into the inbox (D-053: suggest-only and opt-in, replacing "files it by itself"). Both are built (D-052, D-053).

**Previously:** a CLI and TUI that take task text literally. **Now:** the same, plus Todoist-style quick add in both.

**Promise:** "I type a task the way I think it, and ms-todo files it correctly."

**Build:**

- `crates/nlp` ([06](06-natural-language.md)): the rule-table date scanner (D-026), the token grammar and the recurrence grammar. The scanner also reads `--due`, `--start` and `--reminder`. Its whole-string mode, which reads `--due`, `--reminder` and the TUI's date fields, shipped early in the editing fix (D-045); rung 6 adds span mode.
- Quick add in the CLI (parsing by default, `--no-parse`, `tasks parse`) and in the TUI, with live highlighting.
- `@label` applies categories and creates a missing one on request (`--create-categories`).

**New unknowns:** the scanner against BK's own phrases (Q10); the recurrence grammar against Graph's `patternedRecurrence`.

**Demo:** `ms-todo tasks add "Pay rent every 1st #Home p1 !9am"`, then look at the phone.

**Done when:**

- `ms-todo tasks add "Pay rent every 1st #Home p1 !9am"` creates the right task, and the phone shows its recurrence, list, importance and reminder correctly.
- The phrase corpus passes.

**Left out:** My Day (the `+myday` token arrives in rung 7), folders, assignment.

**As built, 6a (2026-09-25, D-052):** span mode over the date rule table and ordered masking passes in `crates/nlp` (`quick_add/`, `recurrence.rs`, `dates/span.rs`); `tasks add` parses by default, `--no-parse`, `--create-categories` and `tasks parse` in the CLI; the TUI's `a` is a centred modal with live highlighting, a preview line, Tab completion and `Ctrl-r`. The S8 corpus reads 105 of 127 phrases as graded inside titles (22 known misses, listed in the tests with reasons); 44 representative inputs and 26 recurrences are snapshotted; proptest checks that parsing never panics and that the title plus the spans cover the input. A parse takes about 20 µs, a TUI keypress with it about 50 µs (release), and `--bench-startup` against the live cache gave a keypress p95 of 0.71 ms with quick-add typing in the script. Driven live on the `livetest` instance, spike list only: `Pay rent every 1st #ms-todo-spike p1 9am` came back importance high, due 1 Oct (London midnight), reminder 09:00 London, `absoluteMonthly` day 1 from 2026-10-01 (sent with `recurrenceTimeZone: Europe/London`; a plain GET reports `UTC`, and the due date didn't move); `Call mum in 2 days` came back "Call mum", due 27 Sep, no reminder; a `--no-parse` add kept its title whole with no fields. The three tasks were deleted; the spike list is back to 31. Not driven live: `--create-categories` (it would write outside the spike list), tested against the fake Graph only.

**6b, list suggestions (2026-09-25, D-053).** *Promise:* "When I capture a task into the inbox, ms-todo tells me which list it probably belongs in, and one key files it there." Opt-in through `[suggest]` in config.toml; the daemon asks TypeSafe's Jev (`crates/daemon/src/suggest/`: config, the key from `TYPESAFE_API_KEY` or `api_key_command` run without a shell, the criteria from the cache per sync generation, the client with its 3-second deadline and 429/529 backoff). `SuggestList` (protocol 11) is answered out of order on its connection. CLI: `tasks suggest-list`, the `note:` after an inbox `tasks add`, and `suggest` in `doctor`. TUI: the modal's `→ List? (Ctrl-l to accept)` hint after a pause in typing. Tests: wiremock for TypeSafe (above and below the threshold, 429 and 529 retried, 500 and a timeout as none), criteria building, config and key-command parsing, the debounce and accept in `update`, the modal's frame, and the CLI end to end against a mock TypeSafe, including a slow suggestion not holding up a `Status` on the same connection. **Driven live** on the `livetest` instance (BK's 30 lists; key from `TYPESAFE_API_KEY`), no task written: "pay council tax" → Finances, "watch Dune Part Two" → Movies, "try the new ramen place in Soho" → Restaurants to try, "read Designing Data-Intensive Applications" → Reading, "practise binary tree problems for interviews" → Coding Interviews, each at confidence 1.00 in about 0.25 s (0.66 s for the first, which built the options); "call John" and "fix the thing" gave no suggestion. The `api_key_command` form with `op read` timed out when the detached daemon ran it (no way to approve), which is why the key came from the environment. **Left out:** the palette's inbox triage.

## Rung 7: Convertible: My Day

**Previously:** quick add in a CLI and TUI. **Now:** the same, plus My Day, kept in our extension and mirrored on the phone through the due date (D-037).

**Promise:** "I plan today in ms-todo, and the phone shows the same plan."

**Build:**

- My Day ([05](05-custom-features.md#my-day)): the extension date, the due-date mirror (`myDayDueSet`), the rollover, suggestions, the My Day view in the TUI, `myday` in the CLI, the `--my-day` flag, the `+myday` quick-add token, and a `doctor` note that the phone mirror depends on an app setting Graph can't show.

**New unknowns:** the rollover across daemon restarts, two ms-todo installs rolling over the same tasks, and how the app treats a due date removed after it put the task in its My Day.

**Demo:** add a task with no due date to My Day, see it in the phone's My Day, finish it or leave it, and after the rollover the leftover has no due date again.

**Done when:**

- A task with no due date added to My Day in ms-todo shows in the phone's My Day (with "Show 'Due Today' tasks in My Day" on).
- After the rollover, an unfinished task whose due date ms-todo set has no due date and is out of ms-todo's My Day; a task with a due date the user set keeps it.
- A task added to My Day on one ms-todo instance shows in `ms-todo myday` on another after its next sync.

**Left out:** the rest of the API surface (rung 8).

**As built (2026-09-25, D-054):** My Day in our extension (`myDay`, `myDayDueSet`) through a new outbox operation, `task_extension`, with the due-date mirror as a due-date edit behind it; `myday list|add|remove|suggest|rollover [--dry-run]`, `tasks list --my-day`, `tasks add --my-day` and `+myday` / `*` in the CLI; the daily rollover on the daemon's minute tick with `[my_day] rollover_time`; suggestions; the My Day view first in the TUI's sidebar with its day, Suggestions and `t`; `doctor`'s `my_day` with the phone note. Protocol 12, migration `0006` (settings, and an index on `myDay`). Tests: the per-task rules and the rollover's plan (unit), the store's merge and rollback, and the CLI end to end against the fake Graph (add and remove with and without a due date, a due date changed since kept, the catch-up rollover over a set, a user's and a completed task, idempotency, a dry run, a task put back elsewhere left there, undo of add, remove and rollover with a refusal, suggestions, quick add, `doctor` and a bad `rollover_time`), the TUI's view, toggle, add and a snapshot. Latency on the demo data: cold start 6.9 ms, keypress p95 2.0 ms, view switch p95 8.1 ms. **Driven live** on the `livetest` instance, spike list only (one spike task, from S2, was the only task on the account with `myDay`, checked in the cache before anything ran): the daemon's first start caught up and rolled that task out (its `assignee` kept); two spike tasks were added (the one with no due date got `myDay`, `myDayDueSet` and a due date on My Day's day; the one with a due date kept it), one removed (its extension deleted, its due date kept), a `--dry-run` showed two tasks and one due date to clear, the rollover cleared exactly that, a second run found nothing, and `undo` put it back; every spike task was then restored to its state before (the S2 task's `myDay` by undoing the first rollover) and the instance deleted. `rollover_time` was forced through a config in a scratch `MS_TODO_CONFIG_DIR`. **Not verified by us:** the phone's My Day showing the task (BK's check, with "Show 'Due Today' tasks in My Day" on).

## Rung 8: Rocket: the rest of the API, one capability per release

**Previously:** a daily-use CLI and TUI with quick add and My Day. **Now:** the same, plus the rest of the Graph To Do surface, one release at a time. Each sub-rung is its own release with its own demo.

- **8a: steps and links.** Promise: "I can break a task into steps and attach links." Demo: add three steps in the CLI, tick one on the phone, see it ticked in the TUI. `steps` and `links` in the CLI and the detail pane. A checklist-item PATCH always includes `isChecked` ([02](02-data-model.md#outbox-semantics)). **Done when:** steps and links added in ms-todo show on the phone, and a step checked on the phone shows as checked in ms-todo.
- **8b: attachments.** Promise: "I can attach files to a task and get them back." Demo: attach a PDF in the CLI, open it on the phone, download it back. Direct upload, upload sessions, download (safe filenames, `.part` then rename, 0600), and the final-chunk `unknown` rule ([03](03-graph-provider.md#endpoints-the-whole-surface)). **Done when:** a 10 MB attachment uploads and downloads with identical bytes (compare the sha256), and it opens on the phone.
- **8c: `tasks move`.** Moved to rung 5e (D-051).
- **8d: assignment.** Promise: "I can track who I'm waiting on." Demo: assign a task, then see it in the TUI's Assigned view. Assignment (`--assignee`) and the Assigned view; folders moved to rung 5c (D-047). **Done when:** `tasks list --assignee` returns the right tasks.
- **8e: categories, extensions, lists and the remaining task fields.** Promise: "Anything the To Do API can do, ms-todo can do." Demo: create a list, recolour a category, and set a start date and a recurrence from the CLI. `categories list|create|recolor|delete`, `extensions`, `lists create|rename|delete`, start dates and recurrence editing. **Done when:** every command in [07](07-cli.md) is implemented, has wiremock tests, and has run live against a throwaway list, behind a feature-gated live smoke test.

**Left out:** the items under "Deferred".

## Deferred (not in v1)

- An optional local-LLM parser behind `QuickAddParser` (D-016).
- Semantic search, behind the same `search` command (D-042).
- Windows support.
