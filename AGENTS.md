# ms-todo: project context for AI agents

> A local-first, keyboard-native terminal client for Microsoft To Do: a daemon, a scriptable CLI and a very fast ratatui TUI, all reading a local SQLite cache kept in sync with Microsoft Graph.

## Status

**There's no code yet.** This repo holds a complete blueprint written in the planning session on 2026-09-24. You're probably the agent that builds it, on a different machine from the one where it was planned.

## Start here

1. Read `docs/blueprint/README.md`, then the documents in the order listed. Read `11-decision-log.md` before proposing any change of direction. Most alternatives you might think of were already considered and rejected there, with reasons.
2. Read `docs/research/microsoft-todo-api.md` and `docs/research/prior-art.md`.
3. Follow `docs/blueprint/KICKOFF.md` for the first session.

## Rules

- **The blueprint is the spec, and the decision log is its history.** If you change a decision, add a new entry to `11-decision-log.md` that replaces the old one. Don't silently drift.
- **Run the spikes before building on anything that depends on them** (`12-open-questions.md`). The Graph docs are silent on several behaviours this design relies on. Observe the real API; don't guess.
- **Every feature is in the CLI.** A feature that only exists in the TUI is incomplete. Verify your work by driving the `ms-todo` binary against the real account (dev instance), not only through unit tests.
- **Reuse before rewriting.** `docs/blueprint/09-reuse-map.md` lists the exact files to adapt from `planetaryescape/mxr` and `planetaryescape/spotuify`, with SHAs. Fetch both and check for newer fixes before copying.
- Issues are tracked as markdown in `docs/issues/`, not GitHub Issues.
- Commits use the format `type: description`. BK is the only author, with no co-author lines.

## Stack

Rust, tokio, ratatui + crossterm, reqwest (rustls), sqlx on SQLite (WAL, FTS5), clap, thiserror and anyhow, serde, tracing. Tests use nextest, insta, wiremock, assert_cmd and proptest. Releases: release-please, GitHub Releases, `install.sh` and a Homebrew formula.
