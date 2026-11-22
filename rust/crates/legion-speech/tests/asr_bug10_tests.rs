//! Tests for `legion_speech::asr` (`BUG-10` ASR + VAD error and retry isolation state machine).

use legion_speech::asr::{AsrEngine, AsrError, AsrSessionState, AsrTranscriberSession};
use legion_speech::pcm16_to_f32;
use legion_speech::types::{TranscriptPreview, TranscriptionOptions};
use legion_speech::vad::{VAD_FRAME_SAMPLES, VadConfig, VadError};

const WAVE_500HZ_PCM16: [i16; 32] = [
    0, 1951, 3827, 5556, 7071, 8315, 9239, 9808, 10000, 9808, 9239, 8315, 7071, 5556, 3827, 1951,
    0, -1951, -3827, -5556, -7071, -8315, -9239, -9808, -10000, -9808, -9239, -8315, -7071, -5556,
    -3827, -1951,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InjectedFailure {
    None,
    Transcribe,
    Feed,
}

#[derive(Debug)]
struct PoisonTrackingEngine {
    poisoned: bool,
    next_failure: InjectedFailure,
    stream_samples: Vec<f32>,
    reset_calls: u64,
    offline_calls: u64,
}

impl PoisonTrackingEngine {
    fn new(next_failure: InjectedFailure) -> Self {
        Self {
            poisoned: false,
            next_failure,
            stream_samples: Vec::new(),
            reset_calls: 0,
            offline_calls: 0,
        }
    }
}

impl AsrEngine for PoisonTrackingEngine {
    type Error = &'static str;

    fn begin_stream(&mut self, _options: &TranscriptionOptions) -> Result<(), Self::Error> {
        if self.poisoned {
            return Err("POISONED STATE LEAKED INTO BEGIN_STREAM");
        }
        self.stream_samples.clear();
        Ok(())
    }

    fn feed_stream(&mut self, pcm: &[f32]) -> Result<Option<TranscriptPreview>, Self::Error> {
        if self.poisoned {
            return Err("POISONED STATE LEAKED INTO FEED_STREAM");
        }
        if self.next_failure == InjectedFailure::Feed {
            self.next_failure = InjectedFailure::None;
            self.poisoned = true;
            return Err("feed_stream failed");
        }
        self.stream_samples.extend_from_slice(pcm);
        Ok(Some(TranscriptPreview::new(
            format!("chunk:{}", self.stream_samples.len()),
            "",
        )))
    }

    fn finish_stream(&mut self) -> Result<String, Self::Error> {
        if self.poisoned {
            return Err("POISONED STATE LEAKED INTO FINISH_STREAM");
        }
        Ok(format!("stream:{}", self.stream_samples.len()))
    }

    fn transcribe_offline(
        &mut self,
        pcm: &[f32],
        _options: &TranscriptionOptions,
    ) -> Result<String, Self::Error> {
        self.offline_calls += 1;
        if self.poisoned {
            return Err("POISONED STATE LEAKED INTO RETRY");
        }
        if self.next_failure == InjectedFailure::Transcribe {
            self.next_failure = InjectedFailure::None;
            self.poisoned = true;
            return Err("transcribe engine crashed");
        }
        Ok(format!("offline:{}", pcm.len()))
    }

    fn reset_asr(&mut self) {
        self.poisoned = false;
        self.stream_samples.clear();
        self.reset_calls += 1;
    }
}

fn voiced_f32(frames: usize) -> Vec<f32> {
    let count = frames * VAD_FRAME_SAMPLES;
    (0..count)
        .map(|i| pcm16_to_f32(WAVE_500HZ_PCM16[i % WAVE_500HZ_PCM16.len()]))
        .collect()
}

fn voiced_pcm16le(samples: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples * 2);
    for i in 0..samples {
        let q = WAVE_500HZ_PCM16[i % WAVE_500HZ_PCM16.len()];
        out.extend_from_slice(&q.to_le_bytes());
    }
    out
}

#[test]
fn bug10_transcribe_error_and_retry_reset_asr_and_rebuild_vad() -> Result<(), AsrError> {
    let engine = PoisonTrackingEngine::new(InjectedFailure::Transcribe);
    let mut session = AsrTranscriberSession::new(engine, VadConfig::default())?;
    assert_eq!(session.state(), AsrSessionState::Idle);
    assert_eq!(session.vad_generation_id(), 1);

    let opts = TranscriptionOptions::default();
    let first_audio = voiced_f32(3);
    let err = session.transcribe_f32(&first_audio, &opts);
    assert!(matches!(err, Err(AsrError::Engine { .. })));

    // BUG-10 invariant: on error, reset_asr() already ran, VAD rebuilt (gen=2), state is Faulted.
    assert_eq!(session.state(), AsrSessionState::Faulted);
    assert_eq!(session.reset_count(), 1);
    assert_eq!(session.engine().reset_calls, 1);
    assert!(!session.engine().poisoned);
    assert_eq!(session.vad_generation_id(), 2);
    assert_eq!(session.vad_rebuild_count(), 1);
    assert_eq!(session.vad().retained_pcm(), [0.0_f32; 0]);
    assert_eq!(session.vad().accumulator().pending_samples(), 0);

    // Faulted state blocks streaming entry points until recovery.
    assert!(matches!(
        session.begin_stream(opts.clone()),
        Err(AsrError::InvalidState {
            current: AsrSessionState::Faulted,
            ..
        })
    ));
    assert!(matches!(
        session.feed_stream_f32(&first_audio),
        Err(AsrError::InvalidState {
            current: AsrSessionState::Faulted,
            ..
        })
    ));
    assert!(matches!(
        session.finish_stream(),
        Err(AsrError::InvalidState {
            current: AsrSessionState::Faulted,
            ..
        })
    ));

    // Retrying transcribe directly from Faulted unconditionally invokes reset_asr() + rebuilds VAD again.
    let retry_audio = voiced_f32(2);
    let outcome = session.transcribe_f32(&retry_audio, &opts)?;
    assert_eq!(session.state(), AsrSessionState::Idle);
    assert_eq!(outcome.generation_id, 3);
    assert_eq!(outcome.text, format!("offline:{}", VAD_FRAME_SAMPLES * 2));
    assert_eq!(session.reset_count(), 2);
    assert_eq!(session.engine().reset_calls, 2);
    assert_eq!(session.vad_generation_id(), 3);
    assert_eq!(session.vad_rebuild_count(), 2);
    assert!(session.last_error().is_none());
    Ok(())
}

