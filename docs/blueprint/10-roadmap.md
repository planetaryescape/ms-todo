# 10: Roadmap

## Build philosophy

Skateboard → scooter → bicycle → car (vault: `Skateboard MVP`). The roadmap is a ladder of **usable releases**, not a list of components. Each rung is a working, installable product that lets BK do something new from start to finish, and it's sized to fit one working session (vault: `Shippable Increment Per Session`). lazydap's roadmap follows the same rule (`~/code/planetaryescape/lazydap/docs/blueprint/14-roadmap.md`, "Build philosophy").

- **Every rung is released and installable.** It goes out through release-please as a GitHub release, and installs with `install.sh`. From rung 5 it installs with Homebrew as well.
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

**Previously:** a CLI with instant, offline-safe reads and writes. **Now:** the same, plus a keyboard TUI BK can live in, installable with Homebrew.

**Promise:** "I can run my day from `ms-todo tui`."

**Build:**

- The TUI over the protocol ([08](08-tui.md)), seeded from the daemon's snapshot: the sidebar, smart views (Important, Planned, All, Completed), the task list, the detail pane, literal add, complete, editing the title, due date and importance, filter, multi-select, undo, the palette, the hint bar, sync markers and the diagnostics page.
- The latency budget, measured through tracing.
- The Homebrew formula.

**New unknowns:** the ClientSeed-plus-events model under real use; meeting the latency budget over IPC.

**Demo:** `brew install`, then `ms-todo tui`, then a morning's worth of tasks.

**Done when:**

- On a clean machine, `brew install` (or `install.sh`) followed by `ms-todo auth login` and `ms-todo tui` works.
- BK uses it for a day and every change made in the TUI shows up on the phone.
- The measured latencies meet [08](08-tui.md)'s budget: under 16 ms per keypress and under 150 ms for a cold start.

**Left out:** quick-add parsing and live highlighting, the My Day and Assigned views, folders in the sidebar.

## Rung 6: Car: quick add

**Previously:** a CLI and TUI that take task text literally. **Now:** the same, plus Todoist-style quick add in both.

**Promise:** "I type a task the way I think it, and ms-todo files it correctly."

**Build:**

- `crates/nlp` ([06](06-natural-language.md)): the rule-table date scanner (D-026), the token grammar and the recurrence grammar. The scanner also reads `--due`, `--start` and `--reminder`.
- Quick add in the CLI (parsing by default, `--no-parse`, `tasks parse`) and in the TUI, with live highlighting.
- `@label` applies categories and creates a missing one on request (`--create-categories`).

**New unknowns:** the scanner against BK's own phrases (Q10); the recurrence grammar against Graph's `patternedRecurrence`.

**Demo:** `ms-todo tasks add "Pay rent every 1st #Home p1 !9am"`, then look at the phone.

**Done when:**

- `ms-todo tasks add "Pay rent every 1st #Home p1 !9am"` creates the right task, and the phone shows its recurrence, list, importance and reminder correctly.
- The phrase corpus passes.

**Left out:** My Day (the `+myday` token arrives in rung 7), folders, assignment.

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

## Rung 8: Rocket: the rest of the API, one capability per release

**Previously:** a daily-use CLI and TUI with quick add and My Day. **Now:** the same, plus the rest of the Graph To Do surface, one release at a time. Each sub-rung is its own release with its own demo.

- **8a: steps and links.** Promise: "I can break a task into steps and attach links." Demo: add three steps in the CLI, tick one on the phone, see it ticked in the TUI. `steps` and `links` in the CLI and the detail pane. A checklist-item PATCH always includes `isChecked` ([02](02-data-model.md#outbox-semantics)). **Done when:** steps and links added in ms-todo show on the phone, and a step checked on the phone shows as checked in ms-todo.
- **8b: attachments.** Promise: "I can attach files to a task and get them back." Demo: attach a PDF in the CLI, open it on the phone, download it back. Direct upload, upload sessions, download (safe filenames, `.part` then rename, 0600), and the final-chunk `unknown` rule ([03](03-graph-provider.md#endpoints-the-whole-surface)). **Done when:** a 10 MB attachment uploads and downloads with identical bytes (compare the sha256), and it opens on the phone.
- **8c: `tasks move`.** Promise: "I can move a task between lists without losing anything." Demo: move a task with steps and an attachment, then `show` it in the new list. The resumable move job ([05](05-custom-features.md#move-between-lists)). **Done when:** `tasks move` keeps the steps, links, attachments and extension (check with `show` before and after), and an agent session using only the skill moves a task with the right exit codes.
- **8d: folders and assignment.** Promise: "I can group lists into folders and track who I'm waiting on." Demo: put two lists in a folder and assign a task, then see both in the TUI. List folders and ordering (`folders`, `lists move|order`), the folder sidebar in the TUI, assignment (`--assignee`) and the Assigned view. **Done when:** a folder created in the CLI shows in the TUI sidebar, and `tasks list --assignee` returns the right tasks.
- **8e: categories, extensions, lists and the remaining task fields.** Promise: "Anything the To Do API can do, ms-todo can do." Demo: create a list, recolour a category, and set a start date and a recurrence from the CLI. `categories list|create|recolor|delete`, `extensions`, `lists create|rename|delete`, start dates and recurrence editing. **Done when:** every command in [07](07-cli.md) is implemented, has wiremock tests, and has run live against a throwaway list, behind a feature-gated live smoke test.

**Left out:** the items under "Deferred".

## Deferred (not in v1)

- An optional local-LLM parser behind `QuickAddParser` (D-016).
- Semantic search, behind the same `search` command (D-042).
- Windows support.
