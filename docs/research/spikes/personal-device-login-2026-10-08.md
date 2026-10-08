# Personal device login succeeded through consumers

On 2026-10-08, the first-run investigation used the bundled public client ID and `offline_access Tasks.ReadWrite MailboxSettings.ReadWrite User.Read`. Common-route device login repeatedly reached a browser `invalid_request` error reporting a missing `redirect_uri`, including after reinstalling 0.1.41 and clearing local state.

A controlled personal-account probe requested and polled a device code through `/consumers/oauth2/v2.0`. Microsoft issued a credential. The orchestrator then independently refreshed it through `/common/oauth2/v2.0/token` and read `/v1.0/me?$select=id`; Graph returned an account ID. No task writes ran. Sanitized probe output:

```json
{"personal_route_login":"credential_received","common_refresh":"success","graph_account_read":true,"scopes":"Tasks.ReadWrite MailboxSettings.ReadWrite User.Read"}
```

This confirms consumers login and common refresh for the tested personal account and registration. It does not establish the cause of Microsoft's common-route browser error or verify a live work/school account. The patched CLI's live journey is a separate release check. Device codes, tokens, account IDs and full session URLs are omitted.

[Microsoft's device-code protocol](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-device-code) requires initiation and polling to use the same tenant. [Its refresh-token documentation](https://learn.microsoft.com/en-us/entra/identity-platform/refresh-tokens) says refresh tokens are bound to user and client rather than tenant. D-072 applies these boundaries: account selection affects device-code login; refresh retains common and the existing token format.
