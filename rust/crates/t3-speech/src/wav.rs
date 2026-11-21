//! Pure Safe Rust `RIFF`/`WAVE` `PCM16LE` & `IEEE-float32` codec, mono downmix, and linear
//! resampler targeting `16 kHz` mono `f32` `PCM`.

use thiserror::Error;

use crate::types::SPEECH_SAMPLE_RATE_HZ;
use crate::wav_codec::{
    WAVE_FORMAT_IEEE_FLOAT, WAVE_FORMAT_PCM, build_wav_header, decode_samples_to_mono,
    f32_to_pcm16, parse_fmt_chunk, validate_encode_args,
};

/// Decoded `WAVE` audio stream before optional resampling.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedWav {
    /// Source sample rate in Hz.
    pub sample_rate_hz: u32,
    /// Original channel count (`1` = mono, `2` = stereo, etc.).
    pub channels: u16,
    /// Downmixed mono normalized `f32` samples in `[-1.0, 1.0]`.
    pub mono_samples: Vec<f32>,
}

/// Errors returned while decoding, encoding, or resampling `RIFF`/`WAVE` audio.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum WavError {
    /// Input buffer failed `RIFF`/`WAVE` framing or truncated mid-chunk.
    #[error("invalid WAVE header or truncated chunk: {reason}")]
    InvalidHeader {
        /// Structural parse diagnostic.
        reason: String,
    },
    /// Audio format tag, bit depth, or channel layout is unsupported.
    #[error("unsupported WAVE format {format_tag:#06x} ({bits_per_sample} bps)")]
    UnsupportedFormat {
        /// `WAVE` format tag (`1` = PCM, `3` = IEEE float32).
        format_tag: u16,
        /// Bits per sample reported by the `"fmt "` chunk.
        bits_per_sample: u16,
    },
    /// Sample payload contained non-finite `f32` values or unaligned trailing bytes.
    #[error("invalid WAVE sample data: {reason}")]
    InvalidSamples {
        /// Validation failure detail.
        reason: String,
    },
    /// Zero or mismatched sample rate/channel argument supplied to encoder or resampler.
    #[error("invalid audio geometry: {reason}")]
    InvalidGeometry {
        /// Geometry validation detail.
        reason: String,
    },
}

/// Decodes `RIFF`/`WAVE` (`PCM16LE` or `IEEE-float32`), downmixing multi-channel frames to mono.
pub fn decode_wav(bytes: &[u8]) -> Result<DecodedWav, WavError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(WavError::InvalidHeader {
            reason: "expected RIFF/WAVE header".to_owned(),
        });
    }

    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut data_slice: Option<&[u8]> = None;
    let mut offset = 12usize;

    while let Some(header) = bytes.get(offset..offset + 8) {
        let id = [header[0], header[1], header[2], header[3]];
        let raw_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        let size = usize::try_from(raw_size).map_err(|_| WavError::InvalidHeader {
            reason: "WAV chunk size overflows usize".to_owned(),
        })?;
        let payload_start = offset + 8;
        let payload_end =
            payload_start
                .checked_add(size)
                .ok_or_else(|| WavError::InvalidHeader {
                    reason: "WAV chunk size overflow".to_owned(),
                })?;
        let chunk =
            bytes
                .get(payload_start..payload_end)
                .ok_or_else(|| WavError::InvalidHeader {
                    reason: "truncated WAV subchunk".to_owned(),
                })?;
        match &id {
            b"fmt " => fmt = Some(parse_fmt_chunk(chunk)?),
            b"data" => data_slice = Some(chunk),
            _ => {}
        }
        offset = payload_end.checked_add(size & 1).unwrap_or(bytes.len());
    }

    let (format_tag, channels, sample_rate_hz, bits_per_sample) =
        fmt.ok_or_else(|| WavError::InvalidHeader {
            reason: "missing fmt chunk".to_owned(),
        })?;
    let raw_data = data_slice.ok_or_else(|| WavError::InvalidHeader {
        reason: "missing data chunk".to_owned(),
    })?;

    let mono_samples = decode_samples_to_mono(format_tag, channels, bits_per_sample, raw_data)?;
    Ok(DecodedWav {
        sample_rate_hz,
        channels,
        mono_samples,
    })
}

