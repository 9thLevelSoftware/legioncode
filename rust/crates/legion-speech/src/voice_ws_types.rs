//! Types, clock abstractions, and wire frames for the `/ws/voice` WebSocket protocol.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::types::{SPEECH_SAMPLE_RATE_HZ, TranscriptPreview, TranscriptionOptions};

/// WebSocket route path for live voice transcription.
pub const VOICE_WS_PATH: &str = "/ws/voice";

/// Default maximum `/ws/voice` session lifetime (`10` minutes = `600_000 ms`).
pub const DEFAULT_MAX_SESSION_DURATION_MS: u64 = 600_000;

/// Default `/ws/voice` inactivity deadline (`15` seconds = `15_000 ms`, `#41075`) so a
/// dropped client behind a proxy releases the speech engine quickly.
pub const DEFAULT_IDLE_TIMEOUT_MS: u64 = 15_000;

/// Default maximum single binary audio chunk size (`16_000 * 4 = 64_000` bytes).
pub const DEFAULT_MAX_CHUNK_BYTES: usize = (SPEECH_SAMPLE_RATE_HZ as usize) * 4;

/// Default maximum JSON control frame size (`128_000` bytes).
pub const DEFAULT_MAX_CONTROL_FRAME_BYTES: usize = 128_000;

/// Default maximum cumulative audio buffer budget (`5` minutes of `16 kHz` `f32` `PCM`).
pub const DEFAULT_MAX_AUDIO_BUFFER_BYTES: usize = (SPEECH_SAMPLE_RATE_HZ as usize) * 4 * 5 * 60;

/// Monotonic millisecond clock abstraction enabling deterministic `/ws/voice` timeout tests.
pub trait VoiceClock {
    /// Returns the current clock time in milliseconds.
    fn now_ms(&self) -> u64;
}

/// Thread-safe injectable virtual clock for deterministic timeout testing.
#[derive(Debug, Clone)]
pub struct VirtualClock {
    now_ms: Arc<AtomicU64>,
}

impl Default for VirtualClock {
    fn default() -> Self {
        Self::new(0)
    }
}

impl VirtualClock {
    /// Creates a [`VirtualClock`] initialized at `initial_ms`.
    pub fn new(initial_ms: u64) -> Self {
        Self {
            now_ms: Arc::new(AtomicU64::new(initial_ms)),
        }
    }

    /// Advances the virtual clock by `delta_ms`.
    pub fn advance_ms(&self, delta_ms: u64) {
        self.now_ms.fetch_add(delta_ms, Ordering::Relaxed);
    }

    /// Sets the virtual clock to `now_ms`.
    pub fn set_ms(&self, now_ms: u64) {
        self.now_ms.store(now_ms, Ordering::Relaxed);
    }
}

impl VoiceClock for VirtualClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::Relaxed)
    }
}

/// Wall-clock [`VoiceClock`] backed by `std::time::SystemTime`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl VoiceClock for SystemClock {
    fn now_ms(&self) -> u64 {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
    }
}

/// Optional wire `PCM` sample encoding validated on incoming `AudioChunk` frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioSampleEncoding {
    /// Little-endian signed 16-bit mono `PCM` (`2` bytes per sample).
    Pcm16Le,
    /// Little-endian 32-bit float mono `PCM` in `[-1.0, 1.0]` (`4` bytes per sample).
    F32Le,
}

/// Configuration limits for [`crate::voice_ws::VoiceWsSession`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceWsConfig {
    /// Maximum connection lifetime in milliseconds before forced timeout (`600_000 ms` = `10` min).
    pub max_session_duration_ms: u64,
    /// Maximum permitted byte length of a single binary `AudioChunk`.
    pub max_chunk_bytes: usize,
    /// Maximum permitted byte length of a single JSON control text frame.
    pub max_control_frame_bytes: usize,
    /// Maximum cumulative buffered audio byte budget per session.
    pub max_audio_buffer_bytes: usize,
    /// Optional `PCM` encoding check applied to incoming `AudioChunk` frames.
    pub audio_encoding: Option<AudioSampleEncoding>,
    /// Whether accepted audio bytes are retained inside `VoiceWsSession` (`false` saves memory).
    pub retain_audio: bool,
    /// Optional client inactivity timeout in milliseconds once received frames are answered.
    pub idle_timeout_ms: Option<u64>,
}

impl Default for VoiceWsConfig {
    fn default() -> Self {
        Self {
            max_session_duration_ms: DEFAULT_MAX_SESSION_DURATION_MS,
            max_chunk_bytes: DEFAULT_MAX_CHUNK_BYTES,
            max_control_frame_bytes: DEFAULT_MAX_CONTROL_FRAME_BYTES,
            max_audio_buffer_bytes: DEFAULT_MAX_AUDIO_BUFFER_BYTES,
            audio_encoding: None,
            retain_audio: true,
            idle_timeout_ms: None,
        }
    }
}

impl VoiceWsConfig {
    /// Returns a copy of this configuration enforcing an inactivity timeout in milliseconds.
    #[must_use]
    pub fn with_idle_timeout_ms(mut self, idle_timeout_ms: u64) -> Self {
        self.idle_timeout_ms = Some(idle_timeout_ms);
        self
    }
}

/// Lifecycle state of [`crate::voice_ws::VoiceWsSession`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceWsState {
    /// Socket open and awaiting the initial `Start` control frame.
    AwaitingStart,
    /// Stream active and accepting `AudioChunk`, `Ping`, and `Stop` frames.
    Streaming,
    /// Terminal event + `CloseFrame` have been emitted; awaiting transport close handshake.
    Closing,
    /// Transport socket closed.
    Closed,
}

