//! Shared speech transcription options, previews, and `PCM` validation/conversion helpers.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Standard mono sample rate (`16 kHz`) expected by T3 speech `VAD` and `ASR` models.
pub const SPEECH_SAMPLE_RATE_HZ: u32 = 16_000;

/// Standard `Silero` `VAD` frame size (`512` samples = `32 ms` at `16 kHz`).
pub const SPEECH_FRAME_SAMPLES: usize = 512;

/// A preferred vocabulary entry with optional spoken aliases (`speechCustomWords`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechCustomWord {
    /// Preferred canonical spelling.
    pub term: String,
    /// Alternative spellings mapped to `term`.
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// Recognition preferences carried by streaming and offline speech transcription requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionOptions {
    /// Requested `BCP-47` language code or `"auto"` for language detection.
    #[serde(default = "default_language")]
    pub speech_language: String,
    /// Whether non-English speech should be translated to English when supported.
    #[serde(default)]
    pub speech_translate_to_english: bool,
    /// Project/personal custom vocabulary entries preserved across wire serialization.
    #[serde(default)]
    pub speech_custom_words: Vec<SpeechCustomWord>,
    /// Whether universal/language filler words should be trimmed from final transcripts.
    #[serde(default = "default_true")]
    pub speech_remove_filler_words: bool,
    /// Optional spoken correction cue (e.g. `"sorry"`) preserved during cleanup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speech_correction_word: Option<String>,
    /// Whether silence frames are filtered before `ASR` decoding.
    #[serde(default = "default_true")]
    pub speech_silence_filtering: bool,
}

impl Default for TranscriptionOptions {
    fn default() -> Self {
        Self {
            speech_language: default_language(),
            speech_translate_to_english: false,
            speech_custom_words: Vec::new(),
            speech_remove_filler_words: true,
            speech_correction_word: None,
            speech_silence_filtering: true,
        }
    }
}

fn default_language() -> String {
    "auto".to_owned()
}

fn default_true() -> bool {
    true
}

/// Incremental streaming `ASR` transcript preview separating committed and tentative text.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TranscriptPreview {
    /// Finalized prefix unlikely to change on subsequent audio frames.
    pub committed: String,
    /// Unstable tail hypothesis still being refined as more context arrives.
    pub tentative: String,
}

impl TranscriptPreview {
    /// Creates a new [`TranscriptPreview`].
    pub fn new(committed: impl Into<String>, tentative: impl Into<String>) -> Self {
        Self {
            committed: committed.into(),
            tentative: tentative.into(),
        }
    }

    /// Returns the combined trimmed transcript text (`committed` + `tentative`).
    pub fn full_text(&self) -> String {
        let mut combined = String::with_capacity(self.committed.len() + self.tentative.len());
        combined.push_str(&self.committed);
        combined.push_str(&self.tentative);
        combined.trim().to_owned()
    }
}

/// Shared `PCM` decoding and sample validation errors.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum PcmError {
    /// Raw byte length was not divisible by sample width (`2` for `PCM16LE`, `4` for `F32LE`).
    #[error("PCM byte length {byte_len} is not aligned to {sample_width}-byte samples")]
    UnalignedByteLength {
        /// Byte count provided.
        byte_len: usize,
        /// Required byte width per sample.
        sample_width: usize,
    },
    /// A normalized `f32` sample was `NaN`, infinite, or outside `[-1.0, 1.0]`.
    #[error("invalid normalized PCM sample at index {index}: {value}")]
    InvalidSample {
        /// Zero-based sample index.
        index: usize,
        /// Invalid sample value observed.
        value: f32,
    },
}

/// Converts a single signed 16-bit `PCM` sample to a normalized `f32` in `[-1.0, 1.0]`.
pub fn pcm16_to_f32(sample: i16) -> f32 {
    f32::from(sample) / 32_768.0_f32
}

/// Decodes little-endian signed 16-bit `PCM` bytes into normalized `f32` samples in `[-1.0, 1.0]`.
pub fn decode_pcm16le_bytes(bytes: &[u8]) -> Result<Vec<f32>, PcmError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(PcmError::UnalignedByteLength {
            byte_len: bytes.len(),
            sample_width: 2,
        });
    }
    let (pairs, _) = bytes.as_chunks::<2>();
    let mut out = Vec::with_capacity(pairs.len());
    for pair in pairs {
        let sample = i16::from_le_bytes(*pair);
        out.push(pcm16_to_f32(sample));
    }
    Ok(out)
}

/// Decodes little-endian `f32` `PCM` bytes and validates every sample is finite in `[-1.0, 1.0]`.
pub fn decode_f32le_bytes(bytes: &[u8]) -> Result<Vec<f32>, PcmError> {
    if !bytes.len().is_multiple_of(4) {
        return Err(PcmError::UnalignedByteLength {
            byte_len: bytes.len(),
            sample_width: 4,
        });
    }
    let (quads, _) = bytes.as_chunks::<4>();
    let mut out = Vec::with_capacity(quads.len());
    for (index, quad) in quads.iter().enumerate() {
        let sample = f32::from_le_bytes(*quad);
        if !sample.is_finite() || !(-1.0_f32..=1.0_f32).contains(&sample) {
            return Err(PcmError::InvalidSample {
                index,
                value: sample,
            });
        }
        out.push(sample);
    }
    Ok(out)
}

/// Validates that every `f32` sample is finite and within `[-1.0, 1.0]`.
pub fn validate_f32_samples(samples: &[f32]) -> Result<(), PcmError> {
    for (index, &sample) in samples.iter().enumerate() {
        if !sample.is_finite() || !(-1.0_f32..=1.0_f32).contains(&sample) {
            return Err(PcmError::InvalidSample {
                index,
                value: sample,
            });
        }
    }
    Ok(())
}
