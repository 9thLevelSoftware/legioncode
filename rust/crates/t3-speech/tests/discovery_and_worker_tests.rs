//! Integration tests for `t3_speech::native_discovery`, `AcousticAsrEngine` inside
//! `run_worker_stdio`, and `VoiceWsSession` + `AsrTranscriberSession` integration.

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use t3_speech::acoustic_model::{AcousticAsrEngine, AcousticModelWeights};
use t3_speech::asr::{AsrEngine, AsrSessionState, AsrTranscriberSession};
use t3_speech::native_discovery::{
    ModelInspectionError, SpeechBackendKind, SpeechDeviceType, SpeechModelFormat,
    discover_speech_devices, inspect_model_file,
};
use t3_speech::types::{TranscriptPreview, TranscriptionOptions};
use t3_speech::vad::{VAD_FRAME_SAMPLES, VadConfig};
use t3_speech::voice_ws::{
    AudioSampleEncoding, CloseFrame, VirtualClock, VoiceClientFrame, VoiceErrorCode,
    VoiceOutboundFrame, VoiceServerEvent, VoiceWsConfig, VoiceWsSession,
};
use t3_speech::worker_protocol::{
    WorkerAudioPayload, WorkerCommand, WorkerEvent, WorkerExitReason, decode_worker_event,
    encode_worker_command, run_worker_stdio,
};

const WAVE_500HZ_PCM16: [i16; 32] = [
    0, 1951, 3827, 5556, 7071, 8315, 9239, 9808, 10000, 9808, 9239, 8315, 7071, 5556, 3827, 1951,
    0, -1951, -3827, -5556, -7071, -8315, -9239, -9808, -10000, -9808, -9239, -8315, -7071, -5556,
    -3827, -1951,
];

fn temp_path(name: &str, ext: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!(
        "t3_speech_disc_{name}_{}_{nanos}.{ext}",
        std::process::id()
    ))
}

