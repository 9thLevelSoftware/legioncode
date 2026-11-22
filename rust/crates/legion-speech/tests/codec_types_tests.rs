//! Additional tests for `TranscriptionOptions` wire compatibility, `F32leBase64` worker payloads,
//! and `VadConfig` validation bounds.

use legion_speech::types::{SpeechCustomWord, TranscriptPreview, TranscriptionOptions};
use legion_speech::vad::{SileroStyleVad, VadConfig, VadError};
use legion_speech::worker_protocol::{
    WorkerAudioDecodeError, WorkerAudioPayload, WorkerCommand, decode_worker_command,
    encode_worker_command,
};

#[test]
fn transcription_options_match_contracts_camel_case_wire_json() -> Result<(), String> {
    let raw = r#"{
      "speechLanguage": "de",
      "speechTranslateToEnglish": true,
      "speechCustomWords": [{"term": "LegionCode", "aliases": ["legion code"]}],
      "speechRemoveFillerWords": false,
      "speechCorrectionWord": "sorry",
      "speechSilenceFiltering": false
    }"#;
    let parsed: TranscriptionOptions = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    assert_eq!(parsed.speech_language, "de");
    assert!(parsed.speech_translate_to_english);
    assert_eq!(
        parsed.speech_custom_words,
        vec![SpeechCustomWord {
            term: "LegionCode".to_owned(),
            aliases: vec!["legion code".to_owned()],
        }]
    );
    assert!(!parsed.speech_remove_filler_words);
    assert_eq!(parsed.speech_correction_word.as_deref(), Some("sorry"));
    assert!(!parsed.speech_silence_filtering);

    let minimal: TranscriptionOptions = serde_json::from_str("{}").map_err(|e| e.to_string())?;
    assert_eq!(minimal, TranscriptionOptions::default());
    assert_eq!(
        TranscriptPreview::new("hello ", "world ").full_text(),
        "hello world"
    );
    Ok(())
}

#[test]
fn f32le_base64_payload_round_trip_and_invalid_inputs() -> Result<(), String> {
    let mut raw = Vec::new();
    for sample in [-1.0_f32, -0.25_f32, 0.0_f32, 0.5_f32, 1.0_f32] {
        raw.extend_from_slice(&sample.to_le_bytes());
    }
    let payload = WorkerAudioPayload::from_f32le_bytes(&raw);
    let cmd = WorkerCommand::Transcribe {
        audio: payload,
        options: None,
    };
    let line = encode_worker_command(&cmd).map_err(|e| e.to_string())?;
    let decoded_cmd = decode_worker_command(&line).map_err(|e| e.to_string())?;
    assert_eq!(decoded_cmd, cmd);

    if let WorkerCommand::Transcribe { audio, .. } = decoded_cmd {
        let samples = audio.decode_to_f32().map_err(|e| e.to_string())?;
        assert_eq!(samples, vec![-1.0_f32, -0.25, 0.0, 0.5, 1.0]);
    }

    // Out-of-range f32 inside F32leBase64 fails validation cleanly.
    let bad_payload = WorkerAudioPayload::from_f32le_bytes(&1.5_f32.to_le_bytes());
    assert!(matches!(
        bad_payload.decode_to_f32(),
        Err(WorkerAudioDecodeError::Pcm(_))
    ));

    // Corrupt Base64 padding fails cleanly.
    let bad_b64 = WorkerAudioPayload::Pcm16leBase64 {
        data: "A".to_owned(),
    };
    assert!(matches!(
        bad_b64.decode_to_f32(),
        Err(WorkerAudioDecodeError::Base64(_))
    ));
    Ok(())
}

#[test]
fn vad_config_validation_rejects_invalid_bounds() {
    let invalid_configs = [
        VadConfig {
            frame_samples: 0,
            ..VadConfig::default()
        },
        VadConfig {
            sample_rate_hz: 0,
            ..VadConfig::default()
        },
        VadConfig {
            min_rms_energy: -0.1,
            ..VadConfig::default()
        },
        VadConfig {
            min_rms_energy: 0.1,
            target_rms_energy: 0.05,
            ..VadConfig::default()
        },
        VadConfig {
            min_zcr: 0.5,
            max_zcr: 0.4,
            ..VadConfig::default()
        },
        VadConfig {
            speech_threshold: 1.5,
            ..VadConfig::default()
        },
        VadConfig {
            onset_frames: 0,
            ..VadConfig::default()
        },
    ];
    for cfg in invalid_configs {
        assert!(matches!(
            SileroStyleVad::new(cfg),
            Err(VadError::InvalidConfig { .. })
        ));
    }
}
