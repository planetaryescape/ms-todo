# ms-todo

A local-first, keyboard-native terminal client for Microsoft To Do. It has a daemon that keeps a local SQLite cache in sync with Microsoft Graph, and two clients of that daemon: a scriptable CLI with stable JSON output and a very fast ratatui TUI.

**Status: planning and Phase 0 spikes complete, not built yet.** The design is in [`docs/blueprint/`](docs/blueprint/README.md), the Phase 0 results are in [`12-open-questions.md`](docs/blueprint/12-open-questions.md), and the evidence is in `docs/research/spikes/`. Still open: the phone halves of spikes S7 and S11, the S4 deltaLink replay, and product questions Q3 and Q6–Q12. Phase 1 starts with a minimal daemon.

What's planned:

- The whole Microsoft Graph To Do API: lists, tasks (every field, including recurrence), steps, links, attachments up to 25 MB, categories, open extensions, delta sync.
- Features the API lacks, built on top of it: My Day (an Outlook category, which the phone app should also show; pending a phone check), folders for lists, and assignment.
- Todoist-style natural-language quick add, parsed deterministically: `Pay rent every 1st #Home p1 !9am`.
- Offline-tolerant instant writes, with an outbox and rollback when Graph rejects a change.

Licensed under MIT or Apache-2.0, at your option.
