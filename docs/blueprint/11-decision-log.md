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
8. My Day went from a special list, to copies of tasks, to an Outlook category.
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
