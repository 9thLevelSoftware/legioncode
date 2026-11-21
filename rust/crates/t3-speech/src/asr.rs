//! `BUG-10` `ASR` transcriber state machine with unconditional `ASR` + `VAD` reset on error or retry.

pub use crate::asr_types::{
    AsrEngine, AsrError, AsrSessionState, StreamFeedOutcome, StreamFinishOutcome, TranscribeOutcome,
};
use crate::types::TranscriptionOptions;
use crate::vad::{VadConfig, VadFrameDecision, VadSessionState};

/// Safe Rust `BUG-10` `ASR` + `VAD` coordinator guaranteeing clean retry/error isolation.
///
/// On **any** `"transcribe"` error or retry (as well as any streaming failure), this session
/// unconditionally calls [`AsrEngine::reset_asr`] AND disposes/rebuilds the active
/// [`VadSessionState`] (incrementing `generation_id` and clearing all residual `PCM` buffers),
/// preventing poisoned native or `VAD` state from leaking into retries.
#[derive(Debug)]
pub struct AsrTranscriberSession<E: AsrEngine> {
    engine: E,
    state: AsrSessionState,
    vad: VadSessionState,
    reset_count: u64,
    vad_rebuild_count: u64,
    revision: u64,
    active_options: TranscriptionOptions,
    last_error: Option<String>,
}

impl<E: AsrEngine> AsrTranscriberSession<E> {
    /// Creates a new session in [`AsrSessionState::Idle`] with `vad_generation_id() == 1`.
    pub fn new(engine: E, vad_config: VadConfig) -> Result<Self, AsrError> {
        let vad = VadSessionState::new(vad_config)?;
        Ok(Self {
            engine,
            state: AsrSessionState::Idle,
            vad,
            reset_count: 0,
            vad_rebuild_count: 0,
            revision: 0,
            active_options: TranscriptionOptions::default(),
            last_error: None,
        })
    }

    /// Returns the current [`AsrSessionState`].
    pub fn state(&self) -> AsrSessionState {
        self.state
    }

    /// Returns the active [`VadSessionState`] monotonic generation identifier.
    pub fn vad_generation_id(&self) -> u64 {
        self.vad.generation_id()
    }

    /// Returns a reference to the active [`VadSessionState`].
    pub fn vad(&self) -> &VadSessionState {
        &self.vad
    }

    /// Returns a reference to the underlying [`AsrEngine`].
    pub fn engine(&self) -> &E {
        &self.engine
    }

    /// Returns a mutable reference to the underlying [`AsrEngine`].
    pub fn engine_mut(&mut self) -> &mut E {
        &mut self.engine
    }

    /// Returns the total number of [`AsrEngine::reset_asr`] calls issued by this session.
    pub fn reset_count(&self) -> u64 {
        self.reset_count
    }

    /// Returns the number of times [`VadSessionState`] has been disposed and rebuilt.
    pub fn vad_rebuild_count(&self) -> u64 {
        self.vad_rebuild_count
    }

    /// Returns the diagnostic message from the most recent fault, if any.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Unconditionally invokes `reset_asr()`, disposes/rebuilds `VAD`, and returns to `Idle`.
    pub fn reset(&mut self) -> Result<(), AsrError> {
        self.reset_and_rebuild_vad()?;
        self.last_error = None;
        self.state = AsrSessionState::Idle;
        Ok(())
    }

    /// Starts a streaming session (`Idle -> Streaming`), resetting `ASR`+`VAD` if setup fails.
    pub fn begin_stream(&mut self, options: TranscriptionOptions) -> Result<(), AsrError> {
        if self.state != AsrSessionState::Idle {
            return Err(AsrError::InvalidState {
                current: self.state,
                action: "begin_stream",
            });
        }
        if let Err(err) = self.vad.clear_buffers_and_reset_detector() {
            return Err(self.fault_with(err));
        }
        if let Err(err) = self.engine.begin_stream(&options) {
            let asr_err = AsrError::Engine {
                message: err.to_string(),
            };
            return Err(self.fault_with(asr_err));
        }
        self.active_options = options;
        self.revision = 0;
        self.last_error = None;
        self.state = AsrSessionState::Streaming;
        Ok(())
    }

