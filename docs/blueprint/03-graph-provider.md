# 03: Graph provider (`crates/graph`)

## App registration (BK does this once, by hand)

1. Go to https://entra.microsoft.com. Under **App registrations**, choose **New registration**.
2. **Supported account types:** "Accounts in any organizational directory and personal Microsoft accounts". That lets `/common` work for both kinds of account.
3. No redirect URI is needed for device code. Under **Authentication**, set **Allow public client flows** to Yes.
4. **API permissions** (all delegated): `Tasks.ReadWrite`, `MailboxSettings.ReadWrite` (for categories), `offline_access`, `User.Read` (to show who is signed in).
5. Copy the **Application (client) ID**. There's no client secret.

The client ID is not a secret; mxr's security audit reached the same conclusion. Take it from config (`auth.client_id`) or an environment variable (`MS_TODO_CLIENT_ID`). Release builds bake in BK's client ID with `option_env!("MS_TODO_CLIENT_ID")`, which is mxr's `BUNDLED_CLIENT_ID` pattern (D-025). `auth login` and the README encourage users to register their own app instead, and `auth status` shows which ID is in use. Registering the app may ask for an Azure signup, which can require card verification; see the research doc's registration section.

## Sign-in

**Device code (RFC 8628) against `https://login.microsoftonline.com/common/oauth2/v2.0/{devicecode,token}`.** Adapt `mxr/crates/provider-outlook/src/auth.rs`. That's mxr at `dfb23d1`, 365 lines, and already a working Microsoft device-code implementation:

- Poll correctly: on `authorization_pending`, keep going; on `slow_down`, add 5 seconds to the interval; on `expired_token` or `access_denied`, stop with a clear error.
- **Tolerate short network failures while polling.** mxr allows up to 6 failures in a row (`DEVICE_POLL_MAX_TRANSPORT_FAILURES`). The lesson is in mxr's `plans/006-auth-poll-transient-errors.md`: a network blip must not cancel sign-in.
- Refresh 300 seconds before the token expires (`REFRESH_MARGIN_SECS`).
- Put a 30-second timeout on HTTP calls during sign-in. mxr's comment notes that the Gmail provider was bitten by exactly this kind of hang.
- Scopes: `offline_access Tasks.ReadWrite MailboxSettings.ReadWrite User.Read`.
- Differences from mxr: use `/common` instead of the `consumers`/`organizations` split, and use Graph scopes instead of IMAP and SMTP.

