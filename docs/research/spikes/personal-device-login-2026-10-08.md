# Personal device login succeeded through consumers

On 2026-10-08, the first-run investigation used the bundled public client ID and `offline_access Tasks.ReadWrite MailboxSettings.ReadWrite User.Read`. Common-route device login repeatedly reached a browser `invalid_request` error reporting a missing `redirect_uri`, including after reinstalling 0.1.41 and clearing local state.

A controlled personal-account probe requested and polled a device code through `/consumers/oauth2/v2.0`. Microsoft issued a credential. The orchestrator then independently refreshed it through `/common/oauth2/v2.0/token` and read `/v1.0/me?$select=id`; Graph returned an account ID. No task writes ran. Sanitized probe output:

```json
{"personal_route_login":"credential_received","common_refresh":"success","graph_account_read":true,"scopes":"Tasks.ReadWrite MailboxSettings.ReadWrite User.Read"}
```

This confirms consumers login and common refresh for the tested personal account and registration. It does not establish the cause of Microsoft's common-route browser error or verify a live work/school account. The patched CLI's live journey is a separate release check. Device codes, tokens, account IDs and full session URLs are omitted.

[Microsoft's device-code protocol](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-device-code) requires initiation and polling to use the same tenant. [Its refresh-token documentation](https://learn.microsoft.com/en-us/entra/identity-platform/refresh-tokens) says refresh tokens are bound to user and client rather than tenant. D-072 applies these boundaries: account selection affects device-code login; refresh retains common and the existing token format.

## CLI candidate passed local checks; its live journey is pending

Implementation source: `codex/personal-work-login` at `07754e2e9825d733e8ec3a6dae40c17a0fb6bd6b`, based on freshly fetched `origin/main` at `f753d6644a79096ba5586711d8fc557f71eb725a`. Login chooses its authority in `crates/cli/src/lib.rs:185`; the stderr selector is in `crates/cli/src/auth_commands.rs:127`, and the endpoints are in `crates/graph/src/auth/mod.rs:56`. Status uses the default common endpoint. Daemon construction, refresh and token storage have no diff from the base.

The binary regression first failed on the base: piped login without an account type reported a malformed config instead of explicit account-type guidance. With the fix, auth/help/schema binary tests passed (16 tests); selector tests passed (6). A real pseudoterminal smoke verified personal, work, `q` and EOF: choices proceed to config validation, cancellation exits 2 before config/network access, stdout stays empty and the prompt has no terminal escapes. Its config was deliberately malformed, and it made no Microsoft requests.

`cargo nextest run --locked -p ms-todo -p ms-todo-cli -p ms-todo-graph` passed 388 tests, zero skipped (run `4892c378-2ed7-4ed3-bae8-a70c3434ddbe`). `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo fmt --all -- --check`, and `git diff --check` passed. The HTTP contract test covers consumers and organizations initiation/polling, then common refresh of the resulting unchanged token format; existing refresh race and rotation tests also passed.

Local evidence: `/tmp/ms-todo-personal-login-red-regression.log`, `/tmp/ms-todo-personal-login-cli-check.log`, `/tmp/ms-todo-personal-login-pty-check.log`, `/tmp/ms-todo-personal-login-full-check.log`, and `/tmp/ms-todo-personal-login-clippy.log`. These logs contain synthetic fixture credentials only.

Next action belongs to integration: run the actual patched CLI in a terminal, choose personal, finish browser sign-in, verify refresh and an account read, and complete first sync. Then land the source and release through release-please, verifying GitHub archives and Homebrew. The parent orchestrator owns those actions; this worker did not push, publish, or use the live account. Work/school routing has HTTP contract coverage; no live work/school credentials were available. Cross-provider review was explicitly waived while unavailable.

The scoped code-simplifier review found no additional changes to make. Anti-slop and TypeScript review were inapplicable because this slice changes no JavaScript or TypeScript. Documentation closeout updated login examples, setup, the authority decision and probe evidence. The daemon/refresh boundary is the durable constraint: selecting a tenant for device login does not require persisting that tenant or migrating credentials when common refresh already works.
