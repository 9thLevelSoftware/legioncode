//! Typed `t3 __speech-worker` `NDJSON` command/event protocol codec and stdio loop re-exports.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::base64::{Base64Error, decode_base64, encode_base64};
use crate::types::{
    PcmError, TranscriptionOptions, decode_f32le_bytes, decode_pcm16le_bytes, pcm16_to_f32,
    validate_f32_samples,
};
use crate::vad::VadFrameDecision;

pub use crate::worker_stdio::{
    WorkerExitReason, WorkerIoError, run_worker_stdio, run_worker_stdio_with_max_line,
};

/// Default maximum `NDJSON` line length accepted by the speech worker (`32 MiB`).
pub const DEFAULT_MAX_WORKER_LINE_BYTES: usize = 32 * 1024 * 1024;

/// Audio payload formats accepted by [`WorkerCommand::AudioChunk`] and [`WorkerCommand::Transcribe`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "encoding", rename_all = "snake_case")]
pub enum WorkerAudioPayload {
    /// Base64-encoded little-endian signed 16-bit mono `PCM` bytes.
    Pcm16leBase64 {
        /// Standard `RFC 4648` Base64 string representing `PCM16LE` bytes.
        data: String,
    },
    /// Base64-encoded little-endian `f32` mono `PCM` bytes in `[-1.0, 1.0]`.
    F32leBase64 {
        /// Standard `RFC 4648` Base64 string representing `F32LE` bytes.
        data: String,
    },
    /// Inline signed 16-bit integer `PCM` samples.
    Pcm16 {
        /// Mono `16 kHz` `i16` samples.
        samples: Vec<i16>,
    },
    /// Inline normalized `f32` `PCM` samples in `[-1.0, 1.0]`.
    F32 {
        /// Mono `16 kHz` `f32` samples.
        samples: Vec<f32>,
    },
}

/// Errors returned when decoding [`WorkerAudioPayload`] into normalized `f32` samples.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum WorkerAudioDecodeError {
    /// Base64 decoding failed.
    #[error(transparent)]
    Base64(#[from] Base64Error),
    /// `PCM` byte alignment or normalized sample bounds validation failed.
    #[error(transparent)]
    Pcm(#[from] PcmError),
}

impl WorkerAudioPayload {
    /// Encodes raw `PCM16LE` bytes into [`WorkerAudioPayload::Pcm16leBase64`].
    pub fn from_pcm16le_bytes(bytes: &[u8]) -> Self {
        Self::Pcm16leBase64 {
            data: encode_base64(bytes),
        }
    }

    /// Encodes raw `F32LE` bytes into [`WorkerAudioPayload::F32leBase64`].
    pub fn from_f32le_bytes(bytes: &[u8]) -> Self {
        Self::F32leBase64 {
            data: encode_base64(bytes),
        }
    }

    /// Wraps signed 16-bit integer samples in [`WorkerAudioPayload::Pcm16`].
    pub fn from_pcm16(samples: Vec<i16>) -> Self {
        Self::Pcm16 { samples }
    }

    /// Wraps normalized `f32` samples in [`WorkerAudioPayload::F32`].
    pub fn from_f32(samples: Vec<f32>) -> Self {
        Self::F32 { samples }
    }

    /// Decodes and validates the payload into normalized `16 kHz` `f32` samples in `[-1.0, 1.0]`.
    pub fn decode_to_f32(&self) -> Result<Vec<f32>, WorkerAudioDecodeError> {
        match self {
            Self::Pcm16leBase64 { data } => {
                let raw = decode_base64(data)?;
                Ok(decode_pcm16le_bytes(&raw)?)
            }
            Self::F32leBase64 { data } => {
                let raw = decode_base64(data)?;
                Ok(decode_f32le_bytes(&raw)?)
            }
            Self::Pcm16 { samples } => Ok(samples.iter().copied().map(pcm16_to_f32).collect()),
            Self::F32 { samples } => {
                validate_f32_samples(samples)?;
                Ok(samples.clone())
            }
        }
    }
}

/// Inbound `NDJSON` commands read by `t3 __speech-worker`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerCommand {
    /// Configures recognition options and optional live streaming mode.
    Config {
        /// Default recognition options for subsequent streaming or offline passes.
        #[serde(default)]
        options: TranscriptionOptions,
        /// When `true`, immediately starts a live `ASR` stream after resetting.
        #[serde(default)]
        streaming: bool,
        /// Optional local `ASR` model path metadata.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_path: Option<String>,
        /// Optional `Silero` `VAD` model path metadata.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vad_path: Option<String>,
    },
    /// Appends a streaming `PCM` audio chunk (and optionally finalizes the stream).
    AudioChunk {
        /// Encoded audio samples.
        audio: WorkerAudioPayload,
        /// Marks the end of the live stream when `true`.
        #[serde(default)]
        final_chunk: bool,
    },
    /// Performs a one-shot offline transcription pass (`BUG-10` reset-isolated on error/retry).
    Transcribe {
        /// Complete recording audio payload.
        audio: WorkerAudioPayload,
        /// Per-command recognition overrides; falls back to configured defaults when `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        options: Option<TranscriptionOptions>,
    },
    /// Explicitly invokes `reset_asr()` and rebuilds `VAD` state.
    Reset,
    /// Resets resources, emits [`WorkerEvent::Stopped`], and terminates the worker loop.
    Shutdown,
}

