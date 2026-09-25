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
- Completed (grouped by the day each task was completed: Today, Yesterday, `Mon 21 Sep`…; rung 5d)
- Assigned (has an assignee)

The **My Day view** has:

- A heading with the date.
- Today's tasks, with a "Suggestions" section below them: due today, overdue, and yesterday's unfinished My Day tasks. One key adds a suggestion.

**Folders** are collapsible groups in the sidebar, from the list extension. As built (rung 5c, D-047): after the smart views come the folders in order, each a heading with the open-task total of its lists and the lists indented under it, then the lists in no folder. `Enter` or `Space` on a heading collapses or expands it, remembered for the session; moving onto a heading shows nothing new. The cursor follows its row by ID, not position, across a collapse and a seed that reorders lists; a shown list hidden in a collapsed folder is stood for by its heading. `M` (and the palette's "Move list to folder…") asks for a folder for the list under the cursor or on screen, prefilled with its folder, suggesting existing folders as you type (`Tab` takes the first); `Enter` on an empty name (`Ctrl-u` clears the prefill) takes it out, and the hint bar says so ("Enter on empty: no folder", rung 5d). The answer redraws the sidebar at once; the palette's "Go to" reaches a list in a collapsed folder and opens the folder.

## Themes

As built (D-049). `crates/tui/src/theme/` is the only place that names a colour or a modifier; the `no_raw_styles_outside_the_theme` test fails on a `Color::` or `Modifier::` anywhere else in the TUI.

- **A palette is a colour per semantic role:** `background`, `text`, `text_dim`, `text_muted`, `border`, `border_focused`, `title`, `selection_bg`, `selection_fg`, `accent`, `overdue`, `due_today`, `important`, `completed`, `sync_pending`, `sync_unknown`, `sync_failed`, `error`, `warning`, `banner_error`, `banner_info`, `search_match`, `cursor`, `header_bar`. `Theme` turns it into the styles `ui` draws with, adding what a role always has (bold titles and keys, a crossed-out completed title). `Reset` means the terminal's own colour; for the quiet roles (`text_dim`, `text_muted`, `completed`, `sync_pending`) it means the terminal's text dimmed.
- **Built in:** `terminal` (the default: ANSI colours, dim and bold, so the terminal's theme carries over), `catppuccin-mocha`, `catppuccin-latte`, `gruvbox-dark`, `gruvbox-light`, `tokyo-night`, `nord` and `high-contrast` (every text colour at least 7:1 on black, checked by a test). Each palette cites its official source. Restful by design: muted borders, dimmed secondary text, colour kept for meaning, and the selection a quiet background rather than an inverted block (the terminal theme uses bright black, the one grey every terminal theme defines).
- **Colour capability:** the RGB palettes are drawn as is when `COLORTERM` is `truecolor` or `24bit`, else as the nearest xterm 256-colour index (`ansi_colours`). `NO_COLOR` (set and not empty) overrides any theme and `[tui.colors]`: bold, dim and reverse only.
- **Choosing:** `mst tui --theme <name>`, else `[tui] theme` in config.toml, else `terminal`. `[tui.colors]` overrides single roles with `#rrggbb`, an ANSI name (`red`, `bright-blue`, `dark-gray`), a 256-colour index or `reset`. An unknown theme, key or colour is an error naming it before the terminal is taken over. `mst tui --list-themes` prints the names.
- **Palette → "Theme…"** opens a picker: moving draws the whole screen in the theme under the cursor, `Enter` keeps it and the runner writes `[tui] theme` with `toml_edit`, keeping the file's other settings and comments (through a symlink, to the file it points to), and `Esc` puts the previous theme back. A failed save keeps the theme for the session and says why in a banner.

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

## Editing fields and typing

As built in the editing fix (D-045):

- **`e` opens a one-line field picker** in the hint bar, from the list or the detail pane: `edit: [t]itle [d]ue [r]eminder [i]mportance [n]otes  I Cycle importance  Esc Cancel`, built from the keybinding registry. A letter opens that field's editor in the detail pane; Enter on a field in the detail pane still edits it directly. The palette lists each field too ("Edit title", "Edit due date", "Edit reminder", "Set importance", "Edit notes", "Cycle importance"), shown with their keys as `e t` and so on. The picker holds the task's ID, so the edit goes to that task only while it's still in the scope on screen (5a).
- **Importance is never typed.** `i` asks for a level, `importance: 1/h High  2/3/n Normal  4/l Low  Esc Cancel`, and a key saves it at once (D-017's mapping, read by `ms_todo_nlp::read_importance`). `I` in the picker cycles low, normal, high and saves.
- **Every prompt is a line editor** (the add, filter, edit and palette prompts): `ratatui-textarea` holds the text and the cursor, and `app/line_editor.rs` picks its keys. Left and Right, Home and End (Ctrl-a, Ctrl-e), a word back and forward (Alt-b, Alt-f, Ctrl- or Alt-arrows), Backspace, Delete, Ctrl-w a word back, Ctrl-u to the line's start, Ctrl-k to its end. The cursor starts after the value, and the character under it is drawn reversed (the cursor glyph past the end). Keys the registry doesn't bind in a prompt go to the editor as `Msg::Key`; the registry lists the prompt keys, and help lists the editor's.
- **Notes are multi-line:** Enter is a new line, and Ctrl-s or Alt-Enter saves.
- **Dates take phrases** (`tomorrow`, `fri 17:30`, `+2w`, `in 2 days`, `12 oct`), read by `crates/nlp` as the CLI's flags are ([07](07-cli.md)). A reminder given only a day is 09:00 on it (`in 2 days` → `Sat 26 Sep 09:00`). What the text resolves to shows under it as it's typed (`→ Fri 2 Oct`, `, in the past`), or why it can't be read; Enter with input it can't read shows the reason in red and sends nothing.

## What I finished; clearing what's overdue

As built in rung 5d (D-048):

- **The Completed view is grouped by day**, newest first: a heading per day a task was completed (`Today`, `Yesterday`, then `Mon 21 Sep`, with the year when it isn't this one), the same headings `ms-todo done` prints. Graph records the day only, as a UTC date (S12), and that date is the day, as in `done`. A completion Graph hasn't answered yet has no day and comes first, under "Not synced yet"; the daemon orders the view that way too, so the cursor moves through the rows in the order drawn.
- **`S` "Set due date…"** asks for one due date for the selection, or the task under the cursor, in the hint bar: `Due date for 3 tasks: next mon█  → Mon 28 Sep`. It reads, previews and refuses what the due-date editor does; empty is refused rather than clearing several tasks' dates. `e` `d` with a selection opens it too. One `ChangeTasks` goes out, so one `u` puts every task back.
- **`R` "Reschedule overdue to…"** does the same for every open task on screen that's overdue (`Due date for 4 overdue tasks:`), or says "Nothing here is overdue". Both are in the palette.
- A change to more than one task says "Changed N tasks; u puts them all back". An undo that left tasks alone because they changed since names them: "Undone, except "Dentist": changed since, so left alone".
