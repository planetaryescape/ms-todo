# ms-todo

A local-first, keyboard-native terminal client for Microsoft To Do. It has a daemon that keeps a local SQLite cache in sync with Microsoft Graph, and two clients of that daemon: a scriptable CLI with stable JSON output and a very fast ratatui TUI.

**Status: rung 3a, instant reads.** ms-todo signs in to your Microsoft account, shows every task in any of your lists, and adds, edits, completes, reopens and deletes tasks, from a terminal or an agent. Reads come from a local cache the daemon keeps in step with Microsoft To Do, so they answer in milliseconds; writes still go straight to Microsoft Graph, then into the cache. Live sync, offline writes and undo come in later rungs of the [roadmap](docs/blueprint/10-roadmap.md).

## Install

macOS (Apple silicon or Intel) and Linux x86_64:

```sh
curl -fsSL https://raw.githubusercontent.com/planetaryescape/ms-todo/main/install.sh | sh
```

This installs the latest release to `~/.local/bin/ms-todo` after checking the archive's sha256. Set `MS_TODO_INSTALL_DIR` to install somewhere else. To pin a release, end the command with `| sh -s -- --version v0.1.0`. The binary isn't signed yet, but macOS doesn't quarantine files downloaded with `curl`, so Gatekeeper doesn't block it.

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

Output is a table in a terminal and JSON when piped. `--format json|jsonl|ids|csv|table` picks one. JSON is `{ "schema_version": 2, "sync": { "state", "generation" }, "items": [...] }`. Each item has every field Graph returns, except that `id` is ms-todo's own stable ID, with Graph's beside it as `graph_id`; commands take either. `sync.state` is `initial` until that list's first sync has finished, so an empty `initial` list isn't really empty (other formats say so on stderr). `--format ids` prints one ID per line. `--format csv` has a header row and fixed columns (tasks: `id,title,status,importance,due,reminder,categories,created,modified`; lists: `id,name,wellknown,is_owner,is_shared`); a task's notes aren't a column, so use JSON for those. A `--list` name that matches more than one list is an error that lists the candidates; ms-todo never picks one for you.

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
- Each change prints the task as Graph returned it and an `op_id`. If a create or a recurring completion gets no clear answer from Graph (a timeout or a server error), ms-todo doesn't resend it: it exits 1 with error kind `outcome_unknown` and the `op_id`. Check `ms-todo tasks list` before trying again, or you may get a duplicate.
- If someone changed the same field on another device since ms-todo last read the task, the change is refused with exit code 5 rather than overwriting theirs.

`ms-todo raw POST|PATCH|DELETE PATH [--body JSON]` sends a request straight to Graph for debugging. It's sent once, never resent, and needs `--yes` when not in a terminal.

Agents can use the skill in [`skills/ms-todo/SKILL.md`](skills/ms-todo/SKILL.md).

Exit codes: 0 success, 1 network or Graph failure (including `outcome_unknown`), 2 invalid input (such as an ambiguous name, or `delete` without `--yes` off a terminal), 3 not found, 4 sign-in needed (run `ms-todo auth login`), 5 conflict or rejected by Graph, 6 rate limited, 7 not supported.

## Keep it fresh

The daemon syncs everything when it starts, every 5 minutes, and when asked:

```sh
ms-todo sync --wait    # returns once a sync that started after it has finished; progress on stderr
ms-todo sync           # just asks for one
ms-todo doctor         # sign-in, daemon, the cache's path and size, each list's sync state, the last error
ms-todo schema tasks list   # the JSON schemas of a command's input and output; `ms-todo schema` for all
```

A change made on your phone shows up after the next sync. A sync fetches every list and task (and ms-todo's own data for tasks that changed), removes what's gone, and never undoes a change ms-todo itself made while it ran.

## The daemon

The first command starts a small background daemon, which is the only process that talks to Graph, refreshes your sign-in and keeps the cache (`ms-todo.db`, SQLite, in the data directory). You don't need to manage it, but you can:

```sh
ms-todo daemon status
ms-todo daemon stop    # returns once the daemon's process has exited
ms-todo daemon start
```

Its socket is private to your user (0600, in a 0700 directory), and its log is `daemon.log` in the data directory's `logs/`. A newer ms-todo restarts an older daemon by itself.

## Plan

The design is in [`docs/blueprint/`](docs/blueprint/README.md), the Phase 0 results are in [`12-open-questions.md`](docs/blueprint/12-open-questions.md), and the evidence is in `docs/research/spikes/`. Still open: the phone halves of spikes S7 and S11, the S4 deltaLink replay, and product questions Q3 and Q6–Q12. The build climbs a ladder of usable releases: a foundation turn (install and sign in), rung 1 (see my tasks), rung 2 (capture and finish tasks), rung 3a (instant reads from a local cache), and next rung 3b (live sync through delta).

What's planned:

- The whole Microsoft Graph To Do API: lists, tasks (every field, including recurrence), steps, links, attachments up to 25 MB, categories, open extensions, delta sync.
- Features the API lacks, built on top of it: My Day (kept by ms-todo, and shown in the phone app's My Day by giving a task with no due date today's date), folders for lists, and assignment.
- Todoist-style natural-language quick add, parsed deterministically: `Pay rent every 1st #Home p1 !9am`.
- Offline-tolerant instant writes, with an outbox and rollback when Graph rejects a change.

Licensed under MIT or Apache-2.0, at your option.
