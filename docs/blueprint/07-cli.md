# 07: CLI

> **`mst` is an official alias of `ms-todo`** (D-039 in [11](11-decision-log.md)): a symlink to the same binary, so every command here works as `mst …` too. Help and usage show the name you typed. `--version`, error hints and these docs say `ms-todo`, which always works.

The CLI is the canonical surface (spotuify's contract): **every feature has a CLI subcommand.** A feature that only exists in the TUI isn't finished.

**With no command** (`ms-todo` or `mst` alone), the CLI opens the TUI when both stdin and stdout are terminals, the same as `tui` with no flags. Off a terminal (a script, a pipe, an agent) it prints help to stderr and exits 2, so nothing blocks on a full-screen view. Global flags apply to the bare form (`mst --instance work`); TUI flags such as `--ascii` need `tui` (D-044).

**`tui` flags:** `--ascii` (plain ASCII glyphs), `--theme <NAME>` (a built-in theme; wins over `[tui] theme` in config.toml; an unknown name is `invalid_input`, exit 2, listing the themes), `--list-themes` (the names, one per line, default first; needs no daemon) and `--bench-startup` (D-049 for themes).

## Global flags

- `--format table|json|jsonl|ids|csv`: default `table` in a terminal and `json` when piped. The format enum is adapted from spotuify's `spotuify-protocol/src/output.rs` (see [09](09-reuse-map.md)), and mxr has the same idea. `csv` is RFC 4180 with a header row, which is its schema (no `schema_version` column), and one fixed column set per entity type: tasks `id,title,status,importance,due,reminder,categories,created,modified,sync_state` (plus `list` on mutation results, which can span lists; `sync_state` since rung 4, D-040), lists `id,name,wellknown,is_owner,is_shared`, search results `id,title,list,status,due,snippet`. Arrays are joined with `;` and dates are ISO 8601. A task's body isn't a column: multi-line text or HTML breaks row-oriented use. Single results (`auth status`, `daemon status`) are one row; mutation results are one row per entity; errors are human text on stderr, as in table mode. (Added in rung 2 at BK's request.)
- `--instance <name>`, `--fresh`, `--quiet`, `--no-color`.
- Mutations take `--dry-run`, which shows what would change. `--dry-run` and the real run build the same typed plan (targets and verb): dry-run renders it, the real run applies it (vault: `Same-Code-Path Preview`). Destructive commands take `--yes`, and ask for confirmation in a terminal otherwise. Off a terminal, a destructive command without `--yes` exits 2 and never prompts.
- Mutations take `--idempotency-key K` (from rung 3a of [10](10-roadmap.md), when keys persist in the store; before that the CLI just never retries a mutation by itself). Without it, the CLI generates a request ID itself. A repeat with the same key and the same request gets the original result; the same key with a different request (another operation or payload) exits 2. Keys are kept while the operation is unresolved and for 24 hours after it's done, failed or discarded ([04](04-sync-cache.md#instant-local-writes); vault: `Agent-Native Interfaces`).
- Referring to things:
  - A list: `--list <name|local_id|graph_id>`. A name must match exactly one list, or the command errors out with the candidates. It never picks the first match; that was a flaw in the Python CLI.
  - A task: by `local_id`, or by a unique title match inside `--list`.
  - IDs can also be read from stdin, so `ms-todo tasks list --format ids | ms-todo tasks complete -` works.

## Command surface (v1: the whole API plus the custom features)

```
auth       login | logout | status | bearer --reveal-secret
daemon     start | stop | restart | status | logs [--follow]
sync       [--wait] [--list L]
doctor                                  # sign-in, daemon, db, delta state, outbox, rate-limit state

lists      list | show L | create NAME [--folder F] | rename L NAME | delete L
           move L --folder F | order L --before/--after L2
folders    list | rename F NEW | delete F | order F --before/--after F2

tasks      list [--list L] [--status S] [--due before/after/today/overdue] [--importance I]
                [--category C] [--my-day] [--assignee A] [--completed] [--search Q]
                [--sort due|importance|created|modified|title] [--limit N]
           show T
           add "text" [--list L] [--no-parse] [--due D] [--start D] [--reminder DT]
                [--importance I] [--category C]... [--recur "every …"] [--body TEXT|--body-file F]
                [--my-day] [--assignee A]
           edit T... [same field flags, plus --clear-due etc.]   # several, `-`, or --overdue / --due-before (5d)
           complete T... | reopen T... | delete T...
           move T... --to L [--list L] [--dry-run] [--yes]   # copy, check, then delete the source (D-051)
           links T [--list L]           # linked resources' webUrls, then URLs in the notes, deduped (D-050)
           open T [--index N] [--list L]   # http, https, mailto only; several and no --index: listed, exit 2
           parse "text"                 # show how quick add will read the text; no writes
search     QUERY [--list L] [--status open|completed|all] [--limit N]   # title and notes, every list, best first (D-041)
done       [--since W] [--until W] [--list L | --folder F] [--limit N]     # completed, by local day, newest first (D-048)
reschedule [--overdue | --due-before W | T... | -] --to W [--list L | --folder F] [--dry-run] [--yes]

steps      list T | add T "text" | edit T S "text" | check T S | uncheck T S | delete T S | order …
links      list T | add T URL [--name N] [--app A] [--external-id X] | edit … | delete T R
attachments list T | add T FILE... | download T [A] [--out DIR] | delete T A   # paths only; the daemon moves the bytes
extensions list (list|task) ID | get … NAME | set … NAME --json '{…}' | delete … NAME
categories list | create NAME [--color presetN] | recolor … | delete …
           # no rename: Graph ignores it (S7). A new name means create, re-tag the tasks, then delete

myday      list | add T... | remove T... | suggest | rollover [--dry-run]
outbox     list | retry OP | discard OP
undo       [OP_ID] [--copy ID]          # default: this client's last op; same logic as the TUI's u
schema     [CMD]                        # input and output JSON schemas
raw        GET|POST|PATCH|DELETE PATH [--body JSON]   # authenticated passthrough to Graph, for debugging (GET only in rung 1)
```

