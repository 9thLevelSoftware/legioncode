//! Pure Safe Rust short-time `DSP` acoustic frame analyzer (`512`-sample `16 kHz` frames).
//!
//! Computes `RMS` energy, zero-crossing rate, `512`-point Hann-windowed `FFT` spectral centroid,
//! and an `8`-band log-energy spectral envelope with zero `unsafe` and zero heap thrashing.

use std::f32::consts::TAU;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::{SPEECH_FRAME_SAMPLES, SPEECH_SAMPLE_RATE_HZ, validate_f32_samples};

/// Number of log-energy spectral bands extracted per `512`-sample frame.
pub const NUM_ACOUSTIC_BANDS: usize = 8;

const BAND_BINS: [(usize, usize); NUM_ACOUSTIC_BANDS] = [
    (2, 8),
    (8, 16),
    (16, 32),
    (32, 56),
    (56, 88),
    (88, 128),
    (128, 184),
    (184, 257),
];

/// Errors produced during acoustic feature extraction.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum AcousticFeatureError {
    /// Input audio contained non-finite or out-of-range `f32` samples.
    #[error("invalid PCM sample in acoustic analyzer: {0}")]
    InvalidPcm(#[from] crate::types::PcmError),
    /// Frame passed to `analyze_exact_frame` did not contain `512` samples.
    #[error("expected {expected}-sample frame, got {actual}")]
    InvalidFrameLength {
        /// Required frame size (`512`).
        expected: usize,
        /// Actual slice length provided.
        actual: usize,
    },
}

/// Short-time `DSP` descriptors for a single `512`-sample (`32 ms`) `16 kHz` `PCM` frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcousticFrameFeatures {
    /// Root-mean-square amplitude in `[0.0, 1.0]`.
    pub rms_energy: f32,
    /// Normalized sign-crossing rate in `[0.0, 1.0]`.
    pub zero_crossing_rate: f32,
    /// Power-weighted spectral centroid in Hz (`0.0 ..= 8_000.0`).
    pub spectral_centroid_hz: f32,
    /// Natural-log mean band powers across `8` subbands spanning `62.5 Hz ..= 8_000 Hz`.
    pub band_log_energies: [f32; NUM_ACOUSTIC_BANDS],
    /// Whether this frame exceeds the analyzer's voiced activity threshold.
    pub voiced: bool,
}

/// Stateless short-time spectral and time-domain analyzer over `16 kHz` mono `f32` frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AcousticFrameAnalyzer {
    /// Minimum `RMS` energy required to mark a frame as voiced speech.
    pub min_voiced_rms: f32,
}

impl Default for AcousticFrameAnalyzer {
    fn default() -> Self {
        Self {
            min_voiced_rms: 0.012_f32,
        }
    }
}

impl AcousticFrameAnalyzer {
    /// Creates an analyzer with an explicit `min_voiced_rms` gate.
    pub fn new(min_voiced_rms: f32) -> Self {
        Self {
            min_voiced_rms: if min_voiced_rms.is_finite() && min_voiced_rms >= 0.0 {
                min_voiced_rms
            } else {
                0.012_f32
            },
        }
    }

    /// Analyzes an exact `512`-sample `16 kHz` `f32` frame.
    pub fn analyze_exact_frame(
        self,
        frame: &[f32],
    ) -> Result<AcousticFrameFeatures, AcousticFeatureError> {
        if frame.len() != SPEECH_FRAME_SAMPLES {
            return Err(AcousticFeatureError::InvalidFrameLength {
                expected: SPEECH_FRAME_SAMPLES,
                actual: frame.len(),
            });
        }
        validate_f32_samples(frame)?;
        Ok(self.compute_features(frame))
    }

    /// Analyzes an arbitrary-length `16 kHz` `f32` slice in `512`-sample frames (zero-padding tail).
    pub fn analyze_slice(
        self,
        samples: &[f32],
    ) -> Result<Vec<AcousticFrameFeatures>, AcousticFeatureError> {
        validate_f32_samples(samples)?;
        if samples.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(samples.len().div_ceil(SPEECH_FRAME_SAMPLES));
        let mut scratch = [0.0_f32; SPEECH_FRAME_SAMPLES];
        for chunk in samples.chunks(SPEECH_FRAME_SAMPLES) {
            if chunk.len() == SPEECH_FRAME_SAMPLES {
                out.push(self.compute_features(chunk));
            } else {
                scratch.fill(0.0_f32);
                scratch[..chunk.len()].copy_from_slice(chunk);
                out.push(self.compute_features(&scratch));
            }
        }
        Ok(out)
    }

