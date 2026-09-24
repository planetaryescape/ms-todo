# ms-todo

A local-first, keyboard-native terminal client for Microsoft To Do. It has a daemon, a scriptable CLI with stable JSON output, and a very fast ratatui TUI, all reading a local SQLite cache kept in sync with Microsoft Graph.

**Status: planning complete, not built yet.** The design is in [`docs/blueprint/`](docs/blueprint/README.md).

What's planned:

- The whole Microsoft Graph To Do API: lists, tasks (every field, including recurrence), steps, links, attachments up to 25 MB, categories, open extensions, delta sync.
- Features the API lacks, built on top of it: My Day (an Outlook category the phone app also shows), folders for lists, and assignment.
- Todoist-style natural-language quick add, parsed deterministically: `Pay rent every 1st #Home p1 !9am`.
- Offline-tolerant instant writes, with an outbox and rollback when Graph rejects a change.

Licensed under MIT or Apache-2.0, at your option.
