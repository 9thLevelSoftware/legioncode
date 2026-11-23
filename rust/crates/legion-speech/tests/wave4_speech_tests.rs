//! Tests for `custom_words` (#39884), `pcm_prebuffer` (#40011), and `apple_speech` (#34027).

use legion_contracts::ipc::{
    DesktopAppleSpeechErrorCode, DesktopAppleSpeechResult, DesktopAppleSpeechTranscribeInput,
};
use legion_speech::apple_speech::{
    AppleSpeechAssetStatus, AppleSpeechCapabilities, AppleSpeechSession, AppleSpeechState,
    encode_apple_speech_wav, map_helper_exit_code,
};
use legion_speech::custom_words::{
    SpeechTextCorrector, apply_project_name_hints, apply_speech_aliases,
    has_distinctive_project_casing, is_ordinary_english_word, normalize_speech_custom_words,
    project_name_recase_hints, resolve_speech_vocabulary, speech_recognition_words,
};
use legion_speech::pcm_prebuffer::{
    EarlyMicPreBuffer, PreBufferError, PreBufferPhase, PreBufferPushOutcome, VoiceWsHandshakeBuffer,
};
use legion_speech::types::{SpeechCustomWord, TranscriptionOptions};
use legion_speech::voice_ws::VoiceClientFrame;
use legion_speech::wav::decode_wav;

#[test]
fn custom_words_keep_hints_separate_and_never_recase_ordinary_words() {
    for ordinary in [
        "code", "web", "app", "main", "server", "Code", "Web", "App", "Main",
    ] {
        assert!(is_ordinary_english_word(ordinary));
        assert!(!has_distinctive_project_casing(ordinary));
    }
    assert!(has_distinctive_project_casing("LegionCode"));
    assert!(has_distinctive_project_casing("OpenAI"));
    assert!(has_distinctive_project_casing("GitHub"));
    assert!(has_distinctive_project_casing("macOS"));
    assert_eq!(
        project_name_recase_hints("my-web-app"),
        Vec::<String>::new()
    );
    assert_eq!(
        project_name_recase_hints("LegionCode App"),
        vec!["LegionCode".to_owned()]
    );

    // Project hints never recase ordinary English words (`"code"`, `"web"`, `"app"`, `"main"`).
    assert_eq!(
        apply_project_name_hints(
            "check code web app main and legioncode",
            "My Web App Main Code LegionCode"
        ),
        "check code web app main and LegionCode"
    );

    // `resolve_speech_vocabulary` keeps recognition hints out of `speech_custom_words`.
    let resolved = resolve_speech_vocabulary(
        &[SpeechCustomWord {
            term: "effect".to_owned(),
            aliases: vec!["a fact".to_owned()],
        }],
        &[SpeechCustomWord {
            term: "Bob".to_owned(),
            aliases: vec!["Robert".to_owned()],
        }],
        &[
            "  Legion   Code ".to_owned(),
            "Server".to_owned(),
            "legion code".to_owned(),
        ],
    );
    assert_eq!(resolved.speech_hints, vec!["Legion Code", "Server"]);
    assert_eq!(resolved.speech_custom_words.len(), 2);

    // Corrector wired from `TranscriptionOptions` never recases via `speech_hints`.
    let options = TranscriptionOptions {
        speech_custom_words: vec![
            SpeechCustomWord {
                term: "GitHub".to_owned(),
                aliases: vec!["git hub".to_owned()],
            },
            SpeechCustomWord {
                term: "Bob".to_owned(),
                aliases: vec!["Robert".to_owned()],
            },
        ],
        speech_hints: vec!["Server".to_owned(), "Code".to_owned()],
        ..TranscriptionOptions::default()
    };
    let corrector = SpeechTextCorrector::from_options(&options);
    assert_eq!(
        corrector.apply("git hub and Robert on the server code"),
        "GitHub and Bob on the server code"
    );
    assert_eq!(
        apply_speech_aliases("a fact", &resolved.speech_custom_words),
        "effect"
    );

    // Recognition hints come first and survive the 100-term cap.
    let many_terms: Vec<String> = (0..100).map(|i| format!("term{i}")).collect();
    let rec = speech_recognition_words(&["Server".to_owned()], &many_terms);
    assert_eq!(rec.len(), 100);
    assert_eq!(rec[0], "Server");
    assert_eq!(rec[99], "term98");
    assert_eq!(
        normalize_speech_custom_words(&[SpeechCustomWord {
            term: "  Legion   Code ".to_owned(),
            aliases: vec!["legion code".to_owned(), "teacup".to_owned()],
        }])[0]
            .aliases,
        vec!["teacup"]
    );
}

