//! The diagnostics page (`D`): `ms-todo doctor` inside the TUI. The daemon
//! answers three requests: `Status` (sign-in and the daemon), `Doctor`
//! (the database, each scope's sync and the outbox) and the `unknown`
//! outbox operations, for the flagged ones. The TUI can't read the token
//! file itself (D-031), so sign-in is whether the daemon holds a
//! credential.

use ms_todo_protocol::{
    DaemonStatus, DoctorReport, ErrorPayload, OutboxOp, OutboxState, Request, ResponseData,
};

use super::{App, Effect, Mode, Tag};

/// Which of the page's requests an answer is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Status,
    Doctor,
    Unknown,
}

/// The page's data, each part as last answered: the answer, or why it
/// failed. Kept while a refresh is out, so the page doesn't blank.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Diagnostics {
    pub status: Option<Result<DaemonStatus, String>>,
    pub report: Option<Result<DoctorReport, String>>,
    pub unknown: Option<Result<Vec<OutboxOp>, String>>,
    /// Answers still to come.
    pub waiting: usize,
    pub scroll: u16,
}

impl Diagnostics {
    /// The operations `outbox list` flags: unknown for over a day.
    pub fn flagged(&self) -> Vec<&OutboxOp> {
        match &self.unknown {
            Some(Ok(ops)) => ops.iter().filter(|op| op.flagged).collect(),
            _ => Vec::new(),
        }
    }

    fn scope_count(&self) -> usize {
        match &self.report {
            Some(Ok(report)) => report.scopes.len(),
            _ => 0,
        }
    }

    /// What needs attention, for people, worded as `ms-todo doctor` words it.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        match &self.status {
            Some(Ok(status)) if !status.signed_in => {
                problems.push("not signed in; run `ms-todo auth login`".to_owned());
            }
            Some(Err(why)) => problems.push(format!("the daemon isn't available: {why}")),
            _ => {}
        }
        let report = match &self.report {
            Some(Ok(report)) => report,
            Some(Err(why)) => {
                problems.push(format!("the daemon couldn't report: {why}"));
                return problems;
            }
            None => return problems,
        };
        let failing = report
            .scopes
            .iter()
            .filter(|scope| scope.last_error.is_some())
            .count();
        if failing > 0 {
            problems.push(format!(
                "{failing} of {} scopes failed their last sync",
                report.scopes.len()
            ));
        }
        let outbox = &report.outbox;
        if outbox.unknown > 0 {
            problems.push(format!(
                "{} write(s) may or may not have reached Microsoft To Do ({} for over a day); \
                 don't resend them yourself: see `ms-todo outbox list --state unknown`",
                outbox.unknown, outbox.flagged
            ));
        }
        if outbox.failed > 0 {
            problems.push(format!(
                "{} write(s) were rejected and kept; see `ms-todo outbox list --state failed`",
                outbox.failed
            ));
        }
        if report.scopes.is_empty() && !report.syncing {
            problems.push("nothing has synced yet; run `ms-todo sync --wait`".to_owned());
        }
        problems
    }
}

/// A part's answer, or why there's none, for the page.
fn answer<T>(
    result: Result<ResponseData, ErrorPayload>,
    pick: impl FnOnce(ResponseData) -> Option<T>,
) -> Result<T, String> {
    let data = result.map_err(|error| error.message)?;
    pick(data).ok_or_else(|| "the daemon sent an unexpected answer".to_owned())
}

impl App {
    /// `D`: show the page and ask for its data.
    pub(super) fn open_diagnostics(&mut self) -> Vec<Effect> {
        self.mode = Mode::Diagnostics;
        self.diagnostics.scroll = 0;
        self.refresh_diagnostics()
    }

    /// `r` on the page: ask again, keeping what's shown until it's answered.
    pub(super) fn refresh_diagnostics(&mut self) -> Vec<Effect> {
        let requests = [
            (Part::Status, Request::Status),
            (Part::Doctor, Request::Doctor),
            (
                Part::Unknown,
                Request::OutboxList {
                    state: Some(OutboxState::Unknown),
                },
            ),
        ];
        self.diagnostics.waiting = requests.len();
        requests
            .into_iter()
            .map(|(part, request)| Effect {
                tag: Tag::Diagnostics(part),
                request,
            })
            .collect()
    }

    pub(super) fn diagnosed(&mut self, part: Part, result: Result<ResponseData, ErrorPayload>) {
        let page = &mut self.diagnostics;
        page.waiting = page.waiting.saturating_sub(1);
        match part {
            Part::Status => {
                page.status = Some(answer(result, |data| match data {
                    ResponseData::Status(status) => Some(status),
                    _ => None,
                }));
            }
            Part::Doctor => {
                page.report = Some(answer(result, |data| match data {
                    ResponseData::Doctor(report) => Some(report),
                    _ => None,
                }));
            }
            Part::Unknown => {
                page.unknown = Some(answer(result, |data| match data {
                    ResponseData::Outbox { items } => Some(items),
                    _ => None,
                }));
            }
        }
    }