**Token storage:** a file at `<data_dir>/ms-todo/auth/token.json`, written atomically with mode 0600 (tmp file, then rename, as in spotuify's `atomic_write_mode_0600`, `crates/spotuify-spotify/src/auth.rs:1487`), behind an `fs2` file lock. We're not using the Keychain; see D-012.

**Only the daemon refreshes.** Microsoft replaces the refresh token each time it's used. On `invalid_grant`, return a typed `AuthRevoked` error. The daemon stops syncing, the outbox keeps its operations, and clients get an `AuthRequired` event telling the user to run `ms-todo auth login`.

**Clients never start interactive sign-in by themselves.** `ms-todo auth login` is the only thing that shows a device code. Every other command fails fast with exit code 4 and a message saying to run `ms-todo auth login`. The MCP server we reviewed hangs in this situation instead; that's the failure we're avoiding.

Other auth commands: `auth status` (who is signed in, scopes, token expiry), `auth logout`, and `auth bearer`, which prints the current access token for raw `curl` checks against Graph. That's spotuify's debugging workflow.

## HTTP client

reqwest with rustls. Base URL `https://graph.microsoft.com/v1.0`. Never use beta unless a spike proves it's needed, and log it if so.

**Retry and rate limiting.** Adapt `spotuify/crates/spotuify-spotify/src/rate_limit.rs` (at `d807e5e`, 498 lines), which spotuify itself adapted from mxr:

- A pure `decide_retry(status, headers, attempt)` function, which can be unit tested.
- **429:** honour `Retry-After`, which can be a number of seconds or an HTTP date. If there's no header, back off exponentially with jitter.
- **5xx and 408:** jittered exponential backoff, at most 3 retries.
- **401:** refresh the token once and retry once. If that fails, return `AuthExpired`.
- Keep the backoff state saved so a restarted daemon doesn't hit Graph again straight away.
- A priority semaphore so interactive writes aren't queued behind a large sync. Graph's documented mailbox limit is **4 concurrent requests** per app and mailbox. Cap concurrency at 4 until a spike shows the To Do limit is different.

**Errors:** a `thiserror` enum modelled on `spotuify-spotify/src/error.rs`:

- `AuthRequired`, `AuthExpired`, `AuthRevoked`
- `RateLimited{retry_after}`, `Forbidden`, `NotFound`, `Conflict`, `PreconditionFailed`
- `Api{status, code, message, request_id}`, where `request_id` is Graph's `request-id` header
- `Network`, `Decode`, `InvalidInput`

Always parse Graph's `{ error: { code, message } }` body. Never hand the user a raw status code or panic on a response you didn't expect.

**Pagination:** every collection call follows `@odata.nextLink` to the end. There's no silent page cap. If a hard safety cap is ever hit, return an error; never return partial results as if they were complete. Use `Prefer: odata.maxpagesize` where it's supported.

**Batch:** use `$batch` (at most 20 requests per batch) for fan-out, such as fetching checklist items for many tasks. A batch returns HTTP 200 even when some requests inside it were throttled, so check each response's status and retry the throttled ones individually. That's MAG&Cie's lesson (`src/graph.ts:246-270`).

**Concurrency control:** send `If-Match` with the stored `@odata.etag` on PATCH and DELETE where Graph accepts it (spike S6). A 412 means someone else changed it; see [04](04-sync-cache.md).

## Endpoints (the whole surface)

All paths are under `/me`:

```
todo/lists                              GET POST
todo/lists/{l}                          GET PATCH DELETE
todo/lists/delta                        GET
todo/lists/{l}/extensions               GET POST        (+ /{name} GET PATCH DELETE)
todo/lists/{l}/tasks                    GET POST        ($filter $orderby $top $expand)
todo/lists/{l}/tasks/delta              GET
todo/lists/{l}/tasks/{t}                GET PATCH DELETE
todo/lists/{l}/tasks/{t}/checklistItems GET POST        (+ /{c} GET PATCH DELETE)
todo/lists/{l}/tasks/{t}/linkedResources GET POST       (+ /{r} GET PATCH DELETE)
todo/lists/{l}/tasks/{t}/attachments    GET POST        (+ /{a} GET DELETE, /{a}/$value)
todo/lists/{l}/tasks/{t}/attachments/createUploadSession POST, then PUT chunks
todo/lists/{l}/tasks/{t}/extensions     GET POST        (+ /{name} GET PATCH DELETE)
outlook/masterCategories                GET POST        (+ /{id} GET PATCH DELETE)
$batch                                  POST
```

**Attachments:**

- Under 3 MB: a single POST with `contentBytes` in base64.
- Up to 25 MB: an upload session. PUT chunks under 4 MB each, in order, and use `nextExpectedRanges` to resume an interrupted upload. The final PUT returns 201 with a `Location` header containing the attachment ID. Cancel with DELETE on the session URL.
- Refuse anything over 25 MB before uploading, with a clear error.

## Testing

Use `wiremock` for every endpoint, including 429 with `Retry-After`, 5xx, 401-then-refresh, pagination, and a batch with a throttled sub-response. Use `insta` snapshots for request bodies. `decide_retry` gets unit tests for every branch. Add a live smoke test behind a feature flag, run only by hand, against BK's account: create a throwaway list, run through every endpoint, then delete the list.
