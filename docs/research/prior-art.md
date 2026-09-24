# Prior art

Reviewed on 2026-09-24 before deciding to build ms-todo. Both tools were cloned into scratch directories and read, their tests were run, and their published packages were compared with their source. Neither is a dependency. Both are useful as references.

## underwear/microsoft-todo-cli (Python)

- Repo: https://github.com/underwear/microsoft-todo-cli, commit `2b8b16eb4308f00b69ef0bba5ea38150c6f59bd0`, PyPI `microsoft-todo-cli` 1.4.1, MIT.
- Size: `todocli/cli.py` 2,075 lines, `todocli/graphapi/wrapper.py` 1,120 lines, `todocli/graphapi/oauth.py` 144 lines. 7 stars, one maintainer, last commit 2026-02-11.

What's good:

- The PyPI 1.4.1 wheel's `todocli/` is byte-identical to the repo source.
- It only contacts `graph.microsoft.com`, `login.microsoftonline.com` and `pypi.org`. There's no `subprocess`, `eval` or shell execution.
- It has three dependencies: `pyyaml`, `requests` and `requests-oauthlib`.
- 273 of 275 tests pass once a dummy `keys.yml` exists.
- It covers lists, tasks, steps, recurrence, links and attachments, and has `--json` on every command.

Why we rejected it:

1. **Sign-in is a web-app flow with a client secret, not device code** (`oauth.py:97-111`). The user pastes the redirect URL back into the terminal, and the secret is stored in plain text in `~/.config/microsoft-todo-cli/keys.yml`. Entra client secrets expire after at most 24 months.
2. **Lists get cut off silently.** `get_tasks` sends `$top=100` and ignores `@odata.nextLink` (`wrapper.py:152-160`).
3. **Graph errors crash the program.** `parse_response` reads `["value"]` without checking the HTTP status (`wrapper.py:81`). A 401, 404 or 429 raises a `KeyError` that `main()` doesn't catch, so the user gets a traceback instead of a JSON error.
4. It has no handling for 429 or `Retry-After`.
5. Lists and tasks are looked up by name; only one command accepts `--id`. When two tasks share a title, it picks the first one.
6. The config is checked at import time (`oauth.py:63-75`), and `sys.exit(1)` fires if the keys are missing, so the tests can't even run without keys.
7. `token.json` is written 0644.
8. The publish workflow targets the retired `ubuntu-18.04` runner, so releases must be uploaded by hand.

The commit history is useful evidence. On 2026-02-08 the author added My Day commands and then replaced them with deep links the same day ("Replace My Day with generic deep links"). That agrees with our finding that the API doesn't expose My Day.

## MAG-Cie/mcp-microsoft-todo (TypeScript MCP server)

- Repo: https://github.com/MAG-Cie/mcp-microsoft-todo, commit `cf612cdadf943e80ea368b94d01a0f8afaa1547b`, npm `@mag-cie/mcp-microsoft-todo` 1.2.2, MIT.
- Size: `src/graph.ts` 1,131 lines, `src/index.ts` 1,262 lines, `src/auth.ts` 124 lines. 12 stars, last commit 2026-05-11.

What's good (this code is worth reading as a reference):

- **Sign-in:** MSAL device code, public client, `/common`. The token cache goes to `~/.mcp-microsoft-todo/token-cache.json`, chmod 0600, with a silent refresh first (`src/auth.ts:32-110`).
- **HTTP core** (`graphFetch`, `src/graph.ts:133-179`):
  - 429 and 5xx get bounded exponential retry that honours `Retry-After`, in both seconds and HTTP-date form.
  - A 401 triggers one forced re-sign-in.
  - Error messages are built from Graph's `error.code` and `error.message`.
- `paginateAll` follows `@odata.nextLink` (`src/graph.ts:193-210`). Throttled `$batch` sub-requests are retried individually (`src/graph.ts:246-270`).
- Everything is addressed by ID, and IDs are URL-encoded.
- Destructive tools are annotated as such.
- Strict `tsc` passes and 50 of 50 vitest tests pass. It has three runtime dependencies. The published npm `dist/` matches a local build, and releases come from tag-triggered CI that checks the version.
- It records one API quirk worth keeping: Graph rejects `$select` on `/me/todo/lists/{id}/tasks` for personal accounts with a `RequestBroker--ParseUri` 400 (`src/graph.ts:397-399`). **Confirmed on 2026-09-24 by spike [S3](spikes/S3.md)**, on every To Do endpoint including delta.

Why we rejected it:

1. **It signs in through the maintainer's app registration by default.** A client ID is hardcoded (`src/auth.ts:27`). Tokens never go to MAG&Cie, but the user consents to their app, which they could change or delete. `MS_CLIENT_ID` can override it. It also asks for `Tasks.ReadWrite.Shared`, which isn't needed.
2. **Pagination is opt-in** (`paginate` defaults to false) and is silently capped at 20 pages.
3. **`move_task` loses data.** It recreates the task in the target list and deletes the original (`src/graph.ts:459-482`). Checklist items, links, attachments and `createdDateTime` are lost, and a failure between the two steps leaves a duplicate.
4. **A device-code prompt can appear in the middle of a tool call** when a silent refresh fails. It writes to stderr, which MCP clients hide, so the call hangs until the code expires. This is inferred from the code and wasn't tested.
5. `npm audit` flags high-severity advisories in `hono`, `express`, `body-parser` and others, pulled in by the MCP SDK. They're in the HTTP transport, which isn't used over stdio.
6. **It has 28 MCP tools**, and their definitions cost context in every session. The owner doesn't want MCP at all (see decision D-008).

## Other tools found but not reviewed in depth

- [kiblee/tod0](https://github.com/kiblee/tod0): Python, 143 stars, active. It's mainly an interactive TUI, and its command mode for scripts is thin.
- [visionik/mstodo](https://github.com/visionik/mstodo): Node, 0 stars, no license.
- [mehmetseckin/todo-cli](https://github.com/mehmetseckin/todo-cli), [pappde/todo-sync](https://github.com/pappde/todo-sync) and [SaulNunez/todo](https://github.com/SaulNunez/todo): .NET.
- [microsofthackathons/tdi](https://github.com/microsofthackathons/tdi): Rust, from a 2022 hackathon, and needs a client secret compiled into the source.
- `microsoftgraph/msgraph-cli` (`mgc`): archived 2025-08-29, with no To Do commands.

## Owner's own Rust projects (the real base)

These are the main reference, not the tools above. See [../blueprint/09-reuse-map.md](../blueprint/09-reuse-map.md).

- [planetaryescape/mxr](https://github.com/planetaryescape/mxr) at `dfb23d10138b1cfc24f8ea7450d3426e5e4da37a`: terminal email client, 28 crates, about 254k lines. **`crates/provider-outlook/src/auth.rs` (365 lines) is a working Microsoft device-code sign-in** that handles RFC 8628 polling (`authorization_pending`, `slow_down`), refreshes 300 seconds before expiry, and writes the token file with tmp-then-rename and 0600 permissions.
- [planetaryescape/spotuify](https://github.com/planetaryescape/spotuify) at `d807e5e4f9d2f09878cdc22309af3589623f7785`: Spotify controller, 18 crates, about 180k lines. It already copies from mxr (`docs/blueprint/14-reuse-strategy.md`) and has the cleanest rate-limit module (`crates/spotuify-spotify/src/rate_limit.rs`) and exit-code mapping (`src/main.rs:1486`).
