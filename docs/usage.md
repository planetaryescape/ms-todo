# Using ms-todo

Look up any command's options, output shape and behaviour here. For a first look, the [README](../README.md) has the tour, the recipes and the TUI keys. `ms-todo <command> --help` is always the last word on flags; this page explains what they do.

Every example uses `ms-todo`; `mst` is the same binary.

## See your tasks

```sh
ms-todo lists list                    # every list, folder by folder
ms-todo tasks list                    # every task in "Tasks", completed ones included
ms-todo tasks list --list Groceries   # by exact name, or by the ID from `lists list`
ms-todo raw GET /me/todo/lists        # any Graph v1.0 path, authenticated
ms-todo auth bearer --reveal-secret   # a valid access token, for curl
```

Output is a table in a terminal and JSON when piped. `--format json|jsonl|ids|csv|table` picks one. JSON is `{ "schema_version": 2, "sync": { "state", "generation" }, "items": [...] }`. Each item has every field Graph returns, except that `id` is ms-todo's own stable ID, with Graph's beside it as `graph_id` (commands take either), plus `sync_state` (below). `sync.state` is `initial` until that list's first sync has finished, so an empty `initial` list isn't really empty (other formats say so on stderr). `--format ids` prints one ID per line. `--format csv` has a header row and fixed columns (tasks: `id,title,status,importance,due,reminder,categories,created,modified,sync_state`; lists: `id,name,wellknown,is_owner,is_shared,folder,sync_state`); a task's notes aren't a column, so use JSON for those. A `--list` name that matches more than one list is an error that lists the candidates; ms-todo never picks one for you.

## Find a task

```sh
ms-todo search insurance                       # open tasks in every list, best match first
ms-todo search car insurance                   # every word must appear, in the title or the notes
ms-todo search '"car insurance"' --status all  # an exact phrase, completed tasks too
ms-todo search 'insur* NOT renew' --list Tasks --format csv
ms-todo tasks list --list Home --search boiler # one list's matches, in the `tasks list` shape
```

A task's links, and opening one:

```sh
ms-todo tasks links <TASK>              # its linked resources, then the URLs in its notes
ms-todo tasks open <TASK> --index 2     # in the browser or mail app; http, https and mailto only
```

`tasks links` numbers them from 1 (every format; CSV is `url,text,source`). `tasks open` with one link opens it; with several and no `--index` it lists them and exits 2, so a script never waits on a choice.

Search looks through every task's title and notes (html notes as text), ignoring case and accents, and ranks title matches first. Words can end in `*` to match a prefix; `OR`, `NOT` and parentheses work too, in capitals. It shows open tasks by default (`--status completed|all` for the rest) and at most 50 (`--limit`). Each JSON item is the task plus `list`, its list's name, and `snippet`, the passage that matched with each match between `**`; CSV has `id,title,list,status,due,snippet`. A query ms-todo can't read, like `OR milk` or an unclosed quote, exits 2. It answers from the local cache, in a few milliseconds.

## Quick add

Type a task the way you'd say it, and ms-todo files it:

```sh
mst tasks add "Pay rent every 1st #Finances p1 9am"
#   "Pay rent" in Finances, importance high, every month on the 1st,
#   due the next 1st, with a reminder at 09:00
mst tasks add "Call mum in 2 days"          # "Call mum", due the day after tomorrow
mst tasks add "Dentist fri 3pm @errands"    # due Friday, reminder at 15:00, category errands
mst tasks add "Stand-up every weekday 9:30" # weekly Monday to Friday, reminder at 09:30
mst tasks parse "Tax return #\"Admin stuff\" 31 jan start mon"   # shows the reading, adds nothing
mst tasks add "Email Friday's report" --no-parse   # the text as the title, exactly as given
```

