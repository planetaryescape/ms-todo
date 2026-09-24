# ms-todo: technical blueprint

> A local-first, keyboard-native terminal client for Microsoft To Do. It has a daemon, a scriptable CLI and a very fast ratatui TUI, all reading from a local SQLite cache that stays in sync with Microsoft Graph.

The whole project was planned in one session on 2026-09-24, before any code existed. It is meant to be built on another machine by a coding agent that never saw that session, so this blueprint has to carry everything: what we're building, how, what we decided against, and why.

## Reading order

| # | Document | What it covers |
|---|---|---|
| 00 | [Overview](00-overview.md) | What ms-todo is, principles, scope, what the API can't do |
| 01 | [Architecture](01-architecture.md) | Daemon, clients, IPC, crate layout |
| 02 | [Data model](02-data-model.md) | SQLite schema, outbox, IDs, how custom data is stored |
| 03 | [Graph provider](03-graph-provider.md) | Sign-in, scopes, HTTP client, retries, pagination, batch |
| 04 | [Sync and cache](04-sync-cache.md) | Delta sync, instant local writes, reconciliation, conflicts |
| 05 | [Custom features](05-custom-features.md) | My Day (category), folders, assignment, move |
| 06 | [Natural language](06-natural-language.md) | Todoist-style quick add, deterministic parser, LLM deferred |
| 07 | [CLI](07-cli.md) | Command surface, output formats, exit codes, agent skill |
| 08 | [TUI](08-tui.md) | Views, My Day view, quick add, latency budget |
| 09 | [Reuse map](09-reuse-map.md) | Exactly what to copy from mxr and spotuify, with SHAs |
| 10 | [Roadmap](10-roadmap.md) | Phases in build order, each with a completion check you can observe |
| 11 | [Decision log](11-decision-log.md) | Every decision, the options we rejected, and the full story of how we got here |
| 12 | [Open questions](12-open-questions.md) | Spikes to run against the real API before coding, plus product questions |
| — | [Kickoff](KICKOFF.md) | The prompt to give the coding agent for the first build session |

Research behind these decisions:

- [../research/microsoft-todo-api.md](../research/microsoft-todo-api.md): the Graph To Do API's surface, auth, limits and gotchas.
- [../research/prior-art.md](../research/prior-art.md): code reviews of the existing CLI and MCP server we chose not to adopt.

## Summary

- **Stack:** Rust, tokio, ratatui and crossterm, reqwest, sqlx on SQLite, and clap. It follows the structure of mxr and spotuify, but is much smaller.
- **Runtime:** a daemon owns sign-in, the cache, sync and the outbox of local writes waiting to be pushed to Graph. The CLI and TUI are clients over a Unix socket.
- **Speed:** the TUI only ever reads SQLite. Writes show up immediately and are pushed to Graph in the background.
- **Sync:** Graph delta queries for lists and tasks. No webhooks.
- **Sign-in:** device code, through BK's own Entra app registration, `/common` authority, no client secret.
- **Scope:** everything the Graph To Do API offers, plus features the API lacks, built on top of it:
  - My Day, done with an Outlook category the phone app also displays
  - folders, stored as list extensions
  - assignment, stored as a task extension
- **Quick add:** Todoist-style natural language, parsed deterministically. LLM parsing is deferred.
- **No MCP server.** Agents use the CLI plus an agent skill.
- **Dropped:** sharing.

## Rules for coding agents

1. **The CLI is the canonical surface.** Anything the TUI can do, a CLI subcommand can do. Agents check their own work through the CLI (the spotuify contract).
2. **Run the spikes first.** Several design points depend on API behaviour the docs don't cover. Run [12-open-questions.md](12-open-questions.md) before writing the code that depends on them, and update this blueprint with what you find.
3. **Update the decision log.** If you change a decision, add an entry to [11-decision-log.md](11-decision-log.md) with the reason. Don't silently change the design.
