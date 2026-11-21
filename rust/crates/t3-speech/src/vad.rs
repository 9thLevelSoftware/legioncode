//! Deterministic `Silero`-style `VAD` endpoint detector and generation-fenced [`VadSessionState`].

pub use crate::vad_frame::{
    VAD_FRAME_SAMPLES, VAD_SAMPLE_RATE_HZ, VadConfig, VadError, VadFrame, VadFrameAccumulator,
};
pub use crate::vad_session::VadSessionState;

/// Endpoint detector transition produced for a single `VAD` frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VadFrameEvent {
    /// Non-speech frame outside onset accumulation and hangover.
    Silence,
    /// Voiced energy observed while accumulating toward `onset_frames`.
    OnsetPending,
    /// Speech onset confirmed (`onset_frames` consecutive voiced frames reached).
    SpeechStart,
    /// Active voiced speech frame continuing an open segment.
    SpeechContinue,
    /// Silent frame retained during the post-speech `hangover_frames` tail window.
    SpeechHangover,
    /// Hangover expired (or stream flushed); open speech segment closed.
    SpeechEnd,
}

/// Deterministic acoustic metrics and endpoint decision for a single `VAD` frame.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VadFrameDecision {
    /// Zero-based frame counter within the current [`VadSessionState`] generation.
    pub frame_index: u64,
    /// Root-mean-square amplitude of the frame in `[0.0, 1.0]`.
    pub rms_energy: f32,
    /// Normalized zero-crossing rate in `[0.0, 1.0]`.
    pub zero_crossing_rate: f32,
    /// Combined deterministic speech probability in `[0.0, 1.0]`.
    pub probability: f32,
    /// Whether this frame belongs to active speech (onset, continuation, or hangover).
    pub is_speech: bool,
    /// Whether this frame is kept purely via the post-speech hangover window.
    pub in_hangover: bool,
    /// State-machine event emitted for this frame.
    pub event: VadFrameEvent,
}

/// Deterministic `16 kHz` energy + zero-crossing speech onset/hangover detector.
#[derive(Debug, Clone)]
pub struct SileroStyleVad {
    config: VadConfig,
    in_speech: bool,
    onset_count: usize,
    hangover_remaining: usize,
    frame_index: u64,
    active_speech_frames: u64,
    segments_completed: u64,
}

impl SileroStyleVad {
    /// Validates `config` and constructs a fresh [`SileroStyleVad`] detector.
    pub fn new(config: VadConfig) -> Result<Self, VadError> {
        config.validate()?;
        Ok(Self {
            config,
            in_speech: false,
            onset_count: 0,
            hangover_remaining: 0,
            frame_index: 0,
            active_speech_frames: 0,
            segments_completed: 0,
        })
    }

    /// Returns the detector configuration.
    pub fn config(&self) -> &VadConfig {
        &self.config
    }

    /// Returns `true` when currently inside an active speech segment or hangover tail.
    pub fn is_in_speech(&self) -> bool {
        self.in_speech
    }

    /// Returns the number of consecutive pre-onset voiced frames currently accumulated.
    pub fn onset_count(&self) -> usize {
        self.onset_count
    }

    /// Returns remaining hangover frames before the current speech segment ends.
    pub fn hangover_remaining(&self) -> usize {
        self.hangover_remaining
    }

    /// Returns the number of frames processed in the current detector lifetime.
    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// Returns the count of frames marked as `is_speech == true`.
    pub fn active_speech_frames(&self) -> u64 {
        self.active_speech_frames
    }

    /// Returns the number of completed speech segments (`SpeechStart` -> `SpeechEnd`).
    pub fn segments_completed(&self) -> u64 {
        self.segments_completed
    }

    /// Resets all onset, hangover, frame index, and segment counters back to initial silence.
    pub fn reset(&mut self) {
        self.in_speech = false;
        self.onset_count = 0;
        self.hangover_remaining = 0;
        self.frame_index = 0;
        self.active_speech_frames = 0;
        self.segments_completed = 0;
    }

