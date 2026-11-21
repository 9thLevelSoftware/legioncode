//! Early microphone PCM frame pre-buffer (`3027c2c840` #40011).
//!
//! Captures audio frames produced immediately on microphone start while asynchronous session
//! preparation / `/ws/voice` handshake is still settling, then drains them in strict FIFO order
//! once streaming settles (or collects them for batch transcription / discards them cleanly on abort).

use std::collections::VecDeque;

use thiserror::Error;

use crate::types::validate_f32_samples;
use crate::voice_ws::{DEFAULT_MAX_AUDIO_BUFFER_BYTES, VoiceClientFrame};

/// Lifecycle phase of [`EarlyMicPreBuffer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreBufferPhase {
    /// Microphone is recording while session setup / WebSocket handshake has not yet settled.
    Capturing,
    /// Streaming handshake settled and all pre-buffered frames have been drained in order.
    Streaming,
    /// Capture was cancelled or preparation failed; all buffered audio is cleared and ignored.
    Aborted,
}

/// Outcome returned when pushing an audio frame into [`EarlyMicPreBuffer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreBufferPushOutcome {
    /// Frame was queued because streaming is not ready yet.
    Buffered {
        buffered_frames: usize,
        buffered_bytes: usize,
    },
    /// Handshake has already settled; caller may stream this frame directly.
    PassThroughReady,
    /// Pre-buffer was aborted/stopped; frame was discarded.
    IgnoredAborted,
}

/// Errors raised by [`EarlyMicPreBuffer`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PreBufferError {
    #[error("early mic pre-buffer budget exceeded ({next_bytes} > {max_bytes} bytes)")]
    BufferBudgetExceeded { next_bytes: usize, max_bytes: usize },
    #[error("early mic PCM frame byte length {byte_len} is not 4-byte f32 aligned")]
    UnalignedF32Bytes { byte_len: usize },
    #[error("early mic pre-buffer cannot settle streaming after abort")]
    AlreadyAborted,
}

/// FIFO pre-buffer holding early PCM chunks recorded before session handshake completes (`#40011`).
#[derive(Debug, Clone)]
pub struct EarlyMicPreBuffer {
    phase: PreBufferPhase,
    max_bytes: usize,
    buffered_bytes: usize,
    frames: VecDeque<Vec<u8>>,
}

impl Default for EarlyMicPreBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_AUDIO_BUFFER_BYTES)
    }
}

impl EarlyMicPreBuffer {
    /// Creates a pre-buffer capped at `max_bytes` total pending audio bytes.
    pub fn new(max_bytes: usize) -> Self {
        Self {
            phase: PreBufferPhase::Capturing,
            max_bytes,
            buffered_bytes: 0,
            frames: VecDeque::new(),
        }
    }

    /// Current lifecycle phase.
    pub const fn phase(&self) -> PreBufferPhase {
        self.phase
    }

    /// Returns `true` when frames may still be captured or streamed (`!Aborted`).
    pub const fn is_accepting(&self) -> bool {
        !matches!(self.phase, PreBufferPhase::Aborted)
    }

    /// Number of queued pre-handshake PCM frames.
    pub fn buffered_frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Total queued pre-handshake PCM bytes.
    pub const fn buffered_bytes(&self) -> usize {
        self.buffered_bytes
    }

    /// Pushes one raw PCM byte frame while `Capturing`; signals `PassThroughReady` once `Streaming`.
    pub fn push_pcm_bytes(
        &mut self,
        frame: Vec<u8>,
    ) -> Result<PreBufferPushOutcome, PreBufferError> {
        match self.phase {
            PreBufferPhase::Aborted => Ok(PreBufferPushOutcome::IgnoredAborted),
            PreBufferPhase::Streaming => Ok(PreBufferPushOutcome::PassThroughReady),
            PreBufferPhase::Capturing => {
                if frame.is_empty() {
                    return Ok(PreBufferPushOutcome::Buffered {
                        buffered_frames: self.frames.len(),
                        buffered_bytes: self.buffered_bytes,
                    });
                }
                let next_bytes = self.buffered_bytes.saturating_add(frame.len());
                if next_bytes > self.max_bytes {
                    return Err(PreBufferError::BufferBudgetExceeded {
                        next_bytes,
                        max_bytes: self.max_bytes,
                    });
                }
                self.buffered_bytes = next_bytes;
                self.frames.push_back(frame);
                Ok(PreBufferPushOutcome::Buffered {
                    buffered_frames: self.frames.len(),
                    buffered_bytes: self.buffered_bytes,
                })
            }
        }
    }