#[test]
fn bug10_transcribe_pcm_or_vad_error_clears_residual_bytes_before_retry() -> Result<(), AsrError> {
    let mut session = AsrTranscriberSession::new(
        PoisonTrackingEngine::new(InjectedFailure::None),
        VadConfig::default(),
    )?;
    let opts = TranscriptionOptions {
        speech_silence_filtering: false,
        ..TranscriptionOptions::default()
    };

    // Odd PCM16LE byte count (301 bytes) leaves a trailing carry byte and triggers VadError on flush.
    let odd_bytes = vec![0x7f_u8; 301];
    let err = session.transcribe_pcm16le(&odd_bytes, &opts);
    assert!(matches!(
        err,
        Err(AsrError::Vad(VadError::UnalignedTrailingPcm16Byte))
    ));
    assert_eq!(session.state(), AsrSessionState::Faulted);
    assert_eq!(session.reset_count(), 1);
    assert_eq!(session.vad_generation_id(), 2);
    assert!(!session.vad().accumulator().has_pending_byte());
    assert_eq!(session.vad().accumulator().pending_samples(), 0);
    assert_eq!(session.vad().retained_pcm(), [0.0_f32; 0]);

    // Subsequent NaN error during retry also cleans up without leaking.
    let nan_err = session.transcribe_f32(&[0.25, f32::NAN], &opts);
    assert!(matches!(nan_err, Err(AsrError::Vad(VadError::Pcm(_)))));
    assert_eq!(session.state(), AsrSessionState::Faulted);
    assert_eq!(session.vad_generation_id(), 4);

    // Final valid PCM16LE retry produces exact expected sample count (zero byte-shift corruption).
    let valid_pcm = voiced_pcm16le(320);
    let outcome = session.transcribe_pcm16le(&valid_pcm, &opts)?;
    assert_eq!(outcome.generation_id, 5);
    assert_eq!(outcome.text, "offline:320");
    assert_eq!(session.state(), AsrSessionState::Idle);
    Ok(())
}

#[test]
fn bug10_streaming_failures_clean_up_and_recover_via_reset() -> Result<(), AsrError> {
    let engine = PoisonTrackingEngine::new(InjectedFailure::Feed);
    let mut session = AsrTranscriberSession::new(engine, VadConfig::default())?;

    let opts = TranscriptionOptions::default();
    session.begin_stream(opts.clone())?;
    assert_eq!(session.state(), AsrSessionState::Streaming);

    // Feed an unaligned chunk (1024*2 + 1 bytes) + voiced frames so engine fails mid-stream.
    let mut voiced_plus_odd = voiced_pcm16le(VAD_FRAME_SAMPLES * 2);
    voiced_plus_odd.push(0x44);
    let err = session.feed_stream_pcm16le(&voiced_plus_odd);
    assert!(matches!(err, Err(AsrError::Engine { .. })));

    assert_eq!(session.state(), AsrSessionState::Faulted);
    assert_eq!(session.reset_count(), 1);
    assert!(!session.engine().poisoned);
    assert_eq!(session.vad_generation_id(), 2);
    assert!(!session.vad().accumulator().has_pending_byte());
    assert_eq!(session.vad().accumulator().pending_samples(), 0);

    // Recover via reset() -> begin_stream -> feed -> finish_stream.
    session.reset()?;
    assert_eq!(session.state(), AsrSessionState::Idle);
    assert_eq!(session.vad_generation_id(), 3);

    session.begin_stream(opts)?;
    let chunk = voiced_pcm16le(VAD_FRAME_SAMPLES * 2 + 100);
    let feed = session.feed_stream_pcm16le(&chunk)?;
    assert_eq!(feed.revision, 1);
    assert_eq!(feed.generation_id, 3);
    let finished = session.finish_stream()?;
    assert_eq!(
        finished.text,
        format!("stream:{}", VAD_FRAME_SAMPLES * 2 + 100)
    );
    assert_eq!(session.state(), AsrSessionState::Idle);
    Ok(())
}

#[test]
fn pure_silence_with_filtering_returns_empty_without_calling_offline_asr() -> Result<(), AsrError> {
    let mut session = AsrTranscriberSession::new(
        PoisonTrackingEngine::new(InjectedFailure::None),
        VadConfig::default(),
    )?;
    let silence = vec![0.0_f32; VAD_FRAME_SAMPLES * 4];
    let outcome = session.transcribe_f32(&silence, &TranscriptionOptions::default())?;
    assert_eq!(outcome.text, "");
    assert_eq!(session.engine().offline_calls, 0);
    assert_eq!(session.state(), AsrSessionState::Idle);
    Ok(())
}