    /// Scores one exact `frame_samples`-length frame and advances onset/hangover state.
    pub fn process_frame(&mut self, frame: &[f32]) -> Result<VadFrameDecision, VadError> {
        if frame.len() != self.config.frame_samples {
            return Err(VadError::InvalidFrameLength {
                expected: self.config.frame_samples,
                actual: frame.len(),
            });
        }
        crate::types::validate_f32_samples(frame)?;

        let (rms_energy, zero_crossing_rate, probability) = score_frame(&self.config, frame);
        let raw_voiced = probability >= self.config.speech_threshold;
        let idx = self.frame_index;
        self.frame_index = self.frame_index.saturating_add(1);

        let (is_speech, in_hangover, event) = if !self.in_speech {
            if raw_voiced {
                self.onset_count = self.onset_count.saturating_add(1);
                if self.onset_count >= self.config.onset_frames {
                    self.in_speech = true;
                    self.onset_count = 0;
                    self.hangover_remaining = self.config.hangover_frames;
                    self.active_speech_frames = self.active_speech_frames.saturating_add(1);
                    (true, false, VadFrameEvent::SpeechStart)
                } else {
                    (false, false, VadFrameEvent::OnsetPending)
                }
            } else {
                self.onset_count = 0;
                (false, false, VadFrameEvent::Silence)
            }
        } else if raw_voiced {
            self.hangover_remaining = self.config.hangover_frames;
            self.active_speech_frames = self.active_speech_frames.saturating_add(1);
            (true, false, VadFrameEvent::SpeechContinue)
        } else if self.hangover_remaining > 0 {
            self.hangover_remaining -= 1;
            self.active_speech_frames = self.active_speech_frames.saturating_add(1);
            (true, true, VadFrameEvent::SpeechHangover)
        } else {
            self.in_speech = false;
            self.onset_count = 0;
            self.segments_completed = self.segments_completed.saturating_add(1);
            (false, false, VadFrameEvent::SpeechEnd)
        };

        Ok(VadFrameDecision {
            frame_index: idx,
            rms_energy,
            zero_crossing_rate,
            probability,
            is_speech,
            in_hangover,
            event,
        })
    }

    /// Closes an open speech segment at stream end without fabricating an audio frame.
    pub fn close_open_segment(&mut self) -> bool {
        if self.in_speech {
            self.in_speech = false;
            self.hangover_remaining = 0;
            self.onset_count = 0;
            self.segments_completed = self.segments_completed.saturating_add(1);
            true
        } else {
            self.onset_count = 0;
            false
        }
    }
}

fn score_frame(config: &VadConfig, frame: &[f32]) -> (f32, f32, f32) {
    let sample_count = u16::try_from(frame.len()).unwrap_or(u16::MAX);
    let denom = f32::from(sample_count.max(1));
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
    let pairs = f32::from(sample_count.saturating_sub(1).max(1));
    let zcr = f32::from(crossings) / pairs;

    let energy_score = if rms <= config.min_rms_energy {
        0.0_f32
    } else {
        ((rms - config.min_rms_energy) / (config.target_rms_energy - config.min_rms_energy))
            .clamp(0.0_f32, 1.0_f32)
    };

    let zcr_score = if zcr >= config.min_zcr && zcr <= config.max_zcr {
        1.0_f32
    } else if zcr < config.min_zcr {
        (zcr / config.min_zcr.max(1e-6_f32)).clamp(0.0_f32, 1.0_f32) * 0.25_f32
    } else {
        ((1.0_f32 - zcr) / (1.0_f32 - config.max_zcr).max(1e-6_f32)).clamp(0.0_f32, 1.0_f32)
            * 0.4_f32
    };

    let probability = (energy_score * zcr_score).clamp(0.0_f32, 1.0_f32);
    (rms, zcr, probability)
}
