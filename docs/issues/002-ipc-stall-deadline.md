# 002: IPC calls use a total deadline, not a stall deadline

**Status:** open. Due by rung 3a, before the first sync and `sync --wait`.

## Problem

In rung 1 a CLI request to the daemon gives up after 300 seconds in total (`REQUEST_TIMEOUT` in `crates/cli/src/daemon_client.rs`). D-033 item 2 ([01](../blueprint/01-architecture.md#transport); vault: `Deadlines Bound Stalls, Not Work`) asks for the opposite: long calls send progress events, and a client gives up only after a period with no progress, never on total time.

## Why it's accepted today

Rung 1's calls are short. The daemon bounds each Graph request at 60 seconds, with at most 3 retries and a page cap, so it always answers. BK's largest list (178 tasks) takes about 1.8 seconds.

## What rung 3a needs

- Progress events from the daemon during the first sync, `sync --wait` and other long calls.
- A client deadline that resets on each progress event, instead of the 300-second total.
