# 11: Decision log

Every decision from the planning session on 2026-09-24, **including the options we rejected and the reversals**, so a later agent understands how the design got here and doesn't argue its way back into an option we already rejected. BK is the decision-maker. Entries marked "BK" are his calls. Change a decision only by adding a new entry that replaces the old one.

## The arc, briefly

1. BK asked whether Microsoft To Do has a usable API. Research: yes. Graph v1.0 covers nearly everything, sign-in has to be delegated, and the old Outlook Tasks API is dead.
2. We asked whether to build or adopt. We first leaned towards **adopting an existing CLI and wrapping it in a skill**, reasoning that sign-in was the hard part and someone had already written it.
3. A code review of the best Python CLI found silent truncation, crashes on errors and a client-secret sign-in flow. We then leaned towards the TypeScript MCP server. Its review found a third-party client ID, a lossy move and the hidden device-code hang.
4. **BK changed the premise:** a coding agent does the build, so code is cheap. Don't accept a tool's flaws to avoid building one. That's now a global rule in BK's AGENTS.md. So: build our own.
5. BK pointed at his Rust projects mxr and spotuify. A study of both showed **mxr already has a working Microsoft device-code sign-in**, so Rust won.
6. The scope grew to the full API surface, a cache for a very fast TUI, and features the API lacks built on top: My Day, folders and assignment. Sharing and task assignment were first included, then sharing was dropped.
7. We proposed no daemon, then reversed that when BK asked what it was optimising for.
8. My Day went from a special list, to copies of tasks, to an Outlook category, and then, when the phone turned out not to show categories, to a date in our extension, mirrored on the phone through the due date (D-037).
9. Todoist-style natural language: deterministic, with the LLM deferred.
10. BK will build on a different machine, so this blueprint exists to carry the whole session.

## Decisions

### D-001: Build our own; don't adopt an existing tool. (BK)
- **Options rejected:** (a) wrap underwear/microsoft-todo-cli in a skill; (b) use MAG-Cie/mcp-microsoft-todo as an MCP server, configured with our own client ID.
- **Why:** (a) truncates lists silently past 100 tasks, crashes with a traceback on Graph errors, uses a client-secret flow where you paste the redirect URL, and looks things up by name. (b) uses a third-party client ID by default, has a lossy `move_task`, hangs silently on device code, carries the context cost of 28 MCP tools, and is MCP at all (see D-008). Details and commit SHAs are in `docs/research/prior-art.md`.
- **Principle (BK):** "Writing code is relatively cheap. No need to needlessly take on disadvantages." Recorded globally as: *weigh a tool's flaws against ongoing maintenance, not against initial build time.* A To Do client needs little maintenance once it's built.

### D-002: Rust, following mxr and spotuify. (BK proposed; agreed)
- **Option rejected:** TypeScript on bun, BK's default for new projects.
- **Why Rust:** mxr's `crates/provider-outlook/src/auth.rs` already implements Microsoft device-code sign-in, so the hard part is adapted rather than written. spotuify shows the copy-from-mxr approach works (rate limiting, output, exit codes, IPC). It uses the same release setup (release-please, Homebrew, `install.sh`). ratatui is the natural TUI. It ships as one static binary.
- **What TypeScript would have given:** the MAG&Cie code could have been adapted directly. That stopped mattering once mxr turned out to have the equivalent.

### D-003: Cover the whole API, plus the features the app has that the API lacks. (BK)
- BK doesn't know yet exactly how he'll use it, so every capability should be ready. The To Do domain is small, so this is bounded.
- We noted the counter-principle ("don't ship features just because you can"). BK decided deliberately.

### D-004: Device-code sign-in, BK's own app registration, `/common`, no secret.
- **Options rejected:** a client-secret web flow (the Python CLI; the secret expires and has to be stored); PKCE with a loopback listener (works, but needs a local port and a browser, and device code suits a terminal and SSH); a third-party client ID (MAG&Cie; a trust and availability risk); mxr's `consumers`/`organizations` split (`/common` covers both with one registration).

### D-005: A local SQLite cache is the read path, with instant local writes. (BK: "I want the cache that will allow the TUI to be blazing fast.")
- **Option rejected:** reading straight from Graph, which puts network latency on every keypress.

