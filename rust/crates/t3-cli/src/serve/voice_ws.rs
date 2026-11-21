//! WebSocket `/ws/voice` live transcription handler enforcing `BUG-10` error-before-close framing.

use axum::extract::ws::{CloseFrame as AxumCloseFrame, Message, WebSocket};
use axum::extract::{FromRequestParts as _, Request, WebSocketUpgrade};
use axum::http::Uri;
use axum::response::{IntoResponse as _, Response};
use t3_speech::acoustic_model::AcousticAsrEngine;
use t3_speech::asr::AsrTranscriberSession;
use t3_speech::vad::VadConfig;
use t3_speech::voice_ws::{
    AudioSampleEncoding, DEFAULT_MAX_AUDIO_BUFFER_BYTES, DEFAULT_MAX_CHUNK_BYTES,
    DEFAULT_MAX_CONTROL_FRAME_BYTES, DEFAULT_MAX_SESSION_DURATION_MS, SystemClock,
    VoiceClientFrame, VoiceOutboundFrame, VoiceWsConfig, VoiceWsSession,
};

use crate::cmd_speech_worker::build_acoustic_engine;

/// Handles `GET /ws/voice`, upgrading to WebSocket driven by `VoiceWsSession` (`BUG-10`).
pub(crate) async fn handle_voice_ws(uri: Uri, req: Request) -> Response {
    let config = config_from_query(uri.query());
    let (mut parts, _body) = req.into_parts();
    let ws = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(upgrade) => upgrade,
        Err(rejection) => return rejection.into_response(),
    };
    ws.on_upgrade(move |socket| run_voice_ws_session(socket, config))
}

fn config_from_query(query: Option<&str>) -> VoiceWsConfig {
    let mut cfg = VoiceWsConfig::default();
    if let Some(ms) = parse_query_u64(query, "maxSessionDurationMs") {
        cfg.max_session_duration_ms = ms.clamp(1, DEFAULT_MAX_SESSION_DURATION_MS);
    }
    if let Some(bytes) = parse_query_usize(query, "maxAudioBufferBytes") {
        cfg.max_audio_buffer_bytes = bytes.clamp(1, DEFAULT_MAX_AUDIO_BUFFER_BYTES);
    }
    if let Some(bytes) = parse_query_usize(query, "maxChunkBytes") {
        cfg.max_chunk_bytes = bytes.clamp(1, DEFAULT_MAX_CHUNK_BYTES);
    }
    if let Some(bytes) = parse_query_usize(query, "maxControlFrameBytes") {
        cfg.max_control_frame_bytes = bytes.clamp(1, DEFAULT_MAX_CONTROL_FRAME_BYTES);
    }
    match query_value(query, "encoding").as_deref() {
        Some("pcm16_le") => cfg.audio_encoding = Some(AudioSampleEncoding::Pcm16Le),
        Some("f32_le") => cfg.audio_encoding = Some(AudioSampleEncoding::F32Le),
        _ => {}
    }
    cfg
}

fn query_value(query: Option<&str>, key: &str) -> Option<String> {
    let q = query?;
    for pair in q.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == key {
            return Some(v.to_owned());
        }
    }
    None
}

fn parse_query_u64(query: Option<&str>, key: &str) -> Option<u64> {
    query_value(query, key)?.trim().parse::<u64>().ok()
}

fn parse_query_usize(query: Option<&str>, key: &str) -> Option<usize> {
    query_value(query, key)?.trim().parse::<usize>().ok()
}

async fn run_voice_ws_session(mut socket: WebSocket, config: VoiceWsConfig) {
    let duration = tokio::time::Duration::from_millis(config.max_session_duration_ms);
    let mut session = VoiceWsSession::new(config.clone(), SystemClock);
    let Ok(mut asr) = AsrTranscriberSession::new(build_acoustic_engine(), VadConfig::default())
    else {
        let _ = socket
            .send(Message::Text(
                r#"{"type":"error","code":"internal_error","message":"Failed to initialize speech VAD."}"#
                    .into(),
            ))
            .await;
        let _ = socket
            .send(Message::Close(Some(AxumCloseFrame {
                code: 1008,
                reason: "Speech initialization failed".into(),
            })))
            .await;
        return;
    };

    let deadline = tokio::time::Instant::now() + duration;
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            () = tokio::time::sleep_until(*deadline) => {
                let frames = session.tick();
                let _ = send_outbound(&mut socket, &mut session, frames).await;
                break;
            }
            maybe_msg = socket.recv() => {
                let Some(Ok(msg)) = maybe_msg else {
                    break;
                };
                if !handle_incoming_message(&mut socket, &mut session, &mut asr, &config, msg).await {
                    break;
                }
                let timed_out = session.tick();
                if !timed_out.is_empty() {
                    let _ = send_outbound(&mut socket, &mut session, timed_out).await;
                    break;
                }
            }
        }
    }
}

async fn handle_incoming_message(
    socket: &mut WebSocket,
    session: &mut VoiceWsSession<SystemClock>,
    asr: &mut AsrTranscriberSession<AcousticAsrEngine>,
    config: &VoiceWsConfig,
    msg: Message,
) -> bool {
    let frame = match msg {
        Message::Text(text) => {
            if text.len() > config.max_control_frame_bytes {
                VoiceClientFrame::Oversized {
                    byte_len: text.len(),
                }
            } else {
                VoiceClientFrame::RawText(text.to_string())
            }
        }
        Message::Binary(bytes) => VoiceClientFrame::AudioChunk(bytes.to_vec()),
        Message::Ping(bytes) => {
            return socket.send(Message::Pong(bytes)).await.is_ok();
        }
        Message::Pong(_) => return true,
        Message::Close(_) => {
            let frames = session.handle_frame_with_asr(VoiceClientFrame::Close, asr);
            let _ = send_outbound(socket, session, frames).await;
            return false;
        }
    };

    let outbound = session.handle_frame_with_asr(frame, asr);
    send_outbound(socket, session, outbound).await
}

async fn send_outbound(
    socket: &mut WebSocket,
    session: &mut VoiceWsSession<SystemClock>,
    frames: Vec<VoiceOutboundFrame>,
) -> bool {
    for frame in frames {
        match frame {
            VoiceOutboundFrame::Event { json, .. } => {
                if socket.send(Message::Text(json.into())).await.is_err() {
                    return false;
                }
            }
            VoiceOutboundFrame::Close(close) => {
                let _ = socket
                    .send(Message::Close(Some(AxumCloseFrame {
                        code: close.code,
                        reason: close.reason.into(),
                    })))
                    .await;
                session.mark_closed();
                return false;
            }
        }
    }
    true
}
