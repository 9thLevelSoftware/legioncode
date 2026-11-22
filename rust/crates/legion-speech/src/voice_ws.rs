//! `BUG-10` `/ws/voice` WebSocket stream protocol state machine with bounded timeout & buffer error framing.

use crate::asr::{AsrEngine, AsrTranscriberSession};
use crate::types::{TranscriptionOptions, decode_f32le_bytes, decode_pcm16le_bytes};
pub use crate::voice_ws_types::{
    AudioSampleEncoding, CloseFrame, DEFAULT_MAX_AUDIO_BUFFER_BYTES, DEFAULT_MAX_CHUNK_BYTES,
    DEFAULT_MAX_CONTROL_FRAME_BYTES, DEFAULT_MAX_SESSION_DURATION_MS, SystemClock, VOICE_WS_PATH,
    VirtualClock, VoiceClientCommand, VoiceClientFrame, VoiceClock, VoiceErrorCode,
    VoiceOutboundFrame, VoiceServerEvent, VoiceWsConfig, VoiceWsState,
};
use crate::voice_ws_types::{decode_raw_text_frame, event_frame};

enum ProtocolStep {
    Emitted(Vec<VoiceOutboundFrame>),
    StartAccepted(TranscriptionOptions),
    AudioAccepted(Vec<u8>),
    StopAccepted,
}

/// `/ws/voice` protocol state machine closing `BUG-10` silent timeout drops.
#[derive(Debug)]
pub struct VoiceWsSession<C: VoiceClock> {
    config: VoiceWsConfig,
    clock: C,
    opened_at_ms: u64,
    state: VoiceWsState,
    buffered_bytes: usize,
    buffered_audio: Vec<u8>,
    revision: u64,
}

impl<C: VoiceClock> VoiceWsSession<C> {
    /// Opens a new `/ws/voice` session anchored to `clock.now_ms()`.
    pub fn new(config: VoiceWsConfig, clock: C) -> Self {
        let opened_at_ms = clock.now_ms();
        Self {
            config,
            clock,
            opened_at_ms,
            state: VoiceWsState::AwaitingStart,
            buffered_bytes: 0,
            buffered_audio: Vec::new(),
            revision: 0,
        }
    }

    /// Returns the current [`VoiceWsState`].
    pub fn state(&self) -> VoiceWsState {
        self.state
    }

    /// Returns the opening timestamp in milliseconds captured from `clock`.
    pub fn opened_at_ms(&self) -> u64 {
        self.opened_at_ms
    }

    /// Returns the cumulative audio byte count accepted in this session.
    pub fn buffered_bytes(&self) -> usize {
        self.buffered_bytes
    }

    /// Returns the retained audio bytes when `config.retain_audio` is `true`.
    pub fn buffered_audio(&self) -> &[u8] {
        &self.buffered_audio
    }

    /// Returns the monotonic `Update` revision counter.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns `true` when the session is in `Closing` or `Closed`.
    pub fn is_terminal(&self) -> bool {
        matches!(self.state, VoiceWsState::Closing | VoiceWsState::Closed)
    }

    /// Marks the WebSocket transport close handshake complete (`Closing -> Closed`).
    pub fn mark_closed(&mut self) {
        if self.state == VoiceWsState::Closing {
            self.state = VoiceWsState::Closed;
        }
    }

    /// Checks whether the `10`-minute (`max_session_duration_ms`) deadline has elapsed.
    ///
    /// On timeout, **always** emits [`VoiceServerEvent::Error`] with
    /// [`VoiceErrorCode::SessionTimeout`] JSON BEFORE [`VoiceOutboundFrame::Close`] and
    /// transitions to [`VoiceWsState::Closing`].
    pub fn tick(&mut self) -> Vec<VoiceOutboundFrame> {
        if self.is_terminal() {
            return Vec::new();
        }
        let elapsed_ms = self.clock.now_ms().saturating_sub(self.opened_at_ms);
        if elapsed_ms >= self.config.max_session_duration_ms {
            return self.fail(
                VoiceErrorCode::SessionTimeout,
                format!(
                    "Voice session timed out after {} ms.",
                    self.config.max_session_duration_ms
                ),
                1008,
                "Voice session timed out",
            );
        }
        Vec::new()
    }