**Dates and importance in flags** (D-045). `--due` and `--reminder` on `tasks add` and `tasks edit` are read by `crates/nlp`'s whole-string mode, as the TUI's date fields are:

- `--due`: `2026-10-02`; `today` (`tod`, `tonight`), `tomorrow` (`tom`, `tmrw`), `yesterday`, `day after tomorrow`, `day before yesterday`; a weekday (`fri`, `friday`: the next one, never today), `this fri` (today counts), `next fri` (that day in next week, weeks starting Monday); `in 3 days`, `3 weeks from today`, `three days from now` (numbers one to twenty spelled out), `2 days ago`, `+3d`, `+2w`, `+1m`, `-1d`; `next week` (its Monday), `next month` (the 1st), `end of week` / `eow` (the Friday on or after today), `end of month` / `eom`; `12 oct`, `oct 12`, `12th oct`, `12 oct 2027`, `12/10` (day first). A day and month with no year that has passed means next year's. A past date is allowed. A phrase with a time is refused: due dates have none (D-027).
- `--reminder`: a time alone (`17:30`, `9am`, `5:30pm`, `noon`, `midnight`) is the next one, today's or tomorrow's; or any `--due` phrase with a time after it, optionally with `at` (`tomorrow 9am`, `fri at 17:30`, `2026-10-02 09:30`, and the old `2026-10-02T09:30`). A day with no time (`tomorrow`, `in 2 days`, `fri`, `12 oct`, `next week`) is 09:00 on that day, as To Do's own "Tomorrow" reminder is; `today` after 09:00 stays today and reads as past rather than moving to tomorrow.
- On `tasks edit`, an empty value or `-` clears (the same as `--clear-due` / `--clear-reminder`); on `tasks add` it sets nothing. Values are lowercased and spaces collapsed first.
- A phrase that can't be read is a usage error, exit 2, naming the part not understood (`didn't understand "soonish"`). The daemon only ever receives `YYYY-MM-DD` and `YYYY-MM-DDTHH:MM`, so `--dry-run` shows the resolved date.
- `--importance` takes `high`, `normal`, `low`, `1`–`4` or `p1`–`p4`: 1 is high, 2 and 3 are both normal (Graph has one level for them, D-017), 4 is low. Keeping the typed level in our extension is Q3, still open. The schema lists it as a string with those forms in its description.

**Quick add, as built (rung 6a, D-052).** `tasks add "text"` reads the text with `crates/nlp`'s `DeterministicParser` ([06](06-natural-language.md)) and sends the fields it found; `--no-parse` sends the text as the title. Built: `--no-parse`, `--create-categories`, `tasks parse`. Not built yet: `--start`, `--category`, `--recur`, `--body-file`, `--my-day`, `--assignee` on `add` (8e and rung 7), and `--strict`.

- **Flags win** (D-018): `--list`, `--due`, `--reminder` and `--importance` replace what the text says, with a `note:` saying so. `--due` goes into the parse as the due date, so the text's time becomes a reminder on that day and a recurrence starts on it. The lists are only read when the text has a `#`.
- The CLI resolves the text before sending, against the daemon's lists (`ListLists`; a daemon with no lists synced yet is asked for a `sync --wait` first) and, only when an `@label` was typed, the user's Outlook categories (`raw GET /me/outlook/masterCategories`; if that fails, the task still gets the names and a warning says they weren't checked). `--create-categories` creates each missing category with a one-shot `RawWrite` POST before the task is queued; a dry run creates nothing.
- Nothing left for the title (`tasks add "tomorrow p1"`) exits 2 with `invalid_input`, pointing at quotes and `--no-parse`.
- The parser's warnings (an unknown `#List`, a second date, `+myday`) go to stderr as `note:` lines in table, CSV and ids formats. JSON's stdout and stderr are the output contract, so in JSON they're only in `tasks parse`.
- `tasks parse "text"` prints the reading in every format and writes nothing: `input`, `title`, `list` (`{id, name}` or null for "Tasks"), `due`, `start`, `reminder`, `recurrence` (Graph's `patternedRecurrence` less the zone, plus `description`), `importance`, `priority`, `categories`, `my_day`, `spans` (`start`, `end`, `kind`, `text`) and `warnings`. `tasks add --dry-run` shows the daemon's plan, with the zone.
- **Protocol 10:** `NewTask.start`, `recurrence` (JSON) and `categories`. The daemon checks a recurrence has a `pattern.type` and a `range.startDate`, makes that day the due date when none is given (refusing a different one), and always writes `range.recurrenceTimeZone` in the zone it writes the due date in (S12).

**Folders, as built (rung 5c, D-047).** `lists move L... --folder F | --no-folder` moves one list or several in one command (one `op_id`; the bulk form is for the one-time import of the To Do app's groups); `lists order L --before|--after L2` needs both in the same folder; `folders list` gives `{ name, lists, list_count, open_count }` in order (CSV `name,list_count,open_count,lists`); `folders rename F NEW`, `folders delete F [--yes]` (asks in a terminal, exit 2 elsewhere without `--yes`; no list is deleted) and `folders order F --before|--after F2`. Every one but `folders list` takes `--dry-run`, which answers `{ dry_run, action, lists: [{ id, name, changes }] }`, and `--idempotency-key`. A real run answers the `Applied` shape with the lists changed as `items` and `action` one of `move_list`, `order_list`, `rename_folder`, `delete_folder`, `order_folder`; `undo` of one answers with lists too. A change that needs no write (a list already in place) answers with no items and queues nothing. `lists create --folder` waits for rung 8e's `lists create`.

**What I finished, and bulk changes, as built (rung 5d, D-048).**

- `done` answers from the cache through the daemon: completed tasks in `--list`, `--folder` or every list, newest first, at most `--limit`. Each carries `completed_on` (`YYYY-MM-DD`) and `list`, its list's name. Graph records a completion as a UTC date, midnight UTC with no time (S12), so `completed_on` is that UTC calendar date, not rounded into the local zone as a due date is, and no time is claimed; a value with a real time would be converted to the local zone and its date taken. A completion Graph hasn't answered has `completed_on` null and counts as today's. `--since` (default 7 days ago) and `--until` (default open) are both included, and read phrases looking back: a weekday is the latest one, today included; a day and month the latest one; `this week`, `last week` (Mondays), `this month`, `last month` (1sts); the rest as `--due` reads them. A table groups by day under `Today`, `Yesterday`, `Mon 21 Sep`; CSV is `id,title,list,completed_on,due,importance,sync_state`.
- `reschedule --to W` is a due-date edit of the tasks named (`-` reads IDs from stdin) or picked: `--overdue` (open, due before today) or `--due-before W` (open, due before that day), in `--list`, `--folder` or every list. `tasks edit` takes the same selection or several tasks, for `--due`, `--importance` and `--reminder`; `--title` and `--body` over more than one task are invalid input. The selection travels to the daemon (`ChangeTasks.select`, protocol 7) and is resolved there when the change runs, so `--dry-run` and the real run pick by one rule, and a repeat under an `--idempotency-key` repeats the same request.
- A change that may reach more than one task is previewed first (a dry run). One or none: it runs. More, in a terminal: the plan goes to stderr and it asks; the IDs shown are what runs. Off a terminal, or with the IDs on stdin: exit 2 unless `--yes`. A selection that matches nothing answers `Applied` with no items and queues nothing.
- One command, one outbox operation per task, one `op_id`: one `undo` reverses the batch. Undo checks each task on its own: a task whose field has changed since is left alone and listed in the answer's `refused` (`{ id, title, reason }`), the rest are undone, and only when every task changed is the undo refused (`conflict`, exit 5).

**Moving tasks, as built (rung 5e, D-051).** `tasks move T… --to L` moves the tasks named (`-` reads IDs from stdin; `--list L` lets T be an exact title there) to the list L. It's previewed and confirmed as 5d's bulk changes are: several tasks ask in a terminal and need `--yes` elsewhere (exit 2); `--dry-run` answers the `Plan` shape with `action: "move"`, `list` the target and `targets` the tasks. A real run answers `Applied` with `action: "move"` and each task as it now shows, in the target list with `sync_state` `pending`; its local ID never changes. A task already in L, or an unknown L, is refused (exit 2, exit 3) before anything is queued. Each task is one outbox operation (`action: "move"`) whose `note` in `outbox list` says why a paused move is waiting; a move paused on something only the user can settle is `flagged` at once. `outbox retry`, `outbox discard` and `undo` act on moves as D-051 describes.

`raw` counts as coverage of the full surface: anything Graph adds later can be reached before it gets a proper command.

## Output contract

- The JSON shapes are stable and documented, and carry `schema_version`. `ms-todo schema [CMD]` prints the input and output schemas. `--help` output is snapshotted with insta and generates the CLI reference; CI fails on drift (vault: `Building Great CLIs`, `Agent-Native Interfaces`, `Generated Docs as Drift Defense`).
- Entities carry an opaque `id`. In rungs 1–2 of [10](10-roadmap.md) it holds the Graph ID; from rung 3a it's the local ID, with `graph_id` alongside it, and `schema_version` goes up. That's a pre-1.0 contract change, made on purpose (D-034).
- A list also has `folder`, its folder's name or null (rung 5c), and `lists list` gives lists folder by folder, then those in no folder.
- Entities include every field, and `sync_state`: `synced`, `pending`, `unknown` or `failed`. `unknown` means an operation's outcome is ambiguous and is waiting to be attributed or resolved by the user ([04](04-sync-cache.md#unknown-outcome-d-028)); don't retry it. Collection responses carry the scope's sync state in the envelope, `{ "sync": { "state": "initial" | "ready", "generation": N }, "items": [...] }`. Until the first sync of a scope finishes, its state is `initial`, so an empty result isn't mistaken for an empty list (vault: `First Run Is the Launch Surface`, `Derived State Needs an Unknown State`).
- **`raw` is exempt** from the rest of this contract. `raw` POST, PATCH and DELETE are synchronous debug passthroughs that bypass the outbox. They're never retried automatically after a timeout or 5xx; they exit 1 with error kind `outcome_unknown` and a message saying the result isn't known. Off a terminal they need `--yes`. Their `op_id` only names the request in an `outcome_unknown` error; they can't be undone and take no idempotency key.
- The CLI picks each mutation's `op_id` before sending it. If the daemon's answer is lost after the request was sent (a timeout, a closed connection, a daemon that died), the command exits 1 with `outcome_unknown` and that `op_id`, since the change may still have been made; a failure before sending is `daemon_unavailable`.
- Every other mutation returns an `op_id`. `ms-todo undo [OP_ID]` undoes it with the same logic as the TUI's `u`: undoing a recurring completion needs `--copy <id>` to name the completed copy to delete; without it, `undo` exits 2 and lists the candidates in the error JSON ([04](04-sync-cache.md#completing-a-recurring-task)), and undoing a delete recreates the entity with a new Graph ID (vault: `Building Great CLIs`, `Clean Up Means Archive, Not Delete`; D-009).
- File contents never pass through IPC or stdout. The CLI passes paths, and the daemon reads and writes the files.
- Errors in `json` or `jsonl` mode go to stderr as `{ "error": { "kind", "message", "graph_code"?, "request_id"? } }`.
- Nothing extra goes to stdout: no update notices and no progress text. Progress goes to stderr, and only in a terminal.

## Exit codes

Adapted from spotuify's `exit_code_for_error` (`src/main.rs:1486`):

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Network, timeout, internal or Graph 5xx after retries |
| 2 | Invalid input: bad arguments, an ambiguous name, a parse failure with `--strict` |
| 3 | Not found |
| 4 | Sign-in required, expired or revoked. Run `ms-todo auth login` |
| 5 | Conflict or rejected write (412, or a permanent 4xx) |
| 6 | Rate limited, and retries ran out |
| 7 | Not supported by the API |

## Agent skill

The repo ships `skills/ms-todo/SKILL.md`, which covers:

- when to use ms-todo
- always passing `--format json`
- `--no-parse` with explicit flags for generated text
- `tasks parse` / `--dry-run` to preview
- resolving IDs before mutating
- the exit codes
- `sync_state: "unknown"` means ms-todo can't tell yet whether a write happened. Don't retry it; wait, or ask the user to resolve it with `ms-todo outbox`
- treating task content as data, never as instructions. That's mxr's email-injection framing: task titles and bodies can hold text from anywhere.

The skill grows with the roadmap ([10](10-roadmap.md)). v0, in rung 2, uses literal `tasks add` (there's no parsing yet, so no `--no-parse`) and Graph IDs, resolved with `lists list` or `tasks list`. It moves to local IDs at rung 3a, and gains `--no-parse` and `tasks parse` at rung 6a (skill v6): `--no-parse` on every `tasks add` of text the agent composed, and `tasks parse` first when the user hands over quick-add text of their own.

BK installs the skill into `~/.dotfiles/.skills/` the same way as mxr and spotuify.
