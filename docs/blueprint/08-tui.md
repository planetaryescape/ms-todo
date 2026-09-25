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
 [a] add  [x] done  [t] my day  [e] edit  [/] filter  [:] palette  [?] help     ● synced
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

As built (rung 7, D-054): My Day is the first smart view, `☼ My Day` (`o` in ASCII) with its open count. The pane and window title is `My Day · Fri 25 Sep`, the day the daemon sends in `Seed.my_day` (it turns over at `my_day.rollover_time`, not at midnight). Its tasks come under a "Today" heading, open ones first, then "Suggestions" in the daemon's order. `t` (in the registry, the hint bar after Quit, help and the palette as "My Day") puts the cursor's task or the selection in My Day, or takes them out when every one is in already; on a suggestion it adds it, and the row moves up into Today. `a` from My Day adds the new task to it, in the default list unless `#List` says otherwise. A suggestion edited in place stays a suggestion until the view is read again. The detail pane doesn't show My Day yet.

**Folders** are collapsible groups in the sidebar, from the list extension. As built (rung 5c, D-047): after the smart views come the folders in order, each a heading with the open-task total of its lists and the lists indented under it, then the lists in no folder. `Enter` or `Space` on a heading collapses or expands it, remembered for the session; moving onto a heading shows nothing new. The cursor follows its row by ID, not position, across a collapse and a seed that reorders lists; a shown list hidden in a collapsed folder is stood for by its heading. `M` (and the palette's "Move list to folder…") asks for a folder for the list under the cursor or on screen, prefilled with its folder, suggesting existing folders as you type (`Tab` takes the first); `Enter` on an empty name (`Ctrl-u` clears the prefill) takes it out, and the hint bar says so ("Enter on empty: no folder", rung 5d). The answer redraws the sidebar at once; the palette's "Go to" reaches a list in a collapsed folder and opens the folder.

## Themes

As built (D-049). `crates/tui/src/theme/` is the only place that names a colour or a modifier; the `no_raw_styles_outside_the_theme` test fails on a `Color::` or `Modifier::` anywhere else in the TUI.

- **A palette is a colour per semantic role:** `background`, `text`, `text_dim`, `text_muted`, `border`, `border_focused`, `title`, `selection_bg`, `selection_fg`, `accent`, `overdue`, `due_today`, `important`, `completed`, `sync_pending`, `sync_unknown`, `sync_failed`, `error`, `warning`, `banner_error`, `banner_info`, `search_match`, `link` (D-050), `cursor`, `header_bar`. `Theme` turns it into the styles `ui` draws with, adding what a role always has (bold titles and keys, a crossed-out completed title). `Reset` means the terminal's own colour; for the quiet roles (`text_dim`, `text_muted`, `completed`, `sync_pending`) it means the terminal's text dimmed.
- **Built in:** `terminal` (the default: ANSI colours, dim and bold, so the terminal's theme carries over), `catppuccin-mocha`, `catppuccin-latte`, `gruvbox-dark`, `gruvbox-light`, `tokyo-night`, `nord`, `one-dark`, `kanagawa` (the "wave" variant), `night-owl`, `cobalt2` and `high-contrast` (every text colour at least 7:1 on black, checked by a test). Each palette cites its official source. Restful by design: muted borders, dimmed secondary text, colour kept for meaning, and the selection a quiet background rather than an inverted block (the terminal theme uses bright black, the one grey every terminal theme defines).
- **Colour capability:** the RGB palettes are drawn as is when `COLORTERM` is `truecolor` or `24bit`, else as the nearest xterm 256-colour index (`ansi_colours`). `NO_COLOR` (set and not empty) overrides any theme and `[tui.colors]`: bold, dim and reverse only.
- **Choosing:** `mst tui --theme <name>`, else `[tui] theme` in config.toml, else `terminal`. `[tui.colors]` overrides single roles with `#rrggbb`, an ANSI name (`red`, `bright-blue`, `dark-gray`), a 256-colour index or `reset`. An unknown theme, key or colour is an error naming it before the terminal is taken over. `mst tui --list-themes` prints the names.
- **Palette → "Theme…"** opens a picker: moving draws the whole screen in the theme under the cursor, `Enter` keeps it and the runner writes `[tui] theme` with `toml_edit`, keeping the file's other settings and comments (through a symlink, to the file it points to), and `Esc` puts the previous theme back. A failed save keeps the theme for the session and says why in a banner.

## Steps and the link

As built (rung 8a, D-055). The detail pane shows **Steps** (`2 of 5 done`, or none), each step under it with its checkbox (`○`/`✓`, `[ ]`/`[x]` in ASCII, a checked one in the `completed` role), then **Link** (its name, and its URL in the `link` role), always, so the cursor can reach them. A task's row in the list ends with `2/5` when it has steps. The detail cursor runs Title, Due, Reminder, Importance, Steps, each step, Link, Notes. It follows the step (or link) it's on by ID, as task selection does, through refreshes and a `local-…` ID becoming Graph's; if that step is deleted or the link changed elsewhere, the cursor moves to the nearest row and the next step or link action only says "That step changed; nothing was done". On the steps or the link the keys are `Context::Steps`: `Space` checks or unchecks the step under the cursor; `a` adds a step at the end (Enter adds it and opens the next, Enter on nothing or Esc stops); `e` or Enter edits the step's text, or the link's URL (adding a link when there's none; a URL that isn't one is refused inline); `d` deletes the step or the link after `y`/`n`. The editors are the line editor inline in the pane, with the hint bar naming the prompt (`New step:`). Every change is a `ChangeTasks` write drawn at once from its answer, so `u` undoes it. No `J`/`K`: Graph can't reorder steps (S15).

## Attachments

As built (rung 8b, D-056). Under the link the detail pane shows **Files** (`2 files`, `none`, or `not synced yet` while only `hasAttachments` is known), then each file with its size, `⎘` before it, or `◌` while it uploads. A task's row ends with `⎘` (`&` in ASCII) when it has files. The cursor runs on from Link through Files and each file to Notes, in `Context::Steps`. `A` on any task, or `a` on the Files rows, asks for a path in a line editor inline under Files (`~` and relative paths are read from where the TUI started; no Tab completion, which would need directory reads in `update`); Enter attaches it, and the daemon checks the file. Enter, `e` or `o` on a file (`e` does what Enter does on the row, as on a field) asks the daemon to save it into `[attachments] download_dir` (config.toml; `~/Downloads` by default; `~/` or an absolute path), answered out of order, and opens the saved file with the system's opener only when the daemon says it wrote it in that directory. `d` deletes the file after `y`. Each change is a `ChangeTasks` write, so `u` undoes it.

## Links

As built (D-050). `o` on a task opens its link, and `y` copies it (OSC 52, so it works over SSH): the linked resources' `webUrl`s first, then the URLs in the notes (found by `linkify`; an html body's hrefs through `html2text`'s footnotes), each once. With several, a picker shows each one's text (a resource's name, a markdown link's text, or the host) and URL; `Enter` or `o` opens, `y` copies, `Esc` closes. Only http, https and mailto open, checked with the `url` crate and refused if there's a control character; others are listed, marked "won't open", and refused with a banner. Opening is `open` (macOS) or `xdg-open`, run directly with the URL as its one argument, never through a shell, behind the `Opener` trait. URLs in the notes are drawn in the theme's `link` role, underlined. They aren't OSC 8 hyperlinks: ratatui has no hyperlink support, its example writes escape sequences into buffer cells with a workaround for a width bug, which would break the wrapped notes and the guarantee that nothing from Graph reaches the terminal as an escape sequence; Ghostty and iTerm already make a URL on screen Cmd-clickable. Opening, copying and saving the theme are `LocalEffect`s the runner does after the frame, so `update` stays pure.

## Quick add

Press `a` and type. The **parse highlights live as you type**: spans from `crates/nlp` are coloured by kind (date, list, label, priority, recurrence). A preview line shows the resulting fields. Enter commits and Esc cancels. Two extra keys:

- Tab accepts a completion for `#List` or `@label`.
- `ctrl+r` switches parsing off for this entry (the `--no-parse` equivalent).

As built (rung 6a, D-052):

- **A modal in the middle of the screen** (BK: "when I press a to add it should be a modal in the middle instead of me having to look down at the bottom"), about 60% of the width (56 to 100 columns, less a margin on a narrow terminal), titled "Add task → <target list>", over the panes dimmed with the theme's `backdrop` style. The status line and the hint bar stay as they are, the hint bar showing the modal's keys. It holds the text (wrapped, up to four rows), the preview line (`→ Finances · p1 · due Thu 1 Oct · every month on the 1st · remind 09:00`), up to three warnings in the `warning` role, and its keys (`Enter add · Tab complete · Ctrl-r literal · Esc cancel`, from the registry's new `Adding` context). The field editor, the due-date prompt and the filter stay in the hint bar: moving them was more than a small, consistent change, and they have their own previews there.
- **Highlighting** uses five new theme roles, `nlp_date` (due, start and `!` reminder), `nlp_list`, `nlp_label`, `nlp_priority` and `nlp_recurrence`, in every theme: plain foreground colours taken from each palette's own (due today, title or accent, link, important, search match), with no bold; `NO_COLOR` underlines them. Escapes and quote marks are drawn muted, `+myday` dim.
- **Parsed on every key** in `update`, stored in `Mode::Adding { parsed }`, so drawing stays a pure read. About 50 µs a key in a release build; `--bench-startup` now types a quick-add line too.
- **Where it goes:** a `#List` typed, else the list on screen, else "Tasks" (D-022). From Important or Planned the task gets high importance or today's due date unless the text sets one.
- **Categories:** the first `a` of a session asks the daemon for `raw GET /me/outlook/masterCategories`, once; `@label` then keeps the category's own spelling and warns about one that isn't there. If the answer fails, no label is called unknown. Tab completes from those categories, or else from the categories on the tasks read so far. The TUI never creates a category; the task still gets the name (the CLI's `--create-categories` creates it).
- Nothing left for the title keeps the modal open with an error banner, pointing at `Ctrl-r`.

**List suggestion, as built (rung 6b, D-053).** When the task is headed for the inbox (no `#List` typed, and the modal wasn't opened from another list), and `[suggest]` is on, the modal asks the daemon for a likely list once typing has paused for two ticks (250–500 ms), never per key, one request at a time, and draws the answer when it comes: a line under the preview, `→ Finances? (Ctrl-l to accept)`, in the `accent` role. `Ctrl-l` appends `#Finances` to the text (quoted if the name has a space), where it's highlighted and can be edited or removed like any typed list; Tab stays completion. The hint is shown only for the title it answered, so it goes as soon as the text changes. The request is an ordinary `Effect` whose answer arrives as a `Msg`, and the daemon answers it out of order, so the modal and the rest of the TUI never wait on TypeSafe; keypress latency is unchanged. An error (suggestions off, or an older daemon) stops asking for the session. Not built: a palette triage of the whole inbox (D-053).

## Other interactions

Copy these from mxr, including its keybinding registry:

- vim-style navigation (`j`/`k`, `g`/`G`, `h`/`l` between panes)
- a command palette (`:`)
- a contextual hint bar
- `/` for incremental filtering (FTS5)
- multi-select (`v`)
- undo of the last mutation (`u`, which queues the inverse operation; the same logic as `ms-todo undo`, see [07](07-cli.md#output-contract)). Undoing a recurring-task completion deletes the completed copy and restores the old due date. The TUI never picks the copy itself: it lists the candidates (title, `createdDateTime`, list) and you choose one. With no candidate yet, it says "can't undo yet". See [04](04-sync-cache.md#completing-a-recurring-task)
- in-place editing of every field
- a steps editor (rung 8a: [Steps and the link](#steps-and-the-link))
- attachments: add a file path, and open or download one (rung 8b: [Attachments](#attachments))
- per-row sync markers: pending (dim), unknown (amber, "outcome unknown: resolve in outbox"), failed (red, with a reason on hover or in the detail pane)
- a status line showing daemon connection, last sync and outbox depth
- until a scope's first sync finishes (`sync_state: "initial"`), a "syncing" state instead of an empty list (vault: `First Run Is the Launch Surface`, `Derived State Needs an Unknown State`)
- a diagnostics page (`ms-todo doctor` output) inside the TUI, like mxr's
- a help screen (`?`) that lists every key in the registry, and the line editor's, by where it's pressed: Navigation, Tasks, Detail pane, Prompts and editing, Views and palette. It's two columns when both fit side by side (109 columns and up with today's keys), otherwise one, with a long label wrapped rather than cut off. When it's taller than the terminal it scrolls: `j`/`k`, the arrows, `PgUp`/`PgDn` and `g`/`G`, with a scrollbar and `↓ more` or `↑ more` on its border. `Esc`, `?` or `q` closes it. This replaces D-044's note that help didn't fit a short terminal. A test fails if a binding in the registry is missing from help

## Editing fields and typing

As built in the editing fix (D-045):

- **`e` opens a one-line field picker** in the hint bar, from the task list: `edit: [t]itle [d]ue [r]eminder [i]mportance [n]otes  I Cycle importance  Esc Cancel`, built from the keybinding registry. A letter opens that field's editor in the detail pane. In the detail pane `e` (hint "Edit field") edits the field under the cursor directly, as Enter does (importance opens its `1`–`4` levels), and with a selection it still opens the picker. The palette lists each field too ("Edit title", "Edit due date", "Edit reminder", "Set importance", "Edit notes", "Cycle importance"), shown with their keys as `e t` and so on. The picker holds the task's ID, so the edit goes to that task only while it's still in the scope on screen (5a).
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

## Moving tasks between lists

As built in rung 5e (D-051): `m`, and the palette's "Move to list…", opens a picker for the selection, or the task under the cursor: `Move 3 tasks to` above a query line and the lists, each with its folder on the right. A list holding every task being moved isn't offered. Typing narrows it as the palette does (exact, prefix, a word's prefix, anywhere, then the letters in order), matching the list's name or its folder's, so `areas` finds the Areas lists; `Up`/`Down` (or `Ctrl-p`/`Ctrl-n`) choose, `Enter` moves, `Esc` cancels. The tasks are the ones on screen when `m` was pressed, never anything outside the scope, and while a scope is loading `m` does nothing. The answer takes them out of the list at once, with "Moving 3 tasks to Groceries; u moves them back", and the selection clears.
