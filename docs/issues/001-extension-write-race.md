# 001: Extension writes can lose a concurrent edit

**Status:** open. Accepted for now.

## Problem

ms-todo writes an open extension as GET, merge, then a PATCH of the whole document, because Graph replaces the extension on PATCH ([S2](../research/spikes/S2.md)). If two devices edit different fields of the same extension inside that GET–PATCH window, one edit is lost without any error.

## Why it's accepted today

There's one user, and the window is short, so it should be rare. It's documented in [04](../blueprint/04-sync-cache.md#children-of-a-task) and in S2's open item in [12](../blueprint/12-open-questions.md#s2-result-2026-09-24).

## Options for later

- Split the data into one extension per field group, so unrelated edits don't share a document.
- Check the etag before the PATCH, if S2's open question (does an extension PATCH honour the parent's `If-Match`?) turns out to be yes.
- Send a warning event when a write may have overwritten a concurrent change.
