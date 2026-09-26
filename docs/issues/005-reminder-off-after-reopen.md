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

Graph derives `isReminderOn` and ignores a written one (S17): writing the reminder's time turns it on, and completing turns it off.

- `tasks reopen` (and the TUI's toggle) of a completed task writes the task's own `reminderDateTime` again with the status when that time is still ahead on this machine's clock, which turns the reminder on. A reminder whose time has passed stays off. `--dry-run` shows it.
- Undoing a completion does the same when the reminder was on before the completion and its time hasn't changed since. Undoing a reopen writes the time back only if the reminder was on before it.
- A move of a task whose reminder is off keeps the time; Graph turns the copy's reminder on, and the move's check accepts that (it expects the copy's reminder on when the source's is off with a time, and compares the copy's actual flag). The cache takes the copy's value. The original is still deleted only once the copy checks out.
- `crates/fake-graph` models the rules S17 found.

## Known limits

- A reopen queued offline and sent after the reminder's time has passed still writes the time, so Graph shows a past reminder as on. A past reminder doesn't fire, so this is accepted.
