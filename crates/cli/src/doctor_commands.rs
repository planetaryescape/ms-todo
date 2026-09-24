//! `ms-todo doctor`: every subsystem in one report. Sign-in is read from
//! the token file (no network); the daemon is started if needed; the cache
//! and sync state come from the daemon. A part that can't be checked is
//! reported as a problem rather than failing the command.

use ms_todo_core::Paths;
use ms_todo_graph::auth::{Authenticator, Endpoints};
use ms_todo_protocol::{
    DoctorReport, OutboxDepth, Request, ResponseData, ScopeStatus, SyncMode, SyncState,
};
use serde::Serialize;

use crate::daemon_client::{self, Inspection};
use crate::daemon_commands::DaemonState;
use crate::error::CliError;
use crate::output::Render;
use crate::time::rfc3339;

#[derive(Serialize)]
pub struct Doctor {
    pub sign_in: SignIn,
    pub daemon: DaemonState,
    pub database: Option<Database>,
    /// Whether a sync is running now.
    pub syncing: Option<bool>,
    pub scopes: Vec<Scope>,
    /// The most recent sync failure of any scope.
    pub last_error: Option<LastError>,
    /// How many writes are in each outbox state.
    pub outbox: Option<OutboxDepth>,
    /// What needs attention, for people.
    pub problems: Vec<String>,
}

#[derive(Serialize)]
pub struct SignIn {
    pub signed_in: bool,
    /// RFC 3339, UTC.
    pub expires_at: Option<String>,
    pub token_path: String,
}

#[derive(Serialize)]
pub struct Database {
    pub path: String,
    pub bytes: u64,
}

