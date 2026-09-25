// Adapted from mxr @ dfb23d10138b1cfc24f8ea7450d3426e5e4da37a:
// crates/daemon/src/ipc_client.rs (request/response correlation by id, the
// protocol-mismatch guidance) and crates/daemon/src/server.rs
// (`ensure_daemon_running`, `inspect_socket_state`, `spawn_daemon_process`,
// `detach_daemon_child`, `shutdown_daemon_for_maintenance`,
// `wait_for_process_exit`, `process_is_zombie` and its unreaped-child
// test). Changes: instance-aware paths from core; the
// daemon's lock file, not a process scan, says whether a daemon is alive;
// a daemon whose protocol or version differs is restarted; the daemon is
// started through `daemon launch` so it never stays a client's child;
// procfs, not `ps`, says whether a process is a zombie on Linux.

//! Talking to the daemon, and starting and stopping it
//! (docs/blueprint/01-architecture.md#daemon-lifecycle). Any client that
//! finds the socket missing or dead starts a detached daemon and waits until
//! `Status` answers with a compatible protocol version.
//!
//! A request gives up only after the daemon has sent nothing for it for a
//! while (a stall), never on total time: each progress event restarts the
//! clock, so a long `sync --wait` runs as long as it keeps moving
//! (docs/blueprint/01-architecture.md#transport, issue 002).

use std::fs::File;
use std::io::{ErrorKind as IoErrorKind, Read, Seek, SeekFrom};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::process::{Child, ExitCode, Stdio};
use std::time::Duration;

use fs2::FileExt;
use futures_util::{SinkExt, StreamExt};
use ms_todo_core::{ErrorKind, Paths};
use ms_todo_protocol::{
    Codec, DaemonStatus, EXIT_DATABASE_TOO_NEW, Event, Message, PROTOCOL_VERSION, Payload, Request,
    Response, ResponseData,
};
use nix::errno::Errno;
use nix::sys::signal::{Signal, kill};
use nix::unistd::{Pid, setsid};
use tokio::net::UnixStream;
use tokio::time::Instant;
use tokio_util::codec::Framed;

use crate::error::CliError;

/// How long a daemon may take to answer `Status` or `Shutdown`.
const QUICK_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a data request may go without an answer or a progress event.
/// Longer than the daemon's worst bounded step: a Graph call is 60 s, with
/// at most 3 retries, and a write may first wait for the first sync.
const STALL_TIMEOUT: Duration = Duration::from_secs(300);
const STALL_TIMEOUT_ENV: &str = "MS_TODO_REQUEST_TIMEOUT_MS";
const READY_TIMEOUT: Duration = Duration::from_secs(15);
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long `daemon launch` relays the daemon's exit: past `READY_TIMEOUT`,
/// so it outlasts its client's wait, but bounded, so a launcher whose client
/// died doesn't stay the daemon's parent.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(READY_TIMEOUT.as_secs() * 2);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const LOG_TAIL_LINES: usize = 5;

pub struct DaemonClient {
    framed: Framed<UnixStream, Codec>,
    next_id: u64,
}

impl DaemonClient {
    async fn connect(socket: &Path) -> std::io::Result<Self> {
        let stream = connect_socket(socket).await?;
        Ok(Self {
            framed: Framed::new(stream, Codec::new()),
            next_id: 0,
        })
    }

    pub async fn request(&mut self, request: Request) -> Result<ResponseData, CliError> {
        self.request_within(request, stall_timeout()).await
    }

    /// Send `request` and pass each event the daemon sends for it to
    /// `on_event` until the answer arrives.
    pub async fn request_with_events(
        &mut self,
        request: Request,
        on_event: impl FnMut(Event),
    ) -> Result<ResponseData, CliError> {
        let stall = stall_timeout();
        let id = self.send(request, stall).await?;
        into_data(self.reply(id, stall, on_event).await?)
    }

