use bytes::BytesMut;
use ms_todo_protocol::{
    Applied, Candidate, Clearable, Codec, DaemonStatus, DoctorReport, ErrorPayload, Event,
    Importance, Message, NewTask, PROTOCOL_VERSION, Payload, Plan, PlannedTask, RawWriteMethod,
    Request, Response, ResponseData, Rolled, ScopeError, ScopeStatus, SyncInfo, SyncProgress,
    SyncReport, SyncState, TaskAction, TaskChange, TaskEdit,
};
use serde_json::json;
use tokio_util::codec::{Decoder, Encoder};

fn round_trip(message: Message) -> Message {
    let mut codec = Codec::new();
    let mut buffer = BytesMut::new();
    codec.encode(message, &mut buffer).expect("encode");
    let decoded = codec
        .decode(&mut buffer)
        .expect("decode")
        .expect("a whole frame");
    assert!(buffer.is_empty(), "the frame was not fully consumed");
    decoded
}

fn decode_json(value: serde_json::Value) -> Message {
    let bytes = serde_json::to_vec(&value).expect("json");
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&u32::try_from(bytes.len()).expect("len").to_be_bytes());
    buffer.extend_from_slice(&bytes);
    Codec::new()
        .decode(&mut buffer)
        .expect("decode")
        .expect("a whole frame")
}

#[test]
fn every_request_and_response_round_trips() {
    let entity =
        json!({ "id": "AAMk=", "displayName": "Tasks", "wellknownListName": "defaultList" });
    let serde_json::Value::Object(entity) = entity else {
        unreachable!("literal object")
    };
    let payloads = [
        Payload::Request(Request::Status),
        Payload::Request(Request::ListLists),
        Payload::Request(Request::ListTasks { list: None }),
        Payload::Request(Request::ListTasks {
            list: Some("Groceries".into()),
        }),
        Payload::Request(Request::RawGet { path: "/me".into() }),
        Payload::Request(Request::Bearer),
        Payload::Request(Request::Shutdown),
        Payload::Response(Response::Ok {
            data: ResponseData::Status(DaemonStatus {
                protocol_version: PROTOCOL_VERSION,
                version: "0.1.0".into(),
                pid: 42,
                instance: "dev".into(),
                started_at: 1_790_000_000,
                signed_in: true,
            }),
        }),
        Payload::Response(Response::Ok {
            data: ResponseData::Lists {
                items: vec![entity.clone()],
                sync: SyncInfo {
                    state: SyncState::Initial,
                    generation: 0,
                },
            },
        }),
        Payload::Response(Response::Ok {
            data: ResponseData::Tasks {
                items: vec![entity.clone()],
                sync: SyncInfo {
                    state: SyncState::Ready,
                    generation: 3,
                },
            },
        }),
        Payload::Request(Request::Sync { wait: true }),
        Payload::Request(Request::Doctor),
        Payload::Event(Event::SyncProgress(SyncProgress {
            scopes_done: 2,
            scopes_total: 31,
            doing: "tasks in Groceries".into(),
        })),
        Payload::Response(Response::Ok {
            data: ResponseData::Sync(SyncReport {
                waited: true,
                scopes: 31,
                changed: 2,
                generation: 4,
            }),
        }),
        Payload::Response(Response::Ok {
            data: ResponseData::Doctor(DoctorReport {
                database_path: "/tmp/ms-todo.db".into(),
                database_bytes: 4096,
                syncing: false,
                scopes: vec![ScopeStatus {
                    scope: "tasks:AAMk=".into(),
                    list_id: Some("5b9c".into()),
                    list_name: Some("Tasks".into()),
                    state: SyncState::Ready,
                    generation: 1,
                    in_progress: false,
                    last_success_at: Some(1_790_000_000),
                    last_changed_count: 0,
                    last_error: Some(ScopeError {
                        kind: "network".into(),
                        message: "timed out".into(),
                        at: None,
                    }),
                }],
            }),
        }),
        Payload::Response(Response::Ok {
            data: ResponseData::Raw {
                body: json!({ "a": [1, 2] }),
            },
        }),
        Payload::Response(Response::Ok {
            data: ResponseData::Ack,
        }),
        Payload::Response(Response::Error {
            error: ErrorPayload {
                kind: "invalid_input".into(),
                message: "two lists are called Groceries".into(),
                graph_code: None,
                request_id: None,
                candidates: vec![Candidate {
                    id: "a".into(),
                    name: "Groceries".into(),
                }],
                op_id: None,
                applied: Vec::new(),
            },
        }),
        Payload::Request(Request::RawWrite {
            method: RawWriteMethod::Patch,
            path: "/me/todo/lists/L".into(),
            body: Some(json!({ "displayName": "x" })),
            op_id: Some("op-raw".into()),
        }),
        Payload::Request(Request::AddTask {
            task: NewTask {
                title: "Buy milk".into(),
                list: None,
                due: Some("2026-09-26".into()),
                reminder: None,
                importance: Some(Importance::High),
                body: None,
            },
            dry_run: true,
            op_id: None,
            idempotency_key: Some("k1".into()),
        }),
        Payload::Request(Request::ChangeTasks {
            tasks: vec!["T1".into(), "T2".into()],
            list: Some("Groceries".into()),
            change: TaskChange::Edit(TaskEdit {
                title: Some("New".into()),
                due: Some(Clearable::Clear),
                reminder: Some(Clearable::Set("2026-09-26T09:00".into())),
                ..TaskEdit::default()
            }),
            dry_run: false,
            op_id: Some("op-edit".into()),
            idempotency_key: None,
        }),
        Payload::Request(Request::ChangeTasks {
            tasks: vec!["T1".into()],
            list: None,
            change: TaskChange::Complete,
            dry_run: false,
            op_id: None,
            idempotency_key: None,
        }),
        Payload::Response(Response::Ok {
            data: ResponseData::Plan(Plan {
                action: TaskAction::Delete,
                list: None,
                targets: vec![PlannedTask {
                    id: "T1".into(),
                    title: "Buy milk".into(),
                    list_id: "L".into(),
                }],
                changes: serde_json::Value::Null,
            }),
        }),
        Payload::Response(Response::Ok {
            data: ResponseData::Applied(Applied {
                op_id: "op-1".into(),
                action: TaskAction::Complete,
                items: vec![entity.clone()],
                list_ids: vec!["L".into()],
                rolled: vec![Rolled {
                    id: "T1".into(),
                    next_due: "2026-09-27".into(),
                }],
            }),
        }),
        Payload::Response(Response::Error {
            error: ErrorPayload {
                kind: "outcome_unknown".into(),
                message: "may or may not".into(),
                graph_code: None,
                request_id: Some("r".into()),
                candidates: Vec::new(),
                op_id: Some("op-1".into()),
                applied: vec!["T1".into()],
            },
        }),
    ];
    for (id, payload) in payloads.into_iter().enumerate() {
        let message = Message {
            id: id as u64,
            payload,
        };
        assert_eq!(round_trip(message.clone()), message);
    }
}

