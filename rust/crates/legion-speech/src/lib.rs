#![forbid(unsafe_code)]
//! Pure Safe Rust speech `ASR`/`VAD` state machine, `WAV` codec/resampler, `DSP` acoustic
//! feature/prototype `ASR`, backend/model discovery, `/ws/voice` WebSocket protocol state machine,
//! and `legioncode __speech-worker` `NDJSON` codec for `LegionCode` (`BUG-10`).
//!
//! # Key Guarantees (`BUG-10`)
//!
//! - [`asr::AsrTranscriberSession`] tracks explicit `Idle` / `Streaming` / `Transcribing` /
//!   `Faulted` lifecycle states and unconditionally invokes `reset_asr()` plus disposes/rebuilds
//!   the active [`vad::VadSessionState`] (incrementing `generation_id` and clearing residual
//!   `PCM` buffers) on **any** `"transcribe"` error or retry.
//! - [`vad::VadFrameAccumulator`] & [`vad::SileroStyleVad`] convert `16 kHz` `PCM16LE` -> `f32`,
//!   chunk into `512`-sample frames, and run deterministic energy + zero-crossing onset/hangover
//!   endpointing that resets cleanly on disposal.
//! - [`voice_ws::VoiceWsSession`] enforces the `10`-minute (`600_000 ms`) timeout and audio
//!   buffer budget using an injectable [`voice_ws::VoiceClock`], always emitting
//!   [`voice_ws::VoiceServerEvent::Error`] JSON **before** [`voice_ws::CloseFrame`] and entering
//!   `Closing`.
//! - [`worker_protocol`] provides typed [`worker_protocol::WorkerCommand`] /
//!   [`worker_protocol::WorkerEvent`] `NDJSON` codecs and [`worker_protocol::run_worker_stdio`].
//!
//! # Examples
//!
//! ```
//! use legion_speech::asr::{AsrEngine, AsrSessionState, AsrTranscriberSession};
//! use legion_speech::types::{TranscriptPreview, TranscriptionOptions};
//! use legion_speech::vad::VadConfig;
//! use legion_speech::voice_ws::{
//!     DEFAULT_MAX_SESSION_DURATION_MS, VirtualClock, VoiceClientFrame, VoiceErrorCode,
//!     VoiceOutboundFrame, VoiceServerEvent, VoiceWsConfig, VoiceWsSession, VoiceWsState,
//! };
//!
//! struct FlakyOnceEngine {
//!     fail_next: bool,
//!     resets: u64,
//! }
//!
//! impl AsrEngine for FlakyOnceEngine {
//!     type Error = &'static str;
//!     fn begin_stream(&mut self, _: &TranscriptionOptions) -> Result<(), Self::Error> { Ok(()) }
//!     fn feed_stream(&mut self, _: &[f32]) -> Result<Option<TranscriptPreview>, Self::Error> {
//!         Ok(None)
//!     }
//!     fn finish_stream(&mut self) -> Result<String, Self::Error> { Ok(String::new()) }
//!     fn transcribe_offline(
//!         &mut self,
//!         pcm: &[f32],
//!         _: &TranscriptionOptions,
//!     ) -> Result<String, Self::Error> {
//!         if self.fail_next {
//!             self.fail_next = false;
//!             return Err("transient ASR failure");
//!         }
//!         Ok(format!("samples:{}", pcm.len()))
//!     }
//!     fn reset_asr(&mut self) {
//!         self.resets += 1;
//!     }
//! }
//!
//! // BUG-10 ASR+VAD error & retry isolation:
//! let mut asr = AsrTranscriberSession::new(
//!     FlakyOnceEngine { fail_next: true, resets: 0 },
//!     VadConfig::default(),
//! )?;
//! let opts = TranscriptionOptions {
//!     speech_silence_filtering: false,
//!     ..TranscriptionOptions::default()
//! };
//! let gen1 = asr.vad_generation_id();
//! assert!(asr.transcribe_f32(&[0.25_f32; 512], &opts).is_err());
//! assert_eq!(asr.state(), AsrSessionState::Faulted);
//! assert_eq!(asr.reset_count(), 1);
//! assert_eq!(asr.vad_generation_id(), gen1 + 1);
//!
//! let ok = asr.transcribe_f32(&[0.25_f32; 512], &opts)?;
//! assert_eq!(ok.text, "samples:512");
//! assert_eq!(asr.state(), AsrSessionState::Idle);
//! assert_eq!(asr.reset_count(), 2);
//! assert_eq!(asr.vad_generation_id(), gen1 + 2);
//!
//! // BUG-10 /ws/voice 10-minute timeout emits Error JSON BEFORE CloseFrame:
//! let clock = VirtualClock::new(1_000);
//! let mut ws = VoiceWsSession::new(VoiceWsConfig::default(), clock.clone());
//! assert!(matches!(
//!     ws.handle_frame(VoiceClientFrame::Start { options: TranscriptionOptions::default() }).as_slice(),
//!     [VoiceOutboundFrame::Event { event: VoiceServerEvent::Ready, .. }]
//! ));
//! clock.advance_ms(DEFAULT_MAX_SESSION_DURATION_MS);
//! let frames = ws.tick();
//! assert_eq!(frames.len(), 2);
//! assert!(matches!(
//!     &frames[0],
//!     VoiceOutboundFrame::Event {
//!         event: VoiceServerEvent::Error { code: VoiceErrorCode::SessionTimeout, .. },
//!         ..
//!     }
//! ));
//! assert!(matches!(&frames[1], VoiceOutboundFrame::Close(_)));
//! assert_eq!(ws.state(), VoiceWsState::Closing);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod acoustic_weights;
mod asr_types;
mod base64;
mod custom_words_dict;
mod custom_words_match;
mod vad_frame;
mod vad_session;
mod voice_ws_types;
mod wav_codec;
mod worker_framing;
mod worker_stdio;

