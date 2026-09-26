# 005: A reopened task's reminder stays off, and moving it then fails

**Status:** fixed in the next release (D-060). Found by the rung 8e live smoke test on 2026-09-25.

## Problem

Completing a task makes Graph turn its reminder off (`isReminderOn: false`) and keep `reminderDateTime`. Reopening it, or undoing the completion, sends only `status`, so the reminder stays off.

A move of such a task then fails every time. The copy is POSTed with `isReminderOn: false` and the reminder's time, and Graph answers with `isReminderOn: true`: it ignores `false` on a create that carries a `reminderDateTime`. The move job's check finds the copy differs (`isReminderOn`), deletes the copy and keeps the original, and the operation is `failed`. Nothing is lost.

## Seen live

On the `livetest` instance, a throwaway list: a task with a reminder, completed (`isReminderOn` false), undone (still false), moved (failed as above). A raw POST of `{"isReminderOn": false, "reminderDateTime": …}` came back `isReminderOn: true`.

## Options

- The move copy leaves out `reminderDateTime` when `isReminderOn` is false, and the check ignores it then.
- Undoing a completion (and `tasks reopen`) puts `isReminderOn` back when the reminder is still ahead.

## Fix

- `tasks reopen` (and the TUI's toggle) sends `isReminderOn: true` with the status when the task's reminder time is still ahead on this machine's clock. A reminder whose time has passed stays off. `--dry-run` shows it.
- Undoing a completion turns the reminder back on when it was on before the completion and its time hasn't changed since.
- A move of a task whose reminder is off with a time keeps the time: the copy is POSTed as before, then PATCHed `isReminderOn: false` before it's checked, so the check compares like with like. The original is still deleted only once the copy checks out.
- `crates/fake-graph` now models both quirks: a completion turns the reminder off, and a create with a time comes back with the reminder on.
