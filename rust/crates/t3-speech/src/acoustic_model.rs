//! [`AcousticAsrEngine`] implementing [`AsrEngine`] over `.t3asr` weights with recursive binary
//! midpoint splitting (`native.ts:69-79`) and unconditional `reset_asr()` clearing (`BUG-10`).

use crate::acoustic_features::{AcousticFrameAnalyzer, AcousticFrameFeatures, NUM_ACOUSTIC_BANDS};
pub use crate::acoustic_weights::{
    AcousticModelError, AcousticModelWeights, AcousticPrototype, DEFAULT_MAX_AUDIO_SAMPLES,
    MIN_SPLIT_AUDIO_SAMPLES, T3ASR_BINARY_MAGIC,
};
use crate::asr::AsrEngine;
use crate::types::{TranscriptPreview, TranscriptionOptions};

/// Pure Safe Rust acoustic prototype `ASR` backend implementing [`AsrEngine`].
///
/// # Examples
///
/// ```
/// use t3_speech::acoustic_model::AcousticAsrEngine;
/// use t3_speech::asr::AsrEngine;
/// use t3_speech::types::TranscriptionOptions;
///
/// let mut engine = AcousticAsrEngine::with_default_english();
/// assert_eq!(engine.transcribe_offline(&[0.0_f32; 512], &TranscriptionOptions::default())?, "");
/// # Ok::<(), t3_speech::acoustic_model::AcousticModelError>(())
/// ```
#[derive(Debug, Clone)]
pub struct AcousticAsrEngine {
    weights: AcousticModelWeights,
    analyzer: AcousticFrameAnalyzer,
    stream_pcm: Vec<f32>,
    committed_tokens: Vec<String>,
    tentative_token: Option<String>,
    active_options: Option<TranscriptionOptions>,
    stream_active: bool,
    split_count: u64,
    reset_count: u64,
}

impl Default for AcousticAsrEngine {
    fn default() -> Self {
        Self::with_default_english()
    }
}

impl AcousticAsrEngine {
    /// Creates an [`AcousticAsrEngine`] validated against `weights`.
    pub fn new(weights: AcousticModelWeights) -> Result<Self, AcousticModelError> {
        weights.validate()?;
        let analyzer = AcousticFrameAnalyzer::new(weights.min_voiced_rms);
        Ok(Self {
            weights,
            analyzer,
            stream_pcm: Vec::new(),
            committed_tokens: Vec::new(),
            tentative_token: None,
            active_options: None,
            stream_active: false,
            split_count: 0,
            reset_count: 0,
        })
    }

    /// Creates an [`AcousticAsrEngine`] backed by the built-in English prototype inventory.
    pub fn with_default_english() -> Self {
        let weights = AcousticModelWeights::default_english();
        let analyzer = AcousticFrameAnalyzer::new(weights.min_voiced_rms);
        Self {
            weights,
            analyzer,
            stream_pcm: Vec::new(),
            committed_tokens: Vec::new(),
            tentative_token: None,
            active_options: None,
            stream_active: false,
            split_count: 0,
            reset_count: 0,
        }
    }

    /// Returns the loaded [`AcousticModelWeights`].
    pub fn weights(&self) -> &AcousticModelWeights {
        &self.weights
    }

    /// Returns the cumulative count of recursive binary midpoint splits executed.
    pub fn split_count(&self) -> u64 {
        self.split_count
    }

    /// Returns the total number of [`AsrEngine::reset_asr`] resets executed.
    pub fn reset_count(&self) -> u64 {
        self.reset_count
    }

    fn decode_recursive(
        &mut self,
        pcm: &[f32],
        options: &TranscriptionOptions,
    ) -> Result<Vec<String>, AcousticModelError> {
        if pcm.is_empty() {
            return Ok(Vec::new());
        }
        if pcm.len() > self.weights.max_audio_samples && pcm.len() > MIN_SPLIT_AUDIO_SAMPLES {
            self.split_count = self.split_count.saturating_add(1);
            let mid = pcm.len() / 2;
            let mut left = self.decode_recursive(&pcm[..mid], options)?;
            let right = self.decode_recursive(&pcm[mid..], options)?;
            for tok in right {
                if left.last().is_none_or(|prev| prev != &tok) {
                    left.push(tok);
                }
            }
            return Ok(left);
        }
        let frames = self.analyzer.analyze_slice(pcm)?;
        Ok(decode_frames_to_tokens(&self.weights, &frames, options))
    }
}

impl AsrEngine for AcousticAsrEngine {
    type Error = AcousticModelError;

    fn begin_stream(&mut self, options: &TranscriptionOptions) -> Result<(), Self::Error> {
        self.stream_pcm.clear();
        self.committed_tokens.clear();
        self.tentative_token = None;
        self.active_options = Some(options.clone());
        self.stream_active = true;
        Ok(())
    }