    /// Feeds raw `16 kHz` `PCM16LE` bytes into an active streaming session.
    pub fn feed_stream_pcm16le(&mut self, bytes: &[u8]) -> Result<StreamFeedOutcome, AsrError> {
        self.ensure_streaming("feed_stream_pcm16le")?;
        let filter_silence = self.active_options.speech_silence_filtering;
        match self.vad.feed_pcm16le(bytes, filter_silence) {
            Ok(decisions) => self.forward_retained_stream(decisions),
            Err(err) => Err(self.fault_with(err)),
        }
    }

    /// Feeds pre-normalized `16 kHz` `f32` samples into an active streaming session.
    pub fn feed_stream_f32(&mut self, samples: &[f32]) -> Result<StreamFeedOutcome, AsrError> {
        self.ensure_streaming("feed_stream_f32")?;
        let filter_silence = self.active_options.speech_silence_filtering;
        match self.vad.feed_f32(samples, filter_silence) {
            Ok(decisions) => self.forward_retained_stream(decisions),
            Err(err) => Err(self.fault_with(err)),
        }
    }

    /// Flushes `VAD` tail frames and finalizes the active stream (`Streaming -> Idle`).
    pub fn finish_stream(&mut self) -> Result<StreamFinishOutcome, AsrError> {
        self.ensure_streaming("finish_stream")?;
        let filter_silence = self.active_options.speech_silence_filtering;
        let vad_decisions = match self.vad.flush(filter_silence) {
            Ok(decisions) => decisions,
            Err(err) => return Err(self.fault_with(err)),
        };
        let tail_pcm = self.vad.take_retained_pcm();
        if !tail_pcm.is_empty()
            && let Err(err) = self.engine.feed_stream(&tail_pcm)
        {
            let asr_err = AsrError::Engine {
                message: err.to_string(),
            };
            return Err(self.fault_with(asr_err));
        }
        let text = match self.engine.finish_stream() {
            Ok(text) => text,
            Err(err) => {
                let asr_err = AsrError::Engine {
                    message: err.to_string(),
                };
                return Err(self.fault_with(asr_err));
            }
        };
        let generation_id = self.vad.generation_id();
        if let Err(err) = self.vad.clear_buffers_and_reset_detector() {
            return Err(self.fault_with(err));
        }
        self.engine.reset_asr();
        self.reset_count = self.reset_count.saturating_add(1);
        self.state = AsrSessionState::Idle;
        Ok(StreamFinishOutcome {
            generation_id,
            text,
            vad_decisions,
        })
    }

    /// Executes a one-shot offline transcription from `PCM16LE` bytes (`Idle | Faulted -> Idle`).
    ///
    /// Retrying from `Faulted` unconditionally invokes `reset_asr()` and rebuilds `VAD` before
    /// execution. Any failure unconditionally invokes `reset_asr()`, disposes/rebuilds `VAD`,
    /// clears residual `PCM` buffers, and transitions to [`AsrSessionState::Faulted`].
    pub fn transcribe_pcm16le(
        &mut self,
        bytes: &[u8],
        options: &TranscriptionOptions,
    ) -> Result<TranscribeOutcome, AsrError> {
        self.prepare_transcribe()?;
        let filter = options.speech_silence_filtering;
        let mut decisions = match self.vad.feed_pcm16le(bytes, filter) {
            Ok(decisions) => decisions,
            Err(err) => return Err(self.fault_with(err)),
        };
        self.finish_transcribe(&mut decisions, options)
    }