    /// j and k on the page, no further than its last row.
    pub(super) fn scroll_diagnostics(&mut self, down: bool) {
        let page = &mut self.diagnostics;
        let rows = page.scope_count() + page.problems().len() + page.flagged().len();
        // Past the scopes, problems and flagged operations: the page's
        // fixed rows and headings.
        let last = u16::try_from(rows + 12).unwrap_or(u16::MAX);
        page.scroll = if down {
            page.scroll.saturating_add(1).min(last)
        } else {
            page.scroll.saturating_sub(1)
        };
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use ms_todo_protocol::{OutboxDepth, ScopeError, ScopeStatus, SyncMode, SyncState};

    use super::*;
    use crate::action::Action;
    use crate::app::Msg;
    use crate::app::tests::{act, clock, seeded};

    pub(crate) fn status() -> DaemonStatus {
        DaemonStatus {
            protocol_version: 5,
            version: "0.1.9".into(),
            pid: 4242,
            instance: "default".into(),
            started_at: clock().unix - 300,
            signed_in: true,
        }
    }

    pub(crate) fn report() -> DoctorReport {
        let scope = |scope: &str, name: Option<&str>| ScopeStatus {
            scope: scope.into(),
            list_id: name.map(|_| "home".into()),
            list_name: name.map(Into::into),
            state: SyncState::Ready,
            generation: 3,
            in_progress: false,
            last_success_at: Some(clock().unix - 12),
            last_changed_count: 2,
            last_error: None,
            mode: SyncMode::Delta,
            last_delta_at: Some(clock().unix - 12),
        };
        let mut failing = scope("tasks:L-work", Some("Work"));
        failing.last_error = Some(ScopeError {
            kind: "network".into(),
            message: "the connection was reset".into(),
            at: Some(clock().unix - 30),
        });
        DoctorReport {
            database_path: "/home/bk/.local/share/ms-todo/default/ms-todo.db".into(),
            database_bytes: 2 * 1024 * 1024,
            syncing: false,
            scopes: vec![
                scope("lists", None),
                scope("tasks:L-home", Some("Home")),
                failing,
            ],
            outbox: OutboxDepth {
                pending: 1,
                done: 40,
                unknown: 1,
                flagged: 1,
                ..OutboxDepth::default()
            },
        }
    }

    pub(crate) fn flagged_op() -> OutboxOp {
        serde_json::from_value(serde_json::json!({
            "op_id": "op-9", "command_id": "op-9", "action": "complete",
            "task_id": "t1", "list_id": "home", "title": "Pay rent",
            "state": "unknown", "attempts": 1, "created_at": clock().unix - 90_000,
            "flagged": true
        }))
        .expect("op")
    }

    /// Answer the page's three requests.
    pub(crate) fn answer(app: &mut App, effects: &[Effect]) {
        for effect in effects {
            let Tag::Diagnostics(part) = effect.tag else {
                unreachable!("{effect:?}");
            };
            let data = match part {
                Part::Status => ResponseData::Status(status()),
                Part::Doctor => ResponseData::Doctor(report()),
                Part::Unknown => ResponseData::Outbox {
                    items: vec![flagged_op()],
                },
            };
            app.update(Msg::Response {
                tag: effect.tag,
                result: Ok(data),
            });
        }
    }

    #[test]
    fn capital_d_asks_for_status_doctor_and_unknown_writes() {
        let mut app = seeded();
        let effects = act(&mut app, Action::Diagnostics);
        assert_eq!(app.mode, Mode::Diagnostics);
        let requests: Vec<_> = effects.iter().map(|e| (e.tag, e.request.clone())).collect();
        assert_eq!(
            requests,
            [
                (Tag::Diagnostics(Part::Status), Request::Status),
                (Tag::Diagnostics(Part::Doctor), Request::Doctor),
                (
                    Tag::Diagnostics(Part::Unknown),
                    Request::OutboxList {
                        state: Some(OutboxState::Unknown)
                    }
                )
            ]
        );
        assert_eq!(app.diagnostics.waiting, 3);
        answer(&mut app, &effects);
        let page = &app.diagnostics;
        assert_eq!(page.waiting, 0);
        assert_eq!(page.status, Some(Ok(status())));
        assert_eq!(page.report, Some(Ok(report())));
        assert_eq!(page.flagged().len(), 1);
        let problems = page.problems();
        assert!(
            problems[0].starts_with("1 of 3 scopes failed"),
            "{problems:?}"
        );
        assert!(
            problems[1].contains("outbox list --state unknown"),
            "{problems:?}"
        );
    }

    #[test]
    fn r_refreshes_keeping_the_page_and_escape_goes_back() {
        let mut app = seeded();
        let effects = act(&mut app, Action::Diagnostics);
        answer(&mut app, &effects);
        let again = act(&mut app, Action::Refresh);
        assert_eq!(again.len(), 3);
        assert_eq!(app.diagnostics.waiting, 3);
        assert!(
            app.diagnostics.report.is_some(),
            "kept while it's asked again"
        );
        // j and k scroll, no further up than the top.
        act(&mut app, Action::MoveUp);
        act(&mut app, Action::MoveDown);
        assert_eq!(app.diagnostics.scroll, 1);
        assert!(act(&mut app, Action::Cancel).is_empty());
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn a_failed_part_is_shown_on_the_page_not_as_a_banner() {
        let mut app = seeded();
        act(&mut app, Action::Diagnostics);
        app.update(Msg::Response {
            tag: Tag::Diagnostics(Part::Doctor),
            result: Err(ErrorPayload {
                kind: "daemon_unavailable".into(),
                message: "the daemon isn't running".into(),
                ..ErrorPayload::default()
            }),
        });
        assert_eq!(
            app.diagnostics.report,
            Some(Err("the daemon isn't running".into()))
        );
        assert!(app.banner.is_none());
        assert_eq!(
            app.diagnostics.problems(),
            ["the daemon couldn't report: the daemon isn't running"]
        );
    }

    #[test]
    fn signed_out_is_a_problem() {
        let mut page = Diagnostics {
            status: Some(Ok(DaemonStatus {
                signed_in: false,
                ..status()
            })),
            ..Diagnostics::default()
        };
        assert_eq!(page.problems(), ["not signed in; run `ms-todo auth login`"]);
        page.status = None;
        assert!(page.problems().is_empty());
    }
}
