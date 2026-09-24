# 05: Custom features (built on top of the API)

These are features the Graph To Do API doesn't have. Each one stores its data in Microsoft's own structures, so it syncs across BK's machines through Graph without any backend of ours. Where possible it's also visible in the official apps.

All ms-todo open extensions use the name `com.planetaryescape.mstodo` (one extension per list or task). The fields are listed below. Extensions are invisible to the official apps.

## My Day

**Approach: an Outlook category called "My Day", plus the date the task was added, stored in an extension. The daemon clears it each day.**

- **Adding a task:** add the category `"My Day"` to its `categories`, and set the extension field `myDay: "YYYY-MM-DD"` to today's local date.
- **Removing a task:** remove the category and clear `myDay`.
- **Daily rollover:** at the first daemon tick after local midnight (setting `my_day.rollover_hour`, default 0), for each task where `myDay` is before today:
  - Remove the category and clear `myDay`, in a batch.
  - Record the rollover in `settings.last_rollover_date` so it only runs once, even across restarts.
  - If the daemon was off for days, run it once when the daemon next starts.
- **A task added on another device.** Someone adds the "My Day" category on the phone, so there's no `myDay` date. The first time the daemon sees it, it sets `myDay` to today. That task then clears at the next midnight like the rest.
- **Setting up the category.** The category has to exist in `/me/outlook/masterCategories` (`MailboxSettings.ReadWrite`). The daemon creates it once, with colour setting `my_day.color`, default `preset4`, a yellowish colour close to To Do's sun icon; check it in the spike. The name is configurable (`my_day.category`), with default `My Day`.
- **Suggestions.** Like the app's "Suggestions" pane, the My Day view offers tasks that are due today, overdue, or were in My Day yesterday and aren't complete. These come from local queries.
- **In the TUI**, My Day is a special view, not a list (see [08](08-tui.md)). The category is hidden from the task's category chips there, because it's shown structurally instead.
- **On the phone**, the task shows a "My Day" category tag, and the app can filter by category. That's the reason for this approach.
- **Keep it separate from the app's own My Day.** The app's My Day is unreachable through the API, and we don't touch it. `ms-todo` docs and the TUI help text say so.

Why not a special list, or copies of tasks? See D-015. In short: tasks can't move between lists without losing data, and copying them into a "My Day" list shows every task twice on the phone.

## Folders (list groups)

- A list's folder is the extension field `folder: "Work"` on that list. There's one level of folders, like the app, and no nesting.
- Folders exist only as a name on lists. A folder with no lists disappears. Renaming a folder patches every list in it (in a batch). Deleting a folder clears the field and doesn't delete any lists.
- Folder order and list order within a folder can be stored in the same extension (`folderOrder`, `order`), because Graph has no ordering for lists.
- The official apps can't see ms-todo folders. That's accepted.
- **Spike S2** checks that list delta reports extension changes. If it doesn't, list extensions get refreshed by a cheap periodic `GET /lists?$expand=extensions`, which is one request.

## Assignment

- The extension field `assignee: "<free text or email>"` on a task. It means something only to ms-todo. There's no notification and no second user.
- BK doesn't share lists (D-014), so this is really a personal "waiting on <person>" label. Pair it with `status = waitingOnOthers` by default, which *is* a real Graph status the app shows.
- CLI filter: `ms-todo tasks list --assignee "Sam"`. The TUI shows a person chip.

## Move between lists

Graph has no move operation. ms-todo implements `task move` as **a copy that checks its work and loses nothing:**

1. Create the task in the target list with every field copied (from `raw_json`).
2. Copy the checklist items, including whether each is checked, plus linked resources, attachments (download, then upload; big ones through an upload session) and the extension.
3. Check the copy: fetch it and compare counts and fields with the source.
4. **Only then** delete the source. If any step fails, delete the half-built copy and leave the source untouched. Report the error.
5. Keep the local ID the same, pointing at the new Graph ID, so clients don't notice the switch.

What can't be kept: `createdDateTime`, which becomes the move time. We store the original in the extension (`originalCreatedAt`) and show it. The Graph ID changes, which is unavoidable.

This fixes MAG&Cie's lossy `move_task` (see prior-art.md).

## Sharing: not built

See D-014. The daemon still reads `isShared` and `isOwner` and shows a marker, so shared lists (shared through the official app) display correctly. Nothing more.