    /// Executes a one-shot offline transcription from normalized `f32` samples (`Idle | Faulted -> Idle`).
    ///
    /// Retrying from `Faulted` unconditionally invokes `reset_asr()` and rebuilds `VAD` before
    /// execution. Any failure unconditionally invokes `reset_asr()`, disposes/rebuilds `VAD`,
    /// clears residual `PCM` buffers, and transitions to [`AsrSessionState::Faulted`].
    pub fn transcribe_f32(
        &mut self,
        samples: &[f32],
        options: &TranscriptionOptions,
    ) -> Result<TranscribeOutcome, AsrError> {
        self.prepare_transcribe()?;
        let filter = options.speech_silence_filtering;
        let mut decisions = match self.vad.feed_f32(samples, filter) {
            Ok(decisions) => decisions,
            Err(err) => return Err(self.fault_with(err)),
        };
        self.finish_transcribe(&mut decisions, options)
    }

    fn ensure_streaming(&self, action: &'static str) -> Result<(), AsrError> {
        if self.state != AsrSessionState::Streaming {
            return Err(AsrError::InvalidState {
                current: self.state,
                action,
            });
        }
        Ok(())
    }

    fn forward_retained_stream(
        &mut self,
        vad_decisions: Vec<VadFrameDecision>,
    ) -> Result<StreamFeedOutcome, AsrError> {
        let retained = self.vad.take_retained_pcm();
        let preview = if retained.is_empty() {
            None
        } else {
            match self.engine.feed_stream(&retained) {
                Ok(preview) => preview,
                Err(err) => {
                    let asr_err = AsrError::Engine {
                        message: err.to_string(),
                    };
                    return Err(self.fault_with(asr_err));
                }
            }
        };
        self.revision = self.revision.saturating_add(1);
        Ok(StreamFeedOutcome {
            generation_id: self.vad.generation_id(),
            revision: self.revision,
            preview,
            vad_decisions,
        })
    }

    fn prepare_transcribe(&mut self) -> Result<(), AsrError> {
        match self.state {
            AsrSessionState::Idle => {
                if let Err(err) = self.vad.clear_buffers_and_reset_detector() {
                    return Err(self.fault_with(err));
                }
            }
            AsrSessionState::Faulted => {
                // Retry from Faulted unconditionally resets ASR and rebuilds VAD before executing.
                self.reset_and_rebuild_vad()?;
            }
            AsrSessionState::Streaming | AsrSessionState::Transcribing => {
                return Err(AsrError::InvalidState {
                    current: self.state,
                    action: "transcribe",
                });
            }
        }
        self.state = AsrSessionState::Transcribing;
        Ok(())
    }

    fn finish_transcribe(
        &mut self,
        decisions: &mut Vec<VadFrameDecision>,
        options: &TranscriptionOptions,
    ) -> Result<TranscribeOutcome, AsrError> {
        match self.vad.flush(options.speech_silence_filtering) {
            Ok(tail) => decisions.extend(tail),
            Err(err) => return Err(self.fault_with(err)),
        }
        let pcm = self.vad.take_retained_pcm();
        let text = if pcm.is_empty() {
            String::new()
        } else {
            match self.engine.transcribe_offline(&pcm, options) {
                Ok(text) => text,
                Err(err) => {
                    let asr_err = AsrError::Engine {
                        message: err.to_string(),
                    };
                    return Err(self.fault_with(asr_err));
                }
            }
        };
        let generation_id = self.vad.generation_id();
        if let Err(err) = self.vad.clear_buffers_and_reset_detector() {
            return Err(self.fault_with(err));
        }
        self.last_error = None;
        self.state = AsrSessionState::Idle;
        Ok(TranscribeOutcome {
            generation_id,
            text,
            vad_decisions: std::mem::take(decisions),
        })
    }

    fn reset_and_rebuild_vad(&mut self) -> Result<(), AsrError> {
        self.engine.reset_asr();
        self.reset_count = self.reset_count.saturating_add(1);
        self.vad = self.vad.rebuild_next()?;
        self.vad_rebuild_count = self.vad_rebuild_count.saturating_add(1);
        Ok(())
    }

    fn fault_with(&mut self, err: impl Into<AsrError>) -> AsrError {
        let error = err.into();
        self.last_error = Some(error.to_string());
        let _ = self.reset_and_rebuild_vad();
        self.state = AsrSessionState::Faulted;
        error
    }
}
