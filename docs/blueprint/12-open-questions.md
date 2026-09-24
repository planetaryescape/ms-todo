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

## Product questions for BK (not blocking the start)

- **Q1.** Default quick-add list when there's no `#List`: the built-in "Tasks" (`defaultList`), or the list currently selected in the TUI?
- **Q2.** Locations. To Do has no location field or location reminders. Store free text in the extension and show it, or drop locations from the parser?
- **Q3.** Keep the original Todoist p1–p4 level in the extension, to avoid losing p2 versus p3 (D-017)?
- **Q4.** When should the My Day rollover run: at midnight, or at a configurable "start of day" such as 04:00, for late-night work?
- **Q5.** Should the bundled release build include BK's client ID (mxr style), so other people can use ms-todo without registering an app? It's public, but it would be BK's app registration that other users consent to.