    async fn request_within(
        &mut self,
        request: Request,
        stall: Duration,
    ) -> Result<ResponseData, CliError> {
        let id = self.send(request, stall).await?;
        into_data(self.reply(id, stall, |_| {}).await?)
    }

    /// Send a mutation and wait for its answer. Once the request is on the
    /// socket the daemon may apply it even if its answer never arrives, so
    /// from then on any failure is `outcome_unknown` with the CLI's `op_id`,
    /// never a plain "daemon unavailable" that invites a blind retry.
    async fn mutate(
        &mut self,
        request: Request,
        op_id: &str,
        check: &str,
    ) -> Result<ResponseData, CliError> {
        let stall = stall_timeout();
        let id = self.send(request, stall).await?;
        let lost = |why: String| CliError {
            op_id: Some(op_id.to_owned()),
            ..CliError::message(
                ErrorKind::OutcomeUnknown,
                format!(
                    "the daemon took the request, but its answer was lost ({why}). The change \
                     may or may not have been made: check {check} before trying again, or a \
                     retry can make a duplicate. op_id {op_id}"
                ),
            )
        };
        match self.reply(id, stall, |_| {}).await {
            Ok(Response::Unknown) => Err(lost("it sent an answer this version can't read".into())),
            Ok(response) => into_data(response),
            Err(error) => Err(lost(error.message)),
        }
    }

    async fn send(&mut self, request: Request, stall: Duration) -> Result<u64, CliError> {
        self.next_id += 1;
        let message = Message {
            id: self.next_id,
            payload: Payload::Request(request),
        };
        tokio::time::timeout(stall, self.framed.send(message))
            .await
            .map_err(|_| unavailable("the daemon didn't take the request in time".into()))?
            .map_err(ipc_error)?;
        Ok(self.next_id)
    }

    /// Wait for the answer to request `id`. Each frame for it (an answer
    /// or a progress event) restarts the `stall` clock; other frames don't.
    async fn reply(
        &mut self,
        id: u64,
        stall: Duration,
        mut on_event: impl FnMut(Event),
    ) -> Result<Response, CliError> {
        let mut deadline = Instant::now() + stall;
        loop {
            let frame = tokio::time::timeout_at(deadline, self.framed.next())
                .await
                .map_err(|_| {
                    unavailable(format!(
                        "the daemon sent nothing for {} seconds",
                        stall.as_secs_f64().round()
                    ))
                })?
                .ok_or_else(|| mismatch("the daemon closed the connection without answering"))?;
            let message = frame.map_err(ipc_error)?;
            match message.payload {
                // Id 0 is the daemon rejecting a frame it couldn't read.
                Payload::Response(response) if message.id == id || message.id == 0 => {
                    return Ok(response);
                }
                Payload::Event(event) if message.id == id => {
                    deadline = Instant::now() + stall;
                    on_event(event);
                }
                // Anything else, including what a newer daemon adds, isn't
                // progress on this request.
                _ => {}
            }
        }
    }
}

fn into_data(response: Response) -> Result<ResponseData, CliError> {
    match response {
        Response::Ok { data } => Ok(data),
        Response::Error { error } => Err(error.into()),
        Response::Unknown => Err(mismatch(
            "the daemon sent an answer this version can't read",
        )),
    }
}

/// [`STALL_TIMEOUT`], or in debug builds `MS_TODO_REQUEST_TIMEOUT_MS`, so
/// tests can reach it without waiting five minutes.
fn stall_timeout() -> Duration {
    if cfg!(debug_assertions)
        && let Some(millis) = std::env::var(STALL_TIMEOUT_ENV)
            .ok()
            .and_then(|value| value.parse().ok())
    {
        return Duration::from_millis(millis);
    }
    STALL_TIMEOUT
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
    ask_with_events(paths, request, |_| {}).await
}

