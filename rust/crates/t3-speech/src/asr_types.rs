//! `ASR` session states, error types, pluggable engine trait, and output structs.

use std::fmt;

use thiserror::Error;

use crate::types::{TranscriptPreview, TranscriptionOptions};
use crate::vad::{VadError, VadFrameDecision};

/// Explicit lifecycle states for [`crate::asr::AsrTranscriberSession`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsrSessionState {
    /// Ready to start either a streaming session or a one-shot offline transcription.
    Idle,
    /// Actively ingesting streaming audio chunks between `begin_stream` and `finish_stream`.
    Streaming,
    /// Executing a one-shot offline `transcribe` pass.
    Transcribing,
    /// An operation failed; `reset_asr()` and `VAD` rebuild have already executed (`BUG-10`).
    Faulted,
}

impl fmt::Display for AsrSessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Idle => f.write_str("idle"),
            Self::Streaming => f.write_str("streaming"),
            Self::Transcribing => f.write_str("transcribing"),
            Self::Faulted => f.write_str("faulted"),
        }
    }
}

/// Errors raised by [`crate::asr::AsrTranscriberSession`].
#[derive(Debug, Clone, PartialEq, Error)]
pub enum AsrError {
    /// `VAD` configuration, framing, or lifecycle error.
    #[error(transparent)]
    Vad(#[from] VadError),
    /// Operation is not permitted in the current [`AsrSessionState`].
    #[error("cannot perform `{action}` while ASR session is in `{current}` state")]
    InvalidState {
        /// Current state of the session.
        current: AsrSessionState,
        /// Attempted operation name.
        action: &'static str,
    },
    /// Underlying `ASR` engine failed during stream setup, feeding, finishing, or transcription.
    #[error("ASR engine error: {message}")]
    Engine {
        /// Engine-reported diagnostic message.
        message: String,
    },
}

/// Pluggable speech-recognition engine driven by [`crate::asr::AsrTranscriberSession`].
///
/// Implementations represent either a native worker (`t3-speech-ffi`) or a deterministic mock.
pub trait AsrEngine {
    /// Engine-specific failure type formatted into [`AsrError::Engine`].
    type Error: fmt::Display;

    /// Initializes a live streaming recognition pass with `options`.
    fn begin_stream(&mut self, options: &TranscriptionOptions) -> Result<(), Self::Error>;

    /// Feeds normalized `16 kHz` mono `f32` `PCM` samples into the active stream.
    fn feed_stream(&mut self, pcm: &[f32]) -> Result<Option<TranscriptPreview>, Self::Error>;

    /// Finalizes the active stream and returns the full transcript.
    fn finish_stream(&mut self) -> Result<String, Self::Error>;

    /// Transcribes a complete recording in one offline pass.
    fn transcribe_offline(
        &mut self,
        pcm: &[f32],
        options: &TranscriptionOptions,
    ) -> Result<String, Self::Error>;

    /// Unconditionally resets any native stream/session state so subsequent retries start clean.
    fn reset_asr(&mut self);
}

/// Output emitted by each streaming feed step on [`crate::asr::AsrTranscriberSession`].
#[derive(Debug, Clone, PartialEq)]
pub struct StreamFeedOutcome {
    /// Active `VAD` generation identifier used for this chunk.
    pub generation_id: u64,
    /// Monotonic streaming revision counter.
    pub revision: u64,
    /// Incremental transcript preview returned by the `ASR` engine, if updated.
    pub preview: Option<TranscriptPreview>,
    /// Frame-level `VAD` decisions produced while ingesting the chunk.
    pub vad_decisions: Vec<VadFrameDecision>,
}

/// Output emitted when finalizing a live stream via [`crate::asr::AsrTranscriberSession::finish_stream`].
#[derive(Debug, Clone, PartialEq)]
pub struct StreamFinishOutcome {
    /// `VAD` generation identifier active during the finished stream.
    pub generation_id: u64,
    /// Finalized stream transcript.
    pub text: String,
    /// Tail `VAD` decisions emitted while flushing the accumulator.
    pub vad_decisions: Vec<VadFrameDecision>,
}

/// Output emitted by a one-shot offline transcription via [`crate::asr::AsrTranscriberSession`].
#[derive(Debug, Clone, PartialEq)]
pub struct TranscribeOutcome {
    /// `VAD` generation identifier active for this transcription attempt.
    pub generation_id: u64,
    /// Final transcript text (empty when silence filtering leaves zero voiced audio).
    pub text: String,
    /// Frame-level `VAD` decisions emitted while scoring the recording.
    pub vad_decisions: Vec<VadFrameDecision>,
}
