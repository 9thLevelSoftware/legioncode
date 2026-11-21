//! `VAD` configuration, error types, and 512-sample `PCM16LE` / `f32` frame accumulator.

use thiserror::Error;

use crate::types::{
    PcmError, SPEECH_FRAME_SAMPLES, SPEECH_SAMPLE_RATE_HZ, pcm16_to_f32, validate_f32_samples,
};

/// Standard sample rate (`16 kHz`) expected by [`crate::vad::SileroStyleVad`].
pub const VAD_SAMPLE_RATE_HZ: u32 = SPEECH_SAMPLE_RATE_HZ;

/// Standard frame size (`512` samples = `32 ms` at `16 kHz`).
pub const VAD_FRAME_SAMPLES: usize = SPEECH_FRAME_SAMPLES;

/// Errors raised by `VAD` configuration, frame accumulation, and session lifecycle checks.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum VadError {
    /// One or more [`VadConfig`] thresholds or dimensions were invalid.
    #[error("invalid VAD configuration: {reason}")]
    InvalidConfig {
        /// Human-readable validation failure reason.
        reason: String,
    },
    /// Input `PCM` decoding or sample validation failed.
    #[error(transparent)]
    Pcm(#[from] PcmError),
    /// A `PCM16LE` stream ended or mixed with `f32` while holding a trailing odd byte.
    #[error("unaligned trailing PCM16LE byte remains in accumulator")]
    UnalignedTrailingPcm16Byte,
    /// A frame passed directly to [`crate::vad::SileroStyleVad::process_frame`] had the wrong length.
    #[error("expected VAD frame of {expected} samples, got {actual}")]
    InvalidFrameLength {
        /// Configured frame sample count.
        expected: usize,
        /// Provided slice length.
        actual: usize,
    },
    /// Operation attempted against a disposed [`crate::vad::VadSessionState`].
    #[error("VAD session generation {generation_id} has been disposed")]
    SessionDisposed {
        /// Disposed generation identifier.
        generation_id: u64,
    },
}

/// Configuration for [`VadFrameAccumulator`] and [`crate::vad::SileroStyleVad`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VadConfig {
    /// Number of mono samples per `VAD` frame (default `512`).
    pub frame_samples: usize,
    /// Expected mono `PCM` sample rate in Hz (default `16_000`).
    pub sample_rate_hz: u32,
    /// Minimum `RMS` energy floor below which frames score `0.0` probability.
    pub min_rms_energy: f32,
    /// `RMS` energy at which the energy score saturates to `1.0`.
    pub target_rms_energy: f32,
    /// Minimum zero-crossing rate expected of voiced speech (rejects DC offset).
    pub min_zcr: f32,
    /// Maximum zero-crossing rate expected of voiced speech (attenuates white hiss/static).
    pub max_zcr: f32,
    /// Probability threshold (`[0.0, 1.0]`) required to classify a frame as voiced.
    pub speech_threshold: f32,
    /// Consecutive voiced frames required to trigger speech onset (default `2`).
    pub onset_frames: usize,
    /// Hangover silence frames kept active after voiced activity drops (default `15`).
    pub hangover_frames: usize,
    /// Number of pre-roll frames retained before confirmed onset (default `15`).
    pub prefill_frames: usize,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            frame_samples: VAD_FRAME_SAMPLES,
            sample_rate_hz: VAD_SAMPLE_RATE_HZ,
            min_rms_energy: 0.015,
            target_rms_energy: 0.06,
            min_zcr: 0.01,
            max_zcr: 0.45,
            speech_threshold: 0.35,
            onset_frames: 2,
            hangover_frames: 15,
            prefill_frames: 15,
        }
    }
}

impl VadConfig {
    /// Validates frame size, energy bounds, zero-crossing window, and onset/hangover counts.
    pub fn validate(&self) -> Result<(), VadError> {
        if self.frame_samples == 0 || self.frame_samples > usize::from(u16::MAX) {
            return Err(VadError::InvalidConfig {
                reason: format!("frame_samples must be in 1..={}", u16::MAX),
            });
        }
        if self.sample_rate_hz == 0 {
            return Err(VadError::InvalidConfig {
                reason: "sample_rate_hz must be non-zero".to_owned(),
            });
        }
        if !self.min_rms_energy.is_finite() || self.min_rms_energy < 0.0 {
            return Err(VadError::InvalidConfig {
                reason: "min_rms_energy must be finite and non-negative".to_owned(),
            });
        }
        if !self.target_rms_energy.is_finite() || self.target_rms_energy <= self.min_rms_energy {
            return Err(VadError::InvalidConfig {
                reason: "target_rms_energy must be finite and greater than min_rms_energy"
                    .to_owned(),
            });
        }
        if !self.min_zcr.is_finite()
            || !self.max_zcr.is_finite()
            || self.min_zcr < 0.0
            || self.max_zcr > 1.0
            || self.min_zcr >= self.max_zcr
        {
            return Err(VadError::InvalidConfig {
                reason: "min_zcr and max_zcr must satisfy 0.0 <= min_zcr < max_zcr <= 1.0"
                    .to_owned(),
            });
        }
        if !self.speech_threshold.is_finite()
            || !(0.0_f32..=1.0_f32).contains(&self.speech_threshold)
        {
            return Err(VadError::InvalidConfig {
                reason: "speech_threshold must be finite within 0.0..=1.0".to_owned(),
            });
        }
        if self.onset_frames == 0 {
            return Err(VadError::InvalidConfig {
                reason: "onset_frames must be at least 1".to_owned(),
            });
        }
        Ok(())
    }
}

