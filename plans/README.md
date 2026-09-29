# Codebase improvement audit

Status: complete; all seven fixes and the verification script are implemented and verified on `codex/reliability-improvements`; BK authorised all seven fixes and the local verification command. See [the implementation plan](001-reliability-improvements.md). Original audit: Reviewed 2026-09-29 against freshly fetched GitHub `main` / parent checkout `origin/main`, commit `30693961074bdcfe91bd1099522c96fbab1596c5` (v0.1.40). All source links below are immutable links to that commit. The working checkout has uncommitted TUI changes and an older HEAD; those changes were excluded. The audit did not change product source; the implementation is in an isolated checkout.

Prioritise recovery and client correctness. The daemon/protocol/store boundaries already suit this product, and the current commit passes CI. Keep those boundaries and the documented Graph constraints.

## Priorities

Effort includes regression tests: S = hours, M = about a day. Risk describes the fix. Confidence is HIGH from source inspection for every row; these are not newly reproduced live-account failures.

| Order | Finding | Category / impact | Effort | Fix risk |
| --- | --- | --- | --- | --- |
| 1 | Redact signed upload URLs from errors | Security: session authorization can enter logs and outbox output | S | Low |
| 2 | Guard duplicate recurring completions | Correctness: rapid completion keys can advance two occurrences | M | Medium |
| 3 | Persist skipped writes atomically | Correctness: interrupted persistence can claim another device's value | S | Low |
| 4 | Retry attachment hydration after batch failure | Correctness: stale attachment metadata can survive successful later sync | S | Low |
| 5 | Include deferred tasks in Raycast Search | Correctness: exact-title searches cannot find deferred/Someday tasks | S | Low |
| 6 | Bound TUI subscription and request stalls | Reliability: an open silent socket leaves requests waiting indefinitely | M | Medium |
| 7 | Protect Raycast title argument boundaries | Correctness: leading-hyphen titles can be interpreted as CLI options | S | Low |

### 1. Redact signed upload URLs from errors