    /// Processes an inbound [`VoiceClientFrame`] in standalone protocol mode.
    pub fn handle_frame(&mut self, frame: VoiceClientFrame) -> Vec<VoiceOutboundFrame> {
        match self.step_protocol(frame) {
            ProtocolStep::Emitted(frames) => frames,
            ProtocolStep::StartAccepted(_) => vec![event_frame(VoiceServerEvent::Ready)],
            ProtocolStep::AudioAccepted(_) => {
                self.revision = self.revision.saturating_add(1);
                vec![event_frame(VoiceServerEvent::Update {
                    revision: self.revision,
                    text: None,
                })]
            }
            ProtocolStep::StopAccepted => self.finish_with_text(String::new()),
        }
    }

    /// Processes an inbound [`VoiceClientFrame`] wired end-to-end with [`AsrTranscriberSession`].
    pub fn handle_frame_with_asr<E: AsrEngine>(
        &mut self,
        frame: VoiceClientFrame,
        asr: &mut AsrTranscriberSession<E>,
    ) -> Vec<VoiceOutboundFrame> {
        match self.step_protocol(frame) {
            ProtocolStep::Emitted(frames) => frames,
            ProtocolStep::StartAccepted(options) => {
                if let Err(err) = asr.begin_stream(options) {
                    return self.fail(
                        VoiceErrorCode::InternalError,
                        err.to_string(),
                        1008,
                        "Speech stream failed",
                    );
                }
                vec![event_frame(VoiceServerEvent::Ready)]
            }
            ProtocolStep::AudioAccepted(bytes) => {
                let outcome = match self.config.audio_encoding {
                    Some(AudioSampleEncoding::F32Le) => match decode_f32le_bytes(&bytes) {
                        Ok(samples) => asr.feed_stream_f32(&samples),
                        Err(err) => {
                            return self.fail(
                                VoiceErrorCode::InvalidAudio,
                                err.to_string(),
                                1008,
                                "Invalid audio frame",
                            );
                        }
                    },
                    Some(AudioSampleEncoding::Pcm16Le) | None => asr.feed_stream_pcm16le(&bytes),
                };
                match outcome {
                    Ok(feed) => {
                        self.revision = feed.revision;
                        vec![event_frame(VoiceServerEvent::Update {
                            revision: feed.revision,
                            text: feed.preview,
                        })]
                    }
                    Err(err) => self.fail(
                        VoiceErrorCode::InternalError,
                        err.to_string(),
                        1008,
                        "Speech stream failed",
                    ),
                }
            }
            ProtocolStep::StopAccepted => match asr.finish_stream() {
                Ok(finished) => self.finish_with_text(finished.text),
                Err(err) => self.fail(
                    VoiceErrorCode::InternalError,
                    err.to_string(),
                    1008,
                    "Speech stream failed",
                ),
            },
        }
    }

    fn step_protocol(&mut self, frame: VoiceClientFrame) -> ProtocolStep {
        let timed_out = self.tick();
        if !timed_out.is_empty() || self.is_terminal() {
            return ProtocolStep::Emitted(timed_out);
        }

        let decoded = match frame {
            VoiceClientFrame::RawText(text) => {
                match decode_raw_text_frame(&text, self.config.max_control_frame_bytes) {
                    Ok(cmd) => cmd,
                    Err((code, msg, close_code, reason)) => {
                        return ProtocolStep::Emitted(self.fail(code, msg, close_code, reason));
                    }
                }
            }
            other => other,
        };

        match decoded {
            VoiceClientFrame::Ping { nonce } => {
                ProtocolStep::Emitted(vec![event_frame(VoiceServerEvent::Pong { nonce })])
            }
            VoiceClientFrame::Oversized { byte_len } => ProtocolStep::Emitted(self.fail(
                VoiceErrorCode::FrameTooLarge,
                format!("Inbound frame of {byte_len} bytes exceeds configured limit."),
                1009,
                "Frame too large",
            )),
            VoiceClientFrame::Malformed { reason } => ProtocolStep::Emitted(self.fail(
                VoiceErrorCode::MalformedFrame,
                reason,
                1008,
                "Malformed frame",
            )),
            VoiceClientFrame::Close => {
                self.buffered_audio.clear();
                self.state = VoiceWsState::Closing;
                ProtocolStep::Emitted(vec![VoiceOutboundFrame::Close(CloseFrame {
                    code: 1000,
                    reason: "Client closed speech stream".to_owned(),
                })])
            }
            VoiceClientFrame::Start { options } => {
                if self.state != VoiceWsState::AwaitingStart {
                    return ProtocolStep::Emitted(self.fail(
                        VoiceErrorCode::InvalidState,
                        "Speech stream already started.",
                        1008,
                        "Speech stream already started",
                    ));
                }
                self.state = VoiceWsState::Streaming;
                ProtocolStep::StartAccepted(options)
            }
            VoiceClientFrame::AudioChunk(bytes) => self.handle_audio_chunk(bytes),
            VoiceClientFrame::Stop => {
                if self.state != VoiceWsState::Streaming {
                    return ProtocolStep::Emitted(self.fail(
                        VoiceErrorCode::InvalidState,
                        "Start speech transcription before stopping.",
                        1008,
                        "Speech stream not started",
                    ));
                }
                ProtocolStep::StopAccepted
            }
            VoiceClientFrame::RawText(_) => ProtocolStep::Emitted(Vec::new()),
        }
    }