### D-006: Have a daemon. (Replaces an earlier "no daemon" proposal.)
- **Earlier proposal:** no daemon. The TUI syncs itself, CLI commands do a quick delta before reading, and SQLite WAL handles several processes.
- **BK asked:** "Are you sure we should skip the daemon? What are you optimising for?" The honest answer was *fewer moving parts*, which is the same "code is expensive" bias D-001 already rejected.
- **Why a daemon:** fast CLI and agent reads; the outbox has to keep going after the TUI exits; Microsoft replaces refresh tokens on use, so only one process should refresh; events can be pushed; the My Day rollover needs scheduling. mxr and spotuify already solved the daemon's costs.

### D-007: Delta polling; no webhooks.
- **Option rejected:** Graph change notifications (`todoTask` subscriptions exist in v1.0, lasting up to about 3 days). They need a public HTTPS endpoint, which isn't worth it for a local tool when delta gives the same freshness.

### D-008: No MCP server. (BK)
- BK: "I'm personally not a fan of MCP servers, especially when the agent has access to a full CLI that it can control fully. If you want an MCP, you'd have to convince me why we need it."
- The only argument for one was clients without a shell (Claude Desktop, mobile). BK doesn't need those. The CLI plus a skill covers agents, and it costs context only when used.
- Revisit only if a real client without a shell needs it.

### D-009: The CLI is the canonical surface; every feature is in the CLI. (Taken from spotuify.)

### D-010: Crates kept to about 10, not 18–28.
- mxr and spotuify are big because of mail and playback. Borrow their patterns, not their size.

### D-011: SQLite FTS5 for search, not Tantivy.
- Task counts are in the thousands at most. FTS5 lives inside the same database, with no second index to keep in sync or unlock. That avoids the Tantivy lockfile problems noted in spotuify's AGENTS.md.

### D-012: The token goes in a 0600 file, not the macOS Keychain.
- Follows mxr's Outlook provider and spotuify. Keychain access from a background daemon can prompt the user or fail when nobody is at the machine (mxr's Gmail path needed a disk fallback for exactly this). A token file is simpler and fits a local-first design. Revisit if BK wants the Keychain.

### D-013: The build happens on another machine; this blueprint carries the session. (BK)
- **Option rejected:** exporting the session transcript. It's local, noisy, and full of claims that were later reversed. A refined blueprint with a decision log is what the next agent actually needs.

### D-014: Don't build sharing. (BK: "we can skip sharing, I don't share my todos with anyone anyway.")
- We had analysed two options: (1) use lists already shared through the official app (`Tasks.ReadWrite.Shared`, `isShared`), or (2) build our own sharing backend, which would be a product with hosting and security costs, not a CLI feature.
- Result: neither. We still display `isShared` and `isOwner`.

### D-015: My Day is an Outlook category plus an extension date. (BK approved)
- **Option 1, rejected:** a special "My Day" list whose tasks are moved in and out. Graph has no move operation; recreating a task loses its ID, steps, attachments and created date.
- **Option 2, rejected (BK):** copies of tasks in a "My Day" list, linked to the originals. BK: "I don't want the double items thing." The phone would show every task twice.
- **Option 3, rejected:** an extension only. The phone couldn't see it.
- **Chosen:** the category "My Day" (visible and filterable on the phone; `categories` is a first-class property, so delta sync definitely picks it up), plus `myDay: date` in the ms-todo extension, and a daily rollover in the daemon. The task never leaves its list. Cost: `MailboxSettings.ReadWrite`, which we need anyway for `@label`.
- The app's own My Day can't be reached through the API and is left alone.
- Note (2026-09-24): the phone-visibility premise is unverified pending the S7 phone check (see D-030).
- **Replaced by D-037:** the phone shows no categories, so the category is dropped. My Day lives in our extension and reaches the phone through the due date.

### D-016: Deterministic natural-language parsing; the LLM is deferred. (BK: "leave llm parsing for now")
- BK wanted Todoist-style quick add: deterministic first, then maybe an optional small local LLM for richer parsing.
- **Deferred because:** build the deterministic parser, see where it actually falls short, and then decide. The `QuickAddParser` trait keeps room for an LLM backend (Ollama or llama.cpp with a fixed JSON output format) at no cost now.

### D-017: Todoist p1–p4 maps to Graph's three importance levels.
- p1 = high, p2 and p3 = normal, p4 = low. That's lossy: Graph only has `low`, `normal` and `high`. Storing the original p-level in the extension is possible if BK wants the distinction (Q3).

