# 002: IPC calls use a total deadline, not a stall deadline

**Status:** closed in rung 3a (2026-09-24).

## Problem

In rung 1 a CLI request to the daemon gives up after 300 seconds in total (`REQUEST_TIMEOUT` in `crates/cli/src/daemon_client.rs`). D-033 item 2 ([01](../blueprint/01-architecture.md#transport); vault: `Deadlines Bound Stalls, Not Work`) asks for the opposite: long calls send progress events, and a client gives up only after a period with no progress, never on total time.

## Why it's accepted today

Rung 1's calls are short. The daemon bounds each Graph request at 60 seconds, with at most 3 retries and a page cap, so it always answers. BK's largest list (178 tasks) takes about 1.8 seconds.

## What rung 3a needs

- Progress events from the daemon during the first sync, `sync --wait` and other long calls.
- A client deadline that resets on each progress event, instead of the 300-second total.

## Resolution

`crates/cli/src/daemon_client.rs` now gives up on a stall, not on total time: a request fails only when the daemon has sent nothing for it for `STALL_TIMEOUT` (300 seconds; `MS_TODO_REQUEST_TIMEOUT_MS` in debug builds). Each `SyncProgress` event the daemon sends for the request restarts the clock. Any request that waits on a sync streams one event per finished scope: `sync --wait`, and a read or write waiting for the first sync. The connection loop gives every request a progress sink (a task-local in `crates/daemon/src/sync/scheduler.rs`), so no request type is a special case. Tests: `a_sync_that_keeps_making_progress_outlives_the_stall_deadline` and `a_sync_that_stops_making_progress_gives_up_after_the_stall_deadline` in `tests/lost_reply_cli.rs`.
