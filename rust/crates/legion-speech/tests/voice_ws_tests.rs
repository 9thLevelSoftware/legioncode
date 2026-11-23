//! Tests for `legion_speech::voice_ws` (`BUG-10` `/ws/voice` timeout error-before-close framing,
//! buffer budgets, malformed/oversized frames, and `ASR` integration).

use legion_speech::types::TranscriptionOptions;
use legion_speech::voice_ws::{
    AudioSampleEncoding, CloseFrame, DEFAULT_MAX_SESSION_DURATION_MS, VOICE_WS_PATH, VirtualClock,
    VoiceClientFrame, VoiceErrorCode, VoiceOutboundFrame, VoiceServerEvent, VoiceWsConfig,
    VoiceWsSession, VoiceWsState,
};

fn assert_error_before_close(
    frames: &[VoiceOutboundFrame],
    expected_code: VoiceErrorCode,
    expected_close_code: u16,
) -> Result<(), String> {
    if frames.len() != 2 {
        return Err(format!("expected 2 frames (Error + Close), got {frames:?}"));
    }
    match &frames[0] {
        VoiceOutboundFrame::Event { event, json } => {
            let parsed: VoiceServerEvent =
                serde_json::from_str(json).map_err(|e| format!("invalid error JSON: {e}"))?;
            if &parsed != event {
                return Err("parsed event did not match emitted event".to_owned());
            }
            match event {
                VoiceServerEvent::Error { code, message } => {
                    if *code != expected_code || message.is_empty() {
                        return Err(format!("unexpected error event: {event:?}"));
                    }
                }
                other => return Err(format!("expected Error frame first, got {other:?}")),
            }
        }
        other @ VoiceOutboundFrame::Close(_) => {
            return Err(format!("expected Event frame first, got {other:?}"));
        }
    }
    match &frames[1] {
        VoiceOutboundFrame::Close(CloseFrame { code, reason }) => {
            if *code != expected_close_code || reason.is_empty() {
                return Err(format!(
                    "unexpected CloseFrame: code={code} reason={reason}"
                ));
            }
        }
        other @ VoiceOutboundFrame::Event { .. } => {
            return Err(format!("expected CloseFrame second, got {other:?}"));
        }
    }
    Ok(())
}

#[test]
fn bug10_ten_minute_timeout_always_emits_error_json_before_close_frame() -> Result<(), String> {
    assert_eq!(VOICE_WS_PATH, "/ws/voice");
    assert_eq!(DEFAULT_MAX_SESSION_DURATION_MS, 600_000);

    let clock = VirtualClock::new(1_000);
    let mut ws = VoiceWsSession::new(VoiceWsConfig::default(), clock.clone());
    assert_eq!(ws.state(), VoiceWsState::AwaitingStart);

    let ready = ws.handle_frame(VoiceClientFrame::Start {
        options: TranscriptionOptions::default(),
    });
    assert!(matches!(
        ready.as_slice(),
        [VoiceOutboundFrame::Event {
            event: VoiceServerEvent::Ready,
            ..
        }]
    ));
    assert_eq!(ws.state(), VoiceWsState::Streaming);

    // 1 ms before 10-minute deadline: still healthy.
    clock.advance_ms(DEFAULT_MAX_SESSION_DURATION_MS - 1);
    assert_eq!(ws.tick(), [] as [VoiceOutboundFrame; 0]);
    let pong = ws.handle_frame(VoiceClientFrame::Ping {
        nonce: Some("keepalive".to_owned()),
    });
    assert!(matches!(
        pong.as_slice(),
        [VoiceOutboundFrame::Event {
            event: VoiceServerEvent::Pong { nonce: Some(n) },
            ..
        }] if n == "keepalive"
    ));

    // At exact 600_000 ms: tick MUST emit Error JSON BEFORE CloseFrame and enter Closing.
    clock.advance_ms(1);
    let timed_out = ws.tick();
    assert_error_before_close(&timed_out, VoiceErrorCode::SessionTimeout, 1008)?;
    assert_eq!(ws.state(), VoiceWsState::Closing);
    assert_eq!(ws.buffered_audio(), [0_u8; 0]);

    // Subsequent tick or frames are ignored once Closing/Closed.
    assert_eq!(ws.tick(), [] as [VoiceOutboundFrame; 0]);
    assert_eq!(
        ws.handle_frame(VoiceClientFrame::AudioChunk(vec![0; 64])),
        [] as [VoiceOutboundFrame; 0]
    );
    ws.mark_closed();
    assert_eq!(ws.state(), VoiceWsState::Closed);

    // Also verify timeout detected directly during handle_frame (even before Start).
    let clock2 = VirtualClock::new(0);
    let mut ws2 = VoiceWsSession::new(VoiceWsConfig::default(), clock2.clone());
    clock2.advance_ms(DEFAULT_MAX_SESSION_DURATION_MS);
    let frames2 = ws2.handle_frame(VoiceClientFrame::Start {
        options: TranscriptionOptions::default(),
    });
    assert_error_before_close(&frames2, VoiceErrorCode::SessionTimeout, 1008)?;
    assert_eq!(ws2.state(), VoiceWsState::Closing);
    Ok(())
}

