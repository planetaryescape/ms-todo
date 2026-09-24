# 07: CLI

> **`mst` is an official alias of `ms-todo`** (D-039 in [11](11-decision-log.md)): a symlink to the same binary, so every command here works as `mst …` too. Help and usage show the name you typed. `--version`, error hints and these docs say `ms-todo`, which always works.

The CLI is the canonical surface (spotuify's contract): **every feature has a CLI subcommand.** A feature that only exists in the TUI isn't finished.

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
           edit T [same field flags, plus --clear-due etc.]
           complete T... | reopen T... | delete T...
           move T --to L
           parse "text"                 # show how quick add will read the text; no writes
search     QUERY [--list L] [--status open|completed|all] [--limit N]   # title and notes, every list, best first (D-041)

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

`raw` counts as coverage of the full surface: anything Graph adds later can be reached before it gets a proper command.

## Output contract

- The JSON shapes are stable and documented, and carry `schema_version`. `ms-todo schema [CMD]` prints the input and output schemas. `--help` output is snapshotted with insta and generates the CLI reference; CI fails on drift (vault: `Building Great CLIs`, `Agent-Native Interfaces`, `Generated Docs as Drift Defense`).
- Entities carry an opaque `id`. In rungs 1–2 of [10](10-roadmap.md) it holds the Graph ID; from rung 3a it's the local ID, with `graph_id` alongside it, and `schema_version` goes up. That's a pre-1.0 contract change, made on purpose (D-034).
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

The skill grows with the roadmap ([10](10-roadmap.md)). v0, in rung 2, uses literal `tasks add` (there's no parsing yet, so no `--no-parse`) and Graph IDs, resolved with `lists list` or `tasks list`. It moves to local IDs at rung 3a, and gains `--no-parse` and `tasks parse` at rung 6.

BK installs the skill into `~/.dotfiles/.skills/` the same way as mxr and spotuify.
