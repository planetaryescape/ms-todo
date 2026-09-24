# 09: Reuse map

Policy, taken from spotuify's `docs/blueprint/14-reuse-strategy.md`: **"Do not rewrite architecture that mxr already solved."** Copy first, adapt, and keep a `// Adapted from <repo> <path> @ <sha>` comment at the top of each copied file. Extract shared crates only when three projects need the same code.

Source commits, as reviewed on 2026-09-24:

- mxr: `planetaryescape/mxr` @ `dfb23d10138b1cfc24f8ea7450d3426e5e4da37a`
- spotuify: `planetaryescape/spotuify` @ `d807e5e4f9d2f09878cdc22309af3589623f7785`

Before copying, `git fetch` both repos and diff these paths against these SHAs. Newer fixes may have landed.

**Re-verified 2026-09-24: 0 commits since pin.** Both remote `HEAD`s (`git ls-remote`) still equal the SHAs above, and every cited path and line count checked out. The refinements from that check are folded into the table.

| ms-todo piece | Copy from | Size | Adapt |
|---|---|---|---|
| Device-code sign-in | mxr `crates/provider-outlook/src/auth.rs` | 365 lines | Use `/common` instead of the Personal/Work split, Graph scopes, ms-todo paths |
| Atomic 0600 token write, plus file lock | spotuify `crates/spotuify-spotify/src/auth.rs` (`atomic_write_mode_0600`, around :1487; `TOKEN_LOCK_TIMEOUT`), plus `crates/spotuify-protocol/src/paths.rs:281` (`ensure_private_dir`), which it needs | — | Take only those helpers. The rest of that file is Spotify PKCE. The non-Unix fallback (around :1529) is neither atomic nor 0600; don't copy it |
| Rate limiting and retry | spotuify `crates/spotuify-spotify/src/rate_limit.rs` | 498 lines | Graph's 429, `Retry-After` and 5xx; concurrency cap of 4 |
| Error enum shape | spotuify `crates/spotuify-spotify/src/error.rs` | 389 lines | Graph error codes, `request-id` |
| IPC codec | mxr `crates/protocol/src/codec.rs` | 54 lines | As is |
| Protocol envelope and forward-compat rules | spotuify `crates/spotuify-protocol` (the pattern), mxr `crates/protocol` | — | ms-todo requests, responses and events |
| Socket server and dispatch | mxr `crates/daemon/src/server.rs` (2,936 lines; take the skeleton only) | — | Much smaller command set |
| IPC client and daemon auto-start | mxr `crates/daemon/src/ipc_client.rs` (185 lines) | — | Instance-aware paths |
| Daemon stop rule, instance separation | spotuify `AGENTS.md` (stop = socket gone *and* PID exited), `SPOTUIFY_INSTANCE` | — | `MS_TODO_INSTANCE` |
| Background loops and backoff | mxr `crates/daemon/src/loops.rs` (3,184 lines; take the pattern only) | — | Sync, outbox and rollover loops |
| Output formats | The `OutputFormat` enum is in spotuify `crates/spotuify-protocol/src/output.rs` (18 lines); spotuify `crates/spotuify-cli/src/output.rs` (3,867 lines) holds only renderers; mxr `crates/daemon/src/output.rs` (23 lines) | — | table, json, jsonl, ids |
| Exit-code mapping | spotuify `src/main.rs:1486` `exit_code_for_error` | — | Codes in [07](07-cli.md) |
| TUI structure | mxr `crates/tui/src/{app/,ui/,runner.rs}` | — | Views in [08](08-tui.md) |
| Keybindings, command palette, hint bar, diagnostics page | mxr TUI (listed as reused in spotuify's reuse strategy) | — | |
| Workspace boundary test | mxr `tests/workspace_boundaries.rs` (114 lines) | — | ms-todo crate rules |
| Test setup | both: `.config/nextest.toml`, `insta`, `wiremock`, `assert_cmd`, `proptest` | — | |
| Release chain | mxr/spotuify `release-please-config.json`, `.github/workflows/{ci,release-please,release}.yml`, `install.sh`, `packaging/homebrew/*.rb`. spotuify also has `.github/workflows/release-lockfile.yml` and `scripts/render_homebrew_formula.sh` | — | Targets: `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` |
| Agent skill | spotuify `skills/spotuify/SKILL.md`, mxr `.agents/skills/mxr/SKILL.md` | — | Contents in [07](07-cli.md) |
| Outbox "unknown outcome" recovery | mxr `plans/014-send-outcome-recovery.md`, `plans/023-interrupted-mutation-jobs.md` (the lessons, not code) | — | Create POSTs whose result we never saw; see [04](04-sync-cache.md#unknown-outcome-d-028) |
| AGENTS.md | spotuify's section "Use spotuify to build spotuify" | — | "Use ms-todo to build ms-todo" |
| Licences | mxr's `LICENSE-MIT` and `LICENSE-APACHE` (already copied into this repo) | — | |

**References outside BK's repos** (read them, don't copy): MAG-Cie/mcp-microsoft-todo `src/graph.ts:133-270`, for the Graph-specific details of `graphFetch` retries and batch sub-request retries. MIT-licensed. See [../research/prior-art.md](../research/prior-art.md).

**Not reused:** mxr's mail crates, Tantivy search (we use SQLite FTS5 instead), `yup-oauth2` (Gmail only), the keychain crate (D-012), spotuify's librespot, audio and player crates, and both projects' MCP crates (D-008).