#[test]
fn buffer_overflow_and_chunk_size_limits_emit_error_before_close() -> Result<(), String> {
    let clock = VirtualClock::new(0);
    let config = VoiceWsConfig {
        max_chunk_bytes: 512,
        max_audio_buffer_bytes: 1_000,
        ..VoiceWsConfig::default()
    };
    let mut ws = VoiceWsSession::new(config.clone(), clock.clone());
    ws.handle_frame(VoiceClientFrame::Start {
        options: TranscriptionOptions::default(),
    });

    let u1 = ws.handle_frame(VoiceClientFrame::AudioChunk(vec![1_u8; 500]));
    assert!(matches!(
        u1.as_slice(),
        [VoiceOutboundFrame::Event {
            event: VoiceServerEvent::Update { revision: 1, .. },
            ..
        }]
    ));
    let u2 = ws.handle_frame(VoiceClientFrame::AudioChunk(vec![2_u8; 500]));
    assert!(matches!(
        u2.as_slice(),
        [VoiceOutboundFrame::Event {
            event: VoiceServerEvent::Update { revision: 2, .. },
            ..
        }]
    ));
    assert_eq!(ws.buffered_bytes(), 1_000);

    // 1 byte over 1_000 budget triggers BufferOverflow Error JSON BEFORE CloseFrame.
    let overflow = ws.handle_frame(VoiceClientFrame::AudioChunk(vec![3_u8; 1]));
    assert_error_before_close(&overflow, VoiceErrorCode::BufferOverflow, 1008)?;
    assert_eq!(ws.buffered_bytes(), 1_000);
    assert_eq!(ws.buffered_audio(), [0_u8; 0]);
    assert_eq!(ws.state(), VoiceWsState::Closing);

    // Single chunk exceeding max_chunk_bytes emits FrameTooLarge Error JSON BEFORE CloseFrame(1009).
    let mut ws_chunk = VoiceWsSession::new(config, clock);
    ws_chunk.handle_frame(VoiceClientFrame::Start {
        options: TranscriptionOptions::default(),
    });
    let too_big = ws_chunk.handle_frame(VoiceClientFrame::AudioChunk(vec![0_u8; 513]));
    assert_error_before_close(&too_big, VoiceErrorCode::FrameTooLarge, 1009)?;
    assert_eq!(ws_chunk.state(), VoiceWsState::Closing);
    Ok(())
}

#[test]
fn malformed_oversized_and_out_of_order_frames_emit_error_before_close() -> Result<(), String> {
    let clock = VirtualClock::new(0);
    let config = VoiceWsConfig {
        max_control_frame_bytes: 64,
        audio_encoding: Some(AudioSampleEncoding::F32Le),
        ..VoiceWsConfig::default()
    };

    // Malformed RawText JSON.
    let mut ws1 = VoiceWsSession::new(config.clone(), clock.clone());
    let bad_json = ws1.handle_frame(VoiceClientFrame::RawText("{not-json".to_owned()));
    assert_error_before_close(&bad_json, VoiceErrorCode::MalformedFrame, 1008)?;

    // Oversized control frame text.
    let mut ws2 = VoiceWsSession::new(config.clone(), clock.clone());
    let big_text = ws2.handle_frame(VoiceClientFrame::RawText("x".repeat(65)));
    assert_error_before_close(&big_text, VoiceErrorCode::FrameTooLarge, 1009)?;

    // Transport Oversized frame.
    let mut ws3 = VoiceWsSession::new(config.clone(), clock.clone());
    let oversized = ws3.handle_frame(VoiceClientFrame::Oversized { byte_len: 999_999 });
    assert_error_before_close(&oversized, VoiceErrorCode::FrameTooLarge, 1009)?;

    // AudioChunk before Start.
    let mut ws4 = VoiceWsSession::new(config.clone(), clock.clone());
    let early_audio = ws4.handle_frame(VoiceClientFrame::AudioChunk(vec![0_u8; 16]));
    assert_error_before_close(&early_audio, VoiceErrorCode::InvalidState, 1008)?;

    // Stop before Start.
    let mut ws5 = VoiceWsSession::new(config.clone(), clock.clone());
    let early_stop = ws5.handle_frame(VoiceClientFrame::Stop);
    assert_error_before_close(&early_stop, VoiceErrorCode::InvalidState, 1008)?;

    // Duplicate Start.
    let mut ws6 = VoiceWsSession::new(config.clone(), clock.clone());
    ws6.handle_frame(VoiceClientFrame::Start {
        options: TranscriptionOptions::default(),
    });
    let dup_start = ws6.handle_frame(VoiceClientFrame::Start {
        options: TranscriptionOptions::default(),
    });
    assert_error_before_close(&dup_start, VoiceErrorCode::InvalidState, 1008)?;

    // Invalid F32Le sample alignment / NaN.
    let mut ws7 = VoiceWsSession::new(config, clock);
    ws7.handle_frame(VoiceClientFrame::Start {
        options: TranscriptionOptions::default(),
    });
    let bad_f32 = ws7.handle_frame(VoiceClientFrame::AudioChunk(
        f32::NAN.to_le_bytes().to_vec(),
    ));
    assert_error_before_close(&bad_f32, VoiceErrorCode::InvalidAudio, 1008)?;
    Ok(())
}
