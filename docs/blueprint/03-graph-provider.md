# 03: Graph provider (`crates/graph`)

## App registration (BK does this once, by hand)

The full step-by-step guide, with troubleshooting and a `curl` check, is in **[../setup/entra-app-registration.md](../setup/entra-app-registration.md)**. The short version: account types are *Any Entra ID Tenant + Personal Microsoft accounts*; turn on **Allow public client flows**; add the delegated permissions `Tasks.ReadWrite`, `MailboxSettings.ReadWrite`, `offline_access` and `User.Read`; no redirect URI; no client secret; register it in a personal directory, not the Contentful tenant.

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

**The daemon is the normal refresher.** Every data request goes through it (D-031). The `auth` commands, which work without a daemon, may also refresh when they need a token (`auth status` does). That's safe because every refresh is the compare-and-swap below, under `auth/token.lock`: whoever takes the lock second finds the new token and uses it instead of spending the old refresh token. Microsoft replaces the refresh token each time it's used. On `invalid_grant`, return a typed `AuthRevoked` error. The daemon stops syncing, the outbox keeps its operations, and clients get an `AuthRequired` event telling the user to run `ms-todo auth login`.

**A refresh is a compare-and-swap** (vault: `Refresh Token Rotation Is Shared State`). Take the token file lock, reload the file under the lock, and use a newer token if one is already there. Otherwise refresh, save the whole credential, and keep the old refresh token if the response doesn't include one. On `invalid_grant`, delete the stored credential only if the failed refresh token is still the one stored. `auth login` works without a healthy daemon, and the daemon watches `token.json` and clears `AuthRequired` when a new one lands (vault: `Credential Prompts Are Side Effects`).

**Clients never start interactive sign-in by themselves.** `ms-todo auth login` is the only thing that shows a device code. Every other command fails fast with exit code 4 and a message saying to run `ms-todo auth login`. The MCP server we reviewed hangs in this situation instead; that's the failure we're avoiding.

Other auth commands: `auth status` (who is signed in, scopes, token expiry), `auth logout`, and `auth bearer`, which prints the current access token for raw `curl` checks against Graph. That's spotuify's debugging workflow. `auth bearer` needs `--reveal-secret`, so a token never lands in a log by accident (vault: `Local Capability Surfaces Need Defense in Depth`).

## HTTP client

reqwest with rustls. Base URL `https://graph.microsoft.com/v1.0`. Never use beta unless a spike proves it's needed, and log it if so. Every request has a timeout: 60 seconds by default, longer for upload chunks (vault: `Deadlines Bound Stalls, Not Work`).

**Retry and rate limiting.** Adapt `spotuify/crates/spotuify-spotify/src/rate_limit.rs` (at `d807e5e`, 498 lines), which spotuify itself adapted from mxr:

- A pure `decide_retry(status, headers, attempt, idempotent)` function, which can be unit tested.
- **429:** honour `Retry-After`, which can be a number of seconds or an HTTP date. If there's no header, back off exponentially with jitter.
- **5xx, 408 and timeouts:** jittered exponential backoff, at most 3 retries, **for idempotent requests only** (below). We only ever send etags Graph gave us, so a 500 follows these same rules (S6 saw a 500 only for a malformed etag or `*`, which we never send).
- **401:** refresh the token once and retry once. If that fails, return `AuthExpired`.
- Keep the backoff state saved so a restarted daemon doesn't hit Graph again straight away.
- A priority semaphore so interactive writes aren't queued behind a large sync. **Cap concurrency at 4**, shared by sync, the outbox and `$batch`. S9 confirmed Graph's mailbox limit applies to To Do: 4 parallel requests are fine, and 8 get 429s. The sub-requests of a parallel `$batch` count towards the cap.
- **Never retry a non-idempotent request automatically after a timeout, 5xx or 408** (D-028). Non-idempotent means every create POST (task, checklist item, linked resource, extension, category, upload-session creation) and any PATCH that completes a recurring task. These are retried only on responses that prove the request didn't run: a 429, a 401 followed by a refresh, or a connection failure before the request was sent. Anything else returns an "outcome unknown" error, and the outbox puts the operation in `unknown` ([04](04-sync-cache.md#unknown-outcome-d-028)). The same rule applies to each sub-request inside a `$batch`. PATCHes that set absolute values, and DELETEs (where a 404 counts as success), keep the normal retry.
- **`Retry-After` is the only throttle signal.** To Do returns it as integer seconds on a 429 (`activityLimitReached`). It never sent the `x-ms-throttle-*` or `x-ms-resource-unit` headers the Graph docs describe, so don't build on them (S9).

**Errors:** a `thiserror` enum modelled on `spotuify-spotify/src/error.rs`:

- `AuthRequired`, `AuthExpired`, `AuthRevoked`
- `RateLimited{retry_after}`, `Forbidden`, `NotFound`, `Conflict`, `PreconditionFailed`
- `Api{status, code, message, request_id}`, where `request_id` is Graph's `request-id` header
- `Network`, `Decode`, `InvalidInput`

Always parse Graph's `{ error: { code, message } }` body. Never hand the user a raw status code or panic on a response you didn't expect. A malformed Graph ID gives 400 `ErrorInvalidIdMalformed`, not 404 (S10).

**Pagination:** every collection call follows `@odata.nextLink` to the end. There's no silent page cap. If a hard safety cap is ever hit, return an error; never return partial results as if they were complete. From P1:

