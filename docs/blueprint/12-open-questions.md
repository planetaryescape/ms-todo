# 12: Open questions and spikes

## Spikes (phase 0; run against BK's real account on a throwaway list)

For each, record the request, the response (with private data removed), the date and the conclusion, then update the blueprint document it affects.

| # | Question | Why it matters | Affects |
|---|---|---|---|
| S1 | Does `tasks/delta` report changes to checklist items, linked resources and attachments, by bumping the parent's `lastModifiedDateTime` or appearing in the results? Does delta accept `$expand=checklistItems`? | Decides between sync strategy (a) and (b) | [04](04-sync-cache.md) |
| S2 | Does `lists/delta` or `tasks/delta` report **open-extension** changes? Does `$expand=extensions` work on delta, list or get? | My Day date, folders and assignment live in extensions | [04](04-sync-cache.md), [05](05-custom-features.md) |
| S3 | Is `$select` on `/me/todo/lists/{id}/tasks` rejected for personal accounts (`RequestBroker--ParseUri`), as MAG&Cie reports? On delta too? | Payload size; whether delta can use `$select` | [03](03-graph-provider.md) |
| S4 | How long does a To Do `deltaLink` last, and what exactly comes back when it's expired (status and error code)? | Triggering reconciliation | [04](04-sync-cache.md) |
| S5 | Can **personal** Microsoft accounts use the To Do API with `/common` plus device code and the scopes listed? Does `MailboxSettings.ReadWrite` consent work for an MSA? | Everything depends on it | [03](03-graph-provider.md) |
| S6 | Does PATCH or DELETE on tasks honour `If-Match` with `@odata.etag` (412 on mismatch)? | The conflict policy | [04](04-sync-cache.md) |
| S7 | Do categories added through Graph show on the phone app's tasks, and does the phone let you filter by them? Which preset colour looks right for "My Day"? | The premise of D-015 | [05](05-custom-features.md) |
| S8 | Evaluate the clockwords and interim crates on a corpus of about 100 real phrases (BK's own phrasing, plus Todoist's documented examples). | Date parser choice | [06](06-natural-language.md) |
| S9 | Actual throttling behaviour for To Do: the concurrency limit, and whether 429s appear at modest rates (a burst test with 20 parallel requests). | The concurrency cap | [03](03-graph-provider.md) |
| S10 | `$batch` against To Do endpoints: does it work for GET and for writes? Any limits specific to To Do? | Fan-out strategy | [03](03-graph-provider.md) |
| S11 | Does setting `dueDateTime` with a time keep the time, or does To Do cut it back to the date? How does the phone show a due date with a time compared with a reminder? | The mapping for "date with a time" | [06](06-natural-language.md) |
| S12 | Does creating a task with `recurrence` and completing it make Graph create the next occurrence (as the app does), and what does delta return for it? | How recurrence and sync interact | [04](04-sync-cache.md) |
| S13 | Can a task create carry a unique marker (our extension with the outbox `opId`), and can a lookup find it? (Added after review, 2026-09-24.) | Attributing a create whose outcome is unknown | [04](04-sync-cache.md#unknown-outcome-d-028) |
| S14 | What does a task copied to another list keep, and which requests copy it? (Rung 5e, 2026-09-25.) | Moving a task without losing anything | [05](05-custom-features.md#move-between-lists), D-051 |
| S15 | How do step and link writes behave: which fields a link needs, whether its PATCH and DELETE honour `If-Match`, whether a field can be cleared, and whether steps can be reordered? (Rung 8a, 2026-09-25.) | Steps and links through the outbox | [07](07-cli.md), D-055 |
| S16 | How does an upload session resume, and what does an attachment look like? (Rung 8b, 2026-09-25.) | Uploads that survive a lost answer; safe downloads | [03](03-graph-provider.md), D-056 |
| P1 | What is the default page size for task lists and delta, and does `Prefer: odata.maxpagesize` work? (Added during phase 0.) | Pagination, and rung 1's "more than 100 tasks" check | [03](03-graph-provider.md) |

### S5 result (2026-09-24)

**Confirmed on BK's personal Microsoft account.** A `POST` to `https://login.microsoftonline.com/common/oauth2/v2.0/devicecode` with client ID `48d9179b-67f3-4969-985e-9690aff42435` and scopes `offline_access Tasks.ReadWrite MailboxSettings.ReadWrite User.Read` returned a device code. After interactive sign-in, a `POST` to the matching `/token` endpoint with the device-code grant returned a bearer access token and refresh token. The returned scope included `Tasks.ReadWrite MailboxSettings.ReadWrite User.Read`.

With that access token, `GET https://graph.microsoft.com/v1.0/me/todo/lists` returned HTTP 200 and a `value` array of 29 lists. `GET https://graph.microsoft.com/v1.0/me/outlook/masterCategories` returned HTTP 200 and a `value` array of 7 categories. List and category contents and all tokens are omitted here. This confirms `/common` device-code sign-in, To Do reads and MailboxSettings consent/access for this personal account. It does not test category writes or phone-app behavior (S7).

Evidence for S1–S4, S6–S13 and P1 is in `docs/research/spikes/`. All Graph spikes ran on 2026-09-24 against BK's personal account, on a throwaway list (`ms-todo-spike-2026-09-24`) and two throwaway categories. BK's other lists were read only to count aggregate shapes (S11, S12); none were changed, and no content is recorded. Tokens and deltaLinks are redacted.

### S1 result (2026-09-24)

**Yes. Every change to a checklist item, linked resource or attachment bumps the parent task's `lastModifiedDateTime` and `@odata.etag`, and the task appears in the next `tasks/delta` round.** Checklist items and linked resources come back inline in every task representation, delta included, with no `$expand` (a task with 42 steps returned all 42). Attachments are never inline: only `hasAttachments` flips, and the list needs `GET …/attachments`. Delta accepts `$expand=checklistItems` but it changes nothing. Also found: an empty child collection is absent from the JSON rather than `[]`, and a checklist-item PATCH without `isChecked` resets it to false. Confidence: high. Evidence: [S1](../research/spikes/S1.md). Changed: [04](04-sync-cache.md#children-of-a-task), [02](02-data-model.md#outbox-semantics), D-029.

### S2 result (2026-09-24)

**Delta detects extension changes but never returns their content.** Creating, patching or deleting an open extension bumps the parent's etag, so the list or task appears in delta. `$expand=extensions` is accepted everywhere and silently returns nothing. `$expand=extensions($filter=id eq 'com.planetaryescape.mstodo')` works on GET and on collection GET for lists and tasks, and is a 400 on delta. Extension PATCH replaces the whole document, and POST of an existing name acts as an upsert. **Open:** whether an extension PATCH honours the parent's `If-Match` is untested, so the GET-merge-PATCH in [05](05-custom-features.md) keeps a small race window, tracked in [issue 001](../issues/001-extension-write-race.md). Confidence: high. Evidence: [S2](../research/spikes/S2.md). Changed: [04](04-sync-cache.md#children-of-a-task), [05](05-custom-features.md), D-029.

### S3 result (2026-09-24)

**Confirmed: `$select` is a 400 (`RequestBroker--ParseUri`) on every To Do endpoint we tried, on v1.0 and beta.** The few values that don't fail (`id`, `*`) are ignored. Delta also rejects `$filter` and `$top`. Confidence: high. Evidence: [S3](../research/spikes/S3.md). Changed: [03](03-graph-provider.md#query-options), [04](04-sync-cache.md#delta-sync).

### S4 result (2026-09-24)

**Error shapes are answered; lifetime is open.**

- A malformed or tampered token returns 400 "Badly formed token."; a valid token that Graph no longer accepts, or one from another scope, returns 410 `SyncStateNotFound`. Both mean "reset this cursor".
- A delta replay also returned a one-off 404 and 500 after a mass delete, then 200 a minute later. Those mean "retry", not "reset".
- A deleted list's tasks delta returns 200 and an empty page. Only `lists/delta` (`@removed`) or a 404 on `GET /lists/{id}` shows the deletion. A POST into a deleted list still returns 201 for a while, so the write goes to a ghost.
- **Open: lifetime.** One lists deltaLink died after about 6 minutes for no reason we could find; five others lasted 12–20 minutes, until the observation stopped. Saved deltaLinks (kept outside the repo) are due to be replayed in a few days to measure real lifetime.
- **Open: how far reads lag behind a write.** Not measured. It no longer risks a duplicate: the [unknown-outcome lookup](04-sync-cache.md#unknown-outcome-d-028) keeps looking for up to 24 hours and never re-sends by itself. A long lag only delays adoption.

Confidence: high on error shapes, low on lifetime. Evidence: [S4](../research/spikes/S4.md). Changed: [04](04-sync-cache.md#reconciliation-after-a-lost-delta-token).

### S6 result (2026-09-24)

**It depends on the call.** `If-Match` is honoured on task PATCH, and on checklist-item PATCH and DELETE (with the parent task's etag): a stale etag gives 412. **Task DELETE and list PATCH ignore it** and apply the change. A malformed `If-Match` (or `*`) gives 500, not 400. DELETE of a task that's already gone gives 404. Any child change moves the parent's etag. Confidence: high. Evidence: [S6](../research/spikes/S6.md). Changed: [03](03-graph-provider.md), [04](04-sync-cache.md#conflicts).

### S7 result (2026-09-24)

**Answered: the iOS app shows no categories at all.** A category created through Graph can be set on a task, round-trips through delta, and can be filtered on server side. A task can carry a category name with no master category; none is created. Master category names can't be renamed (PATCH returns 200 and changes nothing) and are unique ignoring case (409 `CategoryNameExists`). **`preset4` is Green and `preset3` is Yellow** in Microsoft's mapping, so 05's "preset4, yellowish" was wrong.

**Phone check (BK, iPhone, 2026-09-24):** the iOS To Do app shows **no categories anywhere**, neither on the list row nor in the task detail view, so there's no colour to compare and nothing to filter by. The Graph side still works as above. So a category can't carry My Day to the phone, and D-015's premise is false: My Day moves to our extension, mirrored on the phone through the due date (D-037). Categories stay for `@labels`, which Graph and Outlook show and iOS doesn't.

Confidence: high. Evidence: [S7](../research/spikes/S7.md). Changed: [05](05-custom-features.md#my-day), D-030, D-037.

### S8 result (2026-09-24)

**No crate fits.** On 127 graded phrases, `whichtime-sys` passed 53%, `interim` 24–39%, `chrono-english` 35% and `clockwords` 33%. We'll write a rule-table scanner instead; the reasons are in [06](06-natural-language.md#date-and-time-parsing). Several expected values depend on BK's answers to Q6–Q10 below. Confidence: high on the conclusion. Evidence: [S8](../research/spikes/S8.md), corpus [S8-corpus.tsv](../research/spikes/S8-corpus.tsv). Changed: [06](06-natural-language.md#date-and-time-parsing), [10](10-roadmap.md), D-026.

### S9 result (2026-09-24)

**The cap of 4 holds for To Do.** 4 parallel GETs all returned 200; 8 parallel returned two 429s (`Retry-After` 7 and 8). Sub-requests of a parallel `$batch` count towards the same limit. `Retry-After` (integer seconds) is the only throttle header: no `x-ms-throttle-*` or `x-ms-resource-unit` headers ever appeared. Sequential traffic never throttled. Confidence: high for "4 is safe, 8 isn't"; 5–7 weren't tested. Evidence: [S9](../research/spikes/S9.md). Changed: [03](03-graph-provider.md#http-client).

### S10 result (2026-09-24)

**`$batch` works for To Do reads and writes, with rules.** At most 20 requests (21 is a 400). A batch is either fully sequential or fully parallel; mixing them is a 400. After a failed step in a sequential batch, every later step gets 424. A sub-request can't use the ID created by an earlier one. Sequential batches of 20 never throttled; parallel ones did. Confidence: high. Evidence: [S10](../research/spikes/S10.md). Changed: [03](03-graph-provider.md#http-client).

### S11 result (2026-09-24)

**Answered.** `dueDateTime` and `startDateTime` keep only the date: Graph stores midnight in the zone you sent and returns it in UTC. Setting `startDateTime` alone also sets `dueDateTime`. `reminderDateTime` keeps its time. BK's existing due dates are midnight in several zones (23:00Z, 00:00Z and 20:00Z), so reading must round to the nearest local midnight.

**Phone check (BK, iPhone, 2026-09-24): confirmed.** The phone shows due dates as dates only, in every zone: a due date written as 22:00 New York shows as the 26th. A reminder shows as a bell. A due date plus a reminder shows as "Sat 26 Sep •" with a bell icon.

**Residual risk:** rounding to the nearest midnight is exact only when the writer's zone is within 12 hours of the reader's; a date written in Pacific/Kiritimati (UTC+14) reads as the previous day in London. The raw value stays in `raw_json` ([02](02-data-model.md#principles)).

Confidence: high. Evidence: [S11](../research/spikes/S11.md). Changed: [02](02-data-model.md#principles), [06](06-natural-language.md#date-and-time-parsing), D-027.

### S12 result (2026-09-24)

**Completing a recurring task keeps the same ID live and creates a new completed copy.** The PATCH returns 200 with `status: notStarted` and the due date rolled to the next occurrence. A new task with a new ID holds the completed occurrence. Delta returns both. Leaving out `recurrenceTimeZone` moved the due date a day later. After a roll, dates are re-based to UTC midnight. Confidence: high. Evidence: [S12](../research/spikes/S12.md). Changed: [04](04-sync-cache.md#instant-local-writes), [06](06-natural-language.md#recurrence-custom-parser-and-why).

**Explained (2026-09-24):** the iOS app put two of the recurring S12 tasks into its own My Day without BK doing it, while Graph showed no My Day field (v1.0 or beta) and only `lastModifiedDateTime` moved. The cause is the app's setting **"Show 'Due Today' tasks in My Day"**, which is on in BK's app (checked read-only in the Mac app): the app puts any task due today into its own My Day. Those tasks were due today. There's no hidden field. D-037 uses this to mirror ms-todo's My Day on the phone.

### S13 result (2026-09-24)

**Yes for the create, no for server-side filtering.** A task POST can carry our open extension inline (`"extensions": [{…"extensionName":"com.planetaryescape.mstodo","opId":"…"}]`): it returns 201 with the extension in the response, and a GET with `$expand=extensions($filter=id eq 'com.planetaryescape.mstodo')` returns the `opId`. Filtering on extensions server-side (`$filter=extensions/any(…)`) is a 400, so the match on `opId` happens client-side over a filtered collection GET. Checklist items and linked resources have no extensions, so this doesn't cover them. Confidence: high. Evidence: [S13](../research/spikes/S13.md). Changed: [04](04-sync-cache.md#unknown-outcome-d-028), [02](02-data-model.md#outbox-semantics), D-028.

### S14 result (2026-09-25)

**Everything but `createdDateTime`, and one POST makes all of it but the attachments.** A task POST takes its checklist items (with `isChecked` and `checkedDateTime`), its linked resource and our extension inline; attachments copy byte for byte through a POST or an upload session. A task holds at most one linked resource. An upload session's bytes go to `<uploadUrl>/content`, with the token (the bare URL is 404). A recurring task's dates must be written in the zone its recurrence reports, which a GET gives as UTC, or Graph moves the copy's dates a day on. `createdDateTime` is stamped anew and ignored on create. Confidence: high. Evidence: [S14](../research/spikes/S14.md). Changed: [05](05-custom-features.md#move-between-lists), D-051.

### S15 result (2026-09-25)

**Links behave like steps for `If-Match`, but a field can't be cleared; steps can't be reordered.** A link needs `applicationName` (400 without); its PATCH and DELETE honour the parent task's etag as a step's do (412 when stale); its PATCH is partial, and a null or empty value is ignored, so a field can be set and never cleared; any URL scheme is kept, but not a string that isn't a URL. A step can be created checked, with its `checkedDateTime` kept. Steps come back in the order they were added, and nothing reorders them: `createdDateTime` can't be updated and `$orderby` is ignored. Confidence: high. Evidence: [S15](../research/spikes/S15.md). Changed: [07](07-cli.md), [08](08-tui.md), D-055.

### S16 result (2026-09-25)

**A session can't be asked where it stands; each PUT's answer says.** A GET of the upload URL, bare or with `/content`, is 404. Each PUT but the last answers `nextExpectedRanges: ["N"]` (no dash); a range the session already has is 400 `InvalidStart` and the session goes on, so a chunk whose answer was lost can be sent again; skipping ahead is refused (once as a 504 after 29 s, committing nothing). The last PUT's `Location` is the attachment's URL, ending in its ID. A listing never includes `contentBytes` (a POST's answer echoes it); `size` is the bytes plus about 240; an attachment DELETE ignores `If-Match`; Graph keeps any name, `../` included. Confidence: high. Evidence: [S16](../research/spikes/S16.md). Changed: [03](03-graph-provider.md), D-056.

### P1 result (2026-09-24)

**The default page is 50 tasks, on list and delta.** `Prefer: odata.maxpagesize` works on both, but it isn't carried in the `nextLink`, so it has to be sent on every page. Delta can end with an empty page that carries the `deltaLink`, and `lists/delta` returned a short page mid-stream, so a short page doesn't mean the last one. Rung 1's "more than 100 tasks" check still tests pagination. Confidence: high. Evidence: [P1](../research/spikes/P1.md). Changed: [03](03-graph-provider.md#http-client), [10](10-roadmap.md).

## Product questions for BK

Answered on 2026-09-24:

- **Q1. Answered (D-022).** Quick add with no list goes to the built-in "Tasks" list (`defaultList`). `#List` or `--list` picks a specific one.
- **Q2. Answered (D-023).** No locations. BK has never needed them. The parser doesn't recognise them, and there's no location field.
- **Q3. Open.** Keep the original Todoist p1–p4 level in the extension, to avoid losing p2 versus p3 (D-017)? The default until BK decides is no.
- **Q4. Answered (D-024).** The My Day rollover runs at midnight by default. It's configurable with `my_day.rollover_time` in config.toml.
- **Q5. Answered (D-025).** Release builds include BK's client ID, so ms-todo works as soon as it's installed. The docs and `auth login` encourage users to register their own app.

Opened on 2026-09-24 by phase 0 (S4, S7, S8) and its review. The placeholders in parentheses are what ms-todo uses until BK answers.

- **Q6. Open.** A bare weekday that is today: does `thursday` typed on a Thursday mean today or next week? (Placeholder: next week, as Todoist does.)
- **Q7. Open.** What hours do `tonight`, `eod`, `morning` and `evening` mean? (Placeholders: `tonight` is today with no time, `eod` 17:00, `morning` 09:00, `evening` 19:00.)
- **Q8. Open.** Is D/M the default date order, so `12/10` is 12 October? (Placeholder: yes, UK.)
- **Q9. Open.** Does lowercase `tom` mean tomorrow? It clashes with the name Tom. (Placeholder: yes, lowercase only, so `Ask Tom` stays in the title. Quick add applies it to `tod` and `sat` too, so `Sat nav` stays whole; D-052.)
- **Q10. Open.** BK supplies ten of his own phrases for the corpus, [S8-corpus.tsv](../research/spikes/S8-corpus.tsv).
- **Q11. Moot (D-037).** My Day colour: `preset3` (Yellow) or `preset4` (Green)? The iOS app shows no categories (S7), and My Day no longer uses one.
- **Q13. Built with the placeholder (D-054), still BK's to confirm.** A task added to My Day with no due date gets today as its due date (D-037). If it's taken out of My Day by hand before the rollover, should ms-todo clear that due date straight away? Rung 7 does: yes, by the same rule as the rollover (the task is open, `myDayDueSet`, and the due date is still My Day's day).
- **Q12. Open.** A task created into a list that turns out to have been deleted on another device is kept as a `failed` outbox entry ([04](04-sync-cache.md#instant-local-writes)). Should it move to "Tasks" automatically instead? (Placeholder: no, keep it as failed.)
