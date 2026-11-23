//! Generation-fenced `VAD` session state with pre-roll buffering and `BUG-10` disposal isolation.

use std::collections::VecDeque;

use crate::vad::{SileroStyleVad, VadFrameDecision, VadFrameEvent};
use crate::vad_frame::{VadConfig, VadError, VadFrame, VadFrameAccumulator};

/// Managed `VAD` session coupling [`VadFrameAccumulator`], [`SileroStyleVad`], pre-roll,
/// and generation-fenced disposal (`BUG-10`).
#[derive(Debug, Clone)]
pub struct VadSessionState {
    generation_id: u64,
    disposed: bool,
    accumulator: VadFrameAccumulator,
    detector: SileroStyleVad,
    prefill_ring: VecDeque<VadFrame>,
    retained_pcm: Vec<f32>,
}

impl VadSessionState {
    /// Creates an initial `generation_id = 1` [`VadSessionState`].
    pub fn new(config: VadConfig) -> Result<Self, VadError> {
        Self::with_generation(config, 1)
    }

    fn with_generation(config: VadConfig, generation_id: u64) -> Result<Self, VadError> {
        let detector = SileroStyleVad::new(config)?;
        let accumulator = VadFrameAccumulator::new(config.frame_samples);
        let prefill_ring = VecDeque::with_capacity(config.prefill_frames.saturating_add(1));
        Ok(Self {
            generation_id,
            disposed: false,
            accumulator,
            detector,
            prefill_ring,
            retained_pcm: Vec::new(),
        })
    }

    /// Returns the monotonic generation identifier incremented on every dispose/rebuild.
    pub fn generation_id(&self) -> u64 {
        self.generation_id
    }

    /// Returns `true` after [`Self::dispose`] or [`Self::rebuild_next`] has retired this instance.
    pub fn is_disposed(&self) -> bool {
        self.disposed
    }

    /// Returns a reference to the underlying frame accumulator.
    pub fn accumulator(&self) -> &VadFrameAccumulator {
        &self.accumulator
    }

    /// Returns a reference to the underlying [`SileroStyleVad`] detector.
    pub fn detector(&self) -> &SileroStyleVad {
        &self.detector
    }

    /// Returns the currently buffered pre-roll frames waiting for speech onset.
    pub fn prefill_len(&self) -> usize {
        self.prefill_ring.len()
    }

    /// Returns the retained `PCM` slice currently buffered for downstream `ASR` decoding.
    pub fn retained_pcm(&self) -> &[f32] {
        &self.retained_pcm
    }

    /// Drains and returns all newly retained `PCM` samples since the last take.
    pub fn take_retained_pcm(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.retained_pcm)
    }

    /// Feeds `PCM16LE` bytes through frame chunking and `VAD`, retaining speech (or all `PCM`).
    pub fn feed_pcm16le(
        &mut self,
        bytes: &[u8],
        filter_silence: bool,
    ) -> Result<Vec<VadFrameDecision>, VadError> {
        self.ensure_active()?;
        let frames = self.accumulator.push_pcm16le(bytes)?;
        self.score_frames(frames, filter_silence)
    }

    /// Feeds normalized `f32` samples through frame chunking and `VAD`.
    pub fn feed_f32(
        &mut self,
        samples: &[f32],
        filter_silence: bool,
    ) -> Result<Vec<VadFrameDecision>, VadError> {
        self.ensure_active()?;
        let frames = self.accumulator.push_f32(samples)?;
        self.score_frames(frames, filter_silence)
    }

    /// Flushes any partial trailing frame, preserves short onset utterances, and closes open speech.
    pub fn flush(&mut self, filter_silence: bool) -> Result<Vec<VadFrameDecision>, VadError> {
        self.ensure_active()?;
        let partial = self.accumulator.flush()?;
        let mut decisions = Vec::new();
        if let Some(frame) = partial {
            let had_onset = self.detector.onset_count() > 0;
            let decision = self.detector.process_frame(&frame.samples)?;
            if !filter_silence {
                self.retained_pcm.extend_from_slice(frame.valid_slice());
            } else if matches!(decision.event, VadFrameEvent::OnsetPending) {
                self.flush_prefill_into_retained();
                self.retained_pcm.extend_from_slice(frame.valid_slice());
            } else {
                if had_onset && matches!(decision.event, VadFrameEvent::Silence) {
                    self.flush_prefill_into_retained();
                }
                self.apply_frame_decision(frame, &decision, true);
            }
            decisions.push(decision);
        } else if filter_silence && self.detector.onset_count() > 0 {
            self.flush_prefill_into_retained();
        }
        self.detector.close_open_segment();
        self.prefill_ring.clear();
        Ok(decisions)
    }

    /// Clears residual buffers and resets detector state in place while keeping `generation_id`.
    pub fn clear_buffers_and_reset_detector(&mut self) -> Result<(), VadError> {
        self.ensure_active()?;
        self.accumulator.clear();
        self.detector.reset();
        self.prefill_ring.clear();
        self.retained_pcm.clear();
        Ok(())
    }

    /// Disposes this `VAD` session, clearing all residual `PCM` buffers and blocking further feeds.
    pub fn dispose(&mut self) {
        self.disposed = true;
        self.accumulator.clear();
        self.detector.reset();
        self.prefill_ring.clear();
        self.retained_pcm.clear();
    }

    /// Disposes `self` and returns a clean successor [`VadSessionState`] with `generation_id + 1`.
    pub fn rebuild_next(&mut self) -> Result<Self, VadError> {
        let next_gen = self.generation_id.saturating_add(1);
        let config = *self.detector.config();
        self.dispose();
        Self::with_generation(config, next_gen)
    }

    fn ensure_active(&self) -> Result<(), VadError> {
        if self.disposed {
            return Err(VadError::SessionDisposed {
                generation_id: self.generation_id,
            });
        }
        Ok(())
    }

    fn score_frames(
        &mut self,
        frames: Vec<VadFrame>,
        filter_silence: bool,
    ) -> Result<Vec<VadFrameDecision>, VadError> {
        let mut decisions = Vec::with_capacity(frames.len());
        for frame in frames {
            let decision = self.detector.process_frame(&frame.samples)?;
            self.apply_frame_decision(frame, &decision, filter_silence);
            decisions.push(decision);
        }
        Ok(decisions)
    }

    fn apply_frame_decision(
        &mut self,
        frame: VadFrame,
        decision: &VadFrameDecision,
        filter_silence: bool,
    ) {
        if !filter_silence {
            self.retained_pcm.extend_from_slice(frame.valid_slice());
            return;
        }
        match decision.event {
            VadFrameEvent::SpeechStart => {
                self.flush_prefill_into_retained();
                self.retained_pcm.extend_from_slice(frame.valid_slice());
            }
            VadFrameEvent::SpeechContinue | VadFrameEvent::SpeechHangover => {
                self.retained_pcm.extend_from_slice(frame.valid_slice());
            }
            VadFrameEvent::Silence | VadFrameEvent::OnsetPending | VadFrameEvent::SpeechEnd => {
                let cap = self.detector.config().prefill_frames;
                if cap > 0 {
                    while self.prefill_ring.len() >= cap {
                        self.prefill_ring.pop_front();
                    }
                    self.prefill_ring.push_back(frame);
                }
            }
        }
    }

    fn flush_prefill_into_retained(&mut self) {
        while let Some(prefill) = self.prefill_ring.pop_front() {
            self.retained_pcm.extend_from_slice(prefill.valid_slice());
        }
    }
}
