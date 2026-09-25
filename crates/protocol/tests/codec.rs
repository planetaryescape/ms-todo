use bytes::BytesMut;
use ms_todo_protocol::{
    Anchor, Applied, Candidate, Clearable, Codec, Counts, DaemonStatus, DoctorReport,
    EntityChanged, ErrorPayload, Event, Folder, Importance, ListChange, ListSuggestion, Message,
    MyDay, MyDaySeed, MyDayStatus, NewTask, OpError, OutboxDepth, OutboxOp, OutboxState,
    PROTOCOL_VERSION, Payload, Plan, PlannedList, PlannedTask, RawWriteMethod, Refused, Request,
    Response, ResponseData, Rolled, Scope, ScopeError, ScopeStatus, SearchStatus, Seed,
    SuggestStatus, SyncActivity, SyncInfo, SyncMode, SyncProgress, SyncReport, SyncState,
    TaskAction, TaskChange, TaskEdit, TaskSelect, WriteRejected,
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
        Payload::Request(Request::ListTasks {
            list: None,
            search: None,
        }),
        Payload::Request(Request::ListTasks {
            list: Some("Groceries".into()),
            search: Some("milk".into()),
        }),
        Payload::Request(Request::SearchTasks {
            query: "insur* OR \"car tax\"".into(),
            list: Some("Home".into()),
            status: SearchStatus::All,
            limit: Some(10),
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
        Payload::Response(Response::Ok {
            data: ResponseData::SearchResults {
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
                    mode: SyncMode::Delta,
                    last_delta_at: Some(1_790_000_020),
                }],
                outbox: OutboxDepth {
                    pending: 1,
                    unknown: 2,
                    flagged: 1,
                    ..OutboxDepth::default()
                },
                suggest: Some(SuggestStatus {
                    enabled: true,
                    provider: Some("typesafe".into()),
                    problem: Some("TypeSafe answered 500".into()),
                }),
                my_day: Some(MyDayStatus {
                    date: "2026-09-25".into(),
                    count: 2,
                    rollover_time: "04:00".into(),
                    last_rollover: Some("2026-09-25".into()),
                    problem: None,
                }),
            }),
        }),
        Payload::Request(Request::MyDay),
        Payload::Request(Request::MyDayRollover {
            dry_run: true,
            op_id: None,
        }),
        Payload::Response(Response::Ok {
            data: ResponseData::MyDay(MyDay {
                date: "2026-09-25".into(),
                tasks: vec![
                    json!({ "id": "t1", "title": "Call the bank" })
                        .as_object()
                        .cloned()
                        .expect("object"),
                ],
                suggestions: Vec::new(),
                sync: SyncInfo {
                    state: SyncState::Ready,
                    generation: 3,
                },
                last_rollover: None,
            }),
        }),
        Payload::Request(Request::ChangeTasks {
            tasks: vec!["t1".into()],
            list: None,
            select: None,
            change: TaskChange::AddToMyDay,
            dry_run: false,
            op_id: Some("op-my-day".into()),
            idempotency_key: None,
        }),
        Payload::Request(Request::SuggestList {
            title: "pay council tax".into(),
        }),
        Payload::Response(Response::Ok {
            data: ResponseData::ListSuggestion {
                suggestion: Some(ListSuggestion {
                    list_id: "5b9c".into(),
                    list_name: "Finances".into(),
                    confidence: 0.86,
                }),
            },
        }),
        Payload::Response(Response::Ok {
            data: ResponseData::ListSuggestion { suggestion: None },
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
                    ..Candidate::default()
                }],
                op_id: None,
                applied: Vec::new(),
                undo_target: None,
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
                start: Some("2026-09-25".into()),
                recurrence: Some(json!({
                    "pattern": { "type": "daily", "interval": 1 },
                    "range": { "type": "noEnd", "startDate": "2026-09-26" }
                })),
                categories: vec!["Errands".into()],
                my_day: true,
            },
            dry_run: true,
            op_id: None,
            idempotency_key: Some("k1".into()),
        }),
        Payload::Request(Request::ChangeTasks {
            tasks: vec!["T1".into(), "T2".into()],
            list: Some("Groceries".into()),
            select: None,
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
            select: None,
            change: TaskChange::Complete,
            dry_run: false,
            op_id: None,
            idempotency_key: None,
        }),
        Payload::Request(Request::ChangeTasks {
            tasks: Vec::new(),
            list: None,
            select: Some(TaskSelect {
                due_before: "2026-09-25".into(),
                folder: Some("Areas".into()),
            }),
            change: TaskChange::Edit(TaskEdit {
                due: Some(Clearable::Set("2026-09-26".into())),
                ..TaskEdit::default()
            }),
            dry_run: true,
            op_id: None,
            idempotency_key: None,
        }),
        Payload::Request(Request::CompletedTasks {
            since: "2026-09-21".into(),
            until: Some("2026-09-24".into()),
            list: None,
            folder: Some("Areas".into()),
            limit: Some(20),
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
                lists: Vec::new(),
                changes: serde_json::Value::Null,
            }),
        }),
        Payload::Request(Request::ChangeLists {
            change: ListChange::MoveList {
                lists: vec!["Finances".into(), "Health".into()],
                folder: Some("Areas".into()),
            },
            dry_run: false,
            op_id: Some("op-move".into()),
            idempotency_key: Some("k2".into()),
        }),
        Payload::Request(Request::ChangeLists {
            change: ListChange::OrderFolder {
                folder: "Areas".into(),
                anchor: Anchor::Before("Projects".into()),
            },
            dry_run: true,
            op_id: None,
            idempotency_key: None,
        }),
        Payload::Request(Request::ListFolders),
        Payload::Response(Response::Ok {
            data: ResponseData::Folders {
                items: vec![Folder {
                    name: "Areas".into(),
                    lists: vec!["l1".into()],
                    open_count: 4,
                }],
                sync: SyncInfo {
                    state: SyncState::Ready,
                    generation: 2,
                },
            },
        }),
        Payload::Response(Response::Ok {
            data: ResponseData::Plan(Plan {
                action: TaskAction::RenameFolder,
                list: None,
                targets: Vec::new(),
                lists: vec![PlannedList {
                    id: "l1".into(),
                    name: "Finances".into(),
                    changes: json!({ "folder": "Responsibilities" }),
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
                undoes: None,
                refused: vec![Refused {
                    id: "T2".into(),
                    title: "Call Sam".into(),
                    reason: "has changed since (dueDateTime)".into(),
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
                undo_target: Some("op-0".into()),
            },
        }),
        Payload::Request(Request::Undo {
            target: Some("op-1".into()),
            copy: Some("T-copy".into()),
            op_id: Some("op-2".into()),
            idempotency_key: None,
        }),
        Payload::Request(Request::OutboxList {
            state: Some(OutboxState::Unknown),
        }),
        Payload::Response(Response::Ok {
            data: ResponseData::Outbox {
                items: vec![OutboxOp {
                    op_id: "op-1".into(),
                    command_id: "op-1".into(),
                    action: "add".into(),
                    task_id: "t1".into(),
                    list_id: "l1".into(),
                    title: Some("Buy milk".into()),
                    state: OutboxState::Failed,
                    attempts: 1,
                    created_at: 1_790_000_000,
                    next_attempt_at: None,
                    sent_at: Some(1_790_000_001),
                    unknown_since: None,
                    depends_on: None,
                    undoes: None,
                    last_error: Some(OpError {
                        kind: "rejected".into(),
                        message: "the list was deleted".into(),
                    }),
                    note: None,
                    flagged: false,
                    changes: json!({ "title": "Buy milk" }),
                }],
            },
        }),
        Payload::Event(Event::WriteRejected(WriteRejected {
            op_id: "op-1".into(),
            task_id: "t1".into(),
            error: OpError {
                kind: "rejected".into(),
                message: "no".into(),
            },
        })),
        Payload::Request(Request::Seed {
            scope: None,
            search: None,
        }),
        Payload::Request(Request::Seed {
            scope: Some(Scope::List { id: "l1".into() }),
            search: Some("milk*".into()),
        }),
        Payload::Request(Request::Seed {
            scope: Some(Scope::Planned),
            search: None,
        }),
        Payload::Request(Request::Subscribe),
        Payload::Response(Response::Ok {
            data: ResponseData::Seed(Seed {
                scope: Some(Scope::Important),
                lists: vec![entity.clone()],
                lists_sync: SyncInfo {
                    state: SyncState::Ready,
                    generation: 2,
                },
                counts: Counts {
                    my_day: 5,
                    important: 1,
                    planned: 2,
                    all: 3,
                    completed: 4,
                    lists: [("l1".to_owned(), 3)].into_iter().collect(),
                },
                tasks: vec![entity.clone()],
                sync: SyncInfo {
                    state: SyncState::Initial,
                    generation: 0,
                },
                activity: SyncActivity {
                    generation: 7,
                    in_progress: true,
                    last_finished_at: Some(1_790_000_000),
                    last_error: None,
                },
                outbox: OutboxDepth {
                    pending: 1,
                    ..OutboxDepth::default()
                },
                my_day: Some(MyDaySeed {
                    date: "2026-09-25".into(),
                    suggestions: vec![entity.clone()],
                }),
            }),
        }),
        Payload::Event(Event::EntityChanged(EntityChanged {
            lists: vec!["l1".into()],
            tasks: vec!["t1".into(), "t2".into()],
        })),
        Payload::Event(Event::ResyncNeeded),
        Payload::Event(Event::SyncState(SyncActivity {
            generation: 8,
            in_progress: false,
            last_finished_at: Some(1_790_000_020),
            last_error: Some(OpError {
                kind: "network".into(),
                message: "offline".into(),
            }),
        })),
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
        json!({ "id": 0, "payload": { "type": "event", "event": "duplicate_detected", "ids": [] } }),
    );
    assert_eq!(event.payload, Payload::Event(Event::Unknown));

    // A smart view from a newer client.
    let seed = decode_json(json!({
        "id": 0,
        "payload": { "type": "request", "cmd": "seed", "scope": { "view": "assigned" } }
    }));
    assert_eq!(
        seed.payload,
        Payload::Request(Request::Seed {
            scope: Some(Scope::Unknown),
            search: None
        })
    );
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
        Payload::Request(Request::ListTasks {
            list: None,
            search: None
        })
    );
    // A search's filters default to open tasks, every list, no limit.
    let search = decode_json(
        json!({ "id": 6, "payload": { "type": "request", "cmd": "search_tasks", "query": "milk" } }),
    );
    assert_eq!(
        search.payload,
        Payload::Request(Request::SearchTasks {
            query: "milk".into(),
            list: None,
            status: SearchStatus::Open,
            limit: None
        })
    );

    // A rung 3a daemon's scope has no mode; a newer one's may be unknown.
    let scope = |mode: Option<&str>| {
        let mut scope = json!({
            "scope": "lists", "state": "ready", "generation": 1, "in_progress": false,
            "last_success_at": null, "last_changed_count": 0, "last_error": null
        });
        if let Some(mode) = mode {
            scope["mode"] = json!(mode);
        }
        serde_json::from_value::<ScopeStatus>(scope).expect("scope")
    };
    assert_eq!(scope(None).mode, SyncMode::Enumeration);
    assert_eq!(scope(None).last_delta_at, None);
    assert_eq!(scope(Some("delta")).mode, SyncMode::Delta);
    assert_eq!(scope(Some("webhook")).mode, SyncMode::Unknown);
}

#[test]
fn malformed_json_is_invalid_data() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&3_u32.to_be_bytes());
    buffer.extend_from_slice(b"{x}");
    let error = Codec::new().decode(&mut buffer).expect_err("bad json");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}
