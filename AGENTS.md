# ms-todo: project context for AI agents

> A local-first, keyboard-native terminal client for Microsoft To Do: a daemon that keeps a local SQLite cache in sync with Microsoft Graph, and two clients of it over a Unix socket: a scriptable CLI and a very fast ratatui TUI.

## Status

**Foundation turn F1 is built** (install and sign in): the Cargo workspace (`crates/core`, `crates/graph`, `crates/cli`), `ms-todo auth login|status|logout`, CI, the release-please chain and `install.sh`. The blueprint was written in the planning session on 2026-09-24, on a different machine from the build.

**Phase 0 is done** (2026-09-24). The spike results are in `docs/blueprint/12-open-questions.md` and the evidence in `docs/research/spikes/`. Still open: the phone halves of spikes S7 and S11, the S4 deltaLink replay, and product questions Q3 and Q6–Q12. The next session builds rung 1, the first usable rung (D-034). Every rung of `docs/blueprint/10-roadmap.md` is a usable release.

## Start here

1. Read `docs/blueprint/README.md`, then the documents in the order listed. Read `11-decision-log.md` before proposing any change of direction. Most alternatives you might think of were already considered and rejected there, with reasons.
2. Read `docs/research/microsoft-todo-api.md` and `docs/research/prior-art.md`, then the spike evidence in `docs/research/spikes/` for anything you're about to build on.
3. Follow `docs/blueprint/KICKOFF.md` for the first session. Registering the Entra app is covered in `docs/setup/entra-app-registration.md`.

## Rules

- **The blueprint is the spec, and the decision log is its history.** If you change a decision, add a new entry to `11-decision-log.md` that replaces the old one. Don't silently drift.
- **Build on observed API behaviour, not on the docs.** The Phase 0 spikes answered most questions (`12-open-questions.md`). Where something is still open, or a new behaviour matters, run a spike against the real API first and record it in `docs/research/spikes/`. Don't guess.
- **Clients use the daemon protocol only** (D-031). Neither the CLI nor the TUI touches SQLite or Graph directly.
- **Every feature is in the CLI.** A feature that only exists in the TUI is incomplete. Verify your work by driving the `ms-todo` binary against the real account (dev instance), not only through unit tests.
- **Reuse before rewriting.** `docs/blueprint/09-reuse-map.md` lists the exact files to adapt from `planetaryescape/mxr` and `planetaryescape/spotuify`, with SHAs. Fetch both and check for newer fixes before copying.
- Issues are tracked as markdown in `docs/issues/`, not GitHub Issues.
- Commits use the format `type: description`. BK is the only author, with no co-author lines.

## Stack

Rust, tokio, ratatui + crossterm, reqwest (rustls), sqlx on SQLite (WAL, FTS5), clap, thiserror and anyhow, serde, tracing. Tests use nextest, insta, wiremock, assert_cmd and proptest. Releases: release-please, GitHub Releases, `install.sh` and a Homebrew formula.
