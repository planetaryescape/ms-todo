# Personal device login succeeded through consumers

On 2026-10-08, the first-run investigation used the bundled public client ID and `offline_access Tasks.ReadWrite MailboxSettings.ReadWrite User.Read`. Common-route device login repeatedly reached a browser `invalid_request` error reporting a missing `redirect_uri`, including after reinstalling 0.1.41 and clearing local state.

A controlled personal-account probe requested and polled a device code through `/consumers/oauth2/v2.0`. Microsoft issued a credential. The orchestrator then independently refreshed it through `/common/oauth2/v2.0/token` and read `/v1.0/me?$select=id`; Graph returned an account ID. No task writes ran. Sanitized probe output:

```json
{"personal_route_login":"credential_received","common_refresh":"success","graph_account_read":true,"scopes":"Tasks.ReadWrite MailboxSettings.ReadWrite User.Read"}
```

This confirms consumers login and common refresh for the tested personal account and registration. It does not establish the cause of Microsoft's common-route browser error or verify a live work/school account. The patched CLI's live journey is recorded below. Device codes, tokens, account IDs and full session URLs are omitted.

[Microsoft's device-code protocol](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-device-code) requires initiation and polling to use the same tenant. [Its refresh-token documentation](https://learn.microsoft.com/en-us/entra/identity-platform/refresh-tokens) says refresh tokens are bound to user and client rather than tenant. D-072 applies these boundaries: account selection affects device-code login; refresh retains common and the existing token format.

## Patched CLI sign-in and first sync passed

Implementation source: `codex/personal-work-login` at `07754e2e9825d733e8ec3a6dae40c17a0fb6bd6b`, based on freshly fetched `origin/main` at `f753d6644a79096ba5586711d8fc557f71eb725a`. Login chooses its authority in `crates/cli/src/lib.rs:185`; the stderr selector is in `crates/cli/src/auth_commands.rs:127`, and the endpoints are in `crates/graph/src/auth/mod.rs:56`. Status uses the default common endpoint. Daemon construction, refresh and token storage have no diff from the base.

The binary regression first failed on the base: piped login without an account type reported a malformed config instead of explicit account-type guidance. With the fix, auth/help/schema binary tests passed (16 tests); selector tests passed (6). A real pseudoterminal smoke verified personal, work, `q` and EOF: choices proceed to config validation, cancellation exits 2 before config/network access, stdout stays empty and the prompt has no terminal escapes. Its config was deliberately malformed, and it made no Microsoft requests.

`cargo nextest run --locked -p ms-todo -p ms-todo-cli -p ms-todo-graph` passed 388 tests, zero skipped (run `4892c378-2ed7-4ed3-bae8-a70c3434ddbe`). `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo fmt --all -- --check`, and `git diff --check` passed. The HTTP contract test covers consumers and organizations initiation/polling, then common refresh of the resulting unchanged token format; existing refresh race and rotation tests also passed.

Durable verification: [PR #100 checks](https://github.com/planetaryescape/ms-todo/pull/100/checks) passed Linux, macOS and Raycast CI for [source commit `07754e2e9825d733e8ec3a6dae40c17a0fb6bd6b`](https://github.com/planetaryescape/ms-todo/commit/07754e2e9825d733e8ec3a6dae40c17a0fb6bd6b). The source SHA identifies the implementation and tests behind the local results above.

The patched debug CLI completed a real default-instance login after selecting personal in its terminal prompt. Microsoft returned its consumers verification URL, the user approved the passkey, and the CLI reported `signed_in: true` after its Graph account read. The installed 0.1.41 CLI also read the new credential successfully. `ms-todo sync --wait` completed with 31 scopes; doctor reported all 31 ready, no last sync error, and every outbox count zero. No cloud task writes ran. Common refresh was separately verified by the controlled probe above.

Integration and release-please/Homebrew verification remain next. Work/school routing has HTTP contract coverage; no live work/school credentials were available. Cross-provider review was explicitly waived while unavailable.

The scoped code-simplifier review found no additional changes to make. Anti-slop and TypeScript review were inapplicable because this slice changes no JavaScript or TypeScript. Documentation closeout updated login examples, setup, the authority decision and probe evidence. The daemon/refresh boundary is the durable constraint: selecting a tenant for device login does not require persisting that tenant or migrating credentials when common refresh already works.
