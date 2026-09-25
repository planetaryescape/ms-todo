# 01: Architecture

## Shape

```
          ┌──────────────┐     ┌──────────────┐     ┌──────────────┐
          │  ms-todo CLI │     │ ms-todo tui  │     │ agent skill  │
          └──────┬───────┘     └──────┬───────┘     └──────┬───────┘
                 │  length-delimited JSON over a Unix socket (IPC)   │
                 └───────────────┬──────────────────────────────┘
                          ┌──────▼───────┐
                          │    daemon    │  owns: sign-in, token refresh, sync loop,
                          │              │  outbox worker, My Day rollover, events
                          └──┬────────┬──┘
                   ┌─────────▼──┐  ┌──▼──────────────┐
                   │  SQLite    │  │ Microsoft Graph │
                   │ (WAL, sqlx)│  │  v1.0 over HTTPS│
                   └────────────┘  └─────────────────┘
```

**Every client, the TUI included, reads and writes only over the daemon protocol** (D-031). The TUI starts from a cached snapshot the daemon serves (spotuify's `ClientSeed` pattern), then applies `EntityChanged` events. Only the daemon touches `store` and talks to Graph. A Unix-socket round trip costs about 5–10 µs (vault: `Local IPC vs HTTP`), so the latency budget in [08](08-tui.md) still holds.

## Why a daemon

We first proposed no daemon and then reversed that (see D-006). The reasons for having one:

1. **Fast CLI and agent calls.** Without a daemon, every CLI call has to sync over the network before reading, or risk reading stale data. With one, the daemon keeps SQLite fresh, and a CLI read is a socket round-trip to local data.
2. **The outbox needs something that stays running.** Instant writes are queued. Only a long-lived process can reliably send them after the TUI exits, retry them while offline, and roll back ones that are permanently rejected.
3. **One process normally refreshes the token.** Microsoft replaces the refresh token on each use. If two processes refresh at once without coordinating, one ends up holding a revoked token. The daemon does the refreshing; the `auth` commands may refresh too when they need a token, which is safe because every refresh is a compare-and-swap under `auth/token.lock` ([03](03-graph-provider.md#sign-in)).
4. **Changes are pushed to the TUI.** The daemon sends change events to every connected client, so the TUI never polls.
5. **Scheduled work.** The daily My Day rollover and periodic sync belong in a process that runs all the time.

mxr and spotuify already handle the known costs: auto-starting the daemon, stale sockets, version mismatch between client and daemon, and startup races. Copy their solutions ([09-reuse-map.md](09-reuse-map.md)).

## Daemon lifecycle

- Any client auto-starts the daemon if the socket is missing or dead. It spawns a detached process and waits on a readiness check, following mxr's pattern.
- **Readiness is not liveness.** The daemon binds the socket before migrations and the first sync. Ready means `Status` answers with a compatible protocol version, and `Status` reports any subsystem still starting (vault: `Daemon Readiness Is Not Process Liveness`).
- `ms-todo daemon start|stop|status|restart`. A stop only counts as done when the socket is unreachable and the daemon's own PID has exited. That's spotuify's rule, which exists because of stray daemons.
- Auto-start fully detaches the daemon (through `ms-todo daemon launch`, so no client is ever its parent), and a zombie counts as exited (D-046).
- Clients and the daemon exchange a protocol version when they connect. If they don't match, the client restarts the daemon (after an upgrade) or reports a clear error.
- Optional service files for launchd and systemd, like spotuify's `install/`.
- Dev and installed builds are kept apart with an instance name, like spotuify's `SPOTUIFY_INSTANCE`: `MS_TODO_INSTANCE`. Binaries under `target/` default to `ms-todo-dev`. This matters because agents build and run dev daemons.

## Crates

Keep it small. Proposed workspace:

| Crate | Job |
|---|---|
| `ms-todo` (root bin) | Entry point, dispatches to the CLI, TUI or daemon |
| `crates/core` | Domain types (List, Task, ChecklistItem, …), IDs, errors shared by all crates |
| `crates/protocol` | IPC request, response and event types, the codec, protocol version |
| `crates/graph` | Sign-in (device code and token store), the HTTP client (retry, rate limit, pagination, batch), typed endpoints for the whole API |
| `crates/store` | SQLite schema, migrations, queries, outbox |
| `crates/sync` | Delta sync engine, reconciliation, outbox worker, My Day rollover |
| `crates/nlp` | Quick-add parser: dates, recurrence, tokens. Pure, no I/O |
| `crates/daemon` | Socket server, request dispatch, owns the sync and outbox tasks |
| `crates/cli` | clap definitions, output formatting, exit codes |
| `crates/tui` | ratatui app |

Enforce the direction of dependencies with a `tests/workspace_boundaries.rs` like mxr's. `nlp` and `core` must not depend on anything with I/O, and `tui` and `cli` depend on neither `graph` nor `store` (D-031).

## Transport

Length-delimited JSON (`tokio_util::codec::LengthDelimitedCodec`) over a Unix socket, with the message envelope `{ id, payload }`. Unknown event tags decode to an `Unknown` variant, and new fields need `#[serde(default)]`, so old clients and newer daemons get along. This is copied from `mxr/crates/protocol/src/codec.rs`, following spotuify's adaptation.

**Bounds.** Set the codec's `max_frame_length` explicitly rather than relying on tokio-util's 8 MiB default (vault: `Length-Prefixed Framing`). File bytes never cross IPC: the CLI passes paths, and the daemon reads and writes the files. Events that carry collections hold at most 500 IDs; past that, the daemon sends `ResyncNeeded` instead (vault: `Every Event Payload Needs a Bound`).

**Stalls, not totals.** Long calls (`sync --wait`, the first sync, `tasks move`, attachment transfers) send progress events. A client gives up only after a period with no progress, never on total time (vault: `Deadlines Bound Stalls, Not Work`; spotuify's bounded-timeout rule). Windows isn't a target for v1; ms-todo is macOS-first, with Linux supported.

## Files

Resolve data and runtime paths with the `dirs` crate:

- Config: `config.toml` in `$MS_TODO_CONFIG_DIR`, else `$XDG_CONFIG_HOME/ms-todo/`, else `~/.config/ms-todo/`, on macOS as well as Linux. Not `dirs::config_dir()`, which is `~/Library/Application Support` on macOS (D-035). It holds `[auth] client_id` and the TUI's `[tui] theme` and `[tui.colors]` ([08](08-tui.md#themes), D-049).
- Data: `<data_dir>/ms-todo/`, containing:
  - `ms-todo.db`: SQLite, WAL mode
  - `auth/token.json`: mode 0600
  - `logs/`
- Runtime: the socket under `<runtime_dir>`, falling back to `<data_dir>/ms-todo/run/`. The path includes the instance name. The socket is mode 0600 inside a 0700 directory (vault: `Local Capability Surfaces Need Defense in Depth`).
