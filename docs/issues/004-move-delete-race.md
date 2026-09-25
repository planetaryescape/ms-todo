# 004: A move can delete a source edited in the instant before its DELETE

**Status:** open. Accepted: a Graph limitation.

## Problem

A move deletes the source only after the copy checks out, and it reads the source once more right before the DELETE, comparing its etag with the one the copy was checked against. An edit on another device that lands in the moment between that last read and the DELETE (well under a second) is still deleted with the source, and the copy doesn't have it.

## Root cause

Graph's task DELETE ignores `If-Match` and deletes whatever is there, whatever etag is sent ([S6](../research/spikes/S6.md), [12](../blueprint/12-open-questions.md)). So there's no way to make the delete conditional on the task being unchanged: the last read can only narrow the window, not close it.

## Why it's accepted

The window is the time of one request, and BK is one user moving his own tasks. Every other edit during a move is caught: before the copy, by the check (a changed source rolls the move back), and after it, by the read before the DELETE (a changed source pauses the move with both tasks kept). Recorded in D-051.

## Options for later

- If Graph ever honours `If-Match` on task DELETE, send the verified etag with it.
- After the delete, read the source's last delta round for an edit timestamped inside the window, and warn.