    fn feed_stream(&mut self, pcm: &[f32]) -> Result<Option<TranscriptPreview>, Self::Error> {
        if !self.stream_active {
            return Err(AcousticModelError::StreamNotActive);
        }
        let frames = self.analyzer.analyze_slice(pcm)?;
        self.stream_pcm.extend_from_slice(pcm);
        let opts = self.active_options.clone().unwrap_or_default();
        let new_tokens = decode_frames_to_tokens(&self.weights, &frames, &opts);
        if new_tokens.is_empty() {
            return Ok(None);
        }
        if let Some(prev) = self.tentative_token.take()
            && self.committed_tokens.last().is_none_or(|t| t != &prev)
        {
            self.committed_tokens.push(prev);
        }
        let last_idx = new_tokens.len() - 1;
        for tok in &new_tokens[..last_idx] {
            if self.committed_tokens.last().is_none_or(|t| t != tok) {
                self.committed_tokens.push(tok.clone());
            }
        }
        self.tentative_token = Some(new_tokens[last_idx].clone());
        let committed = if self.committed_tokens.is_empty() {
            String::new()
        } else {
            format!("{} ", self.committed_tokens.join(" "))
        };
        Ok(Some(TranscriptPreview::new(
            committed,
            self.tentative_token.clone().unwrap_or_default(),
        )))
    }

    fn finish_stream(&mut self) -> Result<String, Self::Error> {
        if !self.stream_active {
            return Err(AcousticModelError::StreamNotActive);
        }
        let pcm = std::mem::take(&mut self.stream_pcm);
        let opts = self.active_options.take().unwrap_or_default();
        self.committed_tokens.clear();
        self.tentative_token = None;
        self.stream_active = false;
        let tokens = self.decode_recursive(&pcm, &opts)?;
        Ok(tokens.join(" "))
    }

    fn transcribe_offline(
        &mut self,
        pcm: &[f32],
        options: &TranscriptionOptions,
    ) -> Result<String, Self::Error> {
        let tokens = self.decode_recursive(pcm, options)?;
        Ok(tokens.join(" "))
    }

    fn reset_asr(&mut self) {
        self.stream_pcm.clear();
        self.committed_tokens.clear();
        self.tentative_token = None;
        self.active_options = None;
        self.stream_active = false;
        self.reset_count = self.reset_count.saturating_add(1);
    }
}

fn decode_frames_to_tokens(
    weights: &AcousticModelWeights,
    frames: &[AcousticFrameFeatures],
    options: &TranscriptionOptions,
) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut group: Vec<&AcousticFrameFeatures> = Vec::new();
    let mut centroid_sum = 0.0_f32;

    for frame in frames {
        if !frame.voiced {
            if !group.is_empty() {
                push_best_token(weights, &group, options, &mut tokens);
                group.clear();
                centroid_sum = 0.0_f32;
            }
            continue;
        }
        if !group.is_empty() {
            let len_f32 =
                f32::from(u16::try_from(group.len().min(usize::from(u16::MAX))).unwrap_or(1));
            let mean_centroid = centroid_sum / len_f32;
            if (frame.spectral_centroid_hz - mean_centroid).abs() > 350.0_f32 {
                push_best_token(weights, &group, options, &mut tokens);
                group.clear();
                centroid_sum = 0.0_f32;
            }
        }
        centroid_sum += frame.spectral_centroid_hz;
        group.push(frame);
    }
    if !group.is_empty() {
        push_best_token(weights, &group, options, &mut tokens);
    }
    tokens
}

fn push_best_token(
    weights: &AcousticModelWeights,
    group: &[&AcousticFrameFeatures],
    options: &TranscriptionOptions,
    out: &mut Vec<String>,
) {
    let count = f32::from(u16::try_from(group.len().min(usize::from(u16::MAX))).unwrap_or(1));
    let mut centroid = 0.0_f32;
    let mut zcr = 0.0_f32;
    let mut bands = [0.0_f32; NUM_ACOUSTIC_BANDS];
    for frame in group {
        centroid += frame.spectral_centroid_hz;
        zcr += frame.zero_crossing_rate;
        for (dst, &src) in bands.iter_mut().zip(frame.band_log_energies.iter()) {
            *dst += src;
        }
    }
    centroid /= count;
    zcr /= count;
    for b in &mut bands {
        *b /= count;
    }
    let band_mean =
        bands.iter().sum::<f32>() / f32::from(u16::try_from(NUM_ACOUSTIC_BANDS).unwrap_or(8));
    for b in &mut bands {
        *b -= band_mean;
    }

    let mut best: Option<(&str, f32)> = None;
    for p in &weights.prototypes {
        let dc = (centroid - p.centroid_hz) / 1_000.0_f32;
        let dz = (zcr - p.zcr) * 4.0_f32;
        let db: f32 = bands
            .iter()
            .zip(p.band_envelope.iter())
            .map(|(a, b)| (a - b) * (a - b))
            .sum();
        let dist = (dc * dc + dz * dz + db * 0.15_f32) / p.weight;
        if best.is_none_or(|(_, best_d)| dist < best_d) {
            best = Some((p.token.as_str(), dist));
        }
    }

    if let Some((raw_tok, _)) = best {
        let resolved = apply_custom_alias(raw_tok, options);
        if out.last().is_none_or(|prev| prev != &resolved) {
            out.push(resolved);
        }
    }
}

fn apply_custom_alias(token: &str, options: &TranscriptionOptions) -> String {
    let lower = token.to_ascii_lowercase();
    for entry in &options.speech_custom_words {
        if entry.aliases.iter().any(|a| a.eq_ignore_ascii_case(&lower)) {
            return entry.term.clone();
        }
    }
    token.to_owned()
}