/// Structured error codes emitted in [`VoiceServerEvent::Error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceErrorCode {
    /// Session exceeded `max_session_duration_ms` (`BUG-10` explicit error before close).
    SessionTimeout,
    /// Cumulative incoming audio exceeded `max_audio_buffer_bytes`.
    BufferOverflow,
    /// A single control or audio frame exceeded configured size bounds.
    FrameTooLarge,
    /// Text or binary frame could not be decoded according to protocol framing.
    MalformedFrame,
    /// Client sent a frame out of order for the current [`VoiceWsState`].
    InvalidState,
    /// Binary `AudioChunk` failed sample alignment, emptiness, or `[-1.0, 1.0]` checks.
    InvalidAudio,
    /// Underlying `ASR` or `VAD` pipeline failed during streaming execution.
    InternalError,
}

/// JSON control commands accepted on `/ws/voice` text frames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VoiceClientCommand {
    /// Begins live speech streaming with the supplied transcription options.
    Start {
        /// Recognition options (`speechSilenceFiltering` defaults to `true`).
        #[serde(default)]
        options: TranscriptionOptions,
    },
    /// Finishes the active stream and requests the final transcript (`"stop"` or legacy `"finish"`).
    #[serde(alias = "finish")]
    Stop,
    /// Lightweight application-level keep-alive probe.
    Ping {
        /// Optional opaque client correlation token echoed back in `Pong`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nonce: Option<String>,
    },
}

/// Inbound WebSocket frame events modeled by [`crate::voice_ws::VoiceWsSession`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceClientFrame {
    /// Parsed `Start` control frame.
    Start {
        /// Transcription preferences for this session.
        options: TranscriptionOptions,
    },
    /// Raw binary `PCM` audio chunk.
    AudioChunk(Vec<u8>),
    /// Parsed `Stop` (or legacy `"finish"`) control frame.
    Stop,
    /// Parsed application or transport `Ping`.
    Ping {
        /// Optional correlation nonce echoed back in `Pong`.
        nonce: Option<String>,
    },
    /// Unparsed WebSocket UTF-8 text frame to validate and decode into [`VoiceClientCommand`].
    RawText(String),
    /// Frame rejected at transport layer for exceeding frame limits.
    Oversized {
        /// Observed frame byte size.
        byte_len: usize,
    },
    /// Frame rejected at transport layer as malformed.
    Malformed {
        /// Human-readable parse failure reason.
        reason: String,
    },
    /// Client-initiated WebSocket close frame.
    Close,
}

/// JSON server events emitted on `/ws/voice` text frames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VoiceServerEvent {
    /// Stream initialized and ready to receive `AudioChunk` frames.
    Ready,
    /// Reply to a client `Ping`.
    Pong {
        /// Echoed correlation nonce when supplied by the client.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nonce: Option<String>,
    },
    /// Incremental transcription acknowledgment / preview update.
    Update {
        /// Monotonic update revision counter.
        revision: u64,
        /// Optional committed/tentative preview text.
        text: Option<TranscriptPreview>,
    },
    /// Stream finalized with the full transcript.
    Finished {
        /// Final transcript text.
        text: String,
    },
    /// Terminal error emitted BEFORE [`CloseFrame`] (`BUG-10`).
    Error {
        /// Machine-readable protocol/timeout/overflow error code.
        code: VoiceErrorCode,
        /// Human-readable error description.
        message: String,
    },
}

/// WebSocket close frame metadata emitted after terminal [`VoiceServerEvent`] messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseFrame {
    /// RFC 6455 close status code (`1000` normal, `1008` policy/timeout/protocol, `1009` too big).
    pub code: u16,
    /// Short UTF-8 close reason string.
    pub reason: String,
}

/// Ordered outbound WebSocket frames produced by [`crate::voice_ws::VoiceWsSession`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceOutboundFrame {
    /// Text frame carrying a serialized [`VoiceServerEvent`] JSON payload.
    Event {
        /// Typed server event for direct inspection.
        event: VoiceServerEvent,
        /// Exact JSON wire string ready to write to the WebSocket.
        json: String,
    },
    /// WebSocket `CloseFrame` emitted AFTER terminal `Finished` or `Error` events.
    Close(CloseFrame),
}

pub(crate) fn event_frame(event: VoiceServerEvent) -> VoiceOutboundFrame {
    let json = serde_json::to_string(&event).unwrap_or_else(|_| {
        r#"{"type":"error","code":"internal_error","message":"failed to serialize voice event"}"#
            .to_owned()
    });
    VoiceOutboundFrame::Event { event, json }
}

pub(crate) fn decode_raw_text_frame(
    text: &str,
    max_control_frame_bytes: usize,
) -> Result<VoiceClientFrame, (VoiceErrorCode, String, u16, &'static str)> {
    if text.len() > max_control_frame_bytes {
        return Err((
            VoiceErrorCode::FrameTooLarge,
            format!(
                "Control frame of {} bytes exceeds {max_control_frame_bytes} byte limit.",
                text.len()
            ),
            1009,
            "Control frame too large",
        ));
    }
    Ok(match serde_json::from_str::<VoiceClientCommand>(text) {
        Ok(VoiceClientCommand::Start { options }) => VoiceClientFrame::Start { options },
        Ok(VoiceClientCommand::Stop) => VoiceClientFrame::Stop,
        Ok(VoiceClientCommand::Ping { nonce }) => VoiceClientFrame::Ping { nonce },
        Err(err) => VoiceClientFrame::Malformed {
            reason: format!("Invalid voice control JSON: {err}"),
        },
    })
}
