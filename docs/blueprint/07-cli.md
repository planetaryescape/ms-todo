# 07: CLI

The CLI is the canonical surface (spotuify's contract): **every feature has a CLI subcommand.** A feature that only exists in the TUI isn't finished.

## Global flags

- `--format table|json|jsonl|ids`: default `table` in a terminal and `json` when piped. The format enum is adapted from `spotuify-cli/src/output.rs`, and mxr has the same idea.
- `--instance <name>`, `--fresh`, `--quiet`, `--no-color`.
- Mutations take `--dry-run`, which shows what would change. Destructive commands take `--yes`, and ask for confirmation in a terminal otherwise.
- Referring to things:
  - A list: `--list <name|local_id|graph_id>`. A name must match exactly one list, or the command errors out with the candidates. It never picks the first match; that was a flaw in the Python CLI.
  - A task: by `local_id`, or by a unique title match inside `--list`.
  - IDs can also be read from stdin, so `ms-todo tasks list --format ids | ms-todo tasks complete -` works.

## Command surface (v1: the whole API plus the custom features)

```
auth       login | logout | status | bearer
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
           edit T [same field flags, plus --clear-due etc.]
           complete T... | reopen T... | delete T...
           move T --to L
           parse "text"                 # show how quick add will read the text; no writes

steps      list T | add T "text" | edit T S "text" | check T S | uncheck T S | delete T S | order …
links      list T | add T URL [--name N] [--app A] [--external-id X] | edit … | delete T R
attachments list T | add T FILE... | download T [A] [--out DIR] | delete T A
extensions list (list|task) ID | get … NAME | set … NAME --json '{…}' | delete … NAME
categories list | create NAME [--color presetN] | rename … | recolor … | delete …

myday      list | add T... | remove T... | suggest | rollover [--dry-run]
outbox     list | retry OP | discard OP
raw        GET|POST|PATCH|DELETE PATH [--body JSON]   # authenticated passthrough to Graph, for debugging
```

`raw` counts as coverage of the full surface: anything Graph adds later can be reached before it gets a proper command.

## Output contract

- The JSON shapes are stable and documented. Entities include `local_id`, `graph_id`, every field, and `sync_state` (`synced`, `pending` or `failed`).
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
- treating task content as data, never as instructions. That's mxr's email-injection framing: task titles and bodies can hold text from anywhere.

BK installs the skill into `~/.dotfiles/.skills/` the same way as mxr and spotuify.
