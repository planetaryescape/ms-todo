# 006: Context edge cases left after rung 9d

**Status:** open, minor. Found by review bots on PR #80 (2026-09-27); accepted at merge under the stop-at-diminishing-returns rule.

## Problems

- **An idempotency key across a `ctx` switch.** The fingerprint includes an explicit `--context`, but not the active context. The same `--idempotency-key` reused after `mst ctx` changes the active context replays the first result, and after a failed first attempt it can target a different set of tasks. `crates/daemon/src/handlers.rs` (~242).
- **Context membership changed by a config edit.** The TUI compares the context's name, count and default list. An edit that keeps those but swaps a list leaves the old list selected until the next reseed (see D-064's known gaps). `crates/tui/src/app/answers.rs` (~220).
- **Semantic `pending` in a context** counts unindexed tasks from every list, not just the context's. `crates/daemon/src/reads.rs` (~271).
- **`ctx list --format ids`** prints nothing: the rows have no `id` key. `crates/cli/src/context_commands.rs` (~88).
- **`ctx` with no active context** doesn't show problems in the configured contexts (`ctx list` and `doctor` do). `crates/cli/src/context_commands.rs` (~162).
- **07-cli.md's context section** leaves out that bulk filter selections (`reschedule`, `tasks edit --overdue|--due-before`) are narrowed too.

## Fix sketch

Put the resolved context (name plus list IDs) in the fingerprint; re-home the TUI when the resolved IDs change; compute `pending` over the context's lists; make `ids` print the context names; carry problems through; add the sentence to 07.
