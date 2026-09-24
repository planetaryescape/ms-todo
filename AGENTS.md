# ms-todo: project context for AI agents

> A local-first, keyboard-native terminal client for Microsoft To Do: a daemon that keeps a local SQLite cache in sync with Microsoft Graph, and two clients of it over a Unix socket: a scriptable CLI and a very fast ratatui TUI.

## Status

**Status: Rung 4: offline and undo** is built, on top of rung 3b (live sync), rung 3a (instant reads), rung 2 (capture and finish tasks), rung 1 (see my tasks) and F1 (install and sign in). The workspace is `crates/core`, `crates/protocol`, `crates/graph` (sign-in, the HTTP client, endpoints, `$batch`), `crates/store` (SQLite through sqlx; only the daemon uses it), `crates/daemon` and `crates/cli`. The daemon keeps a cache in step with Graph by delta sync, with a whole read of a scope as the first pass and the reset path (on start, every 20 seconds while a client is active, every 5 minutes otherwise, on `sync`, and after the outbox sends), and answers reads from it. Writes are applied to the cache and queued in the outbox in one transaction and answer at once; the daemon's outbox worker sends them (`crates/daemon/src/outbox/`), with `unknown` outcomes attributed by `opId` and never resent by itself (D-028, D-040). Entities carry local IDs with `graph_id` and `sync_state` beside them, at `schema_version` 2. The CLI has `auth`, `lists list`, `tasks list|add|complete|reopen|edit|delete`, `outbox list|retry|discard`, `undo`, `sync [--wait]`, `doctor`, `schema [CMD]`, `raw GET|POST|PATCH|DELETE`, `daemon start|stop|status`, `--dry-run`, `--idempotency-key`, and `--format table|json|jsonl|ids|csv`. The agent skill is `skills/ms-todo/SKILL.md` (v2). Rung 5 (the TUI) is next; what rungs 3a, 3b and 4 didn't build is listed in D-036, D-038 and D-040. The blueprint was written in the planning session on 2026-09-24, on a different machine from the build.

**Phase 0 is done** (2026-09-24). The spike results are in `docs/blueprint/12-open-questions.md` and the evidence in `docs/research/spikes/`. Still open: the S4 deltaLink replay, and product questions Q3, Q6–Q10 and Q12. Every rung of `docs/blueprint/10-roadmap.md` is a usable release.

## Start here

1. Read `docs/blueprint/README.md`, then the documents in the order listed. Read `11-decision-log.md` before proposing any change of direction. Most alternatives you might think of were already considered and rejected there, with reasons.
2. Read `docs/research/microsoft-todo-api.md` and `docs/research/prior-art.md`, then the spike evidence in `docs/research/spikes/` for anything you're about to build on.
3. Follow `docs/blueprint/KICKOFF.md` for the first session. Registering the Entra app is covered in `docs/setup/entra-app-registration.md`.

## Use ms-todo to build ms-todo

**Drive the binary yourself.** Every feature is in the CLI, so you can check your own work end to end without asking BK to reproduce anything:

- Build: `cargo build --release --bin ms-todo`. Binaries from `target/` use the `dev` instance; add `--instance default` only to reach BK's installed sign-in and daemon.
- Run the case: `./target/release/ms-todo tasks list --list <L> --format json`, `tasks add … --dry-run`, and so on. `--dry-run` shows the exact Graph body without writing.
- Probe Graph directly: `ms-todo raw GET <path>`, or `curl -H "Authorization: Bearer $(ms-todo auth bearer --reveal-secret --format table)" https://graph.microsoft.com/v1.0/...`.
- Read the daemon's log: `logs/daemon.log` in the instance's data directory. Restart it with `ms-todo daemon stop`; the next command starts it.
- Check the cache: `ms-todo doctor` shows each scope's sync state and last error; `ms-todo sync --wait` refreshes it. The database is `ms-todo.db` in the instance's data directory; open it read-only (`sqlite3 "file:…/ms-todo.db?mode=ro"`) if you need to look inside.
- Writes against BK's real account go only to a throwaway list he's named for the job, resolved by name first.

If a behaviour is unclear, run it and read the real response rather than guessing (spotuify's lesson, adapted from its `AGENTS.md`).

## Rules

- **The blueprint is the spec, and the decision log is its history.** If you change a decision, add a new entry to `11-decision-log.md` that replaces the old one. Don't silently drift.
- **Build on observed API behaviour, not on the docs.** The Phase 0 spikes answered most questions (`12-open-questions.md`). Where something is still open, or a new behaviour matters, run a spike against the real API first and record it in `docs/research/spikes/`. Don't guess.
- **Clients use the daemon protocol only** (D-031). Neither the CLI nor the TUI touches SQLite or Graph directly.
- **Every feature is in the CLI.** A feature that only exists in the TUI is incomplete. Verify your work by driving the `ms-todo` binary against the real account (dev instance), not only through unit tests.
- **Reuse before rewriting.** `docs/blueprint/09-reuse-map.md` lists the exact files to adapt from `planetaryescape/mxr` and `planetaryescape/spotuify`, with SHAs. Fetch both and check for newer fixes before copying.
- **Instances.** Binaries run from `target/{debug,release}` use the `dev` instance (`ms-todo-dev` data, socket and daemon). Installed binaries use the default instance. Pass `--instance default` (or `MS_TODO_INSTANCE=default`) only when you mean to reach the installed copy's sign-in and daemon.
- **Stopping daemons.** Don't broad-`pkill` ms-todo daemons: stop an instance with `ms-todo [--instance NAME] daemon stop`, so dev and installed daemons don't kill each other. A stop is done only when the socket is unreachable and the daemon's PID has exited; the CLI waits for both. When diagnosing, check the old PID is gone before starting another daemon. (Adapted from spotuify's `AGENTS.md`.)
- Issues are tracked as markdown in `docs/issues/`, not GitHub Issues.
- Commits use the format `type: description`. BK is the only author, with no co-author lines.

## Stack

Rust, tokio, ratatui + crossterm, reqwest (rustls), sqlx on SQLite (WAL, FTS5), clap, thiserror and anyhow, serde, tracing. Tests use nextest, insta, wiremock, assert_cmd and proptest. Releases: release-please, GitHub Releases, `install.sh` and a Homebrew formula.
