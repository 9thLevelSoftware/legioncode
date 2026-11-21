//! Tests for `t3_speech::worker_protocol` (`NDJSON` codec, `run_worker_stdio`,
//! and `BUG-10` retry isolation across stdio worker restarts).

use std::fmt::Write as _;
use std::io::Cursor;

use t3_speech::asr::{AsrEngine, AsrSessionState, AsrTranscriberSession};
use t3_speech::types::{TranscriptPreview, TranscriptionOptions};
use t3_speech::vad::{VAD_FRAME_SAMPLES, VadConfig};
use t3_speech::worker_protocol::{
    WorkerAudioPayload, WorkerCommand, WorkerErrorCode, WorkerEvent, WorkerExitReason,
    decode_worker_command, decode_worker_event, encode_worker_command, encode_worker_event,
    run_worker_stdio, run_worker_stdio_with_max_line,
};

const WAVE_500HZ_PCM16: [i16; 32] = [
    0, 1951, 3827, 5556, 7071, 8315, 9239, 9808, 10000, 9808, 9239, 8315, 7071, 5556, 3827, 1951,
    0, -1951, -3827, -5556, -7071, -8315, -9239, -9808, -10000, -9808, -9239, -8315, -7071, -5556,
    -3827, -1951,
];

#[derive(Default)]
struct WorkerMockEngine {
    poisoned: bool,
    fail_transcribe_calls_left: u32,
    streamed: usize,
}

impl WorkerMockEngine {
    fn new(fail_transcribe_calls_left: u32) -> Self {
        Self {
            fail_transcribe_calls_left,
            ..Self::default()
        }
    }
}

impl AsrEngine for WorkerMockEngine {
    type Error = &'static str;

    fn begin_stream(&mut self, _options: &TranscriptionOptions) -> Result<(), Self::Error> {
        if self.poisoned {
            return Err("poisoned stream start");
        }
        self.streamed = 0;
        Ok(())
    }

    fn feed_stream(&mut self, pcm: &[f32]) -> Result<Option<TranscriptPreview>, Self::Error> {
        if self.poisoned {
            return Err("poisoned stream feed");
        }
        self.streamed += pcm.len();
        Ok(Some(TranscriptPreview::new(
            format!("partial:{}", self.streamed),
            "",
        )))
    }

    fn finish_stream(&mut self) -> Result<String, Self::Error> {
        if self.poisoned {
            return Err("poisoned stream finish");
        }
        Ok(format!("stream-done:{}", self.streamed))
    }

    fn transcribe_offline(
        &mut self,
        pcm: &[f32],
        _options: &TranscriptionOptions,
    ) -> Result<String, Self::Error> {
        if self.poisoned {
            return Err("poisoned transcribe leaked");
        }
        if self.fail_transcribe_calls_left > 0 {
            self.fail_transcribe_calls_left -= 1;
            self.poisoned = true;
            return Err("simulated worker transcribe error");
        }
        Ok(format!("ok:{}", pcm.len()))
    }

    fn reset_asr(&mut self) {
        self.poisoned = false;
        self.streamed = 0;
    }
}

fn voiced_pcm16_samples(count: usize) -> Vec<i16> {
    (0..count)
        .map(|i| WAVE_500HZ_PCM16[i % WAVE_500HZ_PCM16.len()])
        .collect()
}

