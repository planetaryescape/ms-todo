# ms-todo

A fast, local-first terminal client for Microsoft To Do: CLI, TUI and daemon.

[![Release](https://img.shields.io/github/v/release/planetaryescape/ms-todo)](https://github.com/planetaryescape/ms-todo/releases)
[![CI](https://github.com/planetaryescape/ms-todo/actions/workflows/ci.yml/badge.svg)](https://github.com/planetaryescape/ms-todo/actions/workflows/ci.yml)
[![Licence: MIT or Apache-2.0](https://img.shields.io/badge/licence-MIT%20or%20Apache--2.0-blue)](#licence)

![The ms-todo TUI: moving through folders, editing a due date in words, completing and undoing, quick add in the centred box, filtering and switching themes](docs/assets/demo-tui.gif)

Install ms-todo, sign in once, then run `mst` for the full-screen view or `mst tasks add "Pay rent every 1st #Finances p1 9am"` from any shell. Your lists and tasks are the same ones the To Do apps on your phone and the web show.

The demos here run against a fake Microsoft Graph with made-up data; you can run the same demo yourself ([Try it without an account](#try-it-without-an-account)).

## Why ms-todo

- It's instant because it's local-first. A background daemon keeps your tasks in a SQLite cache, and every command and keypress reads from there. Measured on a 654-task account with the daemon running, the TUI paints its first list 6 to 9 ms after it starts, and a keypress takes under 3 ms at p95 ([D-043](docs/blueprint/11-decision-log.md)).
- It works offline and doesn't drop a write. A change goes into the cache and an outbox in one step, answers at once, and is sent when the network is back. A write that may or may not have reached Microsoft is never resent on a guess; it waits for you in `mst outbox list`.
- It's scriptable. Every command prints a table in a terminal and JSON when piped, and `--format json|jsonl|csv|ids` picks one. Changes take `--dry-run` and `--idempotency-key`, `mst schema` prints each command's JSON schema, and agents get a [skill](skills/ms-todo/SKILL.md).
- It's keyboard-native. The TUI runs on single keys, with a palette (`:`) for anything whose key you don't remember. Everything it does is also a CLI command.
- It syncs live with the phone app. A change made on your phone or the web shows up within about 30 seconds while ms-todo is in use, through Graph's delta sync.

## Install

With Homebrew, on macOS (Apple silicon or Intel) and Linux x86_64:

```sh
brew install planetaryescape/ms-todo/ms-todo
```

This installs `ms-todo` and its short alias `mst`. Or use the install script, which checks the archive's sha256 and installs to `~/.local/bin`:

```sh
curl -fsSL https://raw.githubusercontent.com/planetaryescape/ms-todo/main/install.sh | sh
```

- `MS_TODO_INSTALL_DIR=<dir>` installs somewhere else.
- `| sh -s -- --version v0.1.18` pins a release.
- The script links `mst` to `ms-todo`. It never replaces an `mst` that isn't that link, and `MS_TODO_NO_ALIAS=1` skips it.
- The binary isn't signed yet. macOS doesn't quarantine files downloaded with `curl`, so Gatekeeper doesn't block it.

From source, with Rust 1.94 or newer:

```sh
git clone https://github.com/planetaryescape/ms-todo && cd ms-todo
cargo install --locked --path .
ln -s ~/.cargo/bin/ms-todo ~/.cargo/bin/mst   # the alias, if you want it
```

A build from source has no app ID built in, so set one before you sign in ([Sign in](#sign-in)).

Check the install:

```sh
mst --version    # ms-todo 0.1.18
```

## Sign in

```sh
mst auth login     # prints a URL and a code; enter the code in your browser
mst auth status    # the account, token expiry, client ID and scopes
```

`auth login` uses Microsoft's device-code sign-in: you enter the code in any browser, so it works over SSH too. Release builds sign in as the maintainer's Entra app, asking for `Tasks.ReadWrite`, `MailboxSettings.ReadWrite` (your Outlook categories, for `@labels`), `User.Read` and `offline_access`. With that ID you consent to the maintainer's app registration, and sign-in depends on it staying available. So register your own if you can ([the guide](docs/setup/entra-app-registration.md) takes about 10 minutes); you need one if you built from source. Then set its ID:

```sh
export MS_TODO_CLIENT_ID=<your-app-id>
```

or put it in `~/.config/ms-todo/config.toml`:

```toml
[auth]
client_id = "<your-app-id>"
```

`mst auth status` shows which ID is in use and where it came from (`env`, `config` or `bundled`). `mst auth logout` signs out.

## Try it without an account

`demo/run.sh` builds ms-todo and starts it against a fake Graph filled with made-up tasks for "Alex Rivera". It uses its own throwaway home directory and never touches your real ms-todo data or Microsoft:

```sh
demo/run.sh            # a shell where `mst` is the demo; exit the shell to clean up
demo/run.sh -- mst     # or run one command, here the TUI
```

Every command in the tour and the recipes works in it. The seed data is `demo/seed.json`, with dates relative to today; `demo/record.sh` re-records the GIFs with [vhs](https://github.com/charmbracelet/vhs).

## A quick tour

![The ms-todo CLI: lists, a list's tasks, quick add with a preview, search, what was done this week, JSON for scripts and CSV](docs/assets/demo-cli.gif)

| To | Run |
| --- | --- |
| See your lists, by folder | `mst lists list` |
| See a list's tasks | `mst tasks list --list Finances` |
| Filter and sort across lists | `mst tasks list --due overdue`, `mst tasks list --category Errands --sort due` |
| Make, rename or delete a list | `mst lists create Garden --folder Home`, `rename`, `delete` |
| Add a task the way you'd say it | `mst tasks add "Call mum in 2 days p1"` |
| Plan today | `mst myday suggest`, `mst myday add <id>`, `mst myday list` |
| Track who you're waiting on | `mst tasks edit <id> --assignee Sam`, `mst waiting` |
| Preview how it would be read | `mst tasks parse "Call mum in 2 days p1"` |
| Complete, reopen, edit or delete | `mst tasks complete <id>`, `reopen`, `edit`, `delete` |
| Make a task repeat, or give it a start date | `mst tasks edit <id> --recur "every mon"`, `--start fri` |
| Colour your categories | `mst categories create Errands --color preset3`, `recolor`, `delete` |
| Find a task in any list | `mst search rent` |
| See what you finished | `mst done --since mon` |
| Move overdue tasks to today | `mst reschedule --overdue --to today --dry-run` |
| Move tasks to another list | `mst tasks move <id> --to Groceries --dry-run` |
| Undo the last change | `mst undo` |
| Open the TUI | `mst` |

A task's `<id>` is in `mst tasks list --format ids`, or use its exact title with `--list`. Every command's flags are in `mst <command> --help`, and [docs/usage.md](docs/usage.md) explains each one.

### Quick add

`tasks add` reads the list, importance, dates, reminder, recurrence and categories out of the text. What it doesn't recognise stays in the title:

```console
$ mst tasks parse "Pay rent every 1st #Finances p1 9am"
Title       Pay rent
List        Finances
Summary     p1 · due Thu 1 Oct · every month on the 1st · remind 09:00
Recognised  recurrence "every 1st", list "#Finances", priority "p1", date "9am"
```

| Type | For |
| --- | --- |
| `#Finances`, `#"Two words"` | the list, by its name or a prefix only it has. Without one, "Tasks" |
| `p1` to `p4` | importance: `p1` high, `p2` and `p3` normal, `p4` low |
| `tomorrow`, `fri 5pm`, `in 3 days`, `12 oct` | the due date. A time also sets a reminder then |
| `!9am`, `!tomorrow 8:30` | a reminder only |
| `every day`, `every weekday`, `every mon, wed`, `every 1st`, `every last friday` | a recurrence |
| `@errands` | an Outlook category |
| `+myday` or `*` | today's [My Day](#my-day) |
| `"quoted text"`, `\#` | kept as typed |

Flags win over the text (`--list`, `--due`, `--reminder`, `--importance`), and `--no-parse` takes the text as the title exactly as given: use it for text you didn't type, such as an agent's. The full syntax is in [Quick add](docs/usage.md#quick-add).

### List suggestions (optional)

Turn this on and ms-todo suggests a list for a task you add to the inbox, using [TypeSafe](https://typesafe.ai)'s Jev model. It only suggests; nothing moves until you say so:

```console
$ mst tasks add "pay council tax"
…
note: suggested list: Finances (0.93) — move it with `ms-todo tasks move <id> --to Finances`
$ mst tasks suggest-list "watch Dune Part Two"
Title  watch Dune Part Two
List   Movies (1.00)
```

In the TUI's add box, a likely list shows as `→ Finances? (Ctrl-l to accept)`. It's off by default, and when it's on it sends TypeSafe the task's title and, for each list, its folder, name and up to five open task titles. [List suggestions](docs/usage.md#list-suggestions) covers turning it on and the API key.

### TUI keys

`mst` opens the TUI when it runs in a terminal. From a script or a pipe it prints help and exits 2, so nothing waits on it.

| Key | Does |
| --- | --- |
| `j` / `k`, `g` / `G` | down, up, top, bottom |
| `h` / `l`, `Tab` | move between the sidebar, the task list and the detail pane |
| `Enter` / `Space` in the sidebar | open a list or view; on a folder, fold or unfold it |
| `a` | quick add, in a box in the middle of the screen: each part it reads is coloured as you type, with the task it makes underneath. `Tab` completes a `#List` or `@label`, `Ctrl-r` takes the text literally, `Ctrl-l` takes a suggested list, `Enter` adds |
| `x` | complete, or reopen a completed task |
| `e` | edit a field: `t` title, `d` due date, `r` reminder, `i` importance, `a` assignee, `n` notes, `I` cycles importance; in the detail pane, edits the field under the cursor |
| `Enter` in the detail pane | edit the field under the cursor |
| `Space` on a step in the detail pane | check or uncheck it; `a` adds steps, `e` / `Enter` edits a step or the link, `d` deletes one |
| `A` | attach a file by its path; on a file in the detail pane, `Enter`, `e` or `o` saves it to `~/Downloads` and opens it, `d` deletes it |
| `v` / `V` | select a task, or every task in the view; `Esc` clears the selection |
| `t` | put the task or the selection in My Day, or take it out; on a suggestion in the My Day view, add it |
| `W` | assign the task or the selection to someone; empty clears it |
| `m` | move the task or the selection to another list |
| `M` | move the current list into a folder |
| `S` | set one due date on the selection |
| `R` | reschedule every overdue task in the view |
| `d` | delete, after a `y` / `n` confirmation |
| `u` | undo the last change |
| `/` | filter the view as you type |
| `:` | the command palette: any action, list or view by name, and "New list…", "Rename list…", "Delete list…" |
| `D` | diagnostics: sign-in, daemon, cache, each list's sync and the outbox |
| `o` / `y` | open or copy the task's link |
| `r` | sync now |
| `?` | every key, grouped by where it's pressed; `j` / `k`, `PgUp` / `PgDn` and `g` / `G` scroll it, `Esc` closes it |
| `q` | quit |

A due date or reminder takes words (`tomorrow`, `fri 17:30`, `+2w`) and shows the date it resolves to as you type. Editors take the usual line keys; [The TUI](docs/usage.md#the-tui) lists them.

### Themes

`terminal`, the default, uses your terminal's own colours. The others are `catppuccin-mocha`, `catppuccin-latte`, `gruvbox-dark`, `gruvbox-light`, `tokyo-night`, `nord`, `one-dark`, `kanagawa`, `night-owl`, `cobalt2` and `high-contrast`. Pick one in the TUI with `:` then "Theme…" (each previews as you move, `Enter` keeps it), try one with `mst tui --theme nord`, or set it:

```toml
# ~/.config/ms-todo/config.toml
[tui]
theme = "tokyo-night"

[tui.colors]          # optional: change single roles
overdue = "#ff5f5f"   # #rrggbb, an ANSI name, 0-255, or reset
```

The roles, truecolor and `NO_COLOR` are in [Themes](docs/usage.md#themes).

### Folders

Group lists into folders, one level deep, as the To Do app's list groups do:

```sh
mst lists move Finances Health --folder Areas   # the folder is made if it's new
mst folders list
mst folders rename Areas Responsibilities
mst lists move Groceries --no-folder
```

The To Do apps don't show these folders; every ms-todo you sign in to does ([What the To Do API can't do](#what-the-to-do-api-cant-do)).

### Search

```sh
mst search rent                         # open tasks in every list, title matches first
mst search '"car insurance"' --status all
mst search 'renew* NOT passport' --list Finances
```

Search looks through titles and notes, ignoring case and accents, from the local cache. It supports prefixes (`renew*`), phrases, `OR`, `NOT` and parentheses. In the TUI, `/` filters the view with the same search.

### What you finished, and what's overdue

```sh
mst done --since mon                              # grouped by day, newest first
mst reschedule --overdue --to today --dry-run     # which tasks would move
mst reschedule --overdue --to today --yes         # move them
mst undo                                          # one undo puts them all back
```

A change to several tasks lists them and asks first in a terminal. Anywhere else it needs `--yes`, and exits 2 without it.

### Move tasks between lists

```sh
mst tasks move <id> --to Groceries --dry-run
mst tasks move <id> --to Groceries
mst undo                                          # moves it back
```

A move keeps the task's steps, link, attachments and ms-todo's own data, and checks the copy before it deletes the original.

### Steps and links

```sh
mst steps add <id> "Buy paint" "Tape the edges" "Two coats"
mst steps check <id> 1               # by number, ID or exact text
mst steps list <id>
mst links add <id> https://example.com/colours --name "Colour chart"
```

Steps checked on the phone show as checked here after the next sync. A task holds one link, as the To Do apps allow. In the TUI, the detail pane lists the steps and the link: `Space` ticks the step under the cursor. [Steps and links](docs/usage.md#steps-and-links) has the rest.

### Attachments

```sh
mst attachments add <id> ./invoice.pdf           # up to 25 MB each; several at once
mst attachments list <id>
mst attachments download <id> --out ~/Downloads  # every file, or name one by number or name
mst attachments delete <id> 1 --yes              # `mst undo` attaches it again, for a week
```

Files show on the phone once they're uploaded, and files added on the phone show here after the next sync. Downloads never overwrite a file: a second copy is saved as `invoice (1).pdf`. In the TUI, the detail pane lists them under Files: `A` attaches one, and `Enter` on a file saves it to `[attachments] download_dir` (`~/Downloads` by default) and opens it. [Attachments](docs/usage.md#attachments) has the rest.

### My Day

My Day is the list of what you'll do today. The To Do apps have one, but Microsoft Graph can't read or write it, so ms-todo keeps its own, and your phone shows it through the due date:

```sh
mst myday suggest              # due today, overdue, and what was left in yesterday's My Day
mst myday add <id> <id>        # or `mst tasks add "Call the bank +myday"`
mst myday list
mst myday remove <id>
```

- **On the phone:** a task you add with no due date is due today while it's in My Day. With the To Do app's "Show 'Due Today' tasks in My Day" setting on, the app shows it in its own My Day. A task with a due date keeps it, and shows in the app's My Day on the day it's due. ms-todo can't read that setting, so `mst doctor` reminds you to check it.
- **At midnight** the daemon empties the day's My Day. A task still open loses the due date My Day gave it; a due date you set or changed is never touched, and a completed task keeps its date. If the daemon was off, it catches up once when it next starts. `my_day.rollover_time` moves the rollover, for example to `"04:00"` for late nights, and `mst myday rollover --dry-run` shows what it would do.
- **In the TUI**, My Day is the first view in the sidebar, with the day in its title and Suggestions under its tasks. `t` puts the task under the cursor or the selection in My Day, or takes it out.
- **Across machines:** My Day lives in Microsoft To Do, so every ms-todo you sign in to sees it. Tasks you add to My Day in the To Do app don't reach ms-todo's, since the API can't see them.

Adding, removing and the rollover are changes like any other: `mst undo` reverses them.

### Waiting on someone

Mark a task as waiting on a person, then see everything you're waiting on:

```sh
mst tasks edit <id> --assignee Sam   # also "waiting on others", which the To Do app shows
mst waiting                          # every open assigned task, grouped by person
mst tasks list --assignee sam        # one person's, whatever the case
mst tasks edit <id> --clear-assignee
```

The name is ms-todo's own: nobody is told, and the To Do apps don't show it, but the status they do show. In the TUI, the Assigned view groups tasks by person and each row carries a person chip. [Waiting on someone](docs/usage.md#waiting-on-someone) has the rest.

### Undo and the outbox

Every change is queued, sent in the background, and can be undone:

```sh
mst outbox list     # each change and its state: pending, inflight, unknown, failed or done
mst undo            # the latest change; `mst undo <op_id>` for an older one
```

`mst tasks list` marks a task's state in its `SYNC` column. An `unknown` change was sent but no answer said whether it landed; ms-todo never resends it by itself. [Offline, and never lose a write](docs/usage.md#offline-and-never-lose-a-write) covers each state and `outbox retry` and `discard`.

### Formats for scripts and agents

```sh
mst tasks list --list Finances --format json | jq '.items[] | select(.importance == "high") | .title'
mst tasks list --list Finances --format csv > finances.csv
mst tasks list --list Groceries --format ids
mst schema tasks add          # the JSON schemas of a command's input and output
```

JSON is `{ "schema_version": 2, "sync": {…}, "items": [...] }`, each item carrying every field Graph returns plus ms-todo's `id` and `sync_state`. Errors go to stderr, as JSON with `--format json`, with the exit codes in [Exit codes](docs/usage.md#exit-codes).

### Config

`~/.config/ms-todo/config.toml` on macOS and Linux (`$XDG_CONFIG_HOME` or `$MS_TODO_CONFIG_DIR` move it) holds:

| Key | Sets |
| --- | --- |
| `[auth] client_id` | the Entra app to sign in as |
| `[tui] theme` | the TUI's theme |
| `[tui.colors]` | single colour roles over the theme |
| `[suggest]` | optional list suggestions from TypeSafe, off by default ([List suggestions](docs/usage.md#list-suggestions)) |
| `[my_day] rollover_time` | when the day's [My Day](#my-day) is emptied, `"HH:MM"` local; `"00:00"` by default |

Environment variables (`MS_TODO_INSTANCE`, `MS_TODO_CLIENT_ID`, `MS_TODO_CONFIG_DIR`, `NO_COLOR`, `COLORTERM` and more) and where ms-todo keeps its data are in [docs/usage.md](docs/usage.md#environment-variables).

## Recipes

### Plan today

```sh
mst myday suggest
mst myday add <id> <id> <id>
mst myday list
```

`myday suggest` lists open tasks due today, then overdue ones, then those left open in an earlier My Day, each with its list and ID. Add the ones you'll do; one with no due date becomes due today, so your phone's My Day shows it too. In the TUI, open My Day at the top of the sidebar and press `t` on each suggestion.

### Review the morning: what's overdue or due today

```sh
mst tasks list --due "before tomorrow" --status open
```

Every open task due today or earlier, in every list, soonest first, each with its list. `--due overdue` leaves out today's, and `mst reschedule --overdue --to today` moves the overdue ones on. For a view grouped by day, open the TUI's Planned view.

### Copy yesterday's work for a standup

```sh
mst done --since yesterday --format csv | pbcopy
```

The clipboard gets a header row, then `id,title,list,completed_on,due,importance,sync_state` for each task. Use `xclip -selection clipboard` or `wl-copy` on Linux. If it's empty, check `mst done --since yesterday` in a terminal: a completion that hasn't synced yet is listed as "Not synced yet".

### See what you finished this week

```sh
mst done --since mon                 # every list
mst done --since mon --folder Areas  # one folder
```

`mon` is the latest Monday, today included. Microsoft To Do keeps the day of a completion, not the time, so tasks are grouped by day.

### Clear everything overdue, and take it back

```sh
mst reschedule --overdue --to today --dry-run
mst reschedule --overdue --to today --yes
mst undo
```

The dry run names each task. The real run is one change however many tasks it moves, so one `mst undo` puts them all back. A task whose due date changed again since is left alone and named under `refused`.

### Complete a batch from a pipe

```sh
mst tasks list --list Groceries --format json \
  | jq -r '.items[] | select(.status != "completed") | .id' \
  | mst tasks complete - --dry-run
```

Drop `--dry-run` to complete them. `-` reads IDs from stdin, one per line. `--format ids` gives every task's ID, completed ones included, so filter with JSON when that matters.

### Move a batch to another list

```sh
mst tasks list --list Groceries --format ids | mst tasks move - --to Tasks --dry-run
mst tasks list --list Groceries --format ids | mst tasks move - --to Tasks --yes
mst undo
```

A move copies each task, checks the copy, then deletes the original, so a batch takes a few seconds; `mst outbox list` shows where each one is. `mst undo` moves them back once they've finished, and says so if one hasn't yet. A move that can't tell whether a step landed pauses and deletes nothing.

### Capture from anywhere

```sh
alias t='mst tasks add'
t "Buy stamps tomorrow @Errands"
```

Any launcher or hotkey tool that can run a shell command can call `mst tasks add "<text>"` the same way. The add answers at once, even offline.

For Raycast, use the [local extension](raycast/ms-todo/README.md) to add, search, and complete tasks or open My Day.

### Find anything

```sh
mst search rent
mst search 'tax OR rent' --status all --format csv
```

A query ms-todo can't read, such as `OR milk` or an unclosed quote, exits 2 and says why.

### Drive it from a script or an agent

```sh
mst tasks add "Renew car insurance" --no-parse --list Finances --due "next fri" --dry-run --format json
mst tasks add "Renew car insurance" --no-parse --list Finances --due "next fri" \
  --idempotency-key car-2026 --format json | jq -r '.items[0].id'
mst schema tasks add
```

- `--no-parse` stops quick add reading a title as a date; pass the fields as flags.
- `--dry-run` shows the resolved change, including the list's ID and what would be sent.
- `--idempotency-key` makes a retry safe: the same key returns the first result for 24 hours, and the same key with a different change exits 2.

The [agent skill](skills/ms-todo/SKILL.md) has the rules an agent should follow.

### Export a list to a spreadsheet

```sh
mst tasks list --list Finances --format csv > finances.csv
```

The columns are `id,title,status,importance,due,reminder,categories,created,modified,sync_state`. Notes aren't a column; use `--format json` for those.

### Change the look

```toml
# ~/.config/ms-todo/config.toml
[tui]
theme = "gruvbox-dark"

[tui.colors]
important = "bright-yellow"
```

An unknown theme, role or colour stops the TUI with an error naming it, so a typo is caught at once. `mst tui --list-themes` names the themes.

## How it works

```
 mst (CLI) ──┐                    ┌──────────── daemon ────────────┐
             ├── Unix socket ───▶ │ SQLite cache    outbox         │ ── delta sync ──▶ Microsoft Graph
 mst (TUI) ──┘                    │ (reads)         (writes, sent  │ ◀── changes ────  (To Do)
                                  │                  in order)     │
                                  └────────────────────────────────┘
```

The first command starts a small daemon, the only process that talks to Microsoft Graph. It keeps every list and task in a SQLite cache, reading only what changed since the last sync (Graph's delta queries), every 20 seconds while you're using ms-todo and every 5 minutes otherwise. The CLI and the TUI ask the daemon over a private Unix socket and answer from the cache. A change goes into the cache and an outbox together, and the daemon sends the outbox in order, backing off while the network is down. The design is in [docs/blueprint/](docs/blueprint/README.md).

## What the To Do API can't do

Microsoft Graph's To Do API leaves out some things the To Do apps do. ms-todo works around them, with limits:

| The apps have | The API | ms-todo |
| --- | --- | --- |
| My Day | has no My Day | Its own [My Day](#my-day), stored on each task in Microsoft To Do as an open extension, and mirrored on the phone through the due date. A task put in My Day in the app doesn't reach ms-todo's |
| List groups | doesn't expose them | Folders, stored on each list in Microsoft To Do as an open extension. Every ms-todo you sign in to sees them; the To Do apps don't |
| Moving a task to another list | has no move | `tasks move` copies the task with everything it holds, checks the copy, then deletes the original. The To Do apps show the moved task as created at the time of the move; ms-todo keeps the original time |
| A start date and a repeat on one task | counts a recurrence from the start date and moves the due date with it | A repeating task's start date is its first due date: `--start` on a repeating task is refused, and a recurrence set on a task with a start date moves the start along |
| Listing a list's or task's extensions | answers 404 | `extensions list` shows ms-todo's own; `extensions get` reads any other by name |
| Renaming a category | ignores the new name | No `categories rename`: create the new one, re-tag with `tasks edit --category`, delete the old one |

## Status and roadmap

ms-todo is built in rungs, each a usable release; [Releases](https://github.com/planetaryescape/ms-todo/releases) has the current version and what each one added. The [roadmap](docs/blueprint/10-roadmap.md) lists what's next, and the [decision log](docs/blueprint/11-decision-log.md) says why things are the way they are.

## Contributing

```sh
cargo nextest run --workspace                              # the tests, against a fake Graph
cargo clippy --workspace --all-targets -- -D warnings
demo/run.sh                                                # try a change by hand, safely
```

A debug build uses its own `dev` instance, so it never touches an installed ms-todo's sign-in or cache. Read [AGENTS.md](AGENTS.md) and the [blueprint](docs/blueprint/README.md) before a larger change. Issues are tracked as markdown in [docs/issues/](docs/issues/README.md). Commits use `type: description`.

## Licence

MIT or Apache-2.0, at your option: [LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE).