#[derive(Serialize)]
pub struct Scope {
    pub scope: String,
    pub list_id: Option<String>,
    pub list_name: Option<String>,
    pub state: SyncState,
    pub generation: u64,
    pub in_progress: bool,
    /// RFC 3339, UTC.
    pub last_success_at: Option<String>,
    pub last_changed_count: u64,
    pub last_error: Option<ScopeFailure>,
    /// `delta` once a pass has saved a delta link; `enumeration` until
    /// then, or after Graph rejected the link.
    pub mode: SyncMode,
    /// RFC 3339, UTC: when a delta round last checkpointed.
    pub last_delta_at: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct ScopeFailure {
    pub kind: String,
    pub message: String,
    /// RFC 3339, UTC.
    pub at: Option<String>,
}

#[derive(Serialize)]
pub struct LastError {
    pub scope: String,
    #[serde(flatten)]
    pub error: ScopeFailure,
}

pub async fn doctor(paths: &Paths) -> Result<Doctor, CliError> {
    let mut problems = Vec::new();
    let sign_in = sign_in(paths, &mut problems)?;
    let (daemon, report) = match daemon_client::connect(paths).await {
        Ok((mut client, status)) => {
            let daemon = DaemonState::new(paths, Inspection::Ready(status));
            match client.request(Request::Doctor).await {
                Ok(ResponseData::Doctor(report)) => (daemon, Some(report)),
                Ok(_) => {
                    problems.push("the daemon sent an unexpected answer to `doctor`".into());
                    (daemon, None)
                }
                Err(error) => {
                    problems.push(format!("the daemon couldn't report: {}", error.message));
                    (daemon, None)
                }
            }
        }
        Err(error) => {
            problems.push(format!("the daemon isn't available: {}", error.message));
            (
                DaemonState::new(paths, daemon_client::inspect(paths).await),
                None,
            )
        }
    };
    let Some(report) = report else {
        return Ok(Doctor {
            sign_in,
            daemon,
            database: None,
            syncing: None,
            scopes: Vec::new(),
            last_error: None,
            outbox: None,
            problems,
        });
    };
    let DoctorReport {
        database_path,
        database_bytes,
        syncing,
        scopes,
        outbox,
    } = report;
    let scopes: Vec<Scope> = scopes.into_iter().map(Scope::from).collect();
    let last_error = scopes
        .iter()
        .filter_map(|scope| Some((scope, scope.last_error.as_ref()?)))
        .max_by(|(_, a), (_, b)| a.at.cmp(&b.at))
        .map(|(scope, error)| LastError {
            scope: scope.scope.clone(),
            error: error.clone(),
        });
    let failing = scopes
        .iter()
        .filter(|scope| scope.last_error.is_some())
        .count();
    if failing > 0 {
        problems.push(format!(
            "{failing} of {} scopes failed their last sync",
            scopes.len()
        ));
    }
    if outbox.unknown > 0 {
        problems.push(format!(
            "{} write(s) may or may not have reached Microsoft To Do ({} for over a day); don't \
             resend them yourself: see `ms-todo outbox list --state unknown`",
            outbox.unknown, outbox.flagged
        ));
    }
    if outbox.failed > 0 {
        problems.push(format!(
            "{} write(s) were rejected and kept; see `ms-todo outbox list --state failed`",
            outbox.failed
        ));
    }
    if scopes.is_empty() && !syncing {
        problems.push("nothing has synced yet; run `ms-todo sync --wait`".into());
    }
    Ok(Doctor {
        sign_in,
        daemon,
        database: Some(Database {
            path: database_path,
            bytes: database_bytes,
        }),
        syncing: Some(syncing),
        scopes,
        last_error,
        outbox: Some(outbox),
        problems,
    })
}

fn sign_in(paths: &Paths, problems: &mut Vec<String>) -> Result<SignIn, CliError> {
    let auth = Authenticator::new(paths.auth_dir(), Endpoints::default())?;
    let token_path = auth.token_path().display().to_string();
    let expires_at = match auth.stored_token() {
        Ok(Some(token)) => Some(rfc3339(token.expires_at)),
        Ok(None) => {
            problems.push("not signed in; run `ms-todo auth login`".into());
            None
        }
        Err(error) => {
            problems.push(format!("the stored sign-in can't be read: {error}"));
            None
        }
    };
    Ok(SignIn {
        signed_in: expires_at.is_some(),
        expires_at,
        token_path,
    })
}

impl From<ScopeStatus> for Scope {
    fn from(status: ScopeStatus) -> Self {
        Self {
            scope: status.scope,
            list_id: status.list_id,
            list_name: status.list_name,
            state: status.state,
            generation: status.generation,
            in_progress: status.in_progress,
            last_success_at: status.last_success_at.map(rfc3339),
            last_changed_count: status.last_changed_count,
            mode: status.mode,
            last_delta_at: status.last_delta_at.map(rfc3339),
            last_error: status.last_error.map(|error| ScopeFailure {
                kind: error.kind,
                message: error.message,
                at: error.at.map(rfc3339),
            }),
        }
    }
}

impl Render for Doctor {
    fn table_rows(&self) -> Vec<(&'static str, String)> {
        let signed_in = match (&self.sign_in.signed_in, &self.sign_in.expires_at) {
            (true, Some(expires)) => format!("yes (token expires {expires})"),
            (true, None) => "yes".into(),
            (false, _) => "no".into(),
        };
        let daemon = match (self.daemon.running, self.daemon.ready, self.daemon.pid) {
            (true, true, Some(pid)) => format!("running (pid {pid})"),
            (true, _, _) => "running, not ready".into(),
            (false, _, _) => "stopped".into(),
        };
        let mut rows = vec![("Signed in", signed_in), ("Daemon", daemon)];
        if let Some(database) = &self.database {
            rows.push((
                "Database",
                format!("{} ({})", database.path, human_bytes(database.bytes)),
            ));
        }
        if !self.scopes.is_empty() || self.syncing.is_some() {
            let ready = self
                .scopes
                .iter()
                .filter(|scope| scope.state == SyncState::Ready)
                .count();
            let delta = self
                .scopes
                .iter()
                .filter(|scope| scope.mode == SyncMode::Delta)
                .count();
            let mut sync = format!(
                "{ready} of {} scopes ready, {delta} on delta",
                self.scopes.len()
            );
            if let Some(last) = self
                .scopes
                .iter()
                .filter_map(|scope| scope.last_delta_at.as_deref())
                .max()
            {
                sync.push_str(&format!("; last delta {last}"));
            }
            if self.syncing == Some(true) {
                sync.push_str("; syncing now");
            }
            rows.push(("Sync", sync));
        }
        if let Some(outbox) = &self.outbox {
            rows.push((
                "Outbox",
                format!(
                    "{} pending, {} sending, {} unknown, {} failed, {} done",
                    outbox.pending, outbox.inflight, outbox.unknown, outbox.failed, outbox.done
                ),
            ));
        }
        for scope in self
            .scopes
            .iter()
            .filter(|scope| scope.last_error.is_some())
        {
            if let Some(error) = &scope.last_error {
                let name = scope.list_name.as_deref().unwrap_or(&scope.scope);
                rows.push((
                    "Failing",
                    format!("{name}: {} ({})", error.message, error.kind),
                ));
            }
        }
        if let Some(error) = &self.last_error {
            rows.push((
                "Last error",
                format!(
                    "{} at {}: {}",
                    error.scope,
                    error.error.at.as_deref().unwrap_or("?"),
                    error.error.message
                ),
            ));
        }
        for problem in &self.problems {
            rows.push(("Problem", problem.clone()));
        }
        if self.problems.is_empty() {
            rows.push(("Status", "all good".into()));
        }
        rows
    }
}

fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    match bytes {
        b if b >= KIB * KIB => format!("{:.1} MiB", b as f64 / (KIB * KIB) as f64),
        b if b >= KIB => format!("{:.1} KiB", b as f64 / KIB as f64),
        b => format!("{b} bytes"),
    }
}
