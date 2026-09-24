use bytes::BytesMut;
use ms_todo_protocol::{
    Codec, DaemonStatus, ErrorPayload, Event, ListRef, Message, PROTOCOL_VERSION, Payload, Request,
    Response, ResponseData,
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
            },
        }),
        Payload::Response(Response::Ok {
            data: ResponseData::Tasks {
                items: vec![entity],
            },
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
                candidates: vec![ListRef {
                    id: "a".into(),
                    name: "Groceries".into(),
                }],
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
