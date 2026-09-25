# Microsoft To Do via ms-todo

Capture, search, and complete Microsoft To Do tasks from Raycast using the local [ms-todo](https://github.com/planetaryescape/ms-todo) CLI. The extension asks the CLI for cached tasks; the CLI's daemon owns sync and sign-in. This extension does not connect to Microsoft Graph, read the cache directly, or collect analytics.

## Setup

1. Install the official ms-todo Homebrew formula on macOS:

   ```sh
   brew install planetaryescape/ms-todo/ms-todo
   ```

   You can also [install from source](https://github.com/planetaryescape/ms-todo#install). The release binary is not currently code signed; follow the main project's installation guidance when choosing how to install it.

2. Sign in and check the local cache:

   ```sh
   ms-todo auth login
   ms-todo sync --wait
   ms-todo doctor
   ```

3. Import this folder as a local extension in Raycast, or use `npm install` and `npm run dev` from this folder. The extension looks for `ms-todo` in the standard Homebrew paths, `~/.local/bin`, and Raycast's `PATH`. If it cannot find the binary, set the absolute **ms-todo CLI Path** in extension preferences.

## Commands

- **Quick Add Task** uses [ms-todo quick add syntax](https://github.com/planetaryescape/ms-todo/blob/main/docs/usage.md#quick-add). For example, `Buy milk tomorrow #Groceries`. With no list, ms-todo adds it to Tasks. An unknown explicit `#List` is rejected, so a typo cannot silently file the task in Tasks.
- **Search Tasks** searches cached open tasks by title or notes. Select **Complete Task** to complete a result.
- **My Day** shows today's ms-todo My Day and lets you complete an open task. ms-todo's My Day is its own synced view; Microsoft Graph does not expose the To Do app's My Day.

Reads come from the local cache and may lag Microsoft To Do until the daemon syncs. An empty result while initial sync is running is marked as incomplete. Local writes appear immediately and may still be pending upstream; use `ms-todo doctor` or `ms-todo outbox list` to inspect sync problems.

## Development

```sh
npm ci
npm test
npm run typecheck
npm run lint
npm run build
```

The subprocess tests use a fake CLI and do not write to a Microsoft account. The extension is MIT licensed. Store publication is prepared but has not been submitted.
