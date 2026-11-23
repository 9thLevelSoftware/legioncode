use std::collections::HashSet;

use legion_protocol::rpc_codec::{
    AckDisposition, ChunkDisposition, ClientRpcFrame, EVENT_OVERHEAD, MAX_PENDING_BYTES,
    MAX_PENDING_CHUNKS, ORCHESTRATION_PROTOCOL_VERSION, OutputWindow, OutputWindowRegistry,
    RegistryChunkAction, RpcCauseReason, RpcCodecError, RpcExit, RpcId, ServerRpcFrame,
    StreamWindowMode, chunk_bytes, decode_client_frames, decode_client_frames_bytes,
    decode_server_frames, encode_client_frames, encode_server_frame, encode_server_frames,
    queued_event_bytes, stream_window_mode, validate_orchestration_protocol,
};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn decodes_single_and_batched_client_envelopes() -> TestResult {
    let batch_json = r#"[
        {"_tag":"Request","id":"req-1","tag":"terminal.attach","payload":{"threadId":"t1"},"headers":[["x-req","1"]],"isNotification":true,"traceId":"tr-1","spanId":"sp-1","sampled":true},
        {"_tag":"Request","id":42,"tag":"server.ping"},
        {"_tag":"Ack","requestId":"req-1"},
        {"_tag":"Interrupt","requestId":42},
        {"_tag":"Ping"},
        {"_tag":"Eof"}
    ]"#;
    let decoded = decode_client_frames(batch_json)?;
    assert_eq!(decoded.len(), 6);
    assert!(matches!(
        &decoded[0],
        ClientRpcFrame::Request {
            id: RpcId::String(id),
            tag,
            is_notification: true,
            trace_id: Some(trace),
            ..
        } if id == "req-1" && tag == "terminal.attach" && trace == "tr-1"
    ));
    assert!(matches!(
        &decoded[1],
        ClientRpcFrame::Request {
            id: RpcId::Integer(42),
            tag,
            payload,
            headers,
            ..
        } if tag == "server.ping" && payload.is_null() && headers.is_empty()
    ));
    assert_eq!(
        decoded[2],
        ClientRpcFrame::Ack {
            request_id: RpcId::from("req-1")
        }
    );
    assert_eq!(
        decoded[3],
        ClientRpcFrame::Interrupt {
            request_id: RpcId::from(42)
        }
    );
    assert_eq!(decoded[4], ClientRpcFrame::Ping);
    assert_eq!(decoded[5], ClientRpcFrame::Eof);

    let reencoded = encode_client_frames(&decoded)?;
    assert_eq!(decode_client_frames_bytes(reencoded.as_bytes())?, decoded);

    let single = decode_client_frames(r#"{"_tag":"Ping"}"#)?;
    assert_eq!(single, vec![ClientRpcFrame::Ping]);
    assert_eq!(decode_client_frames("[]")?, Vec::<ClientRpcFrame>::new());
    assert!(matches!(
        decode_client_frames("123"),
        Err(RpcCodecError::UnexpectedJsonShape { kind: "number" })
    ));
    Ok(())
}

#[test]
fn decodes_and_encodes_all_server_envelopes() -> TestResult {
    let raw = r#"[
        {"_tag":"Chunk","requestId":"1","values":[{"type":"output","data":"hello"}]},
        {"_tag":"Exit","requestId":"1","exit":{"_tag":"Success"}},
        {"_tag":"Exit","requestId":2,"exit":{"_tag":"Failure","cause":[{"_tag":"Fail","error":{"code":"denied"}},{"_tag":"Die","defect":"boom"},{"_tag":"Interrupt","fiberId":null}]}},
        {"_tag":"Defect","defect":"internal"},
        {"_tag":"Pong"}
    ]"#;
    let frames = decode_server_frames(raw)?;
    assert_eq!(frames.len(), 5);
    assert_eq!(
        frames[1],
        ServerRpcFrame::Exit {
            request_id: RpcId::from("1"),
            exit: RpcExit::Success {
                value: serde_json::Value::Null
            },
        }
    );
    assert!(matches!(
        &frames[2],
        ServerRpcFrame::Exit {
            request_id: RpcId::Integer(2),
            exit: RpcExit::Failure { cause },
        } if cause.len() == 3 && matches!(cause[2], RpcCauseReason::Interrupt { fiber_id: None })
    ));
    let encoded_batch = encode_server_frames(&frames)?;
    assert_eq!(decode_server_frames(&encoded_batch)?, frames);
    let encoded_pong = encode_server_frame(&ServerRpcFrame::Pong)?;
    assert_eq!(
        decode_server_frames(&encoded_pong)?,
        vec![ServerRpcFrame::Pong]
    );
    assert!(validate_orchestration_protocol(Some(ORCHESTRATION_PROTOCOL_VERSION)).is_ok());
    assert!(validate_orchestration_protocol(Some("1")).is_err());
    Ok(())
}