| Typed | Means |
| --- | --- |
| `#Home`, `#"Two words"` | the list: its name or the start of it, when only one list starts that way. With none, "Tasks" |
| `@errands` | a category (Outlook's). One you don't have yet is still put on the task; `--create-categories` creates it |
| `p1` `p2` `p3` `p4` | importance: p1 high, p2 and p3 normal, p4 low. Lower case only, so `P1 incident` stays a title |
| `tomorrow`, `fri 5pm`, `in 3 days`, `on 12 oct`, `9am` | the due date. A time also sets a reminder then (Microsoft To Do keeps no time on a due date); a time alone is its next one |
| `!9am`, `!tomorrow 8:30` | a reminder only; a day alone is 09:00 on it |
| `start mon` | the start date. With no due date, Microsoft To Do makes it the due date too, and ms-todo says so |
| `every day`, `daily`, `every 3 days`, `every weekday`, `every mon, wed`, `every other week`, `every 2 weeks on fri`, `every 1st`, `every month on the 15th`, `every last friday`, `every year`, `every 12 oct` | a recurrence, due first on its next day (or the date you typed). `until 31 dec`, `for 10 times` and a time (`every mon 9am`) can follow |
| `"quoted text"`, `\#`, `\@`, `\!` | kept as typed |
| `+myday`, `*` | recognised, but My Day arrives in rung 7, so it's only a warning for now |

What isn't recognised stays in the title. The date words are whole words only, so `Monitor the build`, `Sat nav update`, `Ask Tom about invoice`, `Call May about the lease`, `Email Friday's report` and `Fix the 9am standup bot` keep their titles; `tom`, `tod` and `sat` count only in lower case. Only the first date counts; others stay in the title with a warning. Flags always win over the text: `--list`, `--due`, `--reminder` and `--importance` replace what it says, and `--due -` or `--reminder -` clears it (a recurrence or a start date needs its due date, so `--due -` with one is refused). A `#List` that two lists share by name is a warning, not a guess. Anything typed but not used (an unknown `#List`, a second date) is a `note:` on stderr, and in `tasks parse`'s `warnings`. An agent's or a script's text should use `--no-parse` with flags, so a title is never read as a date.

## List suggestions

Optional, and off unless you turn it on. For a task headed for the inbox (no `#List`, no `--list`, not added from a list in the TUI), ms-todo asks [TypeSafe](https://typesafe.ai)'s Jev model which of your lists it belongs in, and shows the answer only when the model is at least `min_confidence` sure. It never files a task by itself.

```toml
# ~/.config/ms-todo/config.toml
[suggest]
enabled = true
min_confidence = 0.8           # 0 to 1; how sure the model must be
exclude_folders = ["Archive"]  # lists in these folders are never suggested (any case)
# The API key: TYPESAFE_API_KEY in the daemon's environment, else what this
# command prints on its first line. It runs directly, never through a shell:
# an array is the exact argv; a string is split as a shell would (quote words).
api_key_command = ["op", "read", "op://Private/TypeSafe/credential"]
```

Run `ms-todo daemon stop` after changing it: the daemon reads it when it starts, and runs the key command once. A password manager that asks you to approve each read may not be able to ask from the background daemon; then start the daemon with `TYPESAFE_API_KEY` set instead.

```sh
mst tasks suggest-list "pay council tax"          # Finances (0.93), or "no suggestion"
mst tasks suggest-list "pay council tax" --format json
#   {"schema_version": 2, "title": …, "list_id": …, "list_name": "Finances", "confidence": 0.93}
mst tasks add "pay council tax"                   # adds to Tasks, then a note: suggested list: Finances …
```

- `tasks add` prints the note on stderr only in table, CSV and ids formats; in JSON it doesn't ask. A dry run's note says to add it with `--list` or `#List`; a real add's says how to move it.
- In the TUI's add box, the suggestion comes a moment after you stop typing, as `→ Finances? (Ctrl-l to accept)`. `Ctrl-l` adds `#Finances` to the text, which you can still edit.
- What it sends: the task's title, and for each list that isn't built in or in an excluded folder, its folder, name and up to five open task titles. Only the daemon sends it. `ms-todo doctor` shows whether it's on and what it sends.
- Anything going wrong (no key, no network, TypeSafe slow past 3 seconds or answering with an error) means no suggestion, never a failed command. The daemon log notes it once, and `doctor` lists it as a `suggest: …` problem until a suggestion works again. With it off, `tasks suggest-list` exits 2 and says how to turn it on.

## Add and finish tasks

```sh
ms-todo tasks add "Buy milk" --list Groceries --due tomorrow
ms-todo tasks add "Call the dentist" --reminder "fri 9:30am" --importance p1 --body "re: filling"
ms-todo tasks complete <ID>...        # IDs from `tasks list --format ids`
ms-todo tasks reopen <ID>...
ms-todo tasks edit <ID> --title "Buy oat milk" --due 2026-09-27   # also --clear-due, --reminder, --clear-reminder, --importance, --body
ms-todo tasks delete <ID>...          # asks first; pass --yes when not in a terminal
ms-todo tasks list --format ids | ms-todo tasks complete -   # `-` reads IDs from stdin
```

- `tasks add` reads its text for dates and more ([Quick add](#quick-add)); `--no-parse` takes it as the title, exactly as given. With no `--list` or `#List`, it goes to "Tasks".
- Due dates are dates only; put a time in `--reminder`. Dates are written in your local time zone (`TZ`, or the system's).
- `--due` takes `2026-10-02` or a phrase: `today`, `tomorrow` (`tom`), `yesterday`, `fri` (the next one, never today), `this fri`, `next fri` (next week's), `in 3 days`, `three days from today`, `+2w`, `-1d`, `2 days ago`, `next week` (its Monday), `next month` (the 1st), `eow`, `eom`, `12 oct`, `oct 12`, `12/10` (day first). A day and month already past means next year's. `--reminder` takes the same with a time: `17:30` alone (today's, or tomorrow's once it's past), `tomorrow 9am`, `fri 5:30pm`, `noon`, `2026-10-02 09:30`. On `tasks edit`, an empty value or `-` clears either. A phrase ms-todo can't read exits 2 and names the part it didn't understand.
- `--importance` takes `high`, `normal` or `low`, or Todoist's levels: `1` or `p1` is high, `2`, `3`, `p2` and `p3` are normal (Microsoft To Do has one level for both), `4` or `p4` is low.
- A task can also be named by its exact title, together with `--list`. A title several tasks share is an error listing them.
- `--dry-run` shows what a command would change, resolved exactly as the real run would, and changes nothing.
- Completing a recurring task keeps it open with the next due date, and Microsoft To Do adds the occurrence you finished as a new, completed task.
- `--idempotency-key KEY` makes a change run at most once: repeating the command with the same key returns the first result for 24 hours without sending anything, and the same key with a different change exits 2.
- Each change answers at once with the task as ms-todo now has it and an `op_id`, and the daemon sends it to Microsoft To Do in the background (see [Offline, and never lose a write](#offline-and-never-lose-a-write)).
- If someone changed the same field on another device since ms-todo last read the task, the change is rejected (and rolled back) rather than overwriting theirs.

## What you finished, and clearing what's overdue

```sh
ms-todo done --since yesterday                          # for a standup: grouped by day, newest first
ms-todo done --since mon --list Work --format csv       # a sheet of this week's work
ms-todo reschedule --overdue --to today                 # every overdue open task, due today
ms-todo reschedule --due-before fri --folder Areas --to "next mon" --dry-run
ms-todo tasks edit - --importance high --yes < ids.txt  # one change to several tasks
ms-todo undo                                            # puts the whole batch back
```

- `done` lists completed tasks from the cache, by the day they were completed, newest first: the last 7 days unless `--since` says otherwise. `--since` and `--until` (both included) take a day, read looking back: `yesterday`, `mon` (the latest Monday, today included), `last week` (its Monday), `this month`, `12 sep` (the latest one), `3 days ago`, `2026-09-01`. `--list` or `--folder` narrow it, and `--limit N` keeps the newest N. JSON and CSV give each task `completed_on` (`YYYY-MM-DD`) and `list`; CSV's columns are `id,title,list,completed_on,due,importance,sync_state`.
- Microsoft To Do keeps the day a task was completed, not the time: a UTC date (midnight UTC), which is the day `done` shows. Just after midnight, while your local date is ahead of UTC's, a completion lands on the UTC date, the day before. A completion that hasn't reached Microsoft To Do yet has no day: it's listed first as "Not synced yet", with `completed_on` null.
- `reschedule --to DAY` moves the due date of open tasks: `--overdue` (due before today), `--due-before DAY`, or the tasks named (`-` reads IDs from stdin), in `--list` or `--folder` or every list. `tasks edit` takes the same `--overdue`, `--due-before` and several task IDs for `--due`, `--importance` and `--reminder`; a title or notes change one task at a time.
- A change that may reach more than one task shows the tasks and asks first in a terminal; anywhere else it needs `--yes` (exit 2 otherwise). `--dry-run` shows the plan. What runs after a yes is the tasks you were shown.
- However many tasks it moves, it's one change with one `op_id`, so one `ms-todo undo` puts them all back. A task whose due date has changed again since is left alone and listed under `refused`; only when every task changed is the undo refused (exit 5).

## Move tasks between lists

```sh
ms-todo tasks move <ID> --to Groceries             # steps, link, attachments and all
ms-todo tasks move <ID> <ID> --to Someday --yes    # several at once; asks first in a terminal
ms-todo tasks move <ID> --to Groceries --dry-run
ms-todo undo                                       # moves them back
```

Microsoft To Do has no move, so ms-todo copies the task into the other list with every field, its steps (ticked or not), its link, its attachments byte for byte and ms-todo's own data, reads the copy back to check it matches, and only then deletes the original. The task keeps its ID in ms-todo and shows in the new list at once, `pending` until the move is done. If a step fails before the delete, the half-made copy is deleted and the original is untouched; if Microsoft To Do doesn't answer a step, the move pauses in `ms-todo outbox list` and deletes nothing until it's found or you decide (`outbox retry` or `outbox discard`). The To Do apps show the moved task as created at the time of the move; ms-todo keeps the original time as `originalCreatedAt` in its own data. A task that has changed or moved again since isn't moved back by `undo`.

## Group lists into folders

```sh
ms-todo lists move Finances Health --folder Areas   # one list or several; the folder is made if it's new
ms-todo lists move Groceries --no-folder            # out of its folder
ms-todo folders list                                # each folder, its lists and open tasks
ms-todo folders rename Areas Responsibilities       # every list in it moves with it
ms-todo folders delete Someday --yes                # the lists stay, in no folder; nothing is deleted
ms-todo folders order Projects --before Areas
ms-todo lists order Health --before Finances        # within a folder
```

Folders work like the To Do app's list groups, one level deep, and exist only as a name on each list: a folder with no lists is gone. They're kept in ms-todo's own data on each list in Microsoft To Do, so every ms-todo you sign in to shows them after its next sync, while the To Do apps don't see them. A folder name that exists matches ignoring case. `lists list` gives each list's `folder` (null for none) and lists them folder by folder, then those in no folder; lists without an order go last, in the order ms-todo first saw them. Every folder change is a change like any other: queued, sent in the background, shown `pending` until then, undone with `ms-todo undo`, and it takes `--dry-run` and `--idempotency-key`. One command that moves or renames several lists is one change, so one `undo` reverses all of it.

## Offline, and never lose a write

Every change is applied to the local cache and queued in an outbox in one step, so it answers in milliseconds even with no network. The daemon sends the queue in order, each task's changes one after another, and backs off while the network is down. Each task shows how its changes are doing in `sync_state`, and `tasks list` marks it in its `SYNC` column:

- `synced`: Microsoft To Do has it.
- `pending`: queued or being sent.
- `unknown`: it was sent, but no answer said whether Microsoft To Do applied it (a timeout or a server error on a create or a recurring completion). ms-todo never resends these by itself, since that could make a duplicate. After each sync it looks for the task by the `op_id` it carries; when found, the task becomes `synced`. After 24 hours it's flagged for you.
- `failed`: Microsoft To Do rejected it, for example because the list was deleted on another device. The change is rolled back but kept, with its content.

```sh
ms-todo outbox list                   # every queued change and its state; --state pending|inflight|unknown|failed|done
ms-todo outbox retry <OP>             # send an unknown or failed change again (you chose to)
ms-todo outbox discard <OP> --yes     # drop a change; one that never reached Microsoft To Do is undone locally
ms-todo undo                          # reverse the latest change; or `ms-todo undo <OP_ID>`
```

Undo is itself a change, so it can be undone. An edit, complete, reopen or folder change is undone only while what it set is still there: if a later change or another device has changed that field since, `undo` refuses with exit 5 (`conflict`) rather than overwrite it, and changes nothing. For a change to several tasks the rule is per task: the ones changed since are left alone and named in `refused`, and the rest are undone. Undoing an add deletes the task; an edit, complete or reopen puts the fields back; a delete brings the task back with the same ID (and a new Graph ID). Undoing the completion of a recurring task also deletes the completed copy Microsoft To Do made, so it asks which one: `ms-todo undo <OP_ID> --copy <ID>` (without `--copy`, it lists the candidates and exits 2).

A database upgraded by a newer ms-todo is refused with "this database was upgraded by a newer ms-todo; install the latest version" (error kind `database_too_new`, exit 1).

`ms-todo raw POST|PATCH|DELETE PATH [--body JSON]` sends a request straight to Graph for debugging. It's sent once, never resent, and needs `--yes` when not in a terminal.

Agents can use the skill in [`skills/ms-todo/SKILL.md`](../skills/ms-todo/SKILL.md).

The exit codes are under [Exit codes](#exit-codes).

## The TUI

```sh
mst                  # in a terminal, the same as `mst tui` or `ms-todo tui`
mst tui --ascii      # plain ASCII instead of Unicode symbols
mst tui --theme nord # draw with a theme (mst tui --list-themes names them)
```

`mst` with no command opens the TUI only when both its input and output are a terminal; from a script, a pipe or an agent it prints help and exits 2, as before. Global flags still work (`mst --instance work`); TUI flags such as `--ascii` need `tui`.

A title bar with the version and the view you're in, a sidebar of smart views (Important, Planned, All, Completed), then your folders, each with its lists under it and their total, then the lists in no folder, all with their counts, the task list, and a detail pane. It opens from the local cache, and changes made anywhere, the phone included, show up as the daemon syncs them. A change you make shows at once, marked pending (dim) until it reaches Microsoft To Do; unknown outcomes are amber and rejected changes red, with a banner saying why.

The keys are in the README's [TUI keys](../README.md#tui-keys) table; `?` inside the TUI lists them all.

In the editor, a due date or a reminder takes what `--due` and `--reminder` take (`tomorrow`, `fri 17:30`, `+2w`, `12 oct`), and shows what it resolves to as you type (`→ Fri 2 Oct`, or `, in the past`); empty or `-` clears it, and input it can't read says why and sends nothing. Importance is picked by level: `1` high, `2` or `3` normal, `4` low (or `h`, `n`, `l`), saved at once. Notes are plain text on several lines: notes written as html on another device are shown as text, and only rewritten as text if you change them. A selection holds only tasks in the view on screen: switching views clears it, and a task that leaves the view drops out of it. The Completed view is grouped by the day each task was completed: Today, Yesterday, then `Mon 21 Sep` and so on.

### Themes

The default theme, `terminal`, draws with your terminal's own colours (its ANSI palette, dim and bold), so a Ghostty or iTerm theme carries over. The others are fixed palettes: `catppuccin-mocha`, `catppuccin-latte`, `gruvbox-dark`, `gruvbox-light`, `tokyo-night`, `nord`, `one-dark`, `kanagawa`, `night-owl`, `cobalt2` and `high-contrast`. All of them keep borders and secondary text quiet, mark the selected row with a soft background, and keep colour for what means something: overdue, important, sync state and errors.

Pick one in the TUI with `:` then "Theme…": each theme shows as you move to it, `Enter` keeps it and writes it to `config.toml`, and `Esc` puts the old one back. Or set it yourself:

```toml
# ~/.config/ms-todo/config.toml
[tui]
theme = "catppuccin-mocha"

[tui.colors]           # optional: change single roles
overdue = "#ff5f5f"    # #rrggbb, an ANSI name (red, bright-blue, dark-gray), 0-255, or reset
```

The roles are `background`, `text`, `text_dim`, `text_muted`, `border`, `border_focused`, `title`, `selection_bg`, `selection_fg`, `accent`, `overdue`, `due_today`, `important`, `completed`, `sync_pending`, `sync_unknown`, `sync_failed`, `error`, `warning`, `banner_error`, `banner_info`, `search_match`, `link`, `cursor` and `header_bar`. `--theme` wins over the file. An unknown theme, role or colour stops the TUI with an error naming it. The fixed palettes need truecolor, which the terminal announces with `COLORTERM=truecolor`; without it they're brought to the nearest of the 256 colours. `NO_COLOR` turns colour off whatever the theme: bold, dim and reverse carry the meaning instead.

Everything the TUI does is also a command, so scripts and agents use the commands. `mst tui --bench-startup` measures the start and a run of keys against your cache and prints the timings; `MS_TODO_TUI_TRACE=<file>` writes every keypress's timing to a file.

## Keep it fresh

The daemon syncs when it starts, every 20 seconds while you're using ms-todo (a client is connected, or asked for anything in the last 10 minutes), every 5 minutes otherwise, and when asked:

```sh
ms-todo sync --wait    # returns once a sync that started after it has finished; progress on stderr
ms-todo sync           # just asks for one
ms-todo doctor         # sign-in, daemon, the cache's path and size, each list's sync state and mode, the last error, the outbox by state
ms-todo schema tasks list   # the JSON schemas of a command's input and output; `ms-todo schema` for all
```

A change made on your phone shows up after the next sync. The first sync of each list reads all of it; after that, a sync asks Microsoft Graph only for what changed (delta), fetches ms-todo's own data for those tasks, and removes what's gone. If Graph drops the delta link, that list is read whole again and anything deleted meanwhile is removed. A sync never overwrites or removes a task with a change still queued, pending or unknown, and asks for a sync right after the outbox sends something. `doctor` shows each list's mode (`delta` or `enumeration`) and when its last delta ran.

## The daemon

The first command starts a small background daemon, which is the only process that talks to Graph, refreshes your sign-in and keeps the cache (`ms-todo.db`, SQLite, in the data directory). You don't need to manage it, but you can:

```sh
ms-todo daemon status
ms-todo daemon stop    # returns once the daemon's process has exited
ms-todo daemon start
```

Its socket is private to your user (0600, in a 0700 directory), and its log is `daemon.log` in the data directory's `logs/`. A newer ms-todo restarts an older daemon by itself.

## Files

| What | Where |
| --- | --- |
| Config | `~/.config/ms-todo/config.toml` on macOS and Linux. `$XDG_CONFIG_HOME/ms-todo/` or `$MS_TODO_CONFIG_DIR` move it |
| Data (cache, sign-in, logs) | `~/Library/Application Support/ms-todo/` on macOS, `$XDG_DATA_HOME/ms-todo/` (`~/.local/share/ms-todo/`) on Linux |
| Cache | `ms-todo.db` in the data directory (SQLite) |
| Sign-in | `auth/token.json` in the data directory, mode 0600 |
| Daemon log | `logs/daemon.log` in the data directory |
| Daemon socket | `$XDG_RUNTIME_DIR/ms-todo/` where there is one, else `run/` in the data directory |

An instance other than the default adds its name: `ms-todo-work` for `--instance work`. `ms-todo auth status` prints the token and config paths; `ms-todo doctor` prints the database's.

## Environment variables

| Variable | Does |
| --- | --- |
| `MS_TODO_INSTANCE` | Use a separate copy of ms-todo's data, as `--instance` does. Builds run from Cargo's `target/` default to `dev`; `default` is the installed copy |
| `MS_TODO_CLIENT_ID` | The Entra app to sign in as, over `[auth] client_id` and the ID release builds carry |
| `MS_TODO_CONFIG_DIR` | Where `config.toml` lives |
| `TYPESAFE_API_KEY` | The TypeSafe API key for [list suggestions](#list-suggestions), read by the daemon when it starts |
| `NO_COLOR` | Turn colour off, in the TUI and in `search`'s bold matches |
| `COLORTERM` | `truecolor` lets the TUI's fixed themes use exact colours; without it they use the nearest of 256 |
| `TZ` | The time zone dates are read and written in |
| `MS_TODO_TUI_TRACE` | A file to write every TUI keypress's timing to |
| `MS_TODO_INSTALL_DIR`, `MS_TODO_NO_ALIAS` | For `install.sh`: where to install, and skip the `mst` link |

## Exit codes

| Code | Means |
| --- | --- |
| 0 | Success |
| 1 | A network or Graph failure, including `outcome_unknown` and `database_too_new` |
| 2 | Invalid input: a phrase ms-todo can't read, an ambiguous name, or a change to several tasks off a terminal without `--yes` |
| 3 | Not found |
| 4 | Sign-in needed: run `ms-todo auth login` |
| 5 | A conflict, or rejected by Graph |
| 6 | Rate limited |
| 7 | Not supported |

With `--format json`, an error is a JSON object on stderr: `{"error": {"kind", "message", …}}`.
