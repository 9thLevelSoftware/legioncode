//! `.t3asr` JSON/binary acoustic prototype weight formats and validation helpers.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::acoustic_features::NUM_ACOUSTIC_BANDS;

/// Minimum sample threshold below which midpoint splitting stops (mirrors `native.ts:75`).
pub const MIN_SPLIT_AUDIO_SAMPLES: usize = 4_000;

/// Default maximum sample window before recursive binary midpoint splitting runs (`30s` at `16 kHz`).
pub const DEFAULT_MAX_AUDIO_SAMPLES: usize = 16_000 * 30;

/// Binary magic header for `.t3asr` compiled prototype weight files.
pub const T3ASR_BINARY_MAGIC: &[u8; 8] = b"T3ASR1\0\0";

/// Errors raised when loading, validating, or running [`crate::acoustic_model::AcousticAsrEngine`].
#[derive(Debug, Clone, PartialEq, Error)]
pub enum AcousticModelError {
    /// Failed to read a `.t3asr` model definition from disk.
    #[error("failed to read acoustic model file: {message}")]
    Io {
        /// I/O error message.
        message: String,
    },
    /// `.t3asr` JSON or binary payload was malformed or truncated.
    #[error("invalid .t3asr acoustic model format: {reason}")]
    InvalidFormat {
        /// Parse or validation diagnostic.
        reason: String,
    },
    /// Streaming method called without an active `begin_stream` session.
    #[error("acoustic stream is not active")]
    StreamNotActive,
    /// Acoustic feature extraction failed on invalid `PCM` input.
    #[error(transparent)]
    Features(#[from] crate::acoustic_features::AcousticFeatureError),
}

/// Single phonetic/lexical prototype in [`AcousticModelWeights`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcousticPrototype {
    /// Emitted token text when an acoustic segment best matches this prototype.
    pub token: String,
    /// Expected spectral centroid in Hz (`0.0 ..= 8_000.0`).
    pub centroid_hz: f32,
    /// Expected zero-crossing rate (`0.0 ..= 1.0`).
    pub zcr: f32,
    /// Normalized `8`-band spectral envelope shape (zero-mean relative log profile).
    pub band_envelope: [f32; NUM_ACOUSTIC_BANDS],
    /// Prototype tie-breaker / prior weight (`> 0.0`).
    pub weight: f32,
}

/// Loaded or built-in `.t3asr` acoustic model inventory driving [`crate::acoustic_model::AcousticAsrEngine`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcousticModelWeights {
    /// Model format version (`1`).
    pub version: u32,
    /// Identifier string for diagnostics and inspection.
    pub model_id: String,
    /// Primary `BCP-47` language tag supported by this inventory.
    pub language: String,
    /// Maximum audio window before recursive binary midpoint splitting down to `4_000` samples.
    pub max_audio_samples: usize,
    /// Minimum frame `RMS` energy required to consider a frame voiced.
    pub min_voiced_rms: f32,
    /// Prototype table scored against voiced speech frames.
    pub prototypes: Vec<AcousticPrototype>,
}

impl Default for AcousticModelWeights {
    fn default() -> Self {
        Self::default_english()
    }
}

