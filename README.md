# ms-todo

A local-first, keyboard-native terminal client for Microsoft To Do. It has a daemon that keeps a local SQLite cache in sync with Microsoft Graph, and two clients of that daemon: a scriptable CLI with stable JSON output and a very fast ratatui TUI.

**Status: Rung 4: offline and undo.** ms-todo signs in to your Microsoft account, shows every task in any of your lists, and adds, edits, completes, reopens and deletes tasks, from a terminal or an agent. Reads come from a local cache the daemon keeps in step with Microsoft To Do through delta sync, so they answer in milliseconds and a change on your phone shows up by itself within about 30 seconds while you're using ms-todo. Writes answer at once too, with or without a network: they're queued and sent in the background, and nothing you write is silently dropped. `ms-todo undo` reverses a change. The TUI and quick-add parsing come in later rungs of the [roadmap](docs/blueprint/10-roadmap.md).

## Install

macOS (Apple silicon or Intel) and Linux x86_64:

```sh
curl -fsSL https://raw.githubusercontent.com/planetaryescape/ms-todo/main/install.sh | sh
```

This installs the latest release to `~/.local/bin/ms-todo` after checking the archive's sha256. Set `MS_TODO_INSTALL_DIR` to install somewhere else. To pin a release, end the command with `| sh -s -- --version v0.1.0`. The binary isn't signed yet, but macOS doesn't quarantine files downloaded with `curl`, so Gatekeeper doesn't block it.

`mst` is an official short alias for `ms-todo`. The installer links `~/.local/bin/mst` to `ms-todo`, and the release archives include the link. It never replaces an existing `mst` that isn't that link, and `MS_TODO_NO_ALIAS=1` skips it.

## Sign in

```sh
ms-todo auth login    # prints a URL and a code; enter the code in your browser
ms-todo auth status   # account, token expiry, client ID and scopes
ms-todo auth logout
```

Release builds sign in through the maintainer's Entra app registration. You can [register your own](docs/setup/entra-app-registration.md) in about 10 minutes and set `MS_TODO_CLIENT_ID`, or `client_id` under `[auth]` in `~/.config/ms-todo/config.toml` (`$XDG_CONFIG_HOME` and `$MS_TODO_CONFIG_DIR` move it). `auth status` shows which client ID is in use.

## See your tasks

```sh
ms-todo lists list                    # every list
ms-todo tasks list                    # every task in "Tasks", completed ones included
ms-todo tasks list --list Groceries   # by exact name, or by the ID from `lists list`
ms-todo raw GET /me/todo/lists        # any Graph v1.0 path, authenticated
ms-todo auth bearer --reveal-secret   # a valid access token, for curl
```

Output is a table in a terminal and JSON when piped. `--format json|jsonl|ids|csv|table` picks one. JSON is `{ "schema_version": 2, "sync": { "state", "generation" }, "items": [...] }`. Each item has every field Graph returns, except that `id` is ms-todo's own stable ID, with Graph's beside it as `graph_id` (commands take either), plus `sync_state` (below). `sync.state` is `initial` until that list's first sync has finished, so an empty `initial` list isn't really empty (other formats say so on stderr). `--format ids` prints one ID per line. `--format csv` has a header row and fixed columns (tasks: `id,title,status,importance,due,reminder,categories,created,modified,sync_state`; lists: `id,name,wellknown,is_owner,is_shared`); a task's notes aren't a column, so use JSON for those. A `--list` name that matches more than one list is an error that lists the candidates; ms-todo never picks one for you.

## Add and finish tasks

```sh
ms-todo tasks add "Buy milk" --list Groceries --due 2026-09-26
ms-todo tasks add "Call the dentist" --reminder 2026-09-26T09:30 --importance high --body "re: filling"
ms-todo tasks complete <ID>...        # IDs from `tasks list --format ids`
ms-todo tasks reopen <ID>...
ms-todo tasks edit <ID> --title "Buy oat milk" --due 2026-09-27   # also --clear-due, --reminder, --clear-reminder, --importance, --body
ms-todo tasks delete <ID>...          # asks first; pass --yes when not in a terminal
ms-todo tasks list --format ids | ms-todo tasks complete -   # `-` reads IDs from stdin
```

