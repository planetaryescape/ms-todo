//! `ms-todo daemon start|stop|status`.

use ms_todo_core::Paths;
use ms_todo_protocol::DaemonStatus;
use serde::Serialize;

use crate::daemon_client::{self, Inspection};
use crate::error::CliError;
use crate::output::Render;
use crate::time::rfc3339;

#[derive(Serialize)]
pub struct DaemonState {
    pub running: bool,
    /// False when a daemon runs but can't serve this client; see `problem`.
    pub ready: bool,
    pub pid: Option<u32>,
    pub version: Option<String>,
    pub protocol_version: Option<u32>,
    /// RFC 3339, UTC.
    pub started_at: Option<String>,
    pub signed_in: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    pub instance: String,
    pub socket: String,
}

impl DaemonState {
    pub(crate) fn new(paths: &Paths, inspection: Inspection) -> Self {
        let mut state = Self {
            running: false,
            ready: false,
            pid: None,
            version: None,
            protocol_version: None,
            started_at: None,
            signed_in: None,
            problem: None,
            instance: paths.instance.label().to_owned(),
            socket: paths.socket_path().display().to_string(),
        };
        match inspection {
            Inspection::Ready(status) => state.ready_with(status),
            Inspection::Unhealthy { pid, why } => {
                state.running = true;
                state.pid = pid;
                state.problem = Some(why);
            }
            Inspection::Stopped => {}
        }
        state
    }

    fn ready_with(&mut self, status: DaemonStatus) {
        self.running = true;
        self.ready = true;
        self.pid = Some(status.pid);
        self.version = Some(status.version);
        self.protocol_version = Some(status.protocol_version);
        self.started_at = Some(rfc3339(status.started_at));
        self.signed_in = Some(status.signed_in);
    }
}

impl Render for DaemonState {
    fn table_rows(&self) -> Vec<(&'static str, String)> {
        let state = match (self.running, self.ready) {
            (false, _) => "stopped".to_owned(),
            (true, true) => "running".to_owned(),
            (true, false) => "running, not ready".to_owned(),
        };
        let mut rows = vec![("Daemon", state)];
        let optional = [
            ("PID", self.pid.map(|pid| pid.to_string())),
            ("Version", self.version.clone()),
            ("Protocol", self.protocol_version.map(|v| v.to_string())),
            ("Started", self.started_at.clone()),
            (
                "Signed in",
                self.signed_in
                    .map(|yes| if yes { "yes" } else { "no" }.to_owned()),
            ),
            ("Problem", self.problem.clone()),
        ];
        rows.extend(
            optional
                .into_iter()
                .filter_map(|(label, value)| value.map(|v| (label, v))),
        );
        rows.push(("Instance", self.instance.clone()));
        rows.push(("Socket", self.socket.clone()));
        rows
    }
}

#[derive(Serialize)]
pub struct DaemonStopped {
    /// Always false; mirrors `daemon status` so scripts can read one field.
    pub running: bool,
    /// The PID that was stopped, or null if no daemon was running.
    pub stopped_pid: Option<u32>,
}

impl Render for DaemonStopped {
    fn table_rows(&self) -> Vec<(&'static str, String)> {
        let outcome = match self.stopped_pid {
            Some(pid) => format!("stopped (pid {pid} has exited)"),
            None => "wasn't running".to_owned(),
        };
        vec![("Daemon", outcome)]
    }
}

pub async fn start(paths: &Paths) -> Result<DaemonState, CliError> {
    let (_, status) = daemon_client::connect(paths).await?;
    Ok(DaemonState::new(paths, Inspection::Ready(status)))
}

pub async fn stop(paths: &Paths) -> Result<DaemonStopped, CliError> {
    Ok(DaemonStopped {
        running: false,
        stopped_pid: daemon_client::stop(paths).await?,
    })
}

pub async fn status(paths: &Paths) -> DaemonState {
    DaemonState::new(paths, daemon_client::inspect(paths).await)
}

/// `daemon restart`: stop it if it runs, then start it and wait until
/// it's ready.
pub async fn restart(paths: &Paths) -> Result<DaemonState, CliError> {
    daemon_client::stop(paths).await?;
    start(paths).await
}

/// `daemon logs [--follow]`: the daemon's log file as it is, then with
/// `--follow` each line added to it until interrupted. JSON is `{ path,
/// lines }`; JSON lines, or following, one `{ line }` per line.
pub async fn logs(
    paths: &Paths,
    follow: bool,
    format: crate::output::OutputFormat,
) -> Result<(), CliError> {
    use std::io::Write;

    use crate::output::{OutputFormat, SCHEMA_VERSION, Versioned, print_json};

    let path = paths.daemon_log_file();
    let text = match std::fs::read(&path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let lines: Vec<&str> = text.lines().collect();
    let print_lines = |lines: &[&str]| -> Result<(), CliError> {
        let mut stdout = std::io::stdout().lock();
        for line in lines {
            if format == OutputFormat::Jsonl || (follow && format == OutputFormat::Json) {
                writeln!(stdout, "{}", serde_json::json!({ "line": line }))?;
            } else {
                writeln!(stdout, "{line}")?;
            }
        }
        stdout.flush()?;
        Ok(())
    };
    if format == OutputFormat::Json && !follow {
        #[derive(Serialize)]
        struct Logs<'a> {
            path: String,
            lines: &'a [&'a str],
        }
        return print_json(
            format,
            &Versioned {
                schema_version: SCHEMA_VERSION,
                inner: &Logs {
                    path: path.display().to_string(),
                    lines: &lines,
                },
            },
        );
    }
    print_lines(&lines)?;
    if !follow {
        return Ok(());
    }
    // Polled rather than watched: a log grows a line at a time, and this
    // runs until interrupted.
    let mut offset = u64::try_from(text.len()).unwrap_or(u64::MAX);
    let mut partial = String::new();
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let Ok(length) = std::fs::metadata(&path).map(|file| file.len()) else {
            continue;
        };
        if length < offset {
            // Started again from nothing: read it from the top.
            offset = 0;
            partial.clear();
        }
        if length == offset {
            continue;
        }
        // Only what was added since the last read.
        let mut added = Vec::new();
        let read = std::fs::File::open(&path).and_then(|mut file| {
            use std::io::{Read, Seek, SeekFrom};
            file.seek(SeekFrom::Start(offset))?;
            file.read_to_end(&mut added)
        });
        if read.is_err() {
            continue;
        }
        offset += u64::try_from(added.len()).unwrap_or(0);
        partial.push_str(&String::from_utf8_lossy(&added));
        if let Some(end) = partial.rfind('\n') {
            let complete: String = partial.drain(..=end).collect();
            print_lines(&complete.lines().collect::<Vec<_>>())?;
        }
    }
}