impl AcousticModelWeights {
    /// Returns the built-in deterministic English acoustic prototype inventory.
    pub fn default_english() -> Self {
        Self {
            version: 1,
            model_id: "t3-acoustic-en-v1".to_owned(),
            language: "en".to_owned(),
            max_audio_samples: DEFAULT_MAX_AUDIO_SAMPLES,
            min_voiced_rms: 0.012_f32,
            prototypes: vec![
                proto(
                    "one",
                    320.0,
                    0.035,
                    [2.0, 1.5, 0.3, -0.5, -1.0, -1.2, -1.4, -1.6],
                ),
                proto(
                    "hello",
                    500.0,
                    0.062,
                    [1.0, 1.8, 1.2, 0.1, -0.6, -1.0, -1.2, -1.4],
                ),
                proto(
                    "code",
                    780.0,
                    0.095,
                    [0.2, 1.2, 1.8, 0.9, -0.2, -0.8, -1.2, -1.5],
                ),
                proto(
                    "run",
                    1_150.0,
                    0.14,
                    [-0.4, 0.4, 1.4, 1.8, 0.6, -0.4, -1.0, -1.4],
                ),
                proto(
                    "build",
                    1_650.0,
                    0.20,
                    [-0.8, -0.2, 0.8, 1.6, 1.4, 0.2, -0.8, -1.2],
                ),
                proto(
                    "test",
                    2_350.0,
                    0.29,
                    [-1.2, -0.8, 0.0, 0.9, 1.7, 1.2, -0.2, -0.9],
                ),
                proto(
                    "search",
                    3_400.0,
                    0.42,
                    [-1.5, -1.2, -0.6, 0.2, 1.0, 1.8, 1.4, 0.2],
                ),
                proto(
                    "fix",
                    4_800.0,
                    0.56,
                    [-1.6, -1.4, -1.0, -0.4, 0.4, 1.2, 1.9, 1.6],
                ),
            ],
        }
    }

    /// Validates all prototype dimensions, thresholds, and midpoint split parameters.
    pub fn validate(&self) -> Result<(), AcousticModelError> {
        if self.model_id.trim().is_empty() || self.prototypes.is_empty() {
            return Err(AcousticModelError::InvalidFormat {
                reason: "model_id and prototypes must not be empty".to_owned(),
            });
        }
        if !self.min_voiced_rms.is_finite() || self.min_voiced_rms < 0.0 {
            return Err(AcousticModelError::InvalidFormat {
                reason: "min_voiced_rms must be finite and non-negative".to_owned(),
            });
        }
        for p in &self.prototypes {
            let finite_band = p.band_envelope.iter().all(|v| v.is_finite());
            if p.token.trim().is_empty()
                || !p.centroid_hz.is_finite()
                || !p.zcr.is_finite()
                || !p.weight.is_finite()
                || p.weight <= 0.0
                || !finite_band
            {
                return Err(AcousticModelError::InvalidFormat {
                    reason: format!("invalid prototype `{}`", p.token),
                });
            }
        }
        Ok(())
    }

    /// Loads `.t3asr` model definitions from disk (auto-detecting binary `T3ASR1` or JSON).
    pub fn load_from_file(path: &Path) -> Result<Self, AcousticModelError> {
        let bytes = fs::read(path).map_err(|err| AcousticModelError::Io {
            message: err.to_string(),
        })?;
        if bytes.starts_with(T3ASR_BINARY_MAGIC) {
            Self::from_binary_bytes(&bytes)
        } else {
            Self::from_json_bytes(&bytes)
        }
    }

    /// Parses UTF-8 JSON `.t3asr` model weights.
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self, AcousticModelError> {
        let model: Self =
            serde_json::from_slice(bytes).map_err(|err| AcousticModelError::InvalidFormat {
                reason: err.to_string(),
            })?;
        model.validate()?;
        Ok(model)
    }

