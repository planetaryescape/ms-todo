# 00: Overview

## What ms-todo is

ms-todo is a terminal client for Microsoft To Do. It has three surfaces:

- **`ms-todo` CLI:** every capability, scriptable, stable JSON output. This is the surface agents drive.
- **`ms-todo tui`:** a keyboard-native ratatui interface that feels instant because it never waits on the network.
- **`ms-todo daemon`:** a background process that owns sign-in, the local cache, sync and outgoing writes. The CLI and TUI both talk to it.

Microsoft To Do stays the system of record. The phone and desktop apps keep working; ms-todo is another client of the same data.

## Principles

1. **Local-first reads.** Every read the UI does is a local SQLite query. The network is only for sync.
2. **Instant writes.** A change shows up in the cache and UI immediately, then goes into an outbox queue for the daemon to send. A write Graph rejects is rolled back, and the user is told why.
3. **Never lose or truncate silently.** Always follow pagination. Report every Graph error with its code and message. Never delete data as a side effect. Both tools we reviewed failed at this (see [../research/prior-art.md](../research/prior-art.md)).
4. **Cover the whole API.** If Graph's To Do API can do it, ms-todo can do it. Nothing is left out for being niche.
5. **Build what the API lacks on top of it, and keep it visible elsewhere where we can.** Custom features store their data in Microsoft's own structures (categories, open extensions), so it syncs across devices. Where possible it's also visible in the official apps.
6. **Deterministic before clever.** Natural-language parsing is rule-based and testable. An LLM can be added later behind the same interface.
7. **Right-sized.** mxr is about 254k lines and spotuify about 180k. To Do's domain is far smaller, and so is ms-todo. We borrow their patterns, not their size.

## Scope: the full API surface

Everything below is supported by Graph v1.0 (see the research doc for sources):

- **Task lists (`todoTaskList`):** create, read, update, delete. `wellknownListName` marks the built-in lists (`defaultList` = "Tasks", `flaggedEmails`), which can't be renamed or deleted. Also `isOwner` and `isShared`, plus open extensions and delta.
- **Tasks (`todoTask`):** create, read, update, delete, with every property:
  - `title`, `body` (text or HTML), `status` (`notStarted`, `inProgress`, `completed`, `waitingOnOthers`, `deferred`)
  - `importance` (`low`, `normal`, `high`)
  - `isReminderOn`, `reminderDateTime`, `dueDateTime`, `startDateTime`, `completedDateTime`
  - `recurrence` (`patternedRecurrence`), `categories`, `hasAttachments`
  - timestamps
  - Also open extensions and delta.
- **Checklist items ("steps"):** create, read, update, delete.
- **Linked resources:** create, read, update, delete.
- **File attachments:** a single POST for files under 3 MB; an upload session for files up to 25 MB, in chunks under 4 MB each. Also list, get (download) and delete.
- **Open extensions** on lists and tasks: arbitrary data. The official apps can't see it.
- **Categories:** the user's Outlook master categories (`/me/outlook/masterCategories`), with create, read, update, delete and colours `preset0` to `preset24`. Needs `MailboxSettings.ReadWrite`.
- **Query options:** `$filter`, `$orderby`, `$top`, and `$batch` (up to 20 requests per batch).
- **Delta** for lists and for tasks in each list.

Things the official app shows but ms-todo doesn't need: none known. Everything the app does that the API supports is in the list above.

## What the API can't do, and what we build instead

| App feature | In the API? | ms-todo's approach | Doc |
|---|---|---|---|
| My Day | No | Our own My Day, using an Outlook category named "My Day" plus a date in an extension, cleared daily by the daemon | [05](05-custom-features.md) |
| List groups (folders) | No | Folder stored in an open extension on each list | [05](05-custom-features.md) |
| Assigning a task to someone | No (legacy only) | Assignee stored in an open extension on the task, local meaning only | [05](05-custom-features.md) |
| Sharing lists | Only flags (`isShared`) | **Not built.** BK doesn't share lists | [11](11-decision-log.md) D-014 |
| Moving a task between lists | No move operation | Copy the task completely with all its children, check the copy, then delete the original. The ID changes | [05](05-custom-features.md) |
| Location-based reminders | No | Not built. BK doesn't use locations (D-023) | [11](11-decision-log.md) |
| Suggestions, smart lists | No (they're queries) | Local SQLite views: Important, Planned, All, Completed | [08](08-tui.md) |

## Non-goals

- An MCP server (D-008).
- Sharing or collaboration (D-014).
- Webhooks or any public endpoint (D-007).
- Planner tasks. That's a different Graph resource with its own permissions.
- LLM parsing in v1 (D-016). It's deferred, not rejected.
- A web or GUI front end.
