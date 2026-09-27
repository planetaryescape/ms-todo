# 007: a due-date write to a recurring task makes a second task

**Status:** fixed (D-069): every due-date write to a recurring task keeps one task, and My Day leaves a recurring task's date alone. Found 2026-09-27 while checking the undo picker live (D-065). Evidence: [S20](../research/spikes/S20.md).

## What happens

Graph answers a PATCH of `dueDateTime` on a recurring task by moving the series on to its next occurrence and creating a new open recurring task with the date asked for. So:

- `undo` of a recurring completion deletes the completed copy (as designed, 04), then sends the old due date back, and the user is left with the series moved on plus a new open task on the old date: two tasks where there was one.
- `tasks edit --due`, `reschedule` and My Day's due date on a recurring task probably do the same. Not checked.

## What to decide

- Spike first: is it every due-date PATCH on a recurring task, or only some (a date off the pattern, before `range.startDate`, a `recurrenceTimeZone` other than the one sent)? Does sending the recurrence in the same PATCH (with `range.startDate` moved) keep one task?
- Then undo of a recurring completion: perhaps reopen the copy and delete the moved-on series task instead, or stop at deleting the copy and say the series stays moved on.
- Until then, the TUI's and the CLI's undo of a recurring completion leave a duplicate the user must delete.

## What was done (D-069)

- Tried live (S20, parts 2 and 3): the date and the recurrence in one PATCH still split it; the date with `recurrence: null`, then the recurrence, keeps one task, **if the recurrence's `range.startDate` moves to the new date** (with the old start Graph puts the task back on it).
- The outbox sends every PATCH that sets a due date on a recurring task (and doesn't set the recurrence itself) that way, as one operation, so `tasks edit --due`, `reschedule`, undo of a completion and the undo of each are covered, with one undo each.
- My Day's due date: a task that recurs keeps its date when added to a later My Day or taken out, since clearing it dropped the recurrence (S20, part 3).
- Verified live on daily and weekly tasks for each path, and against a fake Graph that splits as Graph does.
- The recurrence set again is Graph's, read before the first PATCH, and it's kept in the operation, so a retry after the second PATCH was refused sets it from there; where Graph put an off-pattern date is kept for undo (review of b58f9fe).
- Left: if the second PATCH (the recurrence) is rejected for good, the task has no recurrence until `outbox retry`.