#[test]
fn early_mic_prebuffer_captures_pre_handshake_frames_and_flushes_in_fifo_order()
-> Result<(), String> {
    let mut pre = EarlyMicPreBuffer::new(64);
    assert_eq!(pre.phase(), PreBufferPhase::Capturing);

    assert_eq!(
        pre.push_f32_samples(&[0.1_f32, 0.2_f32])
            .map_err(|e| e.to_string())?,
        PreBufferPushOutcome::Buffered {
            buffered_frames: 1,
            buffered_bytes: 8,
        }
    );
    pre.push_pcm_bytes(vec![1, 2, 3, 4])
        .map_err(|e| e.to_string())?;
    assert_eq!(pre.buffered_frame_count(), 2);
    assert_eq!(pre.buffered_bytes(), 12);

    // Overflow rejects cleanly without dropping existing frames.
    assert!(matches!(
        pre.push_pcm_bytes(vec![0_u8; 60]),
        Err(PreBufferError::BufferBudgetExceeded { .. })
    ));

    let flushed = pre.settle_streaming().map_err(|e| e.to_string())?;
    assert_eq!(pre.phase(), PreBufferPhase::Streaming);
    assert_eq!(flushed.len(), 2);
    assert_eq!(flushed[1], vec![1, 2, 3, 4]);

    let mut streamed = Vec::new();
    let outcome = pre
        .feed_or_buffer(vec![9, 8], |chunk| streamed.push(chunk))
        .map_err(|e| e.to_string())?;
    assert_eq!(outcome, PreBufferPushOutcome::PassThroughReady);
    assert_eq!(streamed, vec![vec![9, 8]]);

    // WS handshake buffer integration.
    let mut ws_buf = VoiceWsHandshakeBuffer::new(1024);
    assert_eq!(
        ws_buf.on_mic_pcm(vec![10, 20]).map_err(|e| e.to_string())?,
        Vec::new()
    );
    assert_eq!(
        ws_buf.on_mic_pcm(vec![30, 40]).map_err(|e| e.to_string())?,
        Vec::new()
    );
    assert_eq!(ws_buf.queued_chunks(), 2);
    let ready_frames = ws_buf.on_server_ready().map_err(|e| e.to_string())?;
    assert_eq!(
        ready_frames,
        vec![
            VoiceClientFrame::AudioChunk(vec![10, 20]),
            VoiceClientFrame::AudioChunk(vec![30, 40]),
        ]
    );
    assert_eq!(
        ws_buf.on_mic_pcm(vec![50, 60]).map_err(|e| e.to_string())?,
        vec![VoiceClientFrame::AudioChunk(vec![50, 60])]
    );

    // Abort clears buffer and ignores subsequent frames.
    let mut aborted = EarlyMicPreBuffer::new(1024);
    aborted
        .push_pcm_bytes(vec![1, 2, 3, 4])
        .map_err(|e| e.to_string())?;
    aborted.abort();
    assert_eq!(aborted.buffered_bytes(), 0);
    assert_eq!(
        aborted
            .push_pcm_bytes(vec![5, 6])
            .map_err(|e| e.to_string())?,
        PreBufferPushOutcome::IgnoredAborted
    );
    assert!(matches!(
        aborted.settle_streaming(),
        Err(PreBufferError::AlreadyAborted)
    ));
    Ok(())
}

#[test]
fn apple_speech_session_state_machine_and_contracts_ipc_roundtrip() -> Result<(), String> {
    let mut unavailable = AppleSpeechSession::new(AppleSpeechCapabilities::default());
    assert_eq!(unavailable.state(), &AppleSpeechState::Unavailable);
    assert_eq!(
        unavailable.begin_prepare("en-US"),
        Err(DesktopAppleSpeechResult::err(
            DesktopAppleSpeechErrorCode::Unavailable
        ))
    );

    let caps = AppleSpeechCapabilities::from_available_csv("en-US, fr-FR, pt-BR");
    let mut session = AppleSpeechSession::new(caps);
    assert_eq!(session.state(), &AppleSpeechState::Idle);
    assert_eq!(session.supported_locales(), &["en-US", "fr-FR", "pt-BR"]);

    // Unsupported locale faults cleanly.
    assert_eq!(
        session.begin_prepare("de-DE"),
        Err(DesktopAppleSpeechResult::err(
            DesktopAppleSpeechErrorCode::UnsupportedLocale
        ))
    );

    // Equivalent locale resolves (`en` -> `en-US`) and downloads/prepares cleanly.
    let resolved = session
        .begin_prepare("en")
        .map_err(|e| format!("unexpected prepare err: {e:?}"))?;
    assert_eq!(resolved, "en-US");
    assert_eq!(
        session.finish_prepare(AppleSpeechAssetStatus::Installed),
        DesktopAppleSpeechResult::ok("en-US")
    );
    assert_eq!(
        session.state(),
        &AppleSpeechState::Ready {
            resolved_locale: "en-US".to_owned(),
        }
    );

    // Valid f32le PCM produces 16 kHz mono WAV and transitions Transcribing -> Ready.
    let mut pcm = Vec::new();
    for sample in [0.0_f32, 0.5_f32, -0.5_f32, f32::NAN] {
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    let wav = encode_apple_speech_wav(&pcm).map_err(|e| format!("{e:?}"))?;
    let decoded = decode_wav(&wav).map_err(|e| e.to_string())?;
    assert_eq!(decoded.sample_rate_hz, 16_000);
    assert_eq!(decoded.channels, 1);
    assert_eq!(decoded.mono_samples.len(), 4);

    let input = DesktopAppleSpeechTranscribeInput {
        locale: "en-US".to_owned(),
        pcm,
    };
    let wav_for_call = session
        .begin_transcribe(&input)
        .map_err(|e| format!("{e:?}"))?;
    assert_eq!(wav_for_call, wav);
    assert_eq!(
        session.apply_helper_exit(0, r#"{"value":" hello world "}"#),
        DesktopAppleSpeechResult::ok("hello world")
    );
    assert_eq!(
        session.state(),
        &AppleSpeechState::Ready {
            resolved_locale: "en-US".to_owned(),
        }
    );

    assert_eq!(
        map_helper_exit_code(2),
        DesktopAppleSpeechErrorCode::UnsupportedLocale
    );
    assert_eq!(
        map_helper_exit_code(3),
        DesktopAppleSpeechErrorCode::Unavailable
    );
    assert_eq!(map_helper_exit_code(1), DesktopAppleSpeechErrorCode::Failed);
    session.reset();
    assert_eq!(session.state(), &AppleSpeechState::Idle);
    Ok(())
}
