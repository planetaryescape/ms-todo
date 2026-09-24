//! A mutation whose answer is lost after the CLI sent it: the daemon may
//! still apply it, so the CLI reports `outcome_unknown` with the `op_id` it
//! chose, never a plain `daemon_unavailable` that invites a retry. Uses a
//! fake daemon that answers `Status` and then goes quiet or hangs up.

mod support;

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use ms_todo_protocol::{
    DaemonStatus, Message, PROTOCOL_VERSION, Payload, Request, Response, ResponseData,
};
use serde_json::Value;
use support::Env;

#[derive(Clone, Copy)]
enum AfterRequest {
    /// Keep the connection open and never answer.
    Hang,
    /// Close the connection, as a daemon that died would.
    HangUp,
}

fn read_frame(stream: &mut UnixStream) -> Option<Message> {
    let mut length = [0; 4];
    stream.read_exact(&mut length).ok()?;
    let mut frame = vec![0; u32::from_be_bytes(length) as usize];
    stream.read_exact(&mut frame).ok()?;
    serde_json::from_slice(&frame).ok()
}

fn write_frame(stream: &mut UnixStream, message: &Message) {
    let json = serde_json::to_vec(message).expect("encode");
    let length = u32::try_from(json.len()).expect("small frame");
    stream
        .write_all(&length.to_be_bytes())
        .expect("write length");
    stream.write_all(&json).expect("write frame");
}

/// Serve one connection on `socket`: answer `Status` as a compatible daemon,
/// pass on the next request, then act as `after` says. The thread holds a
/// hung connection until the test drops `release`.
fn fake_daemon(socket: &Path, after: AfterRequest) -> (mpsc::Receiver<Request>, mpsc::Sender<()>) {
    std::fs::create_dir_all(socket.parent().expect("run dir")).expect("mkdir");
    let listener = UnixListener::bind(socket).expect("bind");
    let (requests, received) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        while let Some(message) = read_frame(&mut stream) {
            let Payload::Request(request) = message.payload else {
                continue;
            };
            if request == Request::Status {
                let status = DaemonStatus {
                    protocol_version: PROTOCOL_VERSION,
                    version: env!("CARGO_PKG_VERSION").into(),
                    pid: std::process::id(),
                    instance: "dev".into(),
                    started_at: 0,
                    signed_in: true,
                };
                let answer = Message {
                    id: message.id,
                    payload: Payload::Response(Response::Ok {
                        data: ResponseData::Status(status),
                    }),
                };
                write_frame(&mut stream, &answer);
                continue;
            }
            let _ = requests.send(request);
            match after {
                AfterRequest::Hang => {
                    let _ = released.recv_timeout(Duration::from_secs(30));
                }
                AfterRequest::HangUp => {}
            }
            return;
        }
    });
    (received, release)
}

fn run(env: &Env, args: &[&str]) -> (Value, i32) {
    let output = env
        .cmd()
        .env("MS_TODO_REQUEST_TIMEOUT_MS", "500")
        .args(["--format", "json"])
        .args(args)
        .output()
        .expect("run ms-todo");
    assert!(output.stdout.is_empty(), "nothing on stdout");
    let error = serde_json::from_slice(&output.stderr).expect("json error");
    (error, output.status.code().expect("exit code"))
}

fn sent_op_id(request: &Request) -> Option<&str> {
    match request {
        Request::AddTask { op_id, .. }
        | Request::ChangeTasks { op_id, .. }
        | Request::RawWrite { op_id, .. } => op_id.as_deref(),
        _ => None,
    }
}

#[test]
fn a_sent_mutation_whose_answer_never_comes_is_outcome_unknown_with_its_op_id() {
    for (after, args) in [
        (AfterRequest::Hang, &["tasks", "add", "Buy milk"][..]),
        (AfterRequest::HangUp, &["tasks", "complete", "T1"][..]),
        (
            AfterRequest::Hang,
            &["raw", "DELETE", "/me/todo/lists/L/tasks/T1", "--yes"][..],
        ),
    ] {
        let env = Env::new();
        let socket = env.socket();
        let (received, _release) = fake_daemon(&socket, after);

        let (error, code) = run(&env, args);

        let request = received
            .recv_timeout(Duration::from_secs(10))
            .expect("the daemon got the request");
        let op_id = sent_op_id(&request).expect("the CLI sent an op_id");
        assert_eq!(code, 1, "{args:?}: {error}");
        assert_eq!(error["error"]["kind"], "outcome_unknown", "{args:?}");
        assert_eq!(error["error"]["op_id"], op_id, "{args:?}");
        let message = error["error"]["message"].as_str().expect("message");
        assert!(message.contains("ms-todo"), "{message}");
        let _ = std::fs::remove_file(&socket);
    }
}

#[test]
fn a_dry_run_whose_answer_never_comes_is_just_daemon_unavailable() {
    let env = Env::new();
    let socket = env.socket();
    let (received, _release) = fake_daemon(&socket, AfterRequest::Hang);

    let (error, code) = run(&env, &["tasks", "add", "Buy milk", "--dry-run"]);

    let request = received
        .recv_timeout(Duration::from_secs(10))
        .expect("the daemon got the request");
    assert_eq!(sent_op_id(&request), None, "a dry run has no op_id");
    assert_eq!(code, 1);
    assert_eq!(error["error"]["kind"], "daemon_unavailable");
    assert!(error["error"].get("op_id").is_none());
    let _ = std::fs::remove_file(&socket);
}