/// Machine-readable error categories emitted in [`WorkerEvent::Error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerErrorCode {
    /// Inbound `NDJSON` line could not be parsed as UTF-8 or valid [`WorkerCommand`] JSON.
    InvalidCommand,
    /// Inbound `NDJSON` line exceeded the configured byte ceiling.
    FrameTooLarge,
    /// Audio payload decoding or `[-1.0, 1.0]` / byte-alignment validation failed.
    InvalidAudio,
    /// Command is invalid for the current `AsrSessionState`.
    InvalidState,
    /// `ASR` engine reported an error (triggers unconditional `reset_asr()` + `VAD` rebuild).
    AsrFailure,
    /// `VAD` configuration or lifecycle error occurred.
    VadFailure,
}

/// Outbound `NDJSON` events emitted by `t3 __speech-worker`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerEvent {
    /// Worker is configured/reset and ready under `generation_id`.
    Ready {
        /// Current `VAD` generation identifier.
        generation_id: u64,
        /// Expected sample rate in Hz (`16_000`).
        sample_rate_hz: u32,
        /// Expected frame length in samples (`512`).
        frame_samples: usize,
    },
    /// Frame-level `VAD` decision produced while processing incoming audio.
    Vad {
        /// `VAD` generation identifier that produced this decision.
        generation_id: u64,
        /// Acoustic energy, zero-crossing rate, probability, and endpoint transition.
        decision: VadFrameDecision,
    },
    /// Partial or finalized transcription text.
    Transcript {
        /// `VAD` generation identifier associated with this transcript.
        generation_id: u64,
        /// Combined transcript text.
        text: String,
        /// Committed transcript prefix.
        #[serde(default)]
        committed: String,
        /// Tentative streaming suffix (empty when `is_final` is `true`).
        #[serde(default)]
        tentative: String,
        /// Whether this transcript finalizes the stream or offline transcription.
        is_final: bool,
    },
    /// Structured error report indicating whether `ASR` + `VAD` reset executed.
    Error {
        /// Machine-readable error code.
        code: WorkerErrorCode,
        /// Human-readable error message.
        message: String,
        /// Active `VAD` generation identifier after any error-triggered reset/rebuild.
        generation_id: u64,
        /// `true` when `reset_asr()` and `VAD` rebuild were executed for this error.
        reset_performed: bool,
    },
    /// Terminal event emitted upon [`WorkerCommand::Shutdown`].
    Stopped {
        /// Shutdown reason string.
        reason: String,
        /// Final `VAD` generation identifier after teardown reset.
        generation_id: u64,
    },
}

/// `NDJSON` line encoding/decoding errors for worker commands and events.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WorkerProtocolError {
    /// Line exceeded `max_line_bytes`.
    #[error("worker NDJSON line of {observed_bytes} bytes exceeds max {max_bytes} bytes")]
    LineTooLarge {
        /// Configured ceiling.
        max_bytes: usize,
        /// Observed line length.
        observed_bytes: usize,
    },
    /// JSON serialization or deserialization failed.
    #[error("worker NDJSON JSON error: {message}")]
    Json {
        /// Underlying parser/serializer error message.
        message: String,
    },
}

/// Encodes a [`WorkerCommand`] as a single `\n`-terminated `NDJSON` line.
pub fn encode_worker_command(command: &WorkerCommand) -> Result<String, WorkerProtocolError> {
    encode_ndjson_line(command, DEFAULT_MAX_WORKER_LINE_BYTES)
}

/// Decodes a single `NDJSON` command line (ignoring trailing `\r`/`\n`).
pub fn decode_worker_command(line: &str) -> Result<WorkerCommand, WorkerProtocolError> {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    serde_json::from_str(trimmed).map_err(|err| WorkerProtocolError::Json {
        message: err.to_string(),
    })
}

/// Encodes a [`WorkerEvent`] as a single `\n`-terminated `NDJSON` line.
pub fn encode_worker_event(event: &WorkerEvent) -> Result<String, WorkerProtocolError> {
    encode_ndjson_line(event, DEFAULT_MAX_WORKER_LINE_BYTES)
}

/// Decodes a single `NDJSON` event line (ignoring trailing `\r`/`\n`).
pub fn decode_worker_event(line: &str) -> Result<WorkerEvent, WorkerProtocolError> {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    serde_json::from_str(trimmed).map_err(|err| WorkerProtocolError::Json {
        message: err.to_string(),
    })
}

pub(crate) fn encode_ndjson_line<T: Serialize>(
    value: &T,
    max_line_bytes: usize,
) -> Result<String, WorkerProtocolError> {
    let mut encoded = serde_json::to_string(value).map_err(|err| WorkerProtocolError::Json {
        message: err.to_string(),
    })?;
    if encoded.len() > max_line_bytes {
        return Err(WorkerProtocolError::LineTooLarge {
            max_bytes: max_line_bytes,
            observed_bytes: encoded.len(),
        });
    }
    encoded.push('\n');
    Ok(encoded)
}
