# 05: Custom features (built on top of the API)

These are features the Graph To Do API doesn't have. Each one stores its data in Microsoft's own structures, so it syncs across BK's machines through Graph without any backend of ours. Where possible it's also visible in the official apps.

All ms-todo open extensions use the name `com.planetaryescape.mstodo` (one extension per list or task). The fields are listed below. Extensions are invisible to the official apps.

A task's extension also holds `opId`, the outbox operation ID of the create that made it, so an uncertain create can be attributed later ([04](04-sync-cache.md#unknown-outcome-d-028)).

**An extension write always sends the whole document** (S2). PATCH replaces the extension rather than merging, so a PATCH of `myDay` alone would wipe `assignee`. So before any extension write, GET the current extension with the filtered `$expand` on that task or list, apply only the fields we're changing on top of it, and PATCH the whole merged document. (POST of an existing name also acts as an upsert.) A race window remains: a write from elsewhere between our GET and PATCH is lost. Whether an extension PATCH honours the parent's `If-Match` is untested (open item in [12](12-open-questions.md#s2-result-2026-09-24)). Integers come back with an `@odata.type` annotation key (`order@odata.type`); ignore those keys when reading. How extension content reaches the cache is in [04](04-sync-cache.md#children-of-a-task).

## My Day

**Approach: an Outlook category called "My Day", plus the date the task was added, stored in an extension. The daemon clears it each day.**

- **Adding a task:** add the category `"My Day"` to its `categories`, and set the extension field `myDay: "YYYY-MM-DD"` to today's local date.
- **Removing a task:** remove the category and clear `myDay`.
- **Daily rollover:** at the first daemon tick after the rollover time (setting `my_day.rollover_time = "HH:MM"`, default `"00:00"` local; D-024), for each task where `myDay` is before today:
  - Remove the category and clear `myDay`, in a batch.
  - Record the rollover in `settings.last_rollover_date` so it only runs once, even across restarts.
  - If the daemon was off for days, run it once when the daemon next starts.
- **A task added on another device.** Someone adds the "My Day" category on the phone (if the phone allows it; pending the S7 phone check), so there's no `myDay` date. The first time the daemon sees it, it sets `myDay` to today. That task then clears at the next midnight like the rest.
- **Setting up the category.** The category has to exist in `/me/outlook/masterCategories` (`MailboxSettings.ReadWrite`). The daemon creates it once, with colour setting `my_day.color`. The default is **`preset3`, which is Yellow**, close to To Do's sun icon (D-030). This replaces an earlier `preset4`, which Microsoft's mapping makes Green (S7). The choice is pending BK's phone check (Q11). The name is configurable (`my_day.category`), with default `My Day`.
- **Changing the category name.** Master category names can't be renamed: a PATCH of `displayName` returns 200 and changes nothing (S7). Names are also unique ignoring case. So changing `my_day.category` means creating a new category and re-tagging every task in My Day from the old name to the new one. The old category is left for the user to delete.
- **Suggestions.** Like the app's "Suggestions" pane, the My Day view offers tasks that are due today, overdue, or were in My Day yesterday and aren't complete. These come from local queries.
- **In the TUI**, My Day is a special view, not a list (see [08](08-tui.md)). The category is hidden from the task's category chips there, because it's shown structurally instead.
- **On the phone**, the task should show a "My Day" category tag, and the app should be able to filter by category, pending the S7 phone check. That's the reason for this approach. Graph accepts and returns the category (S7).
- **Keep it separate from the app's own My Day.** The app's My Day is unreachable through the API, and we don't touch it. `ms-todo` docs and the TUI help text say so.

Why not a special list, or copies of tasks? See D-015. In short: tasks can't move between lists without losing data, and copying them into a "My Day" list shows every task twice on the phone.

## Folders (list groups)

- A list's folder is the extension field `folder: "Work"` on that list. There's one level of folders, like the app, and no nesting.
- Folders exist only as a name on lists. A folder with no lists disappears. Renaming a folder patches every list in it (in a batch). Deleting a folder clears the field and doesn't delete any lists.
- Folder order and list order within a folder can be stored in the same extension (`folderOrder`, `order`), because Graph has no ordering for lists.
- The official apps can't see ms-todo folders. That's accepted.
- List delta reports that a list's extension changed but not what it holds (S2). After any `lists/delta` round that reports a list, one filtered-`$expand` GET refreshes every list's folder and order; see [04](04-sync-cache.md#children-of-a-task).

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

## Sharing: not built

See D-014. The daemon still reads `isShared` and `isOwner` and shows a marker, so shared lists (shared through the official app) display correctly. Nothing more.
