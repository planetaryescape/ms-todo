# 005: A reopened task's reminder stays off, and moving it then fails

**Status:** open. Found by the rung 8e live smoke test on 2026-09-25; not fixed in 8e.

## Problem

Completing a task makes Graph turn its reminder off (`isReminderOn: false`) and keep `reminderDateTime`. Reopening it, or undoing the completion, sends only `status`, so the reminder stays off.

A move of such a task then fails every time. The copy is POSTed with `isReminderOn: false` and the reminder's time, and Graph answers with `isReminderOn: true`: it ignores `false` on a create that carries a `reminderDateTime`. The move job's check finds the copy differs (`isReminderOn`), deletes the copy and keeps the original, and the operation is `failed`. Nothing is lost.

## Seen live

On the `livetest` instance, a throwaway list: a task with a reminder, completed (`isReminderOn` false), undone (still false), moved (failed as above). A raw POST of `{"isReminderOn": false, "reminderDateTime": …}` came back `isReminderOn: true`.

## Options

- The move copy leaves out `reminderDateTime` when `isReminderOn` is false, and the check ignores it then.
- Undoing a completion (and `tasks reopen`) puts `isReminderOn` back when the reminder is still ahead.