### D-018: Parse by default, `--no-parse` for agents, explicit flags win.
- This prevents "Email Friday's report" from getting a due date of Friday when an agent adds it.

### D-019: Moving a task is a full copy that checks itself before deleting.
- **Option rejected:** MAG&Cie's recreate-then-delete, which loses children and leaves duplicates on failure.

### D-020: Folders go in list extensions; assignment goes in task extensions.
- Invisible to the official apps, which is accepted. Assignment pairs with the real `waitingOnOthers` status, which is visible.

### D-021: Name `ms-todo`, repo `planetaryescape/ms-todo`, public, dual MIT and Apache-2.0 like mxr. (BK)
- An unrelated tiny `visionik/mstodo` exists. Its name is close but doesn't clash.

### D-022: Quick add goes to "Tasks" by default; you can name a list. (BK)
- With no `#List` in the text and no `--list` flag, the task goes to the built-in "Tasks" list (`wellknownListName = defaultList`), in both the CLI and the TUI. `#List` and `--list` always override that.
- **Option rejected:** defaulting to whichever list is selected in the TUI. The TUI and the CLI would behave differently.

### D-023: No location support. (BK: "I've never really needed to put location to my tasks")
- The API has no location field or location reminders anyway. We won't store location text in the extension, and the parser has no location rule.

### D-024: The My Day rollover is at midnight, configurable. (BK)
- The default is 00:00 local time. `my_day.rollover_time = "HH:MM"` in config.toml changes it, for example "04:00" for late nights.

### D-025: Bundle BK's client ID, and encourage users to use their own. (BK)
- Release builds bake in BK's Entra client ID with `option_env!("MS_TODO_CLIENT_ID")` (mxr's `BUNDLED_CLIENT_ID` pattern), so `brew install` then `ms-todo auth login` works immediately. The ID isn't a secret.
- The README, `docs/`, and the first run of `ms-todo auth login` recommend registering your own app, with a short guide, and explain why: with the bundled ID, users consent to BK's app registration, and its availability depends on BK. `auth.client_id` in config or `MS_TODO_CLIENT_ID` overrides the bundled ID. `ms-todo auth status` shows which one is in use.


### D-026: Dates are read by a custom rule-table scanner, not a library. (Phase 0, from S8)
- **Replaces:** the plan in [06](06-natural-language.md) to use `clockwords`, with `interim` as a fallback. That plan had no entry of its own here. D-016 (deterministic, LLM deferred) still stands.
- **Options rejected:** (a) `clockwords` 0.4.0, 33% of 127 graded phrases: no absolute dates and no bare weekdays. (b) `interim` 0.2.1, 24–39%: whole-string only with no spans, matches words on three-letter prefixes (`Monitor` → Monday), doesn't roll dates forward. (c) `chrono-english` 0.2.1, 35%: the same flaws as `interim`, and no bare times. (d) `whichtime-sys` 0.1.0, the best at 53%: needs `chrono::Local`, reads weekdays towards the past, and still takes `Sat nav` as a date. (e) A library plus patch rules: the missing pieces are the core of the job (absolute dates, spans, leaving titles alone), not edge cases.
- **Why:** finding a date inside a task title without eating the title is narrow, well specified and easy to test with a corpus, and no crate does it. A scanner of roughly 500–700 lines, with ordered masking passes, serves quick add, `!`, `start` and the date flags alike. Evidence: [S8](../research/spikes/S8.md).

### D-027: Due and start dates are dates only; a time goes to the reminder. (Phase 0, from S11)
- **Replaces:** the `nlp.time_sets_reminder` setting in [06](06-natural-language.md), which is dropped. It had no entry of its own here.
- **Options rejected:** (a) keeping the setting with `false` as an option. Graph throws away the time on `dueDateTime` and `startDateTime`, so `false` would silently lose the time. (b) Storing the time in our extension. The phone couldn't see it, and nothing would remind BK.
- **Why:** a reminder is the only place Graph keeps a time. So "tomorrow 5pm" sets the due date to tomorrow and a reminder at 17:00. The cache stores due and start as local dates and reads them by rounding to the nearest local midnight, because BK's data holds due dates written as midnight in several zones ([02](02-data-model.md#principles)). `start` without a due date also sets the due date, so the parser warns. Evidence: [S11](../research/spikes/S11.md).