The upload path sends to session URLs in [attachments.rs:277](https://github.com/planetaryescape/ms-todo/blob/30693961074bdcfe91bd1099522c96fbab1596c5/crates/graph/src/attachments.rs#L277). [error.rs:121](https://github.com/planetaryescape/ms-todo/blob/30693961074bdcfe91bd1099522c96fbab1596c5/crates/graph/src/error.rs#L121) keeps the reqwest error URL, and locked reqwest 0.13.5 includes that URL in its Display output. [handlers.rs:436](https://github.com/planetaryescape/ms-todo/blob/30693961074bdcfe91bd1099522c96fbab1596c5/crates/daemon/src/handlers.rs#L436) formats the cause chain; [send.rs:746](https://github.com/planetaryescape/ms-todo/blob/30693961074bdcfe91bd1099522c96fbab1596c5/crates/daemon/src/outbox/send.rs#L746) logs and stores it; outbox output returns that stored message. D-067 says the URL carries temporary authorization and must stay out of outbox output.

Use reqwest's `without_url()` at the error boundary, preserving retry and connect classification. Check explicit URL interpolation in pagination-validation errors as part of the same boundary. Verify with synthetic URLs that transport/decode errors retain their useful kinds and causes but omit the URL from diagnostics and outbox output. No exposed real URL was observed or copied during this audit.

### 2. Guard duplicate recurring completions

[task_writes.rs:12](https://github.com/planetaryescape/ms-todo/blob/30693961074bdcfe91bd1099522c96fbab1596c5/crates/tui/src/app/task_writes.rs#L12) emits a completion for each keypress; the row changes when the acknowledgement arrives. Two presses before that reply can send two completions. The daemon [queues both](https://github.com/planetaryescape/ms-todo/blob/30693961074bdcfe91bd1099522c96fbab1596c5/crates/daemon/src/task_writes.rs#L250), and [records the old due date](https://github.com/planetaryescape/ms-todo/blob/30693961074bdcfe91bd1099522c96fbab1596c5/crates/daemon/src/task_writes.rs#L429) without making it a send-time precondition. [send.rs:503](https://github.com/planetaryescape/ms-todo/blob/30693961074bdcfe91bd1099522c96fbab1596c5/crates/daemon/src/outbox/send.rs#L503) uses the latest cached etag for each send. The second can therefore complete the newly rolled occurrence. The existing [S12 spike](https://github.com/planetaryescape/ms-todo/blob/30693961074bdcfe91bd1099522c96fbab1596c5/docs/research/spikes/S12.md#L7) establishes that Graph retains the series ID and rolls it on each completion.

Track pending completion per task, including the affected IDs in response correlation; preserve rapid completion of different tasks. Evaluate a daemon occurrence precondition so other clients receive the same protection. Verify a withheld acknowledgement plus two keys produces one mutation, and two queued requests for the same occurrence advance it once. Failures/disconnection must release client pending state without replaying an uncertain write. The recurrence test double must model S12's rollover, not an ordinary completed task.

### 3. Persist skipped writes atomically

[send.rs:166](https://github.com/planetaryescape/ms-todo/blob/30693961074bdcfe91bd1099522c96fbab1596c5/crates/daemon/src/outbox/send.rs#L166) commits the fetched task and `done` through `record_sent`, then saves the `skipped:` note separately. [extension_write.rs:160](https://github.com/planetaryescape/ms-todo/blob/30693961074bdcfe91bd1099522c96fbab1596c5/crates/daemon/src/outbox/extension_write.rs#L160) uses that note to judge whether the preceding edit was made. A crash or note-write failure between commits makes a skipped edit look applied; a dependent My Day or assignment flag can claim another device's due date/status. Undo also relies on this distinction. Startup recovery touches only `inflight`, so it does not repair the already-done row.

Add one store transaction for the fetched task, skipped reason and completed operation state. Verify persistence interruption/reopen cannot expose `done` without its skipped outcome, dependent ownership flags stay false, and undo does not invert the skipped edit. Extend the existing phone-set-date and externally-set-status regression scenarios.

### 4. Retry attachment hydration after batch failure

[hydration.rs:100](https://github.com/planetaryescape/ms-todo/blob/30693961074bdcfe91bd1099522c96fbab1596c5/crates/daemon/src/sync/hydration.rs#L100) leaves task entries in `hydrated.fetched` when the whole attachment batch fails. [pass.rs:233](https://github.com/planetaryescape/ms-todo/blob/30693961074bdcfe91bd1099522c96fbab1596c5/crates/daemon/src/sync/pass.rs#L233) treats them as fully fetched; [tasks.rs:350](https://github.com/planetaryescape/ms-todo/blob/30693961074bdcfe91bd1099522c96fbab1596c5/crates/store/src/tasks.rs#L350) advances their hydration etag while preserving an old attachment listing. On replay, [tasks.rs:161](https://github.com/planetaryescape/ms-todo/blob/30693961074bdcfe91bd1099522c96fbab1596c5/crates/store/src/tasks.rs#L161) then skips the fetch. The scope can checkpoint with stale metadata until another etag change.

Remove affected attachment-bearing tasks from `hydrated.fetched` on outer batch failure, matching the per-item failure path. Verify a cached attachment list, changed remote metadata, successful task hydration, failed outer attachment batch and later successful replay converge without another remote edit. Check attachment listing and filename search, and that the cursor advances only after recovery.

### 5. Include deferred tasks in Raycast Search

[task-list.tsx:130](https://github.com/planetaryescape/ms-todo/blob/30693961074bdcfe91bd1099522c96fbab1596c5/raycast/ms-todo/src/task-list.tsx#L130) fetches `listTasks({ status: "all" })` then filters locally. [cli.ts:232](https://github.com/planetaryescape/ms-todo/blob/30693961074bdcfe91bd1099522c96fbab1596c5/raycast/ms-todo/src/cli.ts#L232) supplies no deferred flag, so the default Hide filter removes deferred and Someday tasks before Search sees them. D-061 requires search to include them.

Add a deferred inclusion option and use `--deferred include` only for Search. Preserve deferred/Someday fields so results explain their state. Verify exact-title searches find both kinds while everyday Browse still hides them. Test argv construction and the actual CLI filtering contract.

### 6. Bound TUI subscription and request stalls

[ipc.rs:140](https://github.com/planetaryescape/ms-todo/blob/30693961074bdcfe91bd1099522c96fbab1596c5/crates/tui/src/ipc.rs#L140) sends Subscribe without an acknowledgement deadline; pending requests at line 149 carry no deadline. Connection establishment is bounded, but an accepted socket that stops answering does not reach reconnect. The CLI already implements request-specific progress-aware stalls.

Bound subscription acknowledgement, sends and each pending request. Relevant progress can extend that request's stall deadline; unrelated events must not. Keep idle subscriptions open. Verify silent Subscribe, silent Seed, useful long-running progress, and silent writes against a fake Unix server. A write timeout must explain uncertain outcome and reconnect without resending it.

### 7. Protect Raycast title argument boundaries

[cli.ts:306](https://github.com/planetaryescape/ms-todo/blob/30693961074bdcfe91bd1099522c96fbab1596c5/raycast/ms-todo/src/cli.ts#L306) puts captured text among normal arguments; line 332 passes `--title` and the value separately. Leading-hyphen text is valid task content but can become a clap option. The subprocess tests record argv without running clap, so they do not cover this boundary.

Place capture flags before `--`, with text after it; send edits as `--title=<value>`. Verify leading-hyphen captures and renames against the real parser as well as argv tests, retaining quick-add parsing and explicit-list precedence.

## Maintenance and direction

- Add one documented local verification entry point. README's contributor commands cover Rust tests/clippy/demo; CI additionally checks fmt/locked dependencies and Raycast typecheck/test/lint/build. Give it explicit Rust/Raycast scopes and avoid implicit dependency installation. Small, low risk; independent of the fixes.
- Report existing sprawl without restructuring yet: `outbox/move_job/mod.rs` has 1,033 lines; `outbox/send.rs` 834; `task_writes.rs` 709; `cli/daemon_client.rs` 740. If maintenance becomes painful, extract move recovery/verification from orchestration and outbox outcome settlement from sending. Keep behavior changes and verbatim moves in separate commits, backed by tests and a clean-checkout run.
- Prefer a reliability release next. The whole planned ladder is built; the defects above affect existing daily workflows. Further capabilities should follow observed use.
- Use the existing BK quick-add corpus for phrases that fail during daily use (Q10 remains open in `12-open-questions.md:150`). That exercises an existing parser without adding another backend; changes should follow actual misses, since stricter parsing can also eat title words.

## Verification and limits

- GitHub CI succeeded for the exact audited commit: [run 36507195215](https://github.com/planetaryescape/ms-todo/actions/runs/36507195215). Linux and macOS Rust checks and Raycast checks passed.
- Locally: `cargo nextest run --locked -p ms-todo-core -p ms-todo-nlp`: 86 passed, zero skipped, in the disposable audit checkout.
- `cargo audit --no-fetch --file Cargo.lock`: no vulnerability advisories in the cached database; one allowed unmaintained warning for transitive `paste` through tokenizers. This is not a fresh advisory-database check.
- Survey covered daemon/store recovery, Graph/auth/error handling, TUI/CLI IPC and writes, Raycast, CI/release/install and relevant decisions/tests. It was weighted toward critical paths, not an exhaustive line-by-line audit. No new fault-injection tests, live-account reproduction, phone UI checks, performance benchmarks or full local workspace suite were run. Third-party processor contracts were not assessed.
- Rejected as improvement targets: replacing SQLite/daemon/protocol architecture, adding MCP/webhooks/sharing, replacing file tokens with Keychain, adding an LLM parser without observed misses, accepted list-extension and move-delete Graph windows, and treating explicitly historical AGENTS text as a current-feature defect.

## Handover

Current state: implementation complete in `/tmp/ms-todo-improvements-clones/ms-todo`, branch `codex/reliability-improvements`. Verified code commit: `e17813ee530d6d9b21f24d7947c8998dfddcadc8`; documentation closeout is committed at `285ebd8b70a0ecdc9a272423902a746b76bfbcb2`. Fresh final fetch confirmed GitHub `origin/main` and the merge base remain `30693961074bdcfe91bd1099522c96fbab1596c5`. The original checkout and its existing edits remain untouched. BK authorised shipping on 2026-09-29. Pre-push review is complete; push, PR checks, main merge, release-please merge and published-artifact verification are next.

Verification:

- `scripts/check.sh rust`: formatting, full Clippy with warnings denied, and **998 tests passed, zero skipped**, nextest run `d8d6320d-6aed-47e0-9770-683ff2ac1bfb`. Full log: `/tmp/ms-todo-reliability-rust-check.log`.
- `scripts/check.sh raycast`: typecheck, **21 tests passed**, lint and build. Only the existing package-title casing warning remains. Dependencies were installed with `npm ci --ignore-scripts` because the local policy blocked esbuild/fsevents install hooks; the actual extension builds passed.
- `cargo build --release --locked --bin ms-todo`: passed. Binary: `/tmp/ms-todo-improve-clones/ms-todo/target/release/ms-todo`; log: `/tmp/ms-todo-reliability-release-build.log`. This target cache is in the audit checkout, but the binary was built from this implementation checkout.
- Release CLI against the signed-in `default` instance: cached list read, `tasks list --status all --deferred include`, literal leading-hyphen capture and rename with `--dry-run` all passed. No tasks were changed. Evidence: `/tmp/ms-todo-reliability-live-smoke.json`.
- ShellCheck, CI YAML parsing, diff whitespace and the changed TypeScript anti-slop check passed.

The regressions demonstrated the original failures before their fixes. Tests cover SQLite failure/reopen, S12 series rollover and deliberate next completion, concurrent enqueue, external rollover conflicts, filename search after attachment recovery, silent/blocked sockets, relevant progress, idle subscriptions and reconnect without replay. Real CLI tests verify Raycast's cross-list deferred search and literal leading-hyphen capture/rename. Review narrowed completion coalescing to recurring tasks, preserving ordinary completion/reminder writes; the existing recurrence fixture now updates remote state on PATCH rather than returning stale GET data.

Exact next action: push `codex/reliability-improvements`, open its PR and wait for remote CI and review. Merge after checks pass, then merge the updated release-please PR, wait for all release jobs and verify the three published archives and Homebrew formula. Before any push, fetch GitHub main again and rebase if it moved, then run any checks invalidated by the rebase. The audit clone's `origin/main` is stale; use this implementation checkout's remote ref for provenance. Local checks and full branch review are complete; the branch has not run remote CI.

Blocker: none for implementation. Live-account write/failure tests have no BK-named throwaway list, so they remain untested. The live smoke uses the installed daemon; the changed daemon paths were verified through fake Graph integration. No manual Raycast UI or phone test was performed.

Documentation closeout: contributor and Raycast setup commands now use the shared check script; Raycast's deferred results and TUI pending-action/stall behavior are documented. Existing architecture/Graph decisions remain valid; [D-071](../docs/blueprint/11-decision-log.md#d-071-reliability-boundaries-apply-before-sending-and-when-recording-outcomes-bk-implement-the-codebase-audit-2026-09-29) records the repaired boundaries. Source, commands and constraints are in [the implementation plan](001-reliability-improvements.md). Big-file restructuring and parser changes based on unobserved phrase misses remain deferred, as the audit recommended. Closeout is kept in the repository; no personal notes were changed.

Unresolved questions: none for implementation.