    fn handle_audio_chunk(&mut self, bytes: Vec<u8>) -> ProtocolStep {
        if self.state != VoiceWsState::Streaming {
            return ProtocolStep::Emitted(self.fail(
                VoiceErrorCode::InvalidState,
                "Start speech transcription before sending audio.",
                1008,
                "Speech stream not started",
            ));
        }
        if bytes.len() > self.config.max_chunk_bytes {
            return ProtocolStep::Emitted(self.fail(
                VoiceErrorCode::FrameTooLarge,
                format!(
                    "Speech audio chunk of {} bytes exceeds {} byte limit.",
                    bytes.len(),
                    self.config.max_chunk_bytes
                ),
                1009,
                "Speech audio chunk too large",
            ));
        }
        if bytes.is_empty() {
            return ProtocolStep::Emitted(self.fail(
                VoiceErrorCode::InvalidAudio,
                "Speech audio chunk must not be empty.",
                1008,
                "Invalid audio frame",
            ));
        }
        if let Some(encoding) = self.config.audio_encoding {
            let valid = match encoding {
                AudioSampleEncoding::Pcm16Le => decode_pcm16le_bytes(&bytes).map(|_| ()),
                AudioSampleEncoding::F32Le => decode_f32le_bytes(&bytes).map(|_| ()),
            };
            if let Err(err) = valid {
                return ProtocolStep::Emitted(self.fail(
                    VoiceErrorCode::InvalidAudio,
                    err.to_string(),
                    1008,
                    "Invalid audio frame",
                ));
            }
        }
        let next_total = self.buffered_bytes.saturating_add(bytes.len());
        if next_total > self.config.max_audio_buffer_bytes {
            return ProtocolStep::Emitted(self.fail(
                VoiceErrorCode::BufferOverflow,
                format!(
                    "Audio buffer budget exceeded ({} > {} bytes).",
                    next_total, self.config.max_audio_buffer_bytes
                ),
                1008,
                "Speech audio buffer limit exceeded",
            ));
        }
        self.buffered_bytes = next_total;
        if self.config.retain_audio {
            self.buffered_audio.extend_from_slice(&bytes);
        }
        ProtocolStep::AudioAccepted(bytes)
    }

    fn finish_with_text(&mut self, text: String) -> Vec<VoiceOutboundFrame> {
        self.buffered_audio.clear();
        self.state = VoiceWsState::Closing;
        vec![
            event_frame(VoiceServerEvent::Finished { text }),
            VoiceOutboundFrame::Close(CloseFrame {
                code: 1000,
                reason: "Speech stream finished".to_owned(),
            }),
        ]
    }

    fn fail(
        &mut self,
        code: VoiceErrorCode,
        message: impl Into<String>,
        close_code: u16,
        close_reason: &str,
    ) -> Vec<VoiceOutboundFrame> {
        self.buffered_audio.clear();
        self.state = VoiceWsState::Closing;
        vec![
            event_frame(VoiceServerEvent::Error {
                code,
                message: message.into(),
            }),
            VoiceOutboundFrame::Close(CloseFrame {
                code: close_code,
                reason: close_reason.to_owned(),
            }),
        ]
    }
}