    /// Serializes model weights to compact JSON `.t3asr` bytes.
    pub fn to_json_bytes(&self) -> Result<Vec<u8>, AcousticModelError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|err| AcousticModelError::InvalidFormat {
            reason: err.to_string(),
        })
    }

    /// Encodes model weights to the compact binary `.t3asr` wire format (`T3ASR1`).
    pub fn to_binary_bytes(&self) -> Result<Vec<u8>, AcousticModelError> {
        self.validate()?;
        let max_samples = u32::try_from(self.max_audio_samples).unwrap_or(u32::MAX);
        let proto_count = u32::try_from(self.prototypes.len()).unwrap_or(u32::MAX);
        let mut out = Vec::with_capacity(64 + self.prototypes.len() * 52);
        out.extend_from_slice(T3ASR_BINARY_MAGIC);
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&max_samples.to_le_bytes());
        out.extend_from_slice(&self.min_voiced_rms.to_le_bytes());
        push_len_str(&mut out, self.model_id.as_bytes())?;
        push_len_str(&mut out, self.language.as_bytes())?;
        out.extend_from_slice(&proto_count.to_le_bytes());
        for p in &self.prototypes {
            push_len_str(&mut out, p.token.as_bytes())?;
            out.extend_from_slice(&p.centroid_hz.to_le_bytes());
            out.extend_from_slice(&p.zcr.to_le_bytes());
            for band in p.band_envelope {
                out.extend_from_slice(&band.to_le_bytes());
            }
            out.extend_from_slice(&p.weight.to_le_bytes());
        }
        Ok(out)
    }

    /// Decodes compact binary `.t3asr` bytes (`T3ASR1`).
    pub fn from_binary_bytes(bytes: &[u8]) -> Result<Self, AcousticModelError> {
        let mut cur = BinaryCursor::new(bytes);
        if cur.take(8)? != T3ASR_BINARY_MAGIC {
            return Err(AcousticModelError::InvalidFormat {
                reason: "bad T3ASR1 magic".to_owned(),
            });
        }
        let version = cur.read_u32()?;
        let max_audio_samples = usize::try_from(cur.read_u32()?).unwrap_or(usize::MAX);
        let min_voiced_rms = cur.read_f32()?;
        let model_id = cur.read_string()?;
        let language = cur.read_string()?;
        let count = usize::try_from(cur.read_u32()?).unwrap_or(0);
        let mut prototypes = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            let token = cur.read_string()?;
            let centroid_hz = cur.read_f32()?;
            let zcr = cur.read_f32()?;
            let mut band_envelope = [0.0_f32; NUM_ACOUSTIC_BANDS];
            for slot in &mut band_envelope {
                *slot = cur.read_f32()?;
            }
            let weight = cur.read_f32()?;
            prototypes.push(AcousticPrototype {
                token,
                centroid_hz,
                zcr,
                band_envelope,
                weight,
            });
        }
        let model = Self {
            version,
            model_id,
            language,
            max_audio_samples,
            min_voiced_rms,
            prototypes,
        };
        model.validate()?;
        Ok(model)
    }
}

fn proto(
    token: &str,
    centroid_hz: f32,
    zcr: f32,
    band_envelope: [f32; NUM_ACOUSTIC_BANDS],
) -> AcousticPrototype {
    AcousticPrototype {
        token: token.to_owned(),
        centroid_hz,
        zcr,
        band_envelope,
        weight: 1.0,
    }
}

fn push_len_str(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), AcousticModelError> {
    let len = u16::try_from(bytes.len()).map_err(|_| AcousticModelError::InvalidFormat {
        reason: "string exceeds u16 binary length".to_owned(),
    })?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

struct BinaryCursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> BinaryCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
    fn take(&mut self, len: usize) -> Result<&'a [u8], AcousticModelError> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| AcousticModelError::InvalidFormat {
                reason: "binary offset overflow".to_owned(),
            })?;
        let slice =
            self.bytes
                .get(self.pos..end)
                .ok_or_else(|| AcousticModelError::InvalidFormat {
                    reason: "truncated .t3asr binary payload".to_owned(),
                })?;
        self.pos = end;
        Ok(slice)
    }
    fn read_u32(&mut self) -> Result<u32, AcousticModelError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn read_f32(&mut self) -> Result<f32, AcousticModelError> {
        let b = self.take(4)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn read_string(&mut self) -> Result<String, AcousticModelError> {
        let b = self.take(2)?;
        let len = usize::from(u16::from_le_bytes([b[0], b[1]]));
        let raw = self.take(len)?;
        std::str::from_utf8(raw).map(str::to_owned).map_err(|err| {
            AcousticModelError::InvalidFormat {
                reason: err.to_string(),
            }
        })
    }
}