### D-028: The outbox has an "unknown outcome" state. (Phase 0, from the reuse-map check)
- **Options rejected:** (a) treating every failure as temporary or permanent, as [04](04-sync-cache.md) first did. A create POST that times out or gets a 5xx after Graph accepted it would be retried and make a duplicate task. (b) An idempotency key. Graph To Do doesn't support one. (c) Never retrying creates. An offline or throttled create would then need a person to retry it.
- **Why:** state `unknown` means "don't resend". The HTTP client never retries a create or a recurring completion automatically after a timeout, 5xx or 408; it retries only on a 429, a 401 then refresh, or a connection failure before sending. Operations still `inflight` at daemon start become `unknown` too. The rule is to adopt only a result we can attribute: a task create carries its `op_id` as `opId` in our extension, inline in the POST (S13), so each sync round's lookup can match it exactly, for up to 24 hours. A recurring completion stays `unknown` for the user even if a GET shows the due date moved, because the phone could have done that; `outbox list` shows what was seen. Checklist-item and linked-resource creates are never matched by content. Anything not attributed goes to the user (`outbox retry` or `outbox discard`); it's never re-sent or deleted automatically. A duplicate `opId` seen in sync raises `DuplicateDetected` for the user. The invariant: not finding something never allows a replay or a delete. This follows mxr's `plans/014-send-outcome-recovery.md` and `plans/023-interrupted-mutation-jobs.md`. Details: [04](04-sync-cache.md#unknown-outcome-d-028).

### D-029: Task children sync through delta alone; extension content is fetched after each round. (Phase 0, from S1 and S2)
- **Settles:** the open choice between strategies (a) and (b) that [04](04-sync-cache.md) left to S1 and S2.
- **Options rejected:** (a) as first written: `$expand=checklistItems,extensions` on delta. Checklist items are inline anyway, and delta never returns extensions, with or without `$expand`. (b) Re-fetching children in batches for every changed task, plus a slow background sweep. Not needed: every child change bumps the parent, so delta never misses one.
- **Why:** checklist items and linked resources arrive inline in delta, attachments flip `hasAttachments`, and extension changes bump the parent without their content. So delta finds every change, and the only extra work is a filtered-`$expand` fetch of extension content after each round. Details: [04](04-sync-cache.md#children-of-a-task). Evidence: [S1](../research/spikes/S1.md), [S2](../research/spikes/S2.md).

### D-030: The My Day category defaults to `preset3` (Yellow). (Proposed; pending BK's phone check)
- **Replaces:** the `preset4` default in [05](05-custom-features.md), which was chosen as "yellowish". Microsoft's mapping makes `preset4` Green and `preset3` Yellow.
- **Option rejected:** keeping `preset4`. It's green, which doesn't match To Do's yellow sun icon.
- **Why:** yellow was the original intent. The doc says the actual colour depends on the Outlook client, so BK compares both test categories on the phone (S7, Q11) before this is final. Evidence: [S7](../research/spikes/S7.md).
- D-015's premise that the phone shows and filters the My Day category also waits on the S7 phone check.
- **Replaced by D-037:** My Day no longer uses a category, so it has no colour.

### D-031: Clients use the daemon protocol only; the TUI doesn't read SQLite. (BK, 2026-09-24)
- **Replaces:** "the TUI may read SQLite directly" in [01](01-architecture.md), the read-only connection pool in [08](08-tui.md), and "the TUI only ever reads SQLite" in [README](README.md). None had an entry of its own here.
- **Option rejected:** direct SQLite reads for the TUI's hot path.
- **Why:** once one client reaches past the protocol, the protocol stops being the product, and the next client can't be built cleanly (vault: `Building Great CLIs`, "can the TUI just reach into the store directly… the answer is no"; `Mxr`). The TUI seeds from a daemon snapshot (spotuify's `ClientSeed`) and then applies `EntityChanged` events. A Unix-socket round trip is about 5–10 µs (vault: `Local IPC vs HTTP`), so the latency budget stands. Only the daemon touches `store`, and the boundary test enforces it.