/// Send one request, starting the daemon first if needed, and pass the
/// events the daemon sends for it to `on_event`.
pub async fn ask_with_events(
    paths: &Paths,
    request: Request,
    on_event: impl FnMut(Event),
) -> Result<ResponseData, CliError> {
    let (mut client, _) = connect(paths).await?;
    client.request_with_events(request, on_event).await
}

/// Send one mutation carrying `op_id`. A lost answer after sending is
/// `outcome_unknown`; `check` says how to look before retrying.
pub async fn ask_mutation(
    paths: &Paths,
    request: Request,
    op_id: &str,
    check: &str,
) -> Result<ResponseData, CliError> {
    let (mut client, _) = connect(paths).await?;
    client.mutate(request, op_id, check).await
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
    let mut client = match DaemonClient::connect(&paths.socket_path()).await {
        Ok(client) => client,
        // Something holds the socket but never accepts: not missing.
        Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {
            return Probe::Incompatible(error.to_string());
        }
        // Missing, or a stale file nobody listens on. A new daemon removes
        // a stale socket itself, under its lock.
        Err(_) => return Probe::Unreachable,
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
    let mut launcher = spawn(paths)?;
    let started = wait_until_ready(paths, &mut launcher).await;
    // The launcher is the daemon's parent only while it relays an early
    // exit. Ending it hands the daemon to launchd or init, which reap it, so
    // this client never parents the daemon and can't leave it a zombie that
    // blocks `daemon stop` (D-046).
    let _ = launcher.kill();
    let _ = launcher.wait();
    started
}

async fn wait_until_ready(
    paths: &Paths,
    launcher: &mut Child,
) -> Result<(DaemonClient, DaemonStatus), CliError> {
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
        // The launcher exits with the daemon's status. Exit 0 means another
        // daemon already held the lock (a racing client started it), so
        // keep waiting for that one.
        if let Ok(Some(exit)) = launcher.try_wait()
            && !exit.success()
        {
            if exit.code() == Some(i32::from(EXIT_DATABASE_TOO_NEW)) {
                return Err(CliError::message(
                    ErrorKind::DatabaseTooNew,
                    format!(
                        "this database was upgraded by a newer ms-todo; install the latest \
                         version ({})",
                        paths.database_file().display()
                    ),
                ));
            }
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

/// Spawn `ms-todo daemon launch`, which starts the daemon detached from
/// this client and relays its exit if it dies during startup.
fn spawn(paths: &Paths) -> Result<Child, CliError> {
    let exe = std::env::current_exe()?;
    let log = open_log(&paths.daemon_log_file())?;
    std::process::Command::new(exe)
        .args(["daemon", "launch", "--instance", paths.instance.label()])
        // Don't hold the caller's directory open for the daemon's lifetime.
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .spawn()
        .map_err(|error| unavailable(format!("cannot start the daemon: {error}")))
}

/// `ms-todo daemon launch`: the middle of a double fork, without `unsafe`
/// (the workspace forbids it, so no `pre_exec`). It leaves the client's
/// session, so the daemon outlives the terminal's SIGHUP, then runs
/// `ms-todo daemon run` and exits with its status if it dies during
/// startup. The client ends this process once the daemon is ready, so the
/// daemon's parent becomes launchd or init, never a long-lived client such
/// as the TUI, whose unreaped zombie daemon once blocked an upgrade (D-046).
/// A launcher whose client died gives up after [`LAUNCH_TIMEOUT`].
pub(crate) fn launch(paths: &Paths) -> ExitCode {
    if let Err(error) = setsid() {
        eprintln!("ms-todo daemon launch: cannot start a new session: {error}");
        return ExitCode::FAILURE;
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("ms-todo daemon launch: cannot find this executable: {error}");
            return ExitCode::FAILURE;
        }
    };
    // Stdio and the working directory are inherited from the client's spawn.
    let mut daemon = match std::process::Command::new(exe)
        .args(["daemon", "run", "--instance", paths.instance.label()])
        .spawn()
    {
        Ok(daemon) => daemon,
        Err(error) => {
            eprintln!("ms-todo daemon launch: cannot start the daemon: {error}");
            return ExitCode::FAILURE;
        }
    };
    let deadline = std::time::Instant::now() + LAUNCH_TIMEOUT;
    while std::time::Instant::now() < deadline {
        match daemon.try_wait() {
            Ok(Some(exit)) => {
                // A daemon killed by a signal has no code; any failure will do.
                return exit
                    .code()
                    .and_then(|code| u8::try_from(code).ok())
                    .map_or(ExitCode::FAILURE, ExitCode::from);
            }
            Ok(None) => std::thread::sleep(POLL_INTERVAL),
            Err(error) => {
                eprintln!("ms-todo daemon launch: cannot watch the daemon: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
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
        let socket_gone = connect_socket(&paths.socket_path()).await.is_err();
        if socket_gone && !pid_alive(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Connect to the daemon's socket, giving up after `QUICK_TIMEOUT`. On
/// macOS a connect that races the daemon closing its socket can wait
/// forever (seen in `daemon stop`, docs/issues/003-flaky-task-writes-tests.md).
async fn connect_socket(socket: &Path) -> std::io::Result<UnixStream> {
    tokio::time::timeout(QUICK_TIMEOUT, UnixStream::connect(socket))
        .await
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "{} didn't accept a connection within {} seconds",
                    socket.display(),
                    QUICK_TIMEOUT.as_secs()
                ),
            )
        })?
}

fn pid_alive(pid: u32) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    // EPERM means it exists but isn't ours to signal.
    if matches!(kill(Pid::from_raw(raw), None), Err(Errno::ESRCH)) {
        return false;
    }
    // `kill(pid, 0)` succeeds for a zombie, but a zombie has already exited:
    // there's nothing left to wait for, and only its parent can reap it. A
    // client from before D-046 (such as an old TUI) parents the daemon it
    // started and never reaps it, so counting a zombie as alive would block
    // every `daemon stop`, and so every upgrade, until that client quits.
    !process_is_zombie(pid)
}

/// Linux has the state in procfs. The field after the command name, which
/// is in parentheses and may itself contain `)` or spaces.
#[cfg(target_os = "linux")]
fn process_is_zombie(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            let (_, after_name) = stat.rsplit_once(')')?;
            after_name.trim_start().chars().next()
        })
        == Some('Z')
}

/// macOS has no procfs, and `proc_pidinfo` needs `unsafe`, which the
/// workspace forbids; `ps` is mxr's probe. A `ps` that can't run says
/// "not a zombie", so the caller keeps waiting as before.
#[cfg(not(target_os = "linux"))]
fn process_is_zombie(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-o", "state=", "-p", &pid.to_string()])
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|output| {
            String::from_utf8_lossy(&output.stdout)
                .trim_start()
                .starts_with('Z')
        })
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
    fn a_zombie_counts_as_exited() {
        // A child that exits and is never reaped stays a zombie, which
        // `kill(pid, 0)` still reports: the old TUI's auto-started daemon.
        let child = std::process::Command::new("true")
            .spawn()
            .expect("spawn true");
        let pid = child.id();
        std::mem::forget(child);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !process_is_zombie(pid) {
            assert!(
                std::time::Instant::now() < deadline,
                "pid {pid} never became a zombie"
            );
            std::thread::sleep(POLL_INTERVAL);
        }
        let raw = i32::try_from(pid).expect("pid fits i32");
        assert!(
            kill(Pid::from_raw(raw), None).is_ok(),
            "the zombie must still answer kill(pid, 0), or this tests nothing"
        );
        assert!(!pid_alive(pid), "zombie pid {pid} counted as alive");
    }

    #[test]
    fn a_running_process_is_alive() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let alive = pid_alive(child.id());
        let _ = child.kill();
        let _ = child.wait();
        assert!(alive);
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
