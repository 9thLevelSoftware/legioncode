//! Internal `RIFF`/`WAVE` header builder, `"fmt "` parser, and sample format converters.

use crate::types::{pcm16_to_f32, validate_f32_samples};
use crate::wav::WavError;

pub(crate) const WAVE_FORMAT_PCM: u16 = 1;
pub(crate) const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xfffe;

const SUBFORMAT_PCM_TAIL: [u8; 14] = [
    0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71, 0x00, 0x00,
];

pub(crate) fn parse_fmt_chunk(chunk: &[u8]) -> Result<(u16, u16, u32, u16), WavError> {
    if chunk.len() < 16 {
        return Err(WavError::InvalidHeader {
            reason: "fmt chunk shorter than 16 bytes".to_owned(),
        });
    }
    let raw_tag = u16::from_le_bytes([chunk[0], chunk[1]]);
    let channels = u16::from_le_bytes([chunk[2], chunk[3]]);
    let sample_rate_hz = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
    let bits_per_sample = u16::from_le_bytes([chunk[14], chunk[15]]);
    if channels == 0 || sample_rate_hz == 0 {
        return Err(WavError::InvalidHeader {
            reason: "WAVE channels and sample rate must be non-zero".to_owned(),
        });
    }

    let format_tag = if raw_tag == WAVE_FORMAT_EXTENSIBLE && chunk.len() >= 40 {
        let sub_tag = u16::from_le_bytes([chunk[24], chunk[25]]);
        if chunk[26..40] == SUBFORMAT_PCM_TAIL[..14] {
            sub_tag
        } else {
            raw_tag
        }
    } else {
        raw_tag
    };

    let supported = (format_tag == WAVE_FORMAT_PCM && bits_per_sample == 16)
        || (format_tag == WAVE_FORMAT_IEEE_FLOAT && bits_per_sample == 32);
    if !supported {
        return Err(WavError::UnsupportedFormat {
            format_tag,
            bits_per_sample,
        });
    }
    Ok((format_tag, channels, sample_rate_hz, bits_per_sample))
}

pub(crate) fn decode_samples_to_mono(
    format_tag: u16,
    channels: u16,
    bits_per_sample: u16,
    raw_data: &[u8],
) -> Result<Vec<f32>, WavError> {
    let ch = usize::from(channels);
    let sample_bytes = usize::from(bits_per_sample / 8);
    let frame_bytes = ch * sample_bytes;
    if !raw_data.len().is_multiple_of(frame_bytes) {
        return Err(WavError::InvalidSamples {
            reason: format!(
                "data length {} is not aligned to {frame_bytes}-byte frames",
                raw_data.len()
            ),
        });
    }

    let mut mono = Vec::with_capacity(raw_data.len() / frame_bytes);
    let inv_ch = 1.0_f32 / f32::from(channels);

    for frame in raw_data.chunks_exact(frame_bytes) {
        let mut sum = 0.0_f32;
        for sample_slice in frame.chunks_exact(sample_bytes) {
            let val = if format_tag == WAVE_FORMAT_PCM {
                pcm16_to_f32(i16::from_le_bytes([sample_slice[0], sample_slice[1]]))
            } else {
                let s = f32::from_le_bytes([
                    sample_slice[0],
                    sample_slice[1],
                    sample_slice[2],
                    sample_slice[3],
                ]);
                if !s.is_finite() {
                    return Err(WavError::InvalidSamples {
                        reason: "non-finite f32 sample in WAVE data".to_owned(),
                    });
                }
                s.clamp(-1.0_f32, 1.0_f32)
            };
            sum += val;
        }
        mono.push((sum * inv_ch).clamp(-1.0_f32, 1.0_f32));
    }
    Ok(mono)
}

pub(crate) fn validate_encode_args(
    samples: &[f32],
    sample_rate_hz: u32,
    channels: u16,
) -> Result<(), WavError> {
    if sample_rate_hz == 0 || channels == 0 {
        return Err(WavError::InvalidGeometry {
            reason: "sample_rate_hz and channels must be non-zero".to_owned(),
        });
    }
    if !samples.len().is_multiple_of(usize::from(channels)) {
        return Err(WavError::InvalidGeometry {
            reason: "sample count must be divisible by channel count".to_owned(),
        });
    }
    validate_f32_samples(samples).map_err(|err| WavError::InvalidSamples {
        reason: err.to_string(),
    })
}

pub(crate) fn build_wav_header(
    format_tag: u16,
    sample_rate_hz: u32,
    channels: u16,
    bits_per_sample: u16,
    data_bytes: u32,
) -> Result<Vec<u8>, WavError> {
    let block_align =
        channels
            .checked_mul(bits_per_sample / 8)
            .ok_or_else(|| WavError::InvalidGeometry {
                reason: "block_align overflow".to_owned(),
            })?;
    let byte_rate = sample_rate_hz
        .checked_mul(u32::from(block_align))
        .ok_or_else(|| WavError::InvalidGeometry {
            reason: "byte_rate overflow".to_owned(),
        })?;
    let riff_size = data_bytes
        .checked_add(36)
        .ok_or_else(|| WavError::InvalidGeometry {
            reason: "RIFF size overflow".to_owned(),
        })?;

    let mut out = Vec::with_capacity(44 + usize::try_from(data_bytes).unwrap_or(0));
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&format_tag.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate_hz.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    Ok(out)
}

/// Converts normalized `f32` in `[-1.0, 1.0]` to `i16` via IEEE-754 magic-offset rounding.
pub(crate) fn f32_to_pcm16(sample: f32) -> i16 {
    const MAGIC_OFFSET_BITS: i32 = 0x4b40_0000;
    const MAGIC_OFFSET_F32: f32 = 12_582_912.0_f32;
    let scaled =
        (sample.clamp(-1.0_f32, 1.0_f32) * 32_767.0_f32).clamp(-32_768.0_f32, 32_767.0_f32);
    let rounded_bits = (scaled + MAGIC_OFFSET_F32).to_bits();
    let offset_int = i32::try_from(rounded_bits)
        .unwrap_or(MAGIC_OFFSET_BITS)
        .saturating_sub(MAGIC_OFFSET_BITS);
    i16::try_from(offset_int.clamp(-32_768, 32_767)).unwrap_or(0)
}