    fn compute_features(self, frame: &[f32]) -> AcousticFrameFeatures {
        let (rms_energy, zero_crossing_rate) = time_domain_stats(frame);
        let mut spectrum = [(0.0_f32, 0.0_f32); SPEECH_FRAME_SAMPLES];
        let last_idx = f32::from(u16::try_from(SPEECH_FRAME_SAMPLES - 1).unwrap_or(511));

        for (idx, (&sample, slot)) in frame.iter().zip(spectrum.iter_mut()).enumerate() {
            let i_f32 = f32::from(u16::try_from(idx).unwrap_or(0));
            let hann = 0.5_f32 * (1.0_f32 - ((TAU * i_f32) / last_idx).cos());
            slot.0 = sample * hann;
        }
        fft_512_in_place(&mut spectrum);

        let bin_hz = f32::from(u16::try_from(SPEECH_SAMPLE_RATE_HZ).unwrap_or(16_000))
            / f32::from(u16::try_from(SPEECH_FRAME_SAMPLES).unwrap_or(512));
        let mut mag_sum = 0.0_f32;
        let mut weighted_freq = 0.0_f32;
        let mut powers = [0.0_f32; 257];

        for (bin, &(re, im)) in spectrum[..=256].iter().enumerate() {
            let pwr = re * re + im * im;
            powers[bin] = pwr;
            if bin > 0 {
                let mag = pwr.sqrt();
                let freq = f32::from(u16::try_from(bin).unwrap_or(0)) * bin_hz;
                mag_sum += mag;
                weighted_freq += freq * mag;
            }
        }

        let spectral_centroid_hz = if mag_sum > 1e-9_f32 {
            (weighted_freq / mag_sum).clamp(0.0_f32, 8_000.0_f32)
        } else {
            0.0_f32
        };

        let mut band_log_energies = [0.0_f32; NUM_ACOUSTIC_BANDS];
        for (band_idx, &(start, end)) in BAND_BINS.iter().enumerate() {
            let count = f32::from(u16::try_from(end - start).unwrap_or(1));
            let sum_pwr: f32 = powers[start..end].iter().sum();
            band_log_energies[band_idx] = ((sum_pwr / count) + 1e-6_f32).ln();
        }

        let voiced = rms_energy >= self.min_voiced_rms
            && (0.005_f32..=0.65_f32).contains(&zero_crossing_rate);

        AcousticFrameFeatures {
            rms_energy,
            zero_crossing_rate,
            spectral_centroid_hz,
            band_log_energies,
            voiced,
        }
    }
}

fn time_domain_stats(frame: &[f32]) -> (f32, f32) {
    let n_u16 = u16::try_from(frame.len()).unwrap_or(512);
    let denom = f32::from(n_u16.max(1));
    let mut sum_sq = 0.0_f32;
    let mut crossings = 0_u16;
    let mut prev_sign = 0_i8;

    for &sample in frame {
        sum_sq += sample * sample;
        let sign = if sample > 1e-6_f32 {
            1
        } else if sample < -1e-6_f32 {
            -1
        } else {
            0
        };
        if sign != 0 {
            if prev_sign != 0 && sign != prev_sign {
                crossings = crossings.saturating_add(1);
            }
            prev_sign = sign;
        }
    }
    let rms = (sum_sq / denom).sqrt();
    let pairs = f32::from(n_u16.saturating_sub(1).max(1));
    let zcr = f32::from(crossings) / pairs;
    (rms, zcr)
}

fn fft_512_in_place(buf: &mut [(f32, f32); SPEECH_FRAME_SAMPLES]) {
    let shift = usize::BITS - 9;
    for idx in 0..SPEECH_FRAME_SAMPLES {
        let rev = idx.reverse_bits() >> shift;
        if idx < rev {
            buf.swap(idx, rev);
        }
    }

    let mut half = 1usize;
    while half < SPEECH_FRAME_SAMPLES {
        let step = half * 2;
        let angle = -TAU / f32::from(u16::try_from(step).unwrap_or(512));
        let (w_step_im, w_step_re) = angle.sin_cos();
        let mut base = 0usize;
        while base < SPEECH_FRAME_SAMPLES {
            let (mut w_re, mut w_im) = (1.0_f32, 0.0_f32);
            for offset in 0..half {
                let even = base + offset;
                let odd = even + half;
                let (o_re, o_im) = buf[odd];
                let t_re = w_re * o_re - w_im * o_im;
                let t_im = w_re * o_im + w_im * o_re;
                let (e_re, e_im) = buf[even];
                buf[even] = (e_re + t_re, e_im + t_im);
                buf[odd] = (e_re - t_re, e_im - t_im);
                let next_re = w_re * w_step_re - w_im * w_step_im;
                w_im = w_re * w_step_im + w_im * w_step_re;
                w_re = next_re;
            }
            base += step;
        }
        half = step;
    }
}