#[test]
fn discover_speech_devices_and_inspect_model_files() -> Result<(), String> {
    let devices = discover_speech_devices();
    assert_ne!(devices.len(), 0);
    assert!(devices.iter().any(|d| {
        d.kind == SpeechBackendKind::Cpu && d.device_type == SpeechDeviceType::Cpu && d.available
    }));

    let weights = AcousticModelWeights::default_english();
    let json_path = temp_path("model_json", "t3asr");
    let bin_path = temp_path("model_bin", "t3asr");
    let gguf_path = temp_path("model_gguf", "gguf");
    let bad_path = temp_path("model_bad", "bin");

    fs::write(
        &json_path,
        weights.to_json_bytes().map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::write(
        &bin_path,
        weights.to_binary_bytes().map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::write(&gguf_path, b"GGUF\x03\x00\x00\x00\x00\x00\x00\x00").map_err(|e| e.to_string())?;
    fs::write(&bad_path, b"NOT_A_SPEECH_MODEL").map_err(|e| e.to_string())?;

    let info_json = inspect_model_file(&json_path).map_err(|e| e.to_string())?;
    let info_bin = inspect_model_file(&bin_path).map_err(|e| e.to_string())?;
    let info_gguf = inspect_model_file(&gguf_path).map_err(|e| e.to_string())?;
    let err_bad = inspect_model_file(&bad_path);
    let err_dir = inspect_model_file(&std::env::temp_dir());
    let err_missing = inspect_model_file(&temp_path("missing", "t3asr"));

    let _ = fs::remove_file(&json_path);
    let _ = fs::remove_file(&bin_path);
    let _ = fs::remove_file(&gguf_path);
    let _ = fs::remove_file(&bad_path);

    assert_eq!(info_json.format, SpeechModelFormat::T3AsrJson);
    assert_eq!(info_json.model_id.as_deref(), Some("t3-acoustic-en-v1"));
    assert_eq!(info_json.prototype_count, weights.prototypes.len());
    assert!(info_json.capabilities.streaming && info_json.capabilities.vocabulary);

    assert_eq!(info_bin.format, SpeechModelFormat::T3AsrBinary);
    assert_eq!(info_bin.prototype_count, weights.prototypes.len());

    assert_eq!(info_gguf.format, SpeechModelFormat::Gguf);
    assert_eq!(info_gguf.prototype_count, 0);

    assert!(matches!(
        err_bad,
        Err(ModelInspectionError::UnsupportedFormat { .. })
    ));
    assert!(matches!(
        err_dir,
        Err(ModelInspectionError::NotAFile { .. })
    ));
    assert!(matches!(err_missing, Err(ModelInspectionError::Io { .. })));
    Ok(())
}

#[test]
fn run_worker_stdio_with_acoustic_engine_emits_non_empty_voiced_transcript() -> Result<(), String> {
    let voiced_samples: Vec<i16> = (0..(VAD_FRAME_SAMPLES * 4))
        .map(|i| WAVE_500HZ_PCM16[i % WAVE_500HZ_PCM16.len()])
        .collect();

    let mut input = String::new();
    for cmd in [
        WorkerCommand::Config {
            options: TranscriptionOptions::default(),
            streaming: false,
            model_path: None,
            vad_path: None,
        },
        WorkerCommand::Transcribe {
            audio: WorkerAudioPayload::from_pcm16(voiced_samples.clone()),
            options: None,
        },
        WorkerCommand::AudioChunk {
            audio: WorkerAudioPayload::from_pcm16(voiced_samples),
            final_chunk: true,
        },
        WorkerCommand::Shutdown,
    ] {
        input.push_str(&encode_worker_command(&cmd).map_err(|e| e.to_string())?);
    }

    let mut session = AsrTranscriberSession::new(
        AcousticAsrEngine::with_default_english(),
        VadConfig::default(),
    )
    .map_err(|e| e.to_string())?;
    let mut output = Vec::new();
    let exit = run_worker_stdio(Cursor::new(input.into_bytes()), &mut output, &mut session)
        .map_err(|e| e.to_string())?;
    assert_eq!(exit, WorkerExitReason::Shutdown);

    let text_out = String::from_utf8(output).map_err(|e| e.to_string())?;
    let events: Vec<WorkerEvent> = text_out
        .lines()
        .map(|l| decode_worker_event(l).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;

    let finals: Vec<&str> = events
        .iter()
        .filter_map(|ev| match ev {
            WorkerEvent::Transcript {
                text,
                is_final: true,
                ..
            } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(finals.len(), 2);
    assert_ne!(finals[0], "");
    assert_ne!(finals[1], "");
    assert_eq!(finals[0], finals[1]);
    Ok(())
}

struct WsMockAsr {
    fail_on_finish: bool,
    samples: usize,
    resets: u64,
}

impl AsrEngine for WsMockAsr {
    type Error = &'static str;

    fn begin_stream(&mut self, _options: &TranscriptionOptions) -> Result<(), Self::Error> {
        self.samples = 0;
        Ok(())
    }

    fn feed_stream(&mut self, pcm: &[f32]) -> Result<Option<TranscriptPreview>, Self::Error> {
        self.samples += pcm.len();
        Ok(Some(TranscriptPreview::new(
            format!("heard:{}", self.samples),
            "",
        )))
    }

    fn finish_stream(&mut self) -> Result<String, Self::Error> {
        if self.fail_on_finish {
            return Err("native finish failed");
        }
        Ok(format!("final:{}", self.samples))
    }

    fn transcribe_offline(
        &mut self,
        pcm: &[f32],
        _options: &TranscriptionOptions,
    ) -> Result<String, Self::Error> {
        Ok(format!("offline:{}", pcm.len()))
    }

    fn reset_asr(&mut self) {
        self.resets += 1;
        self.samples = 0;
    }
}

#[test]
fn voice_ws_integrated_with_asr_handles_legacy_finish_and_errors() -> Result<(), String> {
    let clock = VirtualClock::new(100);
    let config = VoiceWsConfig {
        audio_encoding: Some(AudioSampleEncoding::Pcm16Le),
        ..VoiceWsConfig::default()
    };
    let mut ws = VoiceWsSession::new(config.clone(), clock.clone());
    let mut asr = AsrTranscriberSession::new(
        WsMockAsr {
            fail_on_finish: false,
            samples: 0,
            resets: 0,
        },
        VadConfig::default(),
    )
    .map_err(|e| e.to_string())?;

    ws.handle_frame_with_asr(
        VoiceClientFrame::RawText(r#"{"type":"start"}"#.to_owned()),
        &mut asr,
    );
    let mut pcm16 = Vec::with_capacity(VAD_FRAME_SAMPLES * 4);
    for i in 0..(VAD_FRAME_SAMPLES * 2) {
        pcm16.extend_from_slice(&WAVE_500HZ_PCM16[i % WAVE_500HZ_PCM16.len()].to_le_bytes());
    }
    let update = ws.handle_frame_with_asr(VoiceClientFrame::AudioChunk(pcm16), &mut asr);
    assert!(matches!(
        update.as_slice(),
        [VoiceOutboundFrame::Event {
            event: VoiceServerEvent::Update {
                revision: 1,
                text: Some(_)
            },
            ..
        }]
    ));

    let finished = ws.handle_frame_with_asr(
        VoiceClientFrame::RawText(r#"{"type":"finish"}"#.to_owned()),
        &mut asr,
    );
    assert!(matches!(
        finished.as_slice(),
        [
            VoiceOutboundFrame::Event {
                event: VoiceServerEvent::Finished { text },
                ..
            },
            VoiceOutboundFrame::Close(CloseFrame { code: 1000, .. })
        ] if text == "final:1024"
    ));
    assert_eq!(asr.state(), AsrSessionState::Idle);

    let mut ws_err = VoiceWsSession::new(config, clock);
    let mut asr_err = AsrTranscriberSession::new(
        WsMockAsr {
            fail_on_finish: true,
            samples: 0,
            resets: 0,
        },
        VadConfig::default(),
    )
    .map_err(|e| e.to_string())?;
    ws_err.handle_frame_with_asr(
        VoiceClientFrame::Start {
            options: TranscriptionOptions::default(),
        },
        &mut asr_err,
    );
    let fail = ws_err.handle_frame_with_asr(VoiceClientFrame::Stop, &mut asr_err);
    assert!(matches!(
        fail.as_slice(),
        [
            VoiceOutboundFrame::Event {
                event: VoiceServerEvent::Error {
                    code: VoiceErrorCode::InternalError,
                    ..
                },
                ..
            },
            VoiceOutboundFrame::Close(CloseFrame { code: 1008, .. })
        ]
    ));
    assert_eq!(asr_err.state(), AsrSessionState::Faulted);
    assert_eq!(asr_err.reset_count(), 1);
    assert_eq!(asr_err.vad_generation_id(), 2);
    Ok(())
}
