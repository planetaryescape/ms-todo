# 006: Context edge cases left after rung 9d

**Status:** closed (D-070), all six fixed. Found by review bots on PR #80 (2026-09-27); accepted at merge under the stop-at-diminishing-returns rule.

## Problems

- **An idempotency key across a `ctx` switch.** The fingerprint includes an explicit `--context`, but not the active context. The same `--idempotency-key` reused after `mst ctx` changes the active context replays the first result, and after a failed first attempt it can target a different set of tasks. `crates/daemon/src/handlers.rs` (~242).
- **Context membership changed by a config edit.** The TUI compares the context's name, count and default list. An edit that keeps those but swaps a list leaves the old list selected until the next reseed (see D-064's known gaps). `crates/tui/src/app/answers.rs` (~220).
- **Semantic `pending` in a context** counts unindexed tasks from every list, not just the context's. `crates/daemon/src/reads.rs` (~271).
- **`ctx list --format ids`** prints nothing: the rows have no `id` key. `crates/cli/src/context_commands.rs` (~88).
- **`ctx` with no active context** doesn't show problems in the configured contexts (`ctx list` and `doctor` do). `crates/cli/src/context_commands.rs` (~162).
- **07-cli.md's context section** leaves out that bulk filter selections (`reschedule`, `tasks edit --overdue|--due-before`) are narrowed too.

## Fix sketch

Put the resolved context (name plus list IDs) in the fingerprint; re-home the TUI when the resolved IDs change; compute `pending` over the context's lists; make `ids` print the context names; carry problems through; add the sentence to 07.

## Resolution (D-070)

- The fingerprint of `tasks add` and every task change covers the context it was resolved in, active or `--context`: its name and list IDs (`idempotency::in_context`). The same key after a `ctx` switch, or after config.toml changed the context's lists, exits 2. Test `an_idempotency_key_covers_the_active_context_and_its_lists`.
- The TUI treats a seed whose context has other lists (same name, count and default) as a switch: it drops the old context's cached rows and leaves a list the sidebar no longer has. Test `a_context_whose_lists_changed_leaves_a_list_it_lost`.
- A search by meaning in a context counts `pending` over the context's lists only (`semantic::query::pending_within`).
- `ctx list --format ids` prints each context's name; `ctx` with none active shows the configured contexts' problems. Test `ctx_names_its_contexts_as_ids_and_warns_with_none_active`.
- 07-cli's context section says bulk selections are narrowed too.