#[test]
fn a_frame_split_across_reads_waits_for_the_rest() {
    let mut codec = Codec::new();
    let mut whole = BytesMut::new();
    let message = Message {
        id: 7,
        payload: Payload::Request(Request::Status),
    };
    codec.encode(message.clone(), &mut whole).expect("encode");
    let mut partial = whole.split_to(whole.len() - 3);
    assert!(codec.decode(&mut partial).expect("decode").is_none());
    partial.unsplit(whole);
    assert_eq!(codec.decode(&mut partial).expect("decode"), Some(message));
}

#[test]
fn an_incoming_frame_over_the_cap_is_rejected() {
    let mut codec = Codec::with_max_frame(64);
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&1_000_u32.to_be_bytes());
    buffer.extend_from_slice(&[b' '; 100]);
    let error = codec.decode(&mut buffer).expect_err("over the cap");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn an_outgoing_message_over_the_cap_is_refused_with_its_size() {
    let mut codec = Codec::with_max_frame(64);
    let mut buffer = BytesMut::new();
    let message = Message {
        id: 1,
        payload: Payload::Request(Request::RawGet {
            path: "x".repeat(100),
        }),
    };
    let error = codec
        .encode(message, &mut buffer)
        .expect_err("over the cap");
    assert!(
        error.to_string().contains("64 byte IPC frame cap"),
        "{error}"
    );
    assert!(
        buffer.is_empty(),
        "nothing may be written for a refused frame"
    );
}

#[test]
fn the_default_cap_is_explicit() {
    assert_eq!(ms_todo_protocol::MAX_FRAME_BYTES, 16 * 1024 * 1024);
}

#[test]
fn unknown_tags_decode_to_unknown() {
    let payload = decode_json(json!({ "id": 1, "payload": { "type": "stream_chunk" } }));
    assert_eq!(payload.payload, Payload::Unknown);

    let request = decode_json(
        json!({ "id": 2, "payload": { "type": "request", "cmd": "tasks_add", "title": "x" } }),
    );
    assert_eq!(request.payload, Payload::Request(Request::Unknown));

    let data = decode_json(json!({
        "id": 3,
        "payload": { "type": "response", "status": "ok", "data": { "kind": "sync_report", "n": 1 } }
    }));
    assert_eq!(
        data.payload,
        Payload::Response(Response::Ok {
            data: ResponseData::Unknown
        })
    );

    let event = decode_json(
        json!({ "id": 0, "payload": { "type": "event", "event": "entity_changed", "ids": [] } }),
    );
    assert_eq!(event.payload, Payload::Event(Event::Unknown));
}

#[test]
fn fields_from_a_newer_peer_are_ignored_and_missing_new_fields_default() {
    let status = decode_json(json!({
        "id": 4,
        "payload": {
            "type": "response",
            "status": "ok",
            "data": {
                "kind": "status",
                "protocol_version": 1,
                "version": "9.9.9",
                "pid": 1,
                "instance": "default",
                "started_at": 0,
                "sync": { "state": "ready" }
            }
        }
    }));
    let Payload::Response(Response::Ok {
        data: ResponseData::Status(status),
    }) = status.payload
    else {
        unreachable!("decoded {:?}", status.payload)
    };
    assert!(!status.signed_in);
    assert_eq!(status.version, "9.9.9");

    let tasks =
        decode_json(json!({ "id": 5, "payload": { "type": "request", "cmd": "list_tasks" } }));
    assert_eq!(
        tasks.payload,
        Payload::Request(Request::ListTasks { list: None })
    );
}

#[test]
fn malformed_json_is_invalid_data() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&3_u32.to_be_bytes());
    buffer.extend_from_slice(b"{x}");
    let error = Codec::new().decode(&mut buffer).expect_err("bad json");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}