/// A fixed-size `VAD` frame paired with the number of unpadded valid leading samples.
#[derive(Debug, Clone, PartialEq)]
pub struct VadFrame {
    /// Zero-padded normalized `f32` samples of exact length `frame_samples`.
    pub samples: Vec<f32>,
    /// Count of real unpadded samples in `samples` (`1..=frame_samples`).
    pub valid_samples: usize,
}

impl VadFrame {
    /// Returns only the unpadded valid leading samples for downstream `ASR` consumption.
    pub fn valid_slice(&self) -> &[f32] {
        let len = self.valid_samples.min(self.samples.len());
        &self.samples[..len]
    }
}

/// Chunked `16 kHz` `PCM16LE` -> normalized `f32` converter and `512`-sample frame chunker.
#[derive(Debug, Clone)]
pub struct VadFrameAccumulator {
    frame_samples: usize,
    carry_byte: Option<u8>,
    pending: Vec<f32>,
}

impl Default for VadFrameAccumulator {
    fn default() -> Self {
        Self::new(VAD_FRAME_SAMPLES)
    }
}

impl VadFrameAccumulator {
    /// Creates an empty [`VadFrameAccumulator`] emitting frames of `frame_samples` length.
    ///
    /// Zero-length inputs clamp safely to `1` sample so frame slicing never stalls.
    pub fn new(frame_samples: usize) -> Self {
        let clamped = frame_samples.max(1);
        Self {
            frame_samples: clamped,
            carry_byte: None,
            pending: Vec::with_capacity(clamped),
        }
    }

    /// Returns the configured frame length in samples.
    pub fn frame_samples(&self) -> usize {
        self.frame_samples
    }

    /// Returns the number of residual normalized samples currently waiting for a full frame.
    pub fn pending_samples(&self) -> usize {
        self.pending.len()
    }

    /// Returns `true` when an odd trailing `PCM16LE` byte is waiting for its high byte.
    pub fn has_pending_byte(&self) -> bool {
        self.carry_byte.is_some()
    }

    /// Clears all residual `PCM16LE` carry bytes and buffered partial samples.
    pub fn clear(&mut self) {
        self.carry_byte = None;
        self.pending.clear();
    }

    /// Appends arbitrary `PCM16LE` bytes, normalizing to `[-1.0, 1.0]` `f32` and emitting full frames.
    pub fn push_pcm16le(&mut self, bytes: &[u8]) -> Result<Vec<VadFrame>, VadError> {
        if bytes.is_empty() {
            return Ok(Vec::new());
        }
        let mut cursor = 0usize;
        if let Some(low) = self.carry_byte.take() {
            let sample = i16::from_le_bytes([low, bytes[0]]);
            self.pending.push(pcm16_to_f32(sample));
            cursor = 1;
        }
        let remaining = &bytes[cursor..];
        let (pairs, tail) = remaining.as_chunks::<2>();
        for pair in pairs {
            let sample = i16::from_le_bytes(*pair);
            self.pending.push(pcm16_to_f32(sample));
        }
        if let Some(&odd) = tail.first() {
            self.carry_byte = Some(odd);
        }
        Ok(self.drain_complete_frames())
    }

    /// Appends pre-normalized `f32` samples in `[-1.0, 1.0]` and emits all complete frames.
    pub fn push_f32(&mut self, samples: &[f32]) -> Result<Vec<VadFrame>, VadError> {
        if self.carry_byte.is_some() {
            return Err(VadError::UnalignedTrailingPcm16Byte);
        }
        validate_f32_samples(samples)?;
        self.pending.extend_from_slice(samples);
        Ok(self.drain_complete_frames())
    }

    /// Flushes any residual partial frame by zero-padding up to `frame_samples`.
    ///
    /// Returns [`VadError::UnalignedTrailingPcm16Byte`] and clears residual state if an odd
    /// `PCM16LE` byte remains unclosed at stream end.
    pub fn flush(&mut self) -> Result<Option<VadFrame>, VadError> {
        if self.carry_byte.take().is_some() {
            self.pending.clear();
            return Err(VadError::UnalignedTrailingPcm16Byte);
        }
        if self.pending.is_empty() {
            return Ok(None);
        }
        let valid_samples = self.pending.len();
        let mut samples = std::mem::take(&mut self.pending);
        samples.resize(self.frame_samples, 0.0_f32);
        Ok(Some(VadFrame {
            samples,
            valid_samples,
        }))
    }

    fn drain_complete_frames(&mut self) -> Vec<VadFrame> {
        let mut out = Vec::new();
        while self.pending.len() >= self.frame_samples {
            let tail = self.pending.split_off(self.frame_samples);
            let samples = std::mem::replace(&mut self.pending, tail);
            out.push(VadFrame {
                samples,
                valid_samples: self.frame_samples,
            });
        }
        out
    }
}
