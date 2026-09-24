# ms-todo

A local-first, keyboard-native terminal client for Microsoft To Do. It has a daemon that keeps a local SQLite cache in sync with Microsoft Graph, and two clients of that daemon: a scriptable CLI with stable JSON output and a very fast ratatui TUI.

**Status: F1, install and sign in.** ms-todo installs and signs in to your Microsoft account. Lists and tasks arrive in the next release (rung 1 of the [roadmap](docs/blueprint/10-roadmap.md)).

## Install

macOS (Apple silicon or Intel) and Linux x86_64:

```sh
curl -fsSL https://raw.githubusercontent.com/planetaryescape/ms-todo/main/install.sh | sh
```

This installs the latest release to `~/.local/bin/ms-todo` after checking the archive's sha256. Set `MS_TODO_INSTALL_DIR` to install somewhere else. To pin a release, end the command with `| sh -s -- --version v0.1.0`. The binary isn't signed yet, but macOS doesn't quarantine files downloaded with `curl`, so Gatekeeper doesn't block it.

## Sign in

```sh
ms-todo auth login    # prints a URL and a code; enter the code in your browser
ms-todo auth status   # account, token expiry, client ID and scopes
ms-todo auth logout
```

Release builds sign in through the maintainer's Entra app registration. You can [register your own](docs/setup/entra-app-registration.md) in about 10 minutes and set `MS_TODO_CLIENT_ID`, or `client_id` under `[auth]` in `<config_dir>/ms-todo/config.toml`. `auth status` shows which client ID is in use. Output is a table in a terminal and JSON when piped (`--format table|json`). A command that needs you to sign in exits with code 4.

## Plan

The design is in [`docs/blueprint/`](docs/blueprint/README.md), the Phase 0 results are in [`12-open-questions.md`](docs/blueprint/12-open-questions.md), and the evidence is in `docs/research/spikes/`. Still open: the phone halves of spikes S7 and S11, the S4 deltaLink replay, and product questions Q3 and Q6–Q12. The build climbs a ladder of usable releases, starting with a foundation turn (install and sign in) and then rung 1: see my tasks.

What's planned:

- The whole Microsoft Graph To Do API: lists, tasks (every field, including recurrence), steps, links, attachments up to 25 MB, categories, open extensions, delta sync.
- Features the API lacks, built on top of it: My Day (an Outlook category, which the phone app should also show; pending a phone check), folders for lists, and assignment.
- Todoist-style natural-language quick add, parsed deterministically: `Pay rent every 1st #Home p1 !9am`.
- Offline-tolerant instant writes, with an outbox and rollback when Graph rejects a change.

Licensed under MIT or Apache-2.0, at your option.