#[test]
fn output_window_enforces_chunk_cap_and_error_byte_cap() {
    assert_eq!(
        stream_window_mode("terminal.attach"),
        StreamWindowMode::ByteAndChunk
    );
    assert_eq!(
        stream_window_mode("terminal.observe"),
        StreamWindowMode::ByteAndChunk
    );
    assert_eq!(
        stream_window_mode("subscribeServerConfig"),
        StreamWindowMode::ChunkCountOnly
    );
    assert_eq!(
        stream_window_mode("orchestration.subscribeThread"),
        StreamWindowMode::ChunkCountOnly
    );
    assert_eq!(
        stream_window_mode("orchestration.dispatchCommand"),
        StreamWindowMode::Unwindowed
    );

    // Count-windowed stream opens synthetic ack for chunks 1..7 and stops at chunk 8.
    let mut count_win = OutputWindow::new(StreamWindowMode::ChunkCountOnly);
    for idx in 1..MAX_PENDING_CHUNKS {
        let disp = count_win.on_send_chunk(&[json!({"seq": idx})]);
        assert_eq!(disp, ChunkDisposition::EmitSyntheticAck);
    }
    assert_eq!(
        count_win.on_send_chunk(&[json!({"seq": MAX_PENDING_CHUNKS})]),
        ChunkDisposition::AwaitClientAck
    );
    assert_eq!(count_win.pending_chunk_count(), MAX_PENDING_CHUNKS);
    assert_eq!(count_win.pending_bytes(), 0);
    assert_eq!(count_win.on_client_ack(), AckDisposition::ForwardAck);
    assert_eq!(count_win.on_client_ack(), AckDisposition::Swallow);

    // Error event payload (`message`, `error`, and `snapshot.error`) contributes to byte cap.
    let big_err = "x".repeat(MAX_PENDING_BYTES / 2);
    let err_event = json!({
        "type": "error",
        "threadId": "t1",
        "terminalId": "term-1",
        "message": "pty failed",
        "error": big_err,
        "snapshot": {
            "threadId": "t1",
            "terminalId": "term-1",
            "error": "snap-err"
        }
    });
    let expected_event_bytes = EVENT_OVERHEAD + 2 + 6 + 10 + (MAX_PENDING_BYTES / 2) + 2 + 6 + 8;
    assert_eq!(queued_event_bytes(&err_event), expected_event_bytes);

    let mut byte_win = OutputWindow::new(StreamWindowMode::ByteAndChunk);
    assert_eq!(
        byte_win.on_send_chunk(std::slice::from_ref(&err_event)),
        ChunkDisposition::EmitSyntheticAck
    );
    assert_eq!(
        byte_win.on_send_chunk(std::slice::from_ref(&err_event)),
        ChunkDisposition::AwaitClientAck
    );
    assert!(byte_win.pending_bytes() >= MAX_PENDING_BYTES);
    assert_eq!(byte_win.pending_bytes(), 2 * chunk_bytes(&[err_event]));

    let mut registry = OutputWindowRegistry::new();
    let live: HashSet<u64> = [10].into_iter().collect();
    registry.open_for_request(10, RpcId::from("s1"), "terminal.observe", &live);
    assert!(matches!(
        registry.on_send_chunk(
            10,
            &RpcId::from("s1"),
            &[json!({"type": "output", "data": "ok"})]
        ),
        RegistryChunkAction::Windowed(ChunkDisposition::EmitSyntheticAck)
    ));
    assert!(registry.on_send_exit(10, &RpcId::from("s1")).is_some());
    assert_eq!(registry.active_count(), 0);
}