### D-032: Phase 1 ships a minimal daemon, not a direct-mode CLI. (BK, 2026-09-24)
- **Replaces:** Phase 1's "CLI without a daemon yet, in direct mode" in [10](10-roadmap.md).
- **Option rejected:** direct mode first. The CLI would become a second token refresher, breaking "only one process refreshes" in [01](01-architecture.md#why-a-daemon), and it would be built against internals rather than the protocol.
- **Why:** "Build the CLI client against the documented protocol, not against private daemon internals" (vault: `API-First Design`). Phase 1 builds the protocol, socket server, auto-start and a daemon that owns sign-in and refresh, plus `auth`, `lists list`, `tasks list`, `raw` and `sync` over IPC. Phase 1 syncs by full enumeration only (below); **delta** sync, the outbox and mutations start in Phase 2. Phase 1's completion check is unchanged.
- **Phase 1's cache:** the daemon runs a full enumeration of lists and tasks on start and on `ms-todo sync` (page to the end, upsert, tombstone what wasn't seen, by [04](04-sync-cache.md#reconciliation-after-a-lost-delta-token)'s rules), and reads are served from the store. Phase 2 adds delta on top, and the enumeration becomes its reset path, so nothing is thrown away.
- Note (2026-09-24): the cache now arrives in rung 3a, and rung 1's daemon reads straight from Graph. See D-034.

### D-033: Blueprint aligned with BK's vault notes. (BK approved, 2026-09-24)
BK checked the blueprint against his Obsidian notes and approved folding these gaps in. Each is a sentence or three in the doc named.

| # | Gap | Where | Vault notes |
|---|---|---|---|
| 1 | IPC idempotency: client request ID, `--idempotency-key`, kept 24 h; no re-send after a timeout | [04](04-sync-cache.md#instant-local-writes), [07](07-cli.md#global-flags) | `Agent-Native Interfaces` |
| 2 | Graph request timeouts (60 s default); progress events; give up on stalls, not total time | [03](03-graph-provider.md#http-client), [01](01-architecture.md#transport) | `Deadlines Bound Stalls, Not Work` |
| 3 | No file bytes over IPC; explicit frame cap; collection events capped at 500 IDs, then `ResyncNeeded` | [01](01-architecture.md#transport), [07](07-cli.md#output-contract) | `Every Event Payload Needs a Bound`, `Length-Prefixed Framing` |
| 4 | Token refresh as a compare-and-swap under the file lock; `auth login` without a healthy daemon | [03](03-graph-provider.md#sign-in) | `Refresh Token Rotation Is Shared State`, `Credential Prompts Are Side Effects` |
| 5 | Socket 0600 in a 0700 directory; `auth bearer --reveal-secret`; safe attachment downloads | [01](01-architecture.md#files), [03](03-graph-provider.md), [07](07-cli.md) | `Local Capability Surfaces Need Defense in Depth`, `Attachment Writes Are a Trust Boundary`, `Spotuify Security Audit Synthesis` |
| 6 | `op_id` on every mutation; `ms-todo undo [OP_ID]` shares the TUI's undo logic | [07](07-cli.md#output-contract), [08](08-tui.md) | `Building Great CLIs`, `Clean Up Means Archive, Not Delete` |
| 7 | Dry-run and real run build one typed plan; no prompt off a terminal | [07](07-cli.md#global-flags) | `Same-Code-Path Preview` |
| 8 | `tasks move` as a resumable outbox job | [05](05-custom-features.md#move-between-lists), [04](04-sync-cache.md#instant-local-writes) | `A Detached Child Outlives Its Supervisor` |
| 9 | Bind the socket first; ready means a compatible `Status` | [01](01-architecture.md#daemon-lifecycle) | `Daemon Readiness Is Not Process Liveness` |
| 10 | `sync_state: "initial"` until a scope's first sync; the TUI shows syncing, not empty | [07](07-cli.md#output-contract), [08](08-tui.md) | `First Run Is the Launch Surface`, `Derived State Needs an Unknown State` |
| 11 | Sync generation counter, `in_progress`, `last_success_at`, `last_changed_count`; `sync --wait` waits on the generation | [04](04-sync-cache.md#freshness-guarantee-for-the-cli), [02](02-data-model.md) | `Zero Change Can Be Success`, `Faster Code Breaks Coarse Clocks` |
| 12 | `schema_version`, `ms-todo schema`, `--help` snapshots with a CI drift check | [07](07-cli.md#output-contract) | `Building Great CLIs`, `Agent-Native Interfaces`, `Generated Docs as Drift Defense` |

- **Option rejected:** leaving these to be discovered during the build. Each is already written up in BK's notes from earlier projects.

### D-034: The roadmap is a ladder of usable releases. (BK, 2026-09-24)
- **Replaces:** the component phases in [10](10-roadmap.md) (Foundation; Daemon, sync and outbox; Full API surface; TUI; Custom features and natural language; Ship). Phase 0 stays as it was.
- **Option rejected:** building by component. Nothing would be usable until the TUI phase, and nothing would ship until the last one.
- **Why:** BK's rule is skateboard → car (vault: `Skateboard MVP`, `Shippable Increment Per Session`). Each rung is a released, installable tool that lets BK do something new from start to finish, and fits one working session. Ceremony is pulled into rung 1, and each rung has at most two new unknowns.
- **The ladder:** 1 skateboard (see my tasks), 2 scooter (capture and finish tasks), 3a bicycle (instant reads), 3b live sync, 4 e-bike (offline, never lose a write), 5 motorbike (TUI), 6 car (quick add), 7 convertible (My Day), 8a–8e rocket (the rest of the API, one capability per release).
- **Foundation turns:** up to about three named foundation turns may come before the first usable rung, each saying what it makes possible and ending with something runnable or checkable. ms-todo has one, F1 (install and sign in), so rung 1 arrives in turn 2.
- **Trade-off, accepted:** rung 1's daemon reads straight from Graph, and rung 3a replaces that read handler with the cache. Entity `id`s change meaning at rung 3a (Graph ID to local ID), with a `schema_version` bump. That's rework `Skateboard MVP` accepts on purpose: "more re-work in exchange for de-risking". D-031 and D-032 still hold: clients only ever talk to the daemon, and the daemon is the only token refresher.

### D-035: Config lives in `~/.config/ms-todo` (XDG) on every Unix, not the macOS Application Support dir. (BK, 2026-09-24)
- **Replaces:** `<config_dir>/ms-todo/config.toml` in [01](01-architecture.md#files), which `dirs::config_dir()` put in `~/Library/Application Support` on macOS.
- **Rule:** `config.toml` in `$MS_TODO_CONFIG_DIR`, else `$XDG_CONFIG_HOME/ms-todo`, else `~/.config/ms-todo`. Data (the token, logs and later the cache) stays in the platform data directory.
- **Why:** most CLIs keep config there, on macOS too.

### D-036: Rung 3a's cache, as built. (Rung 3a build, 2026-09-24)
- **Refines** [02](02-data-model.md)'s draft tables, as that document invited, and fills in rung 3a details [04](04-sync-cache.md) left open. Nothing here reverses an earlier decision.
- **Pending work before the outbox.** 04 never tombstones rows with pending outbox work. Rung 3a has no outbox, but the daemon's own synchronous writes can still race a pass that fetched earlier. So each row records the value of a `local_rev` counter when the daemon last wrote it, each scope reads the counter before it fetches, and applying the scope leaves alone every row written since: no overwrite, no tombstone. Rung 4 adds "has an outbox operation in `pending`, `inflight` or `unknown`" to the same exemption. A clock would do the same job less reliably (vault: `Faster Code Breaks Coarse Clocks`).
- **Which tasks need their extension fetched:** those whose etag differs from `hydrated_etag`, the etag at which the extension was last fetched. The first sync fetches every task's (BK's 654 tasks: about 17 seconds); later passes fetch only changed tasks. Lists never need it: the list enumeration itself carries the filtered `$expand` (S2), so one request covers both.
- **The cache is private:** the data directory is 0700 and `ms-todo.db` and its `-wal` and `-shm` files are 0600, repaired on every start, like the token and the socket.
- **Our extension is a column** (`extension_json` on `lists` and `tasks`), not rows in 02's `extensions` table, which is for rung 8e's general `extensions` command.
- **Idempotency keys live in their own table**, `idempotency_keys`, since there's no outbox to hang them on. A key is released only when the request certainly changed nothing on Graph: a failure that proves it (invalid input, not found, conflict, rejected, rate limited, sign-in) before any task was changed. Everything else is kept. A failure that may have followed a change on Graph (a network error or 5xx, or the cache write failing after Graph took the change) is recorded as `outcome_unknown` with the `op_id`, so a repeat never sends it again. A cache write that fails after Graph took a change is `outcome_unknown` with or without a key. After a restart, a key whose request never finished gets an `outcome_unknown` result, kept for 24 hours.
- **Reads of a scope that has never synced.** While a sync runs, the answer is `initial` with no items. With nothing running (the last attempt failed), the read runs a sync and answers from that, so it reports the failure (for example exit 4, signed out) instead of an `initial` that would never become `ready`. Writes wait for the scope's first sync, because they resolve names in the cache.
- **Without `--list`, a task is looked up only in the cache**, by local or Graph ID. Rung 2 asked every list for an ID it hadn't seen; with the cache that fallback is gone, and a task added elsewhere since the last sync is `not_found` with a hint to run `ms-todo sync --wait`.
- **Not built in rung 3a, and still to be placed:** the local read filters (`--status`, `--due`, `--importance`, `--search` through FTS5, `--sort`, `--limit`), `EntityChanged` events with the 500-ID cap and `ResyncNeeded`, `sync --list`, attachment metadata (it needs rung 8b's table), and the optional launchd and systemd files. The build brief for rung 3a left them out.

### D-037: My Day lives in our extension, and reaches the phone through the due date. (BK, after the phone check, 2026-09-24)
- **Replaces:** D-015's Outlook category and D-030's colour.
- **Why:** BK's phone check (S7) found that the iOS To Do app shows no categories at all, so a "My Day" category would be invisible on the phone. But the app has a setting, "Show 'Due Today' tasks in My Day", which is on in BK's app: it puts every task due today into the app's own My Day. That also explains the S12 observation.
- **Chosen:**
  - ms-todo's My Day is the `myDay` date in our extension (`com.planetaryescape.mstodo`), the source of truth, with its own view, suggestions and daily rollover in the CLI and TUI. It syncs across every ms-todo install on the account.
  - **Phone mirror:** adding a task with **no due date** to My Day also sets its due date to today, and records `myDayDueSet: true` in the extension, so the app shows it in its own My Day on devices where that setting is on.
  - **Rollover:** a task whose due date ms-todo set, and that isn't completed, loses that due date again. A due date the user set is never touched.
  - Tasks with a real due date keep it. They appear in the app's My Day only when they're actually due today.
- **Options rejected:** always setting the due date to today, which clobbers real due dates and hides them in the Planned and overdue views; ms-todo-only, which the phone can't see; and keeping the category, which iOS doesn't show.
- **Depends on the app setting,** which isn't in Graph, so `ms-todo doctor` can't read it. The docs and `doctor` say so.
- **Unchanged:** categories stay for `@labels` ([06](06-natural-language.md)), which Graph and Outlook show and iOS doesn't. `MailboxSettings.ReadWrite` is still needed for them.

### D-038: Rung 3b's delta sync, as built. (Rung 3b build, 2026-09-24)
- **Fills in** details [04](04-sync-cache.md#delta-sync) left open. Nothing here reverses an earlier decision.
- **A whole read is a fresh delta round.** A scope's first pass, and its reset, page `…/delta` with no token to the end: that's every item plus a `deltaLink`, in one set of requests. The plain `/tasks` enumeration is no longer used. A whole read tombstones what it didn't see; a delta round tombstones only `@removed` entries and extension fetches that answer 404.
- **Lists always apply an enumeration.** Delta never carries extensions (S2), so when `lists/delta` names any list, the one `GET /me/todo/lists` with the filtered `$expand` that 04 fetches for extensions is applied as a whole read (upsert, tombstone the rest), minus lists delta called removed. A lists delta round that names nothing just checkpoints.
- **Cursors live in `sync_state`** (`delta_link`, `last_delta_at`, migration `0002`). A scope with no link is in enumeration mode; `doctor` shows each scope's `mode` and `last_delta_at`. A 410 or "Badly formed token." drops the link at once, so a reset that then fails stays in enumeration mode.
- **404 and 5xx keep the link.** The Graph client already retries a 5xx with backoff inside the request. After that, or on a 404, the scope fails and the next pass replays the same link. A tasks-delta 404 first checks `GET /me/todo/lists/{id}`: a 404 there tombstones the list and its tasks and drops its cursor (S4).
- **Cadence:** a pass every 20 seconds while any client is connected or has sent a request in the last 10 minutes, otherwise every 5 minutes, measured from the end of the last pass. The first request after an idle spell brings the next pass forward. No `sync.interval_secs` setting yet.
- **Not built, and still to be placed:** `--fresh`, the TUI focus hint, attachment metadata (rung 8b), and syncing after the outbox sends (rung 4).
