# 001: Extension writes can lose a concurrent edit

**Status:** closed (D-070). Tasks: fixed by `If-Match`. Lists: a Graph limitation, narrowed to one round trip and reported.

## Problem

ms-todo writes an open extension as GET, merge, then a PATCH of the whole document, because Graph replaces the extension on PATCH ([S2](../research/spikes/S2.md)). If two devices edit different fields of the same extension inside that GET–PATCH window, one edit is lost without any error.

The fields exposed, by where they live:

- **A list's extension:** `folder`, `order`, `folderOrder` (rung 5c, D-047).
- **A task's extension:** `opId` and `originalCreatedAt` (written with the create or move itself, not by a later GET–PATCH), and since rung 7 `myDay` and `myDayDueSet` (D-054), written by `myday add|remove` and the rollover. A lost `myDay` puts a task in or out of My Day on one device only; a lost `myDayDueSet` can at worst leave a due date My Day set on the task after it leaves My Day. Since rung 8d (D-057), `assignee` and `assigneeStatusSet` share the same document, written by `tasks add|edit --assignee` and `--clear-assignee`: a lost `assignee` leaves a task assigned (or not) on one device only, and a lost `assigneeStatusSet` means clearing the assignee later leaves `waitingOnOthers` rather than setting `notStarted`.

## Why it's accepted today

There's one user, and the window is short, so it should be rare. It's documented in [04](../blueprint/04-sync-cache.md#children-of-a-task) and in S2's open item in [12](../blueprint/12-open-questions.md#s2-result-2026-09-24).

## Options for later

- Split the data into one extension per field group, so unrelated edits don't share a document.
- Check the etag before the PATCH, if S2's open question (does an extension PATCH honour the parent's `If-Match`?) turns out to be yes.
- Send a warning event when a write may have overwritten a concurrent change.

## Resolution (D-070)

The spike ([S2](../research/spikes/S2.md#follow-up-does-an-extension-write-honour-if-match-2026-09-27-issue-001)) found that an extension has no etag of its own, that a **task's** extension PATCH, POST and DELETE honour `If-Match` with the **task's** etag (412 when stale), and that a **list's** extension ignores `If-Match` (as list PATCH does).

- **A task's extension** (`myDay`, `myDayDueSet`, `assignee`, `assigneeStatusSet`, `related`): the write sends `If-Match` with the task's etag from the GET it merged into. Another device's write in between moves that etag, so ours is a 412; the worker reads, merges and writes again, up to 3 times, then waits for the next round. Nothing is lost. `crates/daemon/src/outbox/extension_write.rs`; test `an_extension_field_another_device_wrote_meanwhile_is_kept` (`tests/assignment_cli.rs`).
- **A list's extension** (`folder`, `order`, `folderOrder`): Graph offers no conditional write. **Residual window:** a write by another device that lands between our GET of the list and our PATCH (one round trip; the two are sent back to back) is still replaced by ours, silently. After writing, the worker reads the list again; a field that isn't what it wrote means another device wrote around the same moment, and the operation gets a note naming those fields and a `ConflictOverwritten` event (the TUI's banner). Test `a_folder_write_another_device_raced_is_noted` (`tests/folders_cli.rs`).

