# 003: Two task-write tests fail now and then on Linux CI

**Status:** closed in rung 5b (2026-09-25). The race behind the "no task" failure is fixed, and so is a hang found while looping the tests.

## Problem

Since rung 3b, `task_writes_cli::complete_and_reopen_send_if_match_with_the_etag_last_read` and `task_writes_cli::completing_a_recurring_task_is_never_resent_after_a_5xx` sometimes failed on Linux CI at `tests/support/mod.rs:66` (the `json` helper's `.success()`). The failing command was the first `tasks complete T1` (or `T-r`), and it answered `not_found`: `no task has the ID "T1"`. Both passed on a rerun.

## Cause

In `resolve_by_id` (`crates/daemon/src/task_resolution.rs`), a task ID the cache doesn't have yet waited for the first sync only if `all_ready` said some list's first sync was still running. The two checks weren't atomic:

1. The command arrives after the lists have synced but before the tasks of "Tasks" have been written. The lookup finds nothing.
2. The first sync finishes the remaining lists.
3. `all_ready` now says every list is ready, so the daemon doesn't wait and doesn't look again, and answers `not_found`.

The first command of each test starts the daemon, so the command always races the first sync; a slow CI runner makes step 2 fit between the two queries.

**Evidence.** Neither the tests nor a loop reproduced it without help (50 of 50 passes on macOS and 50 of 50 in a Linux container, before the fix). Widening the window proved the mechanism: with a temporary 1.5-second sleep between the lookup and the check, and a fake Graph that answers task pages 400 ms late (so the lists are ready first), the same test failed every time at `tests/support/mod.rs:66` with exactly the CI message. With the fix and the same sleep, it passed.

## Fix

`resolve_by_id` now asks whether every list is ready *before* its first lookup, as `ensure_ready` already did. If it was, the lookup saw everything the first sync wrote, and a miss is final. If it wasn't, a miss waits for the running sync and looks again. Re-run with the same widened window: the lookup missed with `ready=false`, waited, and found the task.

There's no permanent regression test: the window is two SQLite queries wide, and forcing it needs a hook in production code. The instrumented run above is the evidence; its steps are in this file.

## Also found: `daemon stop` could hang on macOS

In the first 50-run loop on macOS, run 44 didn't finish: the test's `Drop` runs `ms-todo daemon stop`, which had hung for over 8 minutes. The daemon had logged "shutting down" and exited, and its socket file was gone. The `stop` process was parked in tokio with no timer due and a Unix socket still open to a peer no process held. Every step of `stop` has a deadline except `UnixStream::connect`, so a connect that raced the daemon closing its socket waited forever.

Every client connect to the daemon's socket now gives up after 3 seconds: `connect_socket` in `crates/cli/src/daemon_client.rs` (used by `probe`, so by every command, and by `wait_until_gone`), and the TUI's reconnect loop in `crates/tui/src/ipc.rs`. In `wait_until_gone`, a timed-out connect counts as the socket being gone. The PID check still decides.

## Results

50 runs of both tests on each side of the fix. A hang counts as a failure.

| | Before | After |
| --- | --- | --- |
| macOS (arm64) | 50 passed, 1 hang in `daemon stop` (killed by hand, then passed) | 50 passed |
| Linux (Colima, `rust:latest`, aarch64) | 50 passed | 50 passed |

CI's Linux runners are x86_64 and slower. The fix is for the mechanism above, not for anything the loops showed.
