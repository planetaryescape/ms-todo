# Microsoft To Do API

Yes. Microsoft To Do is backed by the Microsoft Graph To Do API (`todoTaskList` / `todoTask`, generally available at `v1.0`). It works the same way for a personal Microsoft account (MSA) and for a work/school (Entra ID) account — same endpoints, same resource shapes, slightly different permission and app-registration details. You can list, create, update, and delete lists and tasks, subtasks (checklist items), file attachments, and linked resources; you can run delta queries and set up webhooks. You cannot do anything that requires no signed-in user (there is no daemon/application-permission path for reading or writing a single personal task list), and a few To Do UI features (My Day, list sharing/assignment) have no corresponding API property, though this is inferred from the documented resource shape rather than stated outright by Microsoft.

Research done 2026-09-24 against Microsoft Learn. Corrections applied the same day are noted inline; the build decisions that follow from this research live in `docs/blueprint/`. Graph surfaces change, so re-check the linked pages before relying on anything version-specific.

## The API itself

The current API is the To Do API: `todoTaskList` at `/me/todo/lists` and `todoTask` at `/me/todo/lists/{id}/tasks`, generally available at `v1.0`. The [v1.0 overview page](https://learn.microsoft.com/en-us/graph/api/resources/todo-overview?view=graph-rest-1.0) and the [To Do API overview concept page](https://learn.microsoft.com/en-us/graph/todo-concept-overview) both describe it as the way to manage tasks across To Do, Outlook, and Teams. A beta version exists at the same paths (`?view=graph-rest-beta`) as a documented superset — same resources, plus whatever fields Microsoft is trialing at any given time.

There is an older, unrelated API: the legacy Outlook Tasks API (`outlookTask`, `outlookTaskFolder`, `outlookTaskGroup`, under `/me/outlook/tasks`). It is dead. The beta resource page for it says outright: "The Outlook tasks API is deprecated and stopped returning data on August 20, 2022. Use the To Do API instead" ([outlookTaskFolder, beta](https://learn.microsoft.com/en-us/graph/api/resources/outlooktaskfolder?view=graph-rest-beta)). It never existed at `v1.0` — only at `graph-rest-beta` — which is itself evidence it never graduated. The Power Automate/Logic Apps connector built on it is also marked deprecated, telling integrators to move to the Microsoft To-Do (Business) or Microsoft To-Do (Consumer) connectors instead, and noting that `outlookTaskGroup` and the `assignedTo`/`owner` properties on `outlookTask` have no equivalent in the new connectors "due to underlying API deprecation" ([Outlook Tasks connector, deprecated](https://learn.microsoft.com/en-us/connectors/outlooktasks/)).

## CRUD surface

Confirmed against the [todoTask resource page](https://learn.microsoft.com/en-us/graph/api/resources/todotask?view=graph-rest-1.0), v1.0 unless noted:

- **Task lists** (`todoTaskList`): full CRUD at `/me/todo/lists`.
- **Tasks** (`todoTask`): full CRUD at `/me/todo/lists/{id}/tasks`. Properties include `title`, `body`, `status`, `importance`, `isReminderOn`, `reminderDateTime`, `dueDateTime`, `startDateTime`, `completedDateTime`, `recurrence`, `categories`. Categories map to the display name of an `outlookCategory` the user has already defined — there's no separate categories-management endpoint scoped to To Do.
  - *Phase 0 note (2026-09-24, [S7](spikes/S7.md)):* a task accepts a category name with no master category behind it; none is created. Master category names can't be renamed.
- **Checklist items** (`checklistItem`), the API name for what the To Do UI calls "steps": full CRUD as a `todoTask` sub-resource (`/tasks/{id}/checklistItems`). This is the subtask feature.
- **Linked resources** (`linkedResource`): full CRUD, used to point a task back at the app or email it came from.
- **File attachments** (`taskFileAttachment`): full CRUD, including small-file attach, an upload-session flow for large files, get, and delete — see [Create taskFileAttachment](https://learn.microsoft.com/en-us/graph/api/todotask-post-attachments?view=graph-rest-1.0) and [Attach files to a To Do task](https://learn.microsoft.com/en-us/graph/todo-attachments). This is a v1.0 resource. If you'd assumed file attachments were API-inaccessible, that assumption is wrong as of this API — they're fully supported.
- **Open extensions** (`extension`): `todoTask` explicitly supports open extensions for stashing arbitrary custom data on a task, per the resource page.
  - *Phase 0 note (2026-09-24, [S2](spikes/S2.md), [S13](spikes/S13.md)):* reading them needs `$expand=extensions($filter=id eq '…')`; delta never returns them; PATCH replaces the whole document; a task POST can carry one inline.
- **Recurrence, reminders, due dates, importance**: all plain properties on `todoTask`, read/write at v1.0.
  - *Phase 0 note (2026-09-24, [S11](spikes/S11.md), [S12](spikes/S12.md)):* `dueDateTime` and `startDateTime` keep only the date; a time survives only in `reminderDateTime`. Completing a recurring task rolls the same ID forward and creates a new completed copy.
- **Delta query**: supported for both `todoTaskList` and `todoTask` (see the delta section below).

Gaps, based on what is and isn't in the documented `todoTask` property/relationship list, not on an explicit Microsoft statement of "unsupported":

- **My Day**: no property exists on `todoTask` for My Day membership. The API has no documented way to read or set what's on today's My Day list.
- **List sharing / task assignment**: no relationship or property for sharing a list with another person or assigning a task to someone. This matches the connector page's note that `assignedTo`/`owner` from the old Outlook Tasks API have no equivalent in the current API.
- **Planner integration**: Planner is a separate Graph resource (`planner*`) with its own permission scopes; To Do and Planner tasks are not the same object even where the UI blends them (Tasks app in Teams).

## Auth

### Scopes

Confirmed directly from the [permissions reference page](https://learn.microsoft.com/en-us/graph/permissions-reference) (the alphabetical `Tasks.*` entries), and cross-checked against the per-operation tables on the [list tasks](https://learn.microsoft.com/en-us/graph/api/todotasklist-list-tasks?view=graph-rest-1.0&tabs=http) and [create task](https://learn.microsoft.com/en-us/graph/api/todotasklist-post-tasks?view=graph-rest-1.0&tabs=http) pages:

| Permission | Type | Admin consent | Notes |
| --- | --- | --- | --- |
| `Tasks.Read` | Delegated | No | Read the signed-in user's tasks and lists, including shared ones. Explicitly listed as "available for consent in personal Microsoft accounts." |
| `Tasks.ReadWrite` | Delegated | No | Create, read, update, delete the signed-in user's tasks and lists. Also explicitly available for personal Microsoft accounts. |
| `Tasks.Read.Shared` | Delegated | No | Read tasks the user has access to, including shared ones. |
| `Tasks.ReadWrite.Shared` | Delegated | No | Create/read/update/delete tasks the user has access to, including shared ones. |
| `Tasks.Read.All` | Application | Yes | Read all users' tasks and lists in the org, no signed-in user. |
| `Tasks.ReadWrite.All` | Application | Yes | Read/write all users' tasks and lists in the org, no signed-in user. |

Crucially: for the actual list/read/create/update/delete operations on `todoTaskList`/`todoTask`, the per-operation tables say something narrower than the general reference page implies. Reading (`GET`) supports `Tasks.Read.All` as an application permission. Writing (`POST`/`PATCH`/`DELETE`) does not — the create-task page states plainly: "Application | Not supported. | Not supported." So there is no application-permission path to create, update, or delete To Do tasks at all, personal or org account, and no application-permission read path for a personal Microsoft account either (`Tasks.Read.All` is an org-only, admin-consented permission — it has no personal-account equivalent, since personal accounts have no tenant admin to grant it). For a personal Microsoft account, every operation is delegated and requires an interactively-consented signed-in user.

### Delegated vs application, and daemon apps

There is no daemon (unattended, no-user) path for the To Do API. Every write operation is delegated-only. Reads support an org-wide application permission (`Tasks.Read.All`), but that's "read everyone's tasks in the tenant," not "read one specific personal task list without a user" — and it doesn't exist for personal accounts. If BK wants a CLI that reads or writes his own personal To Do list unattended, the practical shape is still a delegated token, refreshed silently via a cached refresh token — not a client-credentials/app-only flow.

### OAuth flows for a local CLI

Two public-client flows apply to a CLI with no web server and no client secret:

- **Device code flow** (RFC 8628): works with both personal Microsoft accounts and work/school accounts. There's a documented UX quirk on personal accounts — Microsoft's own guidance notes device-code sign-in on an MSA can prompt for sign-in twice (an artifact of how personal accounts are federated into the flow). The quirk is cosmetic; device code remains the right fit for a headless CLI because it needs no redirect listener or secret.
- **Authorization code + PKCE**: also works for both account types, needs a local redirect listener (loopback) since there's no server-side secret to protect.

Tenant path matters: `/common` accepts both account types, `/consumers` accepts only personal Microsoft accounts, `/organizations` accepts only work/school accounts. For a CLI meant to work regardless of which kind of account BK signs in with, `/common` is the one to use.

### Registering an app: personal account, no Azure subscription

This was the most tangled part of the research and is worth being explicit about the conflicting signals.

The generic [quickstart-register-app](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app) doc lists as a prerequisite: "An Azure account that has an active subscription," linking to the Azure free-account signup. Azure's own free-account page ([azure.microsoft.com/pricing/purchase-options/azure-account](https://azure.microsoft.com/en-us/pricing/purchase-options/azure-account)) confirms signup requires a credit or debit card for identity verification (a temporary $1 authorization, reversed after verification), even on the free tier.

But that quickstart is written for the general case and documents "Personal accounts only" and "Any Entra ID Tenant + Personal Microsoft accounts" as selectable **Supported account types** during registration — meaning the registration UI itself explicitly plans for a personal-account-only app, once you're in the Entra admin center. A [Microsoft Q&A answer from a Microsoft employee](https://learn.microsoft.com/en-us/answers/questions/436510/cost-of-app-registrations-in-azure) states directly that "If you are in Free Edition of Azure AD, you do not have to pay anything for App registration, its free," and that there's no per-Graph-call charge either. This is a forum answer, not a canonical doc, but it is from a Microsoft moderator/employee account and is consistent with long-standing behavior: signing into `portal.azure.com` or `entra.microsoft.com` with any Microsoft account (personal or work) provisions a free default Entra ID directory ("Default Directory") automatically, with App registrations available in it at no cost.

What I could not find is a single canonical Microsoft Learn page that says, in so many words, "you can register an app with only a personal Microsoft account and zero Azure subscription, full stop." The prerequisite line in the quickstart ("an Azure account that has an active subscription") is the one piece of friction, and I can't rule out that it reflects a real requirement to have at least gone through Azure signup (which itself is free but does want a card for verification) rather than an ongoing paid subscription. My honest read: app registration itself costs nothing and doesn't need a paid subscription tier, but you likely still go through the Azure/Entra signup flow at least once, which currently asks for a card for anti-bot verification even though nothing gets charged. If BK already has any Microsoft/Azure sign-in history (an existing Contentful-independent MSA that's ever touched portal.azure.com), this is probably moot. I'm flagging this as the one point in this document that isn't backed by an unambiguous canonical source — treat "should not require an Azure subscription" as "probably true, not fully confirmed."

### Refresh tokens

Default refresh token lifetime for non-SPA public clients (which a local CLI is) is 90 days, and it renews on each use — meaning a CLI run at least every 90 days keeps working indefinitely without a fresh interactive sign-in, until the user or an admin revokes it. (Reference: Microsoft identity platform token lifetime documentation on refresh token configurable lifetimes; this is standard, well-documented platform behavior, not To Do-specific.)

## Rate limits and throttling

The [general throttling limits page](https://learn.microsoft.com/en-us/graph/throttling-limits) lists a global cap of 130,000 requests per 10 seconds per app across all tenants, and a per-mailbox limit under "Outlook service limits": 10,000 API requests in a 10-minute period, four concurrent requests, and 150 MB of upload (`PATCH`/`POST`/`PUT`) in a 5-minute period, all "per app ID and mailbox combination." The same page's Outlook service resources table lists a "To-do tasks API (preview)" row scoped to `outlookTask` — that's the legacy, dead API, not the current `todoTask`/`todoTaskList` resource. I found no separate, `todoTask`-specific throttling row; the current To Do API appears to fall under Graph's general throttling behavior rather than a dedicated Outlook-mailbox bucket, but I could not find a page that says this explicitly for `todoTask`, so treat that as inferred, not confirmed.

On handling 429s, the [throttling guidance page](https://learn.microsoft.com/en-us/graph/throttling-limits) documents a token-bucket model and these response headers: `x-ms-resource-unit` (cost of the request), `x-ms-throttle-limit-percentage` (0.8–1.8, where 1.0 means you've hit your limit and above that increasing percentages of requests get throttled), `x-ms-throttle-scope`, and `x-ms-throttle-information`. Standard advice applies: back off using the `Retry-After` header when present, and reduce request volume/frequency if you keep hitting 429s.

**Observed 2026-09-24 (spike [S9](spikes/S9.md)):** the 4-concurrent limit applies to To Do (8 parallel requests got 429s). The only throttle header To Do returned was `Retry-After`; none of the `x-ms-*` headers above appeared.

## Change notifications (webhooks)

`todoTask` is a supported subscription resource at `v1.0` — confirmed on the [subscription resource page](https://learn.microsoft.com/en-us/graph/api/resources/subscription?view=graph-rest-1.0), which lists it in both the maximum-subscription-lifetime table (4,230 minutes, under three days, with the note "Webhooks for this resource are only available in the global endpoint and not in the national clouds") and the latency table (less than 2 minutes average, 15 minutes maximum). There's no beta-only asterisk on this row — it's v1.0.

One wrinkle worth flagging: the Outlook-specific change-notifications overview page (scoped to `contact`/`event`/`message`) doesn't mention `todoTask` at all, which could look like a contradiction if you only read that page. The subscription resource page above is the authoritative, inclusive list — `todoTask` support for change notifications is real and documented there, just not repeated on the Outlook-scoped overview.

## Delta queries

Both `todoTaskList` and `todoTask` support delta query, confirmed on the [To Do overview page](https://learn.microsoft.com/en-us/graph/api/resources/todo-overview?view=graph-rest-1.0): "The following To Do API resources support delta query: todoTask collection in a task list, todoTaskList." Standard delta mechanics apply (`@odata.deltaLink`, `@odata.nextLink`, `$deltatoken`) per the general [delta query overview](https://learn.microsoft.com/en-us/graph/delta-query-overview) — useful for a CLI that wants to keep a local cache in sync without re-listing everything each run.

**Phase 0 note (2026-09-24, [S3](spikes/S3.md), [S4](spikes/S4.md), [P1](spikes/P1.md)):** To Do delta takes no `$select`, `$filter` or `$top`; the page size comes only from `Prefer: odata.maxpagesize`, resent on every page. An expired cursor returns 410 `SyncStateNotFound`, and a tampered one returns 400 "Badly formed token.".

## SDKs and CLI tooling

- **`@microsoft/microsoft-graph-client`**: the official JS/TS SDK. `npm install @microsoft/microsoft-graph-client`, plus a fetch polyfill for older Node (Node 18+ generally doesn't need one). You provide an `authProvider` — any object with a `getAccessToken()` method — and call `client.api('/me/todo/lists/{id}/tasks').get()`. Confirmed against the [msgraph-sdk-javascript README](https://github.com/microsoftgraph/msgraph-sdk-javascript).
- **`@azure/msal-node`**: the official auth library for Node public/confidential clients, including device code and PKCE flows via `PublicClientApplication`. `acquireTokenByDeviceCode(request)` takes a `deviceCodeCallback` (to print the verification URL/code to the user) and `scopes`, and returns an `AuthenticationResult` with an access token (confirmed against the MSAL.js `msal-node` docs/API surface).
- **`@microsoft/microsoft-graph-types`**: official Microsoft-published TypeScript type definitions (the `microsoftgraph/msgraph-typescript-typings` repo) for Graph resources including `TodoTask`, `TodoTaskList`, `ChecklistItem` — useful with strict TS since the JS client itself is loosely typed.
- **Microsoft Graph CLI (`mgc`)**: exists, but its GitHub repo (`microsoftgraph/msgraph-cli`) was archived by its owner on 2025-08-29 and is now read-only. Its README shows no `todo`/task-related commands at all — the documented commands are limited to `mgc login` (with `DeviceCode`, `InteractiveBrowser`, `ClientCertificate` strategies) and `mgc me get`. There's no statement of full API coverage, and no evidence it ever had dedicated To Do commands. Given the archival, don't build a workflow around it.
- **Microsoft Graph PowerShell SDK**: has task-specific cmdlets (`Get-MgUserTodoListTask`, `New-MgUserTodoListTask`, etc., under the `Microsoft.Graph.Users` module) shown in the auto-generated PowerShell code samples on the API reference pages themselves. Not JS/TS, but a working, maintained option if PowerShell were acceptable.
- **Microsoft Graph Toolkit**: has web components, including one for tasks, aimed at building UI rather than CLI automation — not a fit for a CLI/skill.

## Alternative, unofficial routes

All of the following are unofficial or reverse-engineered relative to the Graph API and should be treated as such:

- **Power Automate / Logic Apps connectors**: "Microsoft To-Do (Business)" and "Microsoft To-Do (Consumer)" are Microsoft-published connectors that wrap the current API for business and personal accounts respectively, referenced from the deprecated Outlook Tasks connector page. These are official Microsoft connectors, but they're still a layer on top of the same Graph API, with their own throttling (100 calls/60s per connection, 1 poll/15s, per the deprecated connector's documented limits — current connector limits weren't separately checked).
- **Zapier / IFTTT / n8n**: third-party automation platforms have Microsoft To Do integrations. These are unofficial from Microsoft's perspective (built by those vendors against the Graph API or its connectors) and weren't independently verified here — treat as "probably works, not vouched for by Microsoft."
- **Community tooling**: this line originally said none existed. That was wrong. Several community CLIs and an MCP server exist; the two strongest were reviewed in [prior-art.md](prior-art.md) and rejected as dependencies, with reasons.

## Gotchas

- The legacy Outlook Tasks API (`/me/outlook/tasks`, `outlookTask`/`outlookTaskFolder`/`outlookTaskGroup`) is dead — stopped returning data August 20, 2022. If you find old sample code or Stack Overflow answers using this, they don't work anymore.
- Beta vs v1.0: almost everything needed for a personal task manager (lists, tasks, checklist items, linked resources, file attachments, delta, webhooks) is already in v1.0. Don't reach for beta unless a specific field you need is documented as beta-only.
- There is no application-permission (daemon) write path, at all, for To Do tasks — every create/update/delete is delegated and needs an interactive sign-in at least once, with silent renewal after that via the refresh token.
- File attachments are supported by the API (`taskFileAttachment`) — don't assume otherwise; this is one of the areas where the UI-vs-API gap people talk about online doesn't actually exist anymore.
- My Day and list sharing/task assignment have no corresponding API property or relationship as far as the documented `todoTask`/`todoTaskList` schema shows — this is inferred from what's absent in the schema, not from an explicit Microsoft "not supported" statement.
- Throttling: general Graph limits (130,000 req/10s per app globally) and Outlook-mailbox-scoped limits (10,000 req/10min, 4 concurrent, 150 MB/5min per app+mailbox) are documented, but no dedicated `todoTask`-specific number was found — the only "To-do tasks API" row in the throttling doc refers to the dead `outlookTask` API, not the current one. *Phase 0 note (2026-09-24, [S9](spikes/S9.md)):* To Do follows the 4-concurrent limit and sends only `Retry-After`.
- `mgc`, the Microsoft Graph CLI, is archived (read-only as of 2025-08-29) and never had To Do commands as far as its README shows. Don't plan around it.

## A minimal TypeScript example (untested)

This sketches device-code auth via `@azure/msal-node` and a task listing via `@microsoft/microsoft-graph-client`, `/common` authority so it works with either a personal Microsoft account or a work/school account. It has not been run — no captured output is claimed. Treat it as a shape to adapt, not a verified working script.

```typescript
import "isomorphic-fetch"; // Node 18+ often doesn't need this; harmless if unused.
import { PublicClientApplication, type Configuration } from "@azure/msal-node";
import { Client } from "@microsoft/microsoft-graph-client";
import type { TodoTask } from "@microsoft/microsoft-graph-types";

// Register an app in the Entra admin center (entra.microsoft.com) with
// "Any Entra ID Tenant + Personal Microsoft accounts" so this works for both
// account types. Public client, no secret needed for device code / PKCE.
const CLIENT_ID = process.env.MS_TODO_CLIENT_ID!;

const msalConfig: Configuration = {
  auth: {
    clientId: CLIENT_ID,
    authority: "https://login.microsoftonline.com/common",
  },
};

const pca = new PublicClientApplication(msalConfig);

async function getAccessToken(): Promise<string> {
  const result = await pca.acquireTokenByDeviceCode({
    scopes: ["Tasks.Read"],
    deviceCodeCallback: (response) => {
      // Print the verification URL and code for the user to complete sign-in
      // on another device/browser.
      console.log(response.message);
    },
  });

  if (!result?.accessToken) {
    throw new Error("Device code flow did not return an access token");
  }

  return result.accessToken;
}

async function listTasks(): Promise<void> {
  const accessToken = await getAccessToken();

  const client = Client.init({
    authProvider: (done) => done(null, accessToken),
  });

  const lists = await client.api("/me/todo/lists").get();

  for (const list of lists.value) {
    const tasks = await client
      .api(`/me/todo/lists/${list.id}/tasks`)
      .get();

    for (const task of tasks.value as TodoTask[]) {
      console.log(`[${list.displayName}] ${task.title} (${task.status})`);
    }
  }
}

listTasks().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
```

For repeated CLI runs without re-prompting every time, cache the MSAL token cache (serialize/deserialize via MSAL's cache plugin APIs) so `acquireTokenSilent` can use the refresh token instead of hitting device code again. That persistence layer isn't sketched above — it's a real piece of work, not a one-liner, and worth designing deliberately rather than bolting on.

## Verdict for a local CLI or agent skill

Buildable, and smaller than it might look. A CLI or skill that reads and writes BK's personal To Do tasks needs: one app registration (personal + org account types enabled), device code or PKCE auth via `msal-node`, a persisted token cache so re-auth isn't needed every run, and `microsoft-graph-client` (or even plain `fetch` against `https://graph.microsoft.com/v1.0/...` — the surface is small enough that pulling in the full SDK is optional) for the actual list/task CRUD. Delta query is there if a local cache/sync loop is wanted later; webhooks are there if a push-based skill is wanted later, though a personal local CLI probably doesn't need to stand up a public HTTPS endpoint just to poll its own tasks.

What it can't do: nothing daemon-style (no unattended, no-token-cache background job — some user-consented token has to exist and get refreshed), no My Day manipulation, no list sharing or task assignment (no API surface for either), and nothing beyond what `todoTask`/`todoTaskList`/`checklistItem`/`linkedResource`/`taskFileAttachment` expose — which is most of what the To Do apps actually do day to day, just not the collaboration-flavored features.

## Sources

- [Use the Microsoft To Do API - v1.0 overview](https://learn.microsoft.com/en-us/graph/api/resources/todo-overview?view=graph-rest-1.0)
- [To Do API overview (concept page)](https://learn.microsoft.com/en-us/graph/todo-concept-overview)
- [todoTask resource - v1.0](https://learn.microsoft.com/en-us/graph/api/resources/todotask?view=graph-rest-1.0)
- [List Todo tasks - v1.0](https://learn.microsoft.com/en-us/graph/api/todotasklist-list-tasks?view=graph-rest-1.0&tabs=http)
- [Create todoTask - v1.0](https://learn.microsoft.com/en-us/graph/api/todotasklist-post-tasks?view=graph-rest-1.0&tabs=http)
- [Create taskFileAttachment - v1.0](https://learn.microsoft.com/en-us/graph/api/todotask-post-attachments?view=graph-rest-1.0)
- [Attach files to a To Do task](https://learn.microsoft.com/en-us/graph/todo-attachments)
- [outlookTaskFolder resource - beta (deprecation notice)](https://learn.microsoft.com/en-us/graph/api/resources/outlooktaskfolder?view=graph-rest-beta)
- [Outlook Tasks connector - deprecated](https://learn.microsoft.com/en-us/connectors/outlooktasks/)
- [Microsoft Graph permissions reference](https://learn.microsoft.com/en-us/graph/permissions-reference)
- [How to register an app in Microsoft Entra ID](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app)
- [Azure free account signup / pricing](https://azure.microsoft.com/en-us/pricing/purchase-options/azure-account)
- [Microsoft Q&A: cost of app registrations in Azure](https://learn.microsoft.com/en-us/answers/questions/436510/cost-of-app-registrations-in-azure)
- [Graph throttling limits](https://learn.microsoft.com/en-us/graph/throttling-limits)
- [subscription resource - v1.0 (change notifications, todoTask row)](https://learn.microsoft.com/en-us/graph/api/resources/subscription?view=graph-rest-1.0)
- [Change notifications API overview](https://learn.microsoft.com/en-us/graph/api/resources/change-notifications-api-overview?view=graph-rest-1.0)
- [Delta query overview](https://learn.microsoft.com/en-us/graph/delta-query-overview)
- [microsoft-graph-client (msgraph-sdk-javascript) README](https://github.com/microsoftgraph/msgraph-sdk-javascript)
- [msal-node device code flow docs (MSAL.js repo)](https://github.com/AzureAD/microsoft-authentication-library-for-js)
- [Microsoft Graph CLI (msgraph-cli) repo, archived](https://github.com/microsoftgraph/msgraph-cli)