- The default page is 50 tasks, for task lists and for delta.
- `Prefer: odata.maxpagesize` works on both, but it isn't carried in the `nextLink`. **Send it on every page request**, or page 2 onwards drops back to 50. Use 200 for the first sync.
- A short or empty page doesn't mean the last one. Delta often ends with an empty page that carries the `@odata.deltaLink`. Stop only when there's no `nextLink` (collections) or a `deltaLink` arrives (delta).

**Batch:** use `$batch` for fan-out, such as bulk writes or fetching extensions for many tasks. A batch returns HTTP 200 even when some requests inside it were throttled, so check each response's status and retry the throttled ones individually. That's MAG&Cie's lesson (`src/graph.ts:246-270`). From S10:

- At most 20 requests. 21 is a 400.
- A batch is **either fully sequential (one `dependsOn` chain) or fully parallel**. Mixing them, for example four independent chains, is a 400.
- Prefer sequential batches: they save round trips without adding concurrency, and batches of 20 never throttled. A parallel batch runs its sub-requests at once, so it counts as that many requests against the cap of 4; use one only with at most 4 sub-requests and nothing else in flight.
- In a sequential batch, every step after a failed one gets **424** `FailedDependency`. A 424 step never ran, so it can be retried. The failed step itself follows the same idempotency rule as a single request: a non-idempotent step that got a 5xx or timed out goes to `unknown`, not back into a batch.
- A sub-request can't refer to an ID created earlier in the same batch. Creating a task with its steps takes a POST, then a batch for the children.

**Concurrency control:** send `If-Match` with the stored `@odata.etag` on the calls that honour it (S6). A 412 means someone else changed it. Which calls honour it, and what to do on a 412, is in [04](04-sync-cache.md#conflicts).

## Query options

From S2 and S3:

- **Never send `$select`.** Every To Do endpoint rejects it with 400 `RequestBroker--ParseUri`, on v1.0 and beta. A task is only 11–15 keys anyway.
- **Delta takes no query options.** `$select`, `$filter` and `$top` are a 400 on delta. `$expand` is accepted but changes nothing: checklist items and linked resources are inline already, and extensions never come back. Send the plain delta request.
- **`$expand=extensions` only works with an ID filter:** `$expand=extensions($filter=id eq 'com.planetaryescape.mstodo')`. Without the filter it silently returns nothing. With it, it works on GET and collection GET for lists and tasks, and combines with `$filter=lastModifiedDateTime ge …`. On delta it's a 400.
- Collection GETs honour `$filter` (including `categories/any(…)`), `$orderby` and `$top`. `$count` is ignored.

## Endpoints (the whole surface)

All paths are under `/me`:

```
todo/lists                              GET POST
todo/lists/{l}                          GET PATCH DELETE
todo/lists/delta                        GET
todo/lists/{l}/extensions               POST            (+ /{name} GET PATCH DELETE; the collection GET is a 404 too, seen live 2026-09-25, D-058)
todo/lists/{l}/tasks                    GET POST        ($filter $orderby $top $expand)
todo/lists/{l}/tasks/delta              GET
todo/lists/{l}/tasks/{t}                GET PATCH DELETE
todo/lists/{l}/tasks/{t}/checklistItems GET POST        (+ /{c} GET PATCH DELETE)
todo/lists/{l}/tasks/{t}/linkedResources GET POST       (+ /{r} GET PATCH DELETE)
todo/lists/{l}/tasks/{t}/attachments    GET POST        (+ /{a} GET DELETE, /{a}/$value)
todo/lists/{l}/tasks/{t}/attachments/createUploadSession POST, then PUT chunks
todo/lists/{l}/tasks/{t}/extensions     POST            (+ /{name} GET PATCH DELETE; the collection GET is a 404, S2)
outlook/masterCategories                GET POST        (+ /{id} GET PATCH DELETE)
$batch                                  POST
```

**Attachments:**

- Under 3 MB: a single POST with `contentBytes` in base64.
- Up to 25 MB: an upload session. PUT chunks under 4 MB each, in order, and use `nextExpectedRanges` to resume an interrupted upload. As built (S16, D-056): a session can't be asked where it stands, so a chunk whose answer was lost is sent again, and 400 `InvalidStart` says it had landed. The final PUT returns 201 with a `Location` header containing the attachment ID. Cancel with DELETE on the session URL. The final PUT is what commits the attachment, so it's non-idempotent: if its response is lost, the outcome is `unknown` ([04](04-sync-cache.md#unknown-outcome-d-028)), and the upload is never retried or recreated automatically.
- Refuse anything over 25 MB before uploading, with a clear error.
- An attachment DELETE ignores `If-Match` (S16), and a listing never includes `contentBytes`.
- **Downloads are a trust boundary** (vault: `Attachment Writes Are a Trust Boundary`, `Spotuify Security Audit Synthesis`). Strip unsafe characters from the filename, reject `..` and symlinks, keep the result under the chosen directory, write to `<name>.part` and then rename, and use mode 0600.

## Testing

Use `wiremock` for every endpoint, including 429 with `Retry-After`, 5xx, 401-then-refresh, pagination, and a batch with a throttled sub-response. Use `insta` snapshots for request bodies. `decide_retry` gets unit tests for every branch. Add a live smoke test behind a feature flag, run only by hand, against BK's account: create a throwaway list, run through every endpoint, then delete the list.
