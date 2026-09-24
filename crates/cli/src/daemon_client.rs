// Adapted from mxr @ dfb23d10138b1cfc24f8ea7450d3426e5e4da37a:
// crates/daemon/src/ipc_client.rs (request/response correlation by id, the
// protocol-mismatch guidance) and crates/daemon/src/server.rs
// (`ensure_daemon_running`, `inspect_socket_state`, `spawn_daemon_process`,
// `detach_daemon_child`, `shutdown_daemon_for_maintenance`,
// `wait_for_process_exit`). Changes: instance-aware paths from core; the
// daemon's lock file, not a process scan, says whether a daemon is alive;
// a daemon whose protocol or version differs is restarted.

//! Talking to the daemon, and starting and stopping it
//! (docs/blueprint/01-architecture.md#daemon-lifecycle). Any client that
//! finds the socket missing or dead starts a detached daemon and waits until
//! `Status` answers with a compatible protocol version.

use std::fs::File;
use std::io::{ErrorKind as IoErrorKind, Read, Seek, SeekFrom};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Stdio};
use std::time::Duration;

use fs2::FileExt;
use futures_util::{SinkExt, StreamExt};
use ms_todo_core::{ErrorKind, Paths};
use ms_todo_protocol::{
    Codec, DaemonStatus, Message, PROTOCOL_VERSION, Payload, Request, Response, ResponseData,
};
use nix::errno::Errno;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use tokio::net::UnixStream;
use tokio::time::Instant;
use tokio_util::codec::Framed;

use crate::error::CliError;

/// How long a daemon may take to answer `Status` or `Shutdown`.
const QUICK_TIMEOUT: Duration = Duration::from_secs(3);
/// A data request's ceiling. The daemon bounds each Graph call at 60 s and
/// its retries and pages, so this only catches a daemon that's stuck.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
const READY_TIMEOUT: Duration = Duration::from_secs(15);
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const LOG_TAIL_LINES: usize = 5;

pub struct DaemonClient {
    framed: Framed<UnixStream, Codec>,
    next_id: u64,
}

impl DaemonClient {
    async fn connect(socket: &Path) -> std::io::Result<Self> {
        let stream = UnixStream::connect(socket).await?;
        Ok(Self {
            framed: Framed::new(stream, Codec::new()),
            next_id: 0,
        })
    }

    pub async fn request(&mut self, request: Request) -> Result<ResponseData, CliError> {
        self.request_within(request, REQUEST_TIMEOUT).await
    }

    async fn request_within(
        &mut self,
        request: Request,
        timeout: Duration,
    ) -> Result<ResponseData, CliError> {
        self.next_id += 1;
        let id = self.next_id;
        let response = tokio::time::timeout(timeout, self.exchange(id, request))
            .await
            .map_err(|_| {
                unavailable(format!(
                    "the daemon didn't answer within {} seconds",
                    timeout.as_secs()
                ))
            })??;
        match response {
            Response::Ok { data } => Ok(data),
            Response::Error { error } => Err(error.into()),
            Response::Unknown => Err(mismatch(
                "the daemon sent an answer this version can't read",
            )),
        }
    }

    async fn exchange(&mut self, id: u64, request: Request) -> Result<Response, CliError> {
        let message = Message {
            id,
            payload: Payload::Request(request),
        };
        self.framed.send(message).await.map_err(ipc_error)?;
        loop {
            let frame =
                self.framed.next().await.ok_or_else(|| {
                    mismatch("the daemon closed the connection without answering")
                })?;
            let message = frame.map_err(ipc_error)?;
            match message.payload {
                // Id 0 is the daemon rejecting a frame it couldn't read.
                Payload::Response(response) if message.id == id || message.id == 0 => {
                    return Ok(response);
                }
                // Events, and anything a newer daemon adds, aren't answers.
                _ => {}
            }
        }
    }
}

/// A daemon that's ready for requests, and the status it reported: started
/// now if it wasn't running, and restarted if it speaks another protocol or
/// is another version.
pub async fn connect(paths: &Paths) -> Result<(DaemonClient, DaemonStatus), CliError> {
    match probe(paths).await {
        Probe::Ready(client, status) => return Ok((*client, status)),
        Probe::Incompatible(why) => {
            eprintln!("Restarting the ms-todo daemon: {why}.");
            stop(paths).await?;
        }
        Probe::Unreachable => {}
    }
    start(paths).await
}

