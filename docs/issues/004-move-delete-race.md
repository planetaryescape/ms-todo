# 004: A move can delete a source edited in the instant before its DELETE

**Status:** closed (D-070): Graph limitation; now checked by Graph itself, down to the gap between two steps of one `$batch`.

## Problem

A move deletes the source only after the copy checks out, and it reads the source once more right before the DELETE, comparing its etag with the one the copy was checked against. An edit on another device that lands in the moment between that last read and the DELETE (well under a second) is still deleted with the source, and the copy doesn't have it.

## Root cause

Graph's task DELETE ignores `If-Match` and deletes whatever is there, whatever etag is sent ([S6](../research/spikes/S6.md), [12](../blueprint/12-open-questions.md)). So there's no way to make the delete conditional on the task being unchanged: the last read can only narrow the window, not close it.

## Why it's accepted

The window is the time of one request, and BK is one user moving his own tasks. Every other edit during a move is caught: before the copy, by the check (a changed source rolls the move back), and after it, by the read before the DELETE (a changed source pauses the move with both tasks kept). Recorded in D-051.

## Options for later

- If Graph ever honours `If-Match` on task DELETE, send the verified etag with it.
- After the delete, read the source's last delta round for an edit timestamped inside the window, and warn.

## Resolution (D-070)

The second option above can't work: a delta round after a task was edited and then deleted reports only `{"id": …, "@removed": {"reason": "deleted"}}`, with no timestamp or content, and the source can no longer be read ([S6](../research/spikes/S6.md#follow-up-a-conditional-delete-through-batch-2026-09-27-issue-004)). There's nothing to detect the edit from after the delete.

The same spike found a better way: a `$batch` of an empty `PATCH` carrying `If-Match` with the verified etag, and the `DELETE` with `dependsOn` it. Task PATCH honours `If-Match`, so a changed source makes the PATCH a 412 and Graph never runs the DELETE (424). The move then pauses with both tasks kept, as for a change the last read sees, with the note `the task changed on another device during the move (an edit just before the delete)` and the usual `WriteRejected`-style state in `outbox list` and the TUI. The last read before the delete stays, so most changes are still caught before anything is sent.

**Residual window:** an edit landing inside Graph between the batch's PATCH and its DELETE, which run one after the other on Graph's side; no longer a network round trip. Test `an_edit_right_after_the_last_read_stops_the_conditional_delete` (`tests/move_cli.rs`).

