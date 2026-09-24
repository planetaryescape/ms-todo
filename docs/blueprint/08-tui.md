# 08: TUI

The goal is that the TUI feels instant. That's the reason the cache exists.

## Latency budget

- Keypress to repaint: **under 16 ms** for navigation, filtering and view switches. These run only against memory and the daemon's cache, over IPC.
- A mutation should show up in **under 16 ms**: it's applied locally and the daemon confirms it asynchronously.
- Cold start to the first painted list: **under 150 ms** from the cache.
- Measure these. Add a `--bench-startup` or tracing span, and track them in rung 5's completion check.

How we get there:

- The TUI starts from a snapshot the daemon serves from its cache (spotuify's `ClientSeed` pattern), keeps the current view's rows in memory, and updates them when `EntityChanged` events arrive (D-031). It never opens SQLite itself.
- It sends writes over IPC too. A Unix-socket round trip is about 5–10 µs (vault: `Local IPC vs HTTP`), well inside the budget.
- Nothing in the render loop does network I/O.

## Structure

Use mxr's split, which is Elm-style:

- `app/`: state plus an update function over actions.
- `ui/`: pure render functions.
- `runner.rs`: the event loop, a `tokio::select!` over the crossterm `EventStream`, daemon events and ticks.

This is copied from `mxr/crates/tui/src/{app,ui,runner.rs}`. Avoid spotuify's 11.6k-line monolithic `App`.

## Layout

```
┌ Sidebar ─────────┬ Task list ─────────────────────────────┬ Detail ───────────────┐
│ ☀ My Day      4  │ ○ Pay rent            #Home  ↻  Oct 1  │ Title, notes (body)   │
│ ★ Important   2  │ ○ Call Sam            ⏰ 9:00  @calls  │ Due / start / remind  │
│ ▦ Planned     9  │ ● Ship blueprint      ✓               │ Recurrence            │
│ ∞ All            │                                        │ Steps (checklist)     │
│ ✓ Completed      │                                        │ Links, attachments    │
│ ─ Work ▾         │                                        │ Categories, assignee  │
│   Sprint      12 │                                        │ Sync state            │
│   Admin        3 │                                        │                       │
│ ─ Tasks       5  │                                        │                       │
└──────────────────┴────────────────────────────────────────┴───────────────────────┘
 [a] add  [x] done  [m] my day  [e] edit  [/] filter  [:] palette  [?] help     ● synced
```

Use glyphs from a Nerd Font or Unicode symbol set, with an ASCII fallback option. No emoji (BK's rule).

**Smart views** are all queries the daemon answers from its local cache:

- My Day
- Important (`importance = high`)
- Planned (has a due date, grouped into Overdue, Today, Tomorrow, This week, Later)
- All
- Completed
- Assigned (has an assignee)

The **My Day view** has:

- A heading with the date.
- Today's tasks, with a "Suggestions" section below them: due today, overdue, and yesterday's unfinished My Day tasks. One key adds a suggestion.

**Folders** are collapsible groups in the sidebar, from the list extension.

## Quick add

Press `a` and type. The **parse highlights live as you type**: spans from `crates/nlp` are coloured by kind (date, list, label, priority, recurrence). A preview line shows the resulting fields. Enter commits and Esc cancels. Two extra keys:

- Tab accepts a completion for `#List` or `@label`.
- `ctrl+r` switches parsing off for this entry (the `--no-parse` equivalent).

## Other interactions

Copy these from mxr, including its keybinding registry:

- vim-style navigation (`j`/`k`, `g`/`G`, `h`/`l` between panes)
- a command palette (`:`)
- a contextual hint bar
- `/` for incremental filtering (FTS5)
- multi-select (`v`)
- undo of the last mutation (`u`, which queues the inverse operation; the same logic as `ms-todo undo`, see [07](07-cli.md#output-contract)). Undoing a recurring-task completion deletes the completed copy and restores the old due date. The TUI never picks the copy itself: it lists the candidates (title, `createdDateTime`, list) and you choose one. With no candidate yet, it says "can't undo yet". See [04](04-sync-cache.md#completing-a-recurring-task)
- in-place editing of every field
- a steps editor
- attachments: add a file path, and open or download one
- per-row sync markers: pending (dim), unknown (amber, "outcome unknown: resolve in outbox"), failed (red, with a reason on hover or in the detail pane)
- a status line showing daemon connection, last sync and outbox depth
- until a scope's first sync finishes (`sync_state: "initial"`), a "syncing" state instead of an empty list (vault: `First Run Is the Launch Surface`, `Derived State Needs an Unknown State`)
- a diagnostics page (`ms-todo doctor` output) inside the TUI, like mxr's