/// The running daemon's status, without starting one.
pub async fn inspect(paths: &Paths) -> Inspection {
    match probe(paths).await {
        Probe::Ready(_, status) => Inspection::Ready(status),
        Probe::Incompatible(why) => Inspection::Unhealthy {
            pid: read_pid_file(paths),
            why,
        },
        Probe::Unreachable if daemon_lock_held(paths) => Inspection::Unhealthy {
            pid: read_pid_file(paths),
            why: "it's running but not answering on its socket".into(),
        },
        Probe::Unreachable => Inspection::Stopped,
    }
}

/// Send one request, starting the daemon first if needed.
pub async fn ask(paths: &Paths, request: Request) -> Result<ResponseData, CliError> {
    let (mut client, _) = connect(paths).await?;
    client.request(request).await
}

pub enum Inspection {
    Ready(DaemonStatus),
    Unhealthy { pid: Option<u32>, why: String },
    Stopped,
}

/// Stop the daemon. Done only when the socket is unreachable and the
/// daemon's PID has exited (spotuify's rule). Returns the PID it stopped.
pub async fn stop(paths: &Paths) -> Result<Option<u32>, CliError> {
    let pid = match probe(paths).await {
        Probe::Ready(mut client, status) => {
            // The daemon answers, then exits; a lost answer is fine, since
            // the wait below is what counts.
            let _ = client
                .request_within(Request::Shutdown, QUICK_TIMEOUT)
                .await;
            status.pid
        }
        Probe::Incompatible(_) | Probe::Unreachable => {
            if !daemon_lock_held(paths) {
                return Ok(None);
            }
            // It holds the lock but can't be asked to stop, so signal it.
            // The PID is safe to signal: the daemon wrote it under the lock
            // it still holds, so it can't have been reused.
            let pid = read_pid_file(paths).ok_or_else(|| {
                unavailable(format!(
                    "a daemon holds {} but wrote no PID to {}",
                    paths.daemon_lock_file().display(),
                    paths.pid_file().display()
                ))
            })?;
            terminate(pid)?;
            pid
        }
    };
    if wait_until_gone(paths, pid, EXIT_TIMEOUT).await {
        return Ok(Some(pid));
    }
    terminate(pid)?;
    if wait_until_gone(paths, pid, EXIT_TIMEOUT).await {
        return Ok(Some(pid));
    }
    Err(unavailable(format!(
        "the daemon (pid {pid}) didn't exit; check {}",
        paths.daemon_log_file().display()
    )))
}

enum Probe {
    Ready(Box<DaemonClient>, DaemonStatus),
    /// Something answers, but not in a way this build can use.
    Incompatible(String),
    Unreachable,
}

async fn probe(paths: &Paths) -> Probe {
    let Ok(mut client) = DaemonClient::connect(&paths.socket_path()).await else {
        // Missing, or a stale file nobody listens on. A new daemon removes
        // a stale socket itself, under its lock.
        return Probe::Unreachable;
    };
    match client.request_within(Request::Status, QUICK_TIMEOUT).await {
        Ok(ResponseData::Status(status)) => match incompatibility(&status) {
            None => Probe::Ready(Box::new(client), status),
            Some(why) => Probe::Incompatible(why),
        },
        Ok(_) => Probe::Incompatible("its status answer isn't one this version can read".into()),
        Err(error) => Probe::Incompatible(error.message),
    }
}

fn incompatibility(status: &DaemonStatus) -> Option<String> {
    let ours = env!("CARGO_PKG_VERSION");
    if status.protocol_version != PROTOCOL_VERSION {
        Some(format!(
            "it speaks protocol {}, this ms-todo speaks {PROTOCOL_VERSION}",
            status.protocol_version
        ))
    } else if status.version != ours {
        Some(format!(
            "it's version {}, this ms-todo is {ours}",
            status.version
        ))
    } else {
        None
    }
}

