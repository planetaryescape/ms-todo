# 05: Custom features (built on top of the API)

These are features the Graph To Do API doesn't have. Each one stores its data in Microsoft's own structures, so it syncs across BK's machines through Graph without any backend of ours. Where possible it's also visible in the official apps.

All ms-todo open extensions use the name `com.planetaryescape.mstodo` (one extension per list or task). The fields are listed below. Extensions are invisible to the official apps.

A task's extension also holds `opId`, the outbox operation ID of the create that made it, so an uncertain create can be attributed later ([04](04-sync-cache.md#unknown-outcome-d-028)).

**An extension write always sends the whole document** (S2). PATCH replaces the extension rather than merging, so a PATCH of `myDay` alone would wipe `assignee`. So before any extension write, GET the current extension with the filtered `$expand` on that task or list, apply only the fields we're changing on top of it, and PATCH the whole merged document. (POST of an existing name also acts as an upsert.) A race window remains: a write from elsewhere between our GET and PATCH is lost. Whether an extension PATCH honours the parent's `If-Match` is untested (open item in [12](12-open-questions.md#s2-result-2026-09-24)). Integers come back with an `@odata.type` annotation key (`order@odata.type`); ignore those keys when reading. How extension content reaches the cache is in [04](04-sync-cache.md#children-of-a-task).

## My Day

**Approach: ms-todo's My Day is a date in our extension. A task with no due date also gets today as its due date, so the phone app shows it in its own My Day, and ms-todo takes that date away again at the rollover (D-037).**

- **Adding a task:** set the extension field `myDay: "YYYY-MM-DD"` to today's local date. Like every extension write, it reads, merges and PATCHes the whole document ([04](04-sync-cache.md#children-of-a-task)).
  - **If the task has no due date,** also set `dueDateTime` to today and record `myDayDueSet: true` in the extension. The app then shows the task in its own My Day, on devices where "Show 'Due Today' tasks in My Day" is on.
  - **If it has a due date,** leave it. The app shows it in its My Day only on the day it's actually due.
- **Removing a task:** clear `myDay`. Whether a due date ms-todo set is cleared at once is Q13 in [12](12-open-questions.md#product-questions-for-bk) (placeholder: yes, by the rollover's rule).
- **Daily rollover:** at the first daemon tick after the rollover time (setting `my_day.rollover_time = "HH:MM"`, default `"00:00"` local; D-024), for each task where `myDay` is before today:
  - Clear `myDay`, in a batch.
  - If `myDayDueSet` is true, clear it too, and if the task isn't completed and its due date is still the one ms-todo set, remove the due date. A due date the user set or moved is never touched. A completed task keeps its date.
  - Record the rollover in `settings.last_rollover_date` so it only runs once, even across restarts.
  - If the daemon was off for days, run it once when the daemon next starts.
- **Across machines.** The extension lives in Graph, so every ms-todo install on the account sees the same My Day. An extension change bumps the task's etag, delta reports the task, and the daemon fetches the extension (D-029). Two machines rolling over make the same changes, which is harmless.
- **Suggestions.** Like the app's "Suggestions" pane, the My Day view offers tasks that are due today, overdue, or were in My Day yesterday and aren't complete. These come from local queries.
- **In the TUI**, My Day is a special view, not a list (see [08](08-tui.md)). In the CLI it's `ms-todo myday`, the `--my-day` flag and the `+myday` quick-add token.
- **The app's own My Day** can't be read or written through Graph. With "Show 'Due Today' tasks in My Day" on, it holds every task due today, including ones ms-todo never added (S12), and tasks added in the app don't reach ms-todo's My Day. The setting isn't in Graph, so `ms-todo doctor` can't check it; the docs and `doctor` say so.

Why not a special list, copies of tasks, an Outlook category, or always setting the due date? See D-015 and D-037. In short: tasks can't move between lists without losing data, copying them into a "My Day" list shows every task twice on the phone, the phone doesn't show categories, and overwriting real due dates would hide them in the Planned and overdue views.

## Folders (list groups)

- A list's folder is the extension field `folder: "Work"` on that list. There's one level of folders, like the app, and no nesting.
- Folders exist only as a name on lists. A folder with no lists disappears. Renaming a folder patches every list in it (in a batch). Deleting a folder clears the field and doesn't delete any lists.
- Folder order and list order within a folder can be stored in the same extension (`folderOrder`, `order`), because Graph has no ordering for lists.
- The official apps can't see ms-todo folders. That's accepted.
- List delta reports that a list's extension changed but not what it holds (S2). After any `lists/delta` round that reports a list, one filtered-`$expand` GET refreshes every list's folder and order; see [04](04-sync-cache.md#children-of-a-task).

**As built (rung 5c, D-047):**

- The fields are `folder` (a string), `order` (the list's place in its folder, or among the lists in no folder) and `folderOrder` (its folder's place among the folders, the same on every list in it). A blank `folder` is no folder. Missing numbers sort after every number; ties keep the order ms-todo first saw the lists in. `lists order` and `folders order` number the whole group 1, 2, 3, … and write only the lists whose number changed. A list moved into a folder drops its `order`, so it goes after the folder's numbered lists, and takes the folder's `folderOrder`.
- Folder names match ignoring case (an exact match first), and an existing folder's spelling wins. Renaming to another folder's name is refused rather than merging by accident; `lists move … --folder` merges on purpose.
- Every change is an outbox operation per list (op `extension`, `entity_kind` `list`): the cache changes at once and the worker sends GET (the list with the filtered `$expand`), merge and write. The write is a PATCH of the whole document; a list with no extension yet gets a POST (an upsert, S2); and a document left with no fields is **deleted**, because Graph answers a PATCH of `{}` with 400 `RequestBroker--ParseUri` (seen live on 2026-09-25). All three are idempotent, so a folder write is resent after a failure and is never `unknown`. Graph's `id`, `extensionName` and `@odata` annotations are never sent back.

## Assignment

- The extension field `assignee: "<free text or email>"` on a task. It means something only to ms-todo. There's no notification and no second user.
- BK doesn't share lists (D-014), so this is really a personal "waiting on <person>" label. Pair it with `status = waitingOnOthers` by default, which *is* a real Graph status the app shows.
- CLI filter: `ms-todo tasks list --assignee "Sam"`. The TUI shows a person chip.

## Move between lists

Graph has no move operation. ms-todo implements `task move` as **a copy that checks its work and loses nothing:**

1. Create the task in the target list with every field copied (from `raw_json`), carrying the move job's own `opId`. The source's `opId` is never copied.
2. Copy the checklist items, including whether each is checked, plus linked resources, attachments (download, then upload; big ones through an upload session) and the extension.
3. Check the copy: fetch it and compare counts and fields with the source.
4. **Only then** delete the source. If a step before this one fails, delete the half-built copy and leave the source untouched, and report the error. Once the source DELETE may have started, that promise no longer applies; recovery follows [04](04-sync-cache.md#instant-local-writes)'s rules instead.
5. Keep the local ID the same, pointing at the new Graph ID, so clients don't notice the switch.

If any copy step's outcome is unknown (a lost response to a child create or to the final upload chunk), the move pauses: it keeps the source and the partial target, deletes nothing, and waits for the user in `ms-todo outbox list`. The half-built copy is deleted only when every step's outcome is known.

The move runs as an outbox job that saves each step as it goes. If the daemon stops half-way, it resumes or rolls back the move on start, and never deletes the source before the copy checks out. Once the source DELETE may have started, it never deletes the target; the recovery rules are in [04](04-sync-cache.md#instant-local-writes) (vault: `A Detached Child Outlives Its Supervisor`).

What can't be kept: `createdDateTime`, which becomes the move time. We store the original in the extension (`originalCreatedAt`) and show it. The Graph ID changes, which is unavoidable.

This fixes MAG&Cie's lossy `move_task` (see prior-art.md).

**As built (rung 5e, D-051, S14):**

- Steps 1 and 2 are one POST for everything but attachments: the fields, the checklist items (checked or not, with when), the linked resource (Graph allows one per task) and our extension with the move's `opId` and `originalCreatedAt` (kept from an earlier move). So the copy and its children are one create, attributable by `opId`. Attachments follow one at a time: a POST under 3 MiB, else an upload session to `<uploadUrl>/content`. Their bytes are read from the source first and kept in a 0600 spool until the move is settled; one over 25 MB is refused before anything is written.
- A recurring task's due and start dates are written as their local day in the zone its recurrence reports, the rest as their local day in the user's zone (S14).
- Step 3 compares the copy with the source field by field, children by what they hold, attachments by byte count and sha256, and checks the target list is live and the source unchanged since it was read. A mismatch deletes the copy and keeps the source.
- `tasks move T… --to L` and the TUI's `m` move several tasks as one command; `undo` moves them back the same way, per task, and not a task that has moved or changed since.
- The To Do apps show the copy as a new task: its `createdDateTime` is the move's. ms-todo shows `originalCreatedAt` in the task's JSON, under its extension.

## Sharing: not built

See D-014. The daemon still reads `isShared` and `isOwner` and shows a marker, so shared lists (shared through the official app) display correctly. Nothing more.