    /// Encodes normalized `f32` samples as little-endian bytes and pushes them as one frame.
    pub fn push_f32_samples(
        &mut self,
        samples: &[f32],
    ) -> Result<PreBufferPushOutcome, PreBufferError> {
        validate_f32_samples(samples).map_err(|_| PreBufferError::UnalignedF32Bytes {
            byte_len: samples.len() * 4,
        })?;
        let mut bytes = Vec::with_capacity(samples.len() * 4);
        for &s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        self.push_pcm_bytes(bytes)
    }

    /// Transitions `Capturing -> Streaming` and returns all buffered frames in strict FIFO order.
    pub fn settle_streaming(&mut self) -> Result<Vec<Vec<u8>>, PreBufferError> {
        if self.phase == PreBufferPhase::Aborted {
            return Err(PreBufferError::AlreadyAborted);
        }
        self.phase = PreBufferPhase::Streaming;
        self.buffered_bytes = 0;
        Ok(self.frames.drain(..).collect())
    }

    /// Pushes `frame` into `sink` immediately if settled, or buffers it until `settle_streaming`.
    pub fn feed_or_buffer<F>(
        &mut self,
        frame: Vec<u8>,
        mut sink: F,
    ) -> Result<PreBufferPushOutcome, PreBufferError>
    where
        F: FnMut(Vec<u8>),
    {
        match self.phase {
            PreBufferPhase::Aborted => Ok(PreBufferPushOutcome::IgnoredAborted),
            PreBufferPhase::Streaming => {
                if !frame.is_empty() {
                    sink(frame);
                }
                Ok(PreBufferPushOutcome::PassThroughReady)
            }
            PreBufferPhase::Capturing => self.push_pcm_bytes(frame),
        }
    }

    /// Flattens all buffered audio for batch/offline transcription when streaming is unsupported.
    pub fn take_all_for_batch(&mut self) -> Vec<u8> {
        let mut combined = Vec::with_capacity(self.buffered_bytes);
        while let Some(chunk) = self.frames.pop_front() {
            combined.extend_from_slice(&chunk);
        }
        self.buffered_bytes = 0;
        combined
    }

    /// Aborts capture (`stopAcceptingAudio`), dropping any un-flushed audio immediately.
    pub fn abort(&mut self) {
        self.phase = PreBufferPhase::Aborted;
        self.frames.clear();
        self.buffered_bytes = 0;
    }
}

/// Client-side `/ws/voice` capture bridge queuing `AudioChunk`s until `VoiceServerEvent::Ready`.
#[derive(Debug, Clone, Default)]
pub struct VoiceWsHandshakeBuffer {
    prebuffer: EarlyMicPreBuffer,
}

impl VoiceWsHandshakeBuffer {
    /// Creates a new handshake buffer with `max_bytes` pending audio capacity.
    pub fn new(max_bytes: usize) -> Self {
        Self {
            prebuffer: EarlyMicPreBuffer::new(max_bytes),
        }
    }

    /// Returns the current [`PreBufferPhase`].
    pub const fn phase(&self) -> PreBufferPhase {
        self.prebuffer.phase()
    }

    /// Returns the number of queued pre-handshake audio chunks.
    pub fn queued_chunks(&self) -> usize {
        self.prebuffer.buffered_frame_count()
    }

    /// Accepts a mic PCM chunk: buffers while awaiting `Ready`, or returns `[AudioChunk]` once ready.
    pub fn on_mic_pcm(&mut self, bytes: Vec<u8>) -> Result<Vec<VoiceClientFrame>, PreBufferError> {
        if bytes.is_empty() {
            return Ok(Vec::new());
        }
        match self.prebuffer.push_pcm_bytes(bytes.clone())? {
            PreBufferPushOutcome::Buffered { .. } | PreBufferPushOutcome::IgnoredAborted => {
                Ok(Vec::new())
            }
            PreBufferPushOutcome::PassThroughReady => Ok(vec![VoiceClientFrame::AudioChunk(bytes)]),
        }
    }

    /// Marks the `/ws/voice` handshake settled (`Ready`) and emits queued chunks in FIFO order.
    pub fn on_server_ready(&mut self) -> Result<Vec<VoiceClientFrame>, PreBufferError> {
        let queued = self.prebuffer.settle_streaming()?;
        Ok(queued
            .into_iter()
            .map(VoiceClientFrame::AudioChunk)
            .collect())
    }

    /// Cancels capture and clears buffered pre-handshake audio.
    pub fn cancel(&mut self) {
        self.prebuffer.abort();
    }
}