async fn start(paths: &Paths) -> Result<(DaemonClient, DaemonStatus), CliError> {
    let mut child = spawn(paths)?;
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        match probe(paths).await {
            Probe::Ready(client, status) => return Ok((*client, status)),
            Probe::Incompatible(why) => {
                return Err(unavailable(format!(
                    "another ms-todo daemon answered on {}: {why}; stop it with `ms-todo daemon stop`",
                    paths.socket_path().display()
                )));
            }
            Probe::Unreachable => {}
        }
        // Exit 0 means another daemon already held the lock (a racing
        // client started it), so keep waiting for that one.
        if let Ok(Some(exit)) = child.try_wait()
            && !exit.success()
        {
            return Err(unavailable(format!(
                "the daemon exited during startup ({exit}){}",
                log_tail(paths)
            )));
        }
        if Instant::now() >= deadline {
            return Err(unavailable(format!(
                "the daemon wasn't ready after {} seconds{}",
                READY_TIMEOUT.as_secs(),
                log_tail(paths)
            )));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Launch `ms-todo daemon run` detached, in its own process group, so it
/// outlives this command and the terminal's SIGHUP.
fn spawn(paths: &Paths) -> Result<Child, CliError> {
    let exe = std::env::current_exe()?;
    let log = open_log(&paths.daemon_log_file())?;
    std::process::Command::new(exe)
        .args(["daemon", "run", "--instance", paths.instance.label()])
        // Don't hold the caller's directory open for the daemon's lifetime.
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .process_group(0)
        .spawn()
        .map_err(|error| unavailable(format!("cannot start the daemon: {error}")))
}

fn open_log(path: &Path) -> Result<File, CliError> {
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    Ok(std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?)
}

fn log_tail(paths: &Paths) -> String {
    let path = paths.daemon_log_file();
    let Ok(mut file) = File::open(&path) else {
        return String::new();
    };
    // The log only grows; its last few KiB hold the last lines.
    let length = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(length.saturating_sub(4096)));
    let mut text = String::new();
    let _ = file.read_to_string(&mut text);
    let lines: Vec<&str> = text.lines().rev().take(LOG_TAIL_LINES).collect();
    let mut tail = format!(". Log: {}", path.display());
    for line in lines.into_iter().rev() {
        tail.push_str("\n  ");
        tail.push_str(line);
    }
    tail
}

async fn wait_until_gone(paths: &Paths, pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let socket_gone = UnixStream::connect(paths.socket_path()).await.is_err();
        if socket_gone && !pid_alive(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn pid_alive(pid: u32) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    // EPERM means it exists but isn't ours to signal.
    !matches!(kill(Pid::from_raw(raw), None), Err(Errno::ESRCH))
}

fn terminate(pid: u32) -> Result<(), CliError> {
    let raw = i32::try_from(pid).map_err(|_| unavailable(format!("invalid daemon PID {pid}")))?;
    match kill(Pid::from_raw(raw), Signal::SIGTERM) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(unavailable(format!(
            "cannot stop the daemon (pid {pid}): {error}"
        ))),
    }
}

/// Whether a daemon holds its lock, so is alive, even if it doesn't answer.
fn daemon_lock_held(paths: &Paths) -> bool {
    let Ok(file) = File::open(paths.daemon_lock_file()) else {
        return false;
    };
    match FileExt::try_lock_shared(&file) {
        // Closing the file releases the probe's shared lock at once.
        Ok(()) => false,
        Err(error) => error.kind() == IoErrorKind::WouldBlock,
    }
}

fn read_pid_file(paths: &Paths) -> Option<u32> {
    std::fs::read_to_string(paths.pid_file())
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn unavailable(message: String) -> CliError {
    CliError::message(ErrorKind::DaemonUnavailable, message)
}

pub(crate) fn mismatch(what: &str) -> CliError {
    unavailable(format!(
        "{what}; it may be another version. Run `ms-todo daemon stop` and try again"
    ))
}

fn ipc_error(error: std::io::Error) -> CliError {
    // A frame that framed fine but didn't decode means the daemon's
    // messages have a shape this build doesn't know (mxr's hint).
    let undecodable = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<serde_json::Error>())
        .is_some_and(|json| json.classify() == serde_json::error::Category::Data);
    if undecodable {
        mismatch(&format!(
            "the daemon sent a message this version can't read ({error})"
        ))
    } else {
        unavailable(format!("IPC error talking to the daemon: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(protocol_version: u32, version: &str) -> DaemonStatus {
        DaemonStatus {
            protocol_version,
            version: version.into(),
            pid: 1,
            instance: "dev".into(),
            started_at: 0,
            signed_in: true,
        }
    }

    #[test]
    fn only_a_daemon_of_this_protocol_and_version_is_used() {
        let ours = env!("CARGO_PKG_VERSION");
        assert_eq!(incompatibility(&status(PROTOCOL_VERSION, ours)), None);
        let other_protocol = incompatibility(&status(PROTOCOL_VERSION + 1, ours));
        assert!(other_protocol.is_some_and(|why| why.contains("protocol")));
        let upgraded = incompatibility(&status(PROTOCOL_VERSION, "0.0.1"));
        assert!(upgraded.is_some_and(|why| why.contains("0.0.1")));
    }
}
