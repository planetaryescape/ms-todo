# 007: a due-date write to a recurring task makes a second task

Found 2026-09-27 while checking the undo picker live (D-065). Evidence: [S20](../research/spikes/S20.md).

## What happens

Graph answers a PATCH of `dueDateTime` on a recurring task by moving the series on to its next occurrence and creating a new open recurring task with the date asked for. So:

- `undo` of a recurring completion deletes the completed copy (as designed, 04), then sends the old due date back, and the user is left with the series moved on plus a new open task on the old date: two tasks where there was one.
- `tasks edit --due`, `reschedule` and My Day's due date on a recurring task probably do the same. Not checked.

## What to decide

- Spike first: is it every due-date PATCH on a recurring task, or only some (a date off the pattern, before `range.startDate`, a `recurrenceTimeZone` other than the one sent)? Does sending the recurrence in the same PATCH (with `range.startDate` moved) keep one task?
- Then undo of a recurring completion: perhaps reopen the copy and delete the moved-on series task instead, or stop at deleting the copy and say the series stays moved on.
- Until then, the TUI's and the CLI's undo of a recurring completion leave a duplicate the user must delete.