pub mod acoustic_features;
pub mod acoustic_model;
pub mod apple_speech;
pub mod asr;
pub mod custom_words;
pub mod native_discovery;
pub mod pcm_prebuffer;
pub mod types;
pub mod vad;
pub mod voice_ws;
pub mod wav;
pub mod worker_protocol;

pub use crate::acoustic_features::{
    AcousticFeatureError, AcousticFrameAnalyzer, AcousticFrameFeatures, NUM_ACOUSTIC_BANDS,
};
pub use crate::acoustic_model::{
    AcousticAsrEngine, AcousticModelError, AcousticModelWeights, AcousticPrototype,
    DEFAULT_MAX_AUDIO_SAMPLES, LCASR_BINARY_MAGIC, MIN_SPLIT_AUDIO_SAMPLES,
};
pub use crate::asr::{
    AsrEngine, AsrError, AsrSessionState, AsrTranscriberSession, StreamFeedOutcome,
    StreamFinishOutcome, TranscribeOutcome,
};
pub use crate::native_discovery::{
    ModelInspectionError, SpeechBackendKind, SpeechDeviceInfo, SpeechDeviceType,
    SpeechModelCapabilities, SpeechModelFormat, SpeechModelInspection, discover_speech_devices,
    inspect_model_file,
};
pub use crate::types::{
    PcmError, SPEECH_FRAME_SAMPLES, SPEECH_SAMPLE_RATE_HZ, SpeechCustomWord, TranscriptPreview,
    TranscriptionOptions, decode_f32le_bytes, decode_pcm16le_bytes, pcm16_to_f32,
    validate_f32_samples,
};
pub use crate::vad::{
    SileroStyleVad, VAD_FRAME_SAMPLES, VAD_SAMPLE_RATE_HZ, VadConfig, VadError, VadFrame,
    VadFrameAccumulator, VadFrameDecision, VadFrameEvent, VadSessionState,
};
pub use crate::voice_ws::{
    AudioSampleEncoding, CloseFrame, DEFAULT_MAX_AUDIO_BUFFER_BYTES, DEFAULT_MAX_CHUNK_BYTES,
    DEFAULT_MAX_CONTROL_FRAME_BYTES, DEFAULT_MAX_SESSION_DURATION_MS, SystemClock, VOICE_WS_PATH,
    VirtualClock, VoiceClientCommand, VoiceClientFrame, VoiceClock, VoiceErrorCode,
    VoiceOutboundFrame, VoiceServerEvent, VoiceWsConfig, VoiceWsSession, VoiceWsState,
};
pub use crate::wav::{
    DecodedWav, WavError, decode_wav, decode_wav_to_16khz_mono, encode_wav_f32le,
    encode_wav_pcm16le, resample_linear,
};
pub use crate::worker_protocol::{
    DEFAULT_MAX_WORKER_LINE_BYTES, WorkerAudioDecodeError, WorkerAudioPayload, WorkerCommand,
    WorkerErrorCode, WorkerEvent, WorkerExitReason, WorkerIoError, WorkerProtocolError,
    decode_worker_command, decode_worker_event, encode_worker_command, encode_worker_event,
    run_worker_stdio, run_worker_stdio_with_max_line,
};