/// Decodes a `RIFF`/`WAVE` buffer, downmixes to mono, and resamples linearly to `16 kHz`.
///
/// # Examples
///
/// ```
/// use t3_speech::wav::{decode_wav_to_16khz_mono, encode_wav_pcm16le};
///
/// let wav = encode_wav_pcm16le(&[0.25_f32, 0.25, -0.25, -0.25], 32_000, 2)?;
/// let mono_16k = decode_wav_to_16khz_mono(&wav)?;
/// assert_eq!(mono_16k.len(), 1);
/// # Ok::<(), t3_speech::wav::WavError>(())
/// ```
pub fn decode_wav_to_16khz_mono(bytes: &[u8]) -> Result<Vec<f32>, WavError> {
    let decoded = decode_wav(bytes)?;
    resample_linear(
        &decoded.mono_samples,
        decoded.sample_rate_hz,
        SPEECH_SAMPLE_RATE_HZ,
    )
}

/// Resamples mono normalized `f32` audio linearly from `src_rate_hz` to `dst_rate_hz`.
pub fn resample_linear(
    samples: &[f32],
    src_rate_hz: u32,
    dst_rate_hz: u32,
) -> Result<Vec<f32>, WavError> {
    if src_rate_hz == 0 || dst_rate_hz == 0 {
        return Err(WavError::InvalidGeometry {
            reason: "sample rates must be non-zero".to_owned(),
        });
    }
    if samples.is_empty() {
        return Ok(Vec::new());
    }
    if src_rate_hz == dst_rate_hz {
        return Ok(samples.to_vec());
    }

    let src_len = u128::try_from(samples.len()).map_err(|_| WavError::InvalidGeometry {
        reason: "input slice too large".to_owned(),
    })?;
    let out_len_u128 = ((src_len * u128::from(dst_rate_hz)) / u128::from(src_rate_hz)).max(1);
    let out_len = usize::try_from(out_len_u128).map_err(|_| WavError::InvalidGeometry {
        reason: "resampled length overflows usize".to_owned(),
    })?;

    let mut out = Vec::with_capacity(out_len);
    let src_rate = u128::from(src_rate_hz);
    let dst_rate = u128::from(dst_rate_hz);

    for out_idx in 0..out_len {
        let num = u128::try_from(out_idx).unwrap_or(0) * src_rate;
        let src_idx = usize::try_from(num / dst_rate).unwrap_or(samples.len() - 1);
        let clamped_idx = src_idx.min(samples.len() - 1);
        let rem = num % dst_rate;
        let frac_q16 = u16::try_from((rem * 65_536) / dst_rate).unwrap_or(0);
        let frac = f32::from(frac_q16) / 65_536.0_f32;
        let s0 = samples[clamped_idx];
        let s1 = samples.get(clamped_idx + 1).copied().unwrap_or(s0);
        out.push(s0 + (s1 - s0) * frac);
    }
    Ok(out)
}

/// Encodes normalized `f32` samples as a standard `RIFF`/`WAVE` `PCM16LE` byte stream.
pub fn encode_wav_pcm16le(
    samples: &[f32],
    sample_rate_hz: u32,
    channels: u16,
) -> Result<Vec<u8>, WavError> {
    validate_encode_args(samples, sample_rate_hz, channels)?;
    let data_bytes =
        u32::try_from(samples.len().saturating_mul(2)).map_err(|_| WavError::InvalidGeometry {
            reason: "WAVE data exceeds 4 GiB RIFF limit".to_owned(),
        })?;
    let mut out = build_wav_header(WAVE_FORMAT_PCM, sample_rate_hz, channels, 16, data_bytes)?;
    for &sample in samples {
        out.extend_from_slice(&f32_to_pcm16(sample).to_le_bytes());
    }
    Ok(out)
}

/// Encodes normalized `f32` samples as a `RIFF`/`WAVE` `IEEE-float32` (`32-bit`) byte stream.
pub fn encode_wav_f32le(
    samples: &[f32],
    sample_rate_hz: u32,
    channels: u16,
) -> Result<Vec<u8>, WavError> {
    validate_encode_args(samples, sample_rate_hz, channels)?;
    let data_bytes =
        u32::try_from(samples.len().saturating_mul(4)).map_err(|_| WavError::InvalidGeometry {
            reason: "WAVE data exceeds 4 GiB RIFF limit".to_owned(),
        })?;
    let mut out = build_wav_header(
        WAVE_FORMAT_IEEE_FLOAT,
        sample_rate_hz,
        channels,
        32,
        data_bytes,
    )?;
    for &sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(out)
}