fn voiced_pcm16le_bytes(count: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(count * 2);
    for s in voiced_pcm16_samples(count) {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    bytes
}

#[test]
fn worker_command_and_event_ndjson_round_trips() -> Result<(), String> {
    let bytes = voiced_pcm16le_bytes(8);
    let commands = [
        WorkerCommand::Config {
            options: TranscriptionOptions::default(),
            streaming: true,
            model_path: Some("parakeet.gguf".to_owned()),
            vad_path: Some("silero.gguf".to_owned()),
        },
        WorkerCommand::AudioChunk {
            audio: WorkerAudioPayload::from_pcm16le_bytes(&bytes),
            final_chunk: false,
        },
        WorkerCommand::Transcribe {
            audio: WorkerAudioPayload::from_pcm16(vec![100, -100, 0]),
            options: Some(TranscriptionOptions {
                speech_silence_filtering: false,
                ..TranscriptionOptions::default()
            }),
        },
        WorkerCommand::Reset,
        WorkerCommand::Shutdown,
    ];
    for cmd in &commands {
        let encoded = encode_worker_command(cmd).map_err(|e| e.to_string())?;
        assert!(encoded.ends_with('\n'));
        let decoded = decode_worker_command(&encoded).map_err(|e| e.to_string())?;
        assert_eq!(&decoded, cmd);
    }

    let events = [
        WorkerEvent::Ready {
            generation_id: 1,
            sample_rate_hz: 16_000,
            frame_samples: 512,
        },
        WorkerEvent::Transcript {
            generation_id: 2,
            text: "hello world".to_owned(),
            committed: "hello ".to_owned(),
            tentative: "world".to_owned(),
            is_final: false,
        },
        WorkerEvent::Error {
            code: WorkerErrorCode::AsrFailure,
            message: "boom".to_owned(),
            generation_id: 3,
            reset_performed: true,
        },
        WorkerEvent::Stopped {
            reason: "shutdown".to_owned(),
            generation_id: 4,
        },
    ];
    for ev in &events {
        let encoded = encode_worker_event(ev).map_err(|e| e.to_string())?;
        let decoded = decode_worker_event(&encoded).map_err(|e| e.to_string())?;
        assert_eq!(&decoded, ev);
    }

    // Odd base64 PCM16LE byte count fails decoding cleanly.
    let odd = WorkerAudioPayload::from_pcm16le_bytes(&[1, 2, 3]);
    assert!(odd.decode_to_f32().is_err());
    Ok(())
}

fn push_cmd(buf: &mut String, cmd: &WorkerCommand) -> Result<(), String> {
    let encoded = encode_worker_command(cmd).map_err(|e| e.to_string())?;
    buf.push_str(&encoded);
    Ok(())
}

fn parse_events(output: Vec<u8>) -> Result<Vec<WorkerEvent>, String> {
    let text = String::from_utf8(output).map_err(|e| e.to_string())?;
    text.lines()
        .map(|l| decode_worker_event(l).map_err(|e| e.to_string()))
        .collect()
}

#[test]
fn run_worker_stdio_bug10_transcribe_error_and_retry_resets_asr_and_vad() -> Result<(), String> {
    let raw_bytes = voiced_pcm16le_bytes(VAD_FRAME_SAMPLES * 2);
    let mut input = String::new();
    push_cmd(
        &mut input,
        &WorkerCommand::Config {
            options: TranscriptionOptions::default(),
            streaming: false,
            model_path: None,
            vad_path: None,
        },
    )?;
    push_cmd(
        &mut input,
        &WorkerCommand::Transcribe {
            audio: WorkerAudioPayload::from_pcm16le_bytes(&raw_bytes),
            options: None,
        },
    )?;
    push_cmd(
        &mut input,
        &WorkerCommand::Transcribe {
            audio: WorkerAudioPayload::from_pcm16(voiced_pcm16_samples(VAD_FRAME_SAMPLES * 3)),
            options: None,
        },
    )?;
    push_cmd(&mut input, &WorkerCommand::Shutdown)?;

    let mut session = AsrTranscriberSession::new(WorkerMockEngine::new(1), VadConfig::default())
        .map_err(|e| e.to_string())?;
    let mut output = Vec::new();
    let exit = run_worker_stdio(Cursor::new(input.into_bytes()), &mut output, &mut session)
        .map_err(|e| e.to_string())?;
    assert_eq!(exit, WorkerExitReason::Shutdown);

    let events = parse_events(output)?;

    assert!(matches!(
        events.first(),
        Some(WorkerEvent::Ready {
            generation_id: 2,
            sample_rate_hz: 16_000,
            frame_samples: 512
        })
    ));

    assert!(events.iter().any(|ev| matches!(
        ev,
        WorkerEvent::Error {
            code: WorkerErrorCode::AsrFailure,
            generation_id: 3,
            reset_performed: true,
            ..
        }
    )));
    assert!(events.iter().any(|ev| matches!(
        ev,
        WorkerEvent::Transcript {
            generation_id: 4,
            text,
            is_final: true,
            ..
        } if text == "ok:1536"
    )));

    assert!(matches!(
        events.last(),
        Some(WorkerEvent::Stopped {
            reason,
            generation_id: 5
        }) if reason == "shutdown"
    ));
    assert_eq!(session.state(), AsrSessionState::Idle);
    assert!(!session.engine().poisoned);
    Ok(())
}

#[test]
fn run_worker_stdio_streaming_oversized_and_eof_handling() -> Result<(), String> {
    let mut input = String::new();
    push_cmd(
        &mut input,
        &WorkerCommand::Config {
            options: TranscriptionOptions {
                speech_silence_filtering: false,
                ..TranscriptionOptions::default()
            },
            streaming: true,
            model_path: None,
            vad_path: None,
        },
    )?;
    input.push_str("{invalid json}\n");
    let _ = writeln!(input, r#"{{"type":"reset","pad":"{}"}}"#, "a".repeat(5000));
    push_cmd(
        &mut input,
        &WorkerCommand::AudioChunk {
            audio: WorkerAudioPayload::from_f32(vec![0.1_f32; 512]),
            final_chunk: false,
        },
    )?;
    push_cmd(
        &mut input,
        &WorkerCommand::AudioChunk {
            audio: WorkerAudioPayload::from_f32(vec![0.1_f32; 128]),
            final_chunk: true,
        },
    )?;
    push_cmd(&mut input, &WorkerCommand::Reset)?;

    let mut session = AsrTranscriberSession::new(WorkerMockEngine::new(0), VadConfig::default())
        .map_err(|e| e.to_string())?;
    let mut output = Vec::new();
    let exit = run_worker_stdio_with_max_line(
        Cursor::new(input.into_bytes()),
        &mut output,
        &mut session,
        4096,
    )
    .map_err(|e| e.to_string())?;
    assert_eq!(exit, WorkerExitReason::Eof);

    let events = parse_events(output)?;

    assert!(events.iter().any(|e| matches!(
        e,
        WorkerEvent::Error {
            code: WorkerErrorCode::InvalidCommand,
            reset_performed: false,
            ..
        }
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        WorkerEvent::Error {
            code: WorkerErrorCode::FrameTooLarge,
            reset_performed: false,
            ..
        }
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        WorkerEvent::Transcript {
            text,
            is_final: false,
            ..
        } if text == "partial:512"
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        WorkerEvent::Transcript {
            text,
            is_final: true,
            ..
        } if text == "stream-done:640"
    )));
    assert_eq!(session.state(), AsrSessionState::Idle);
    Ok(())
}