- The text of `tasks add` is the title, exactly as given. With no `--list`, it goes to "Tasks".
- Due dates are dates only; put a time in `--reminder`. Dates are written in your local time zone (`TZ`, or the system's).
- A task can also be named by its exact title, together with `--list`. A title several tasks share is an error listing them.
- `--dry-run` shows what a command would change, resolved exactly as the real run would, and changes nothing.
- Completing a recurring task keeps it open with the next due date, and Microsoft To Do adds the occurrence you finished as a new, completed task.
- `--idempotency-key KEY` makes a change run at most once: repeating the command with the same key returns the first result for 24 hours without sending anything, and the same key with a different change exits 2.
- Each change answers at once with the task as ms-todo now has it and an `op_id`, and the daemon sends it to Microsoft To Do in the background (see [Offline, and never lose a write](#offline-and-never-lose-a-write)).
- If someone changed the same field on another device since ms-todo last read the task, the change is rejected (and rolled back) rather than overwriting theirs.

## Offline, and never lose a write

Every change is applied to the local cache and queued in an outbox in one step, so it answers in milliseconds even with no network. The daemon sends the queue in order, each task's changes one after another, and backs off while the network is down. Each task shows how its changes are doing in `sync_state`, and `tasks list` marks it in its `SYNC` column:

- `synced`: Microsoft To Do has it.
- `pending`: queued or being sent.
- `unknown`: it was sent, but no answer said whether Microsoft To Do applied it (a timeout or a server error on a create or a recurring completion). ms-todo never resends these by itself, since that could make a duplicate. After each sync it looks for the task by the `op_id` it carries; when found, the task becomes `synced`. After 24 hours it's flagged for you.
- `failed`: Microsoft To Do rejected it, for example because the list was deleted on another device. The change is rolled back but kept, with its content.

```sh
ms-todo outbox list                   # every queued change and its state; --state pending|inflight|unknown|failed|done
ms-todo outbox retry <OP>             # send an unknown or failed change again (you chose to), or a pending one now
ms-todo outbox discard <OP> --yes     # drop a change; one that never reached Microsoft To Do is undone locally
ms-todo undo                          # reverse the latest change; or `ms-todo undo <OP_ID>`
```

Undo is itself a change, so it can be undone. Undoing an add deletes the task; an edit, complete or reopen puts the fields back; a delete brings the task back with the same ID (and a new Graph ID). Undoing the completion of a recurring task also deletes the completed copy Microsoft To Do made, so it asks which one: `ms-todo undo <OP_ID> --copy <ID>` (without `--copy`, it lists the candidates and exits 2).

A database upgraded by a newer ms-todo is refused with "this database was upgraded by a newer ms-todo; install the latest version" (error kind `database_too_new`, exit 1).

`ms-todo raw POST|PATCH|DELETE PATH [--body JSON]` sends a request straight to Graph for debugging. It's sent once, never resent, and needs `--yes` when not in a terminal.

Agents can use the skill in [`skills/ms-todo/SKILL.md`](skills/ms-todo/SKILL.md).

Exit codes: 0 success, 1 network or Graph failure (including `outcome_unknown` and `database_too_new`), 2 invalid input (such as an ambiguous name, or `delete` without `--yes` off a terminal), 3 not found, 4 sign-in needed (run `ms-todo auth login`), 5 conflict or rejected by Graph, 6 rate limited, 7 not supported.

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

## Plan

The design is in [`docs/blueprint/`](docs/blueprint/README.md), the Phase 0 results are in [`12-open-questions.md`](docs/blueprint/12-open-questions.md), and the evidence is in `docs/research/spikes/`. Still open: the S4 deltaLink replay, and product questions Q3, Q6–Q10 and Q12. The build climbs a ladder of usable releases: a foundation turn (install and sign in), rung 1 (see my tasks), rung 2 (capture and finish tasks), rung 3a (instant reads from a local cache), rung 3b (live sync through delta), rung 4 (offline writes that are never lost, and undo), and next rung 5 (a fast TUI).

What's planned:

- The whole Microsoft Graph To Do API: lists, tasks (every field, including recurrence), steps, links, attachments up to 25 MB, categories, open extensions, delta sync.
- Features the API lacks, built on top of it: My Day (kept by ms-todo, and shown in the phone app's My Day by giving a task with no due date today's date), folders for lists, and assignment.
- Todoist-style natural-language quick add, parsed deterministically: `Pay rent every 1st #Home p1 !9am`.

Licensed under MIT or Apache-2.0, at your option.
