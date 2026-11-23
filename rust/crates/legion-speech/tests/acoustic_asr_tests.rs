//! Unit and integration tests for `legion_speech::acoustic_features` and `legion_speech::acoustic_model`
//! (`DSP` frame analysis, `.lcasr` model loading, midpoint splitting, and `BUG-10` resets).

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use legion_speech::acoustic_features::{AcousticFrameAnalyzer, NUM_ACOUSTIC_BANDS};
use legion_speech::acoustic_model::{
    AcousticAsrEngine, AcousticModelError, AcousticModelWeights, MIN_SPLIT_AUDIO_SAMPLES,
};
use legion_speech::asr::AsrEngine;
use legion_speech::pcm16_to_f32;
use legion_speech::types::{SPEECH_FRAME_SAMPLES, SpeechCustomWord, TranscriptionOptions};

const WAVE_500HZ_PCM16: [i16; 32] = [
    0, 1951, 3827, 5556, 7071, 8315, 9239, 9808, 10000, 9808, 9239, 8315, 7071, 5556, 3827, 1951,
    0, -1951, -3827, -5556, -7071, -8315, -9239, -9808, -10000, -9808, -9239, -8315, -7071, -5556,
    -3827, -1951,
];

/// Exact 4-sample cycle at `16 kHz` = `4_000 Hz`.
const WAVE_4000HZ_PCM16: [i16; 4] = [0, 10_000, 0, -10_000];

fn tone_500hz(samples: usize) -> Vec<f32> {
    (0..samples)
        .map(|i| pcm16_to_f32(WAVE_500HZ_PCM16[i % WAVE_500HZ_PCM16.len()]))
        .collect()
}

fn tone_4000hz(samples: usize) -> Vec<f32> {
    (0..samples)
        .map(|i| pcm16_to_f32(WAVE_4000HZ_PCM16[i % WAVE_4000HZ_PCM16.len()]))
        .collect()
}

fn temp_file_path(tag: &str, ext: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!(
        "legion_speech_{tag}_{}_{nanos}.{ext}",
        std::process::id()
    ))
}

#[test]
fn dsp_frame_analyzer_extracts_centroid_zcr_and_8_band_envelope() -> Result<(), AcousticModelError>
{
    let analyzer = AcousticFrameAnalyzer::default();
    let silence = vec![0.0_f32; SPEECH_FRAME_SAMPLES];
    let f_sil = analyzer.analyze_exact_frame(&silence)?;
    assert!(!f_sil.voiced);
    assert_eq!(f_sil.rms_energy, 0.0);
    assert_eq!(f_sil.spectral_centroid_hz, 0.0);
    assert_eq!(f_sil.band_log_energies.len(), NUM_ACOUSTIC_BANDS);

    let low = analyzer.analyze_exact_frame(&tone_500hz(SPEECH_FRAME_SAMPLES))?;
    assert!(low.voiced);
    assert!((low.spectral_centroid_hz - 500.0).abs() < 60.0);
    assert!(low.band_log_energies[2] > low.band_log_energies[6]);

    let high = analyzer.analyze_exact_frame(&tone_4000hz(SPEECH_FRAME_SAMPLES))?;
    assert!(high.voiced);
    assert!((high.spectral_centroid_hz - 4_000.0).abs() < 250.0);
    assert!(high.band_log_energies[6] > high.band_log_energies[1]);
    Ok(())
}

#[test]
fn acoustic_asr_engine_produces_voiced_transcripts_silence_empty_and_resets()
-> Result<(), AcousticModelError> {
    let mut engine = AcousticAsrEngine::with_default_english();
    let opts = TranscriptionOptions::default();

    // Pure silence returns "" offline and None / "" streaming.
    let silence = vec![0.0_f32; SPEECH_FRAME_SAMPLES * 4];
    assert_eq!(engine.transcribe_offline(&silence, &opts)?, "");

    engine.begin_stream(&opts)?;
    assert!(engine.feed_stream(&silence)?.is_none());
    assert_eq!(engine.finish_stream()?, "");

    // Voiced audio produces deterministic non-empty streaming previews and final transcripts.
    let low = tone_500hz(SPEECH_FRAME_SAMPLES * 2);
    let high = tone_4000hz(SPEECH_FRAME_SAMPLES * 2);
    let low_text = engine.transcribe_offline(&low, &opts)?;
    let high_text = engine.transcribe_offline(&high, &opts)?;
    assert_ne!(low_text, "");
    assert_ne!(high_text, "");
    assert_ne!(low_text, high_text);

    // Custom vocabulary aliases rewrite decoded prototype tokens.
    let custom_opts = TranscriptionOptions {
        speech_custom_words: vec![SpeechCustomWord {
            term: "LegionCode".to_owned(),
            aliases: vec![low_text.clone()],
        }],
        ..TranscriptionOptions::default()
    };
    assert_eq!(engine.transcribe_offline(&low, &custom_opts)?, "LegionCode");

    // Live streaming accumulates committed + tentative previews and finalizes cleanly.
    engine.begin_stream(&opts)?;
    let p1 = engine
        .feed_stream(&low)?
        .ok_or_else(|| AcousticModelError::InvalidFormat {
            reason: "expected voiced preview 1".to_owned(),
        })?;
    assert_ne!(p1.tentative, "");
    let p2 = engine
        .feed_stream(&high)?
        .ok_or_else(|| AcousticModelError::InvalidFormat {
            reason: "expected voiced preview 2".to_owned(),
        })?;
    assert_ne!(p2.committed, "");
    assert_ne!(p2.tentative, "");
    let final_stream = engine.finish_stream()?;
    assert_ne!(final_stream, "");
    assert!(final_stream.contains(&low_text));

    // BUG-10 reset_asr() clears active streaming state cleanly.
    engine.begin_stream(&opts)?;
    let _ = engine.feed_stream(&low)?;
    engine.reset_asr();
    assert_eq!(engine.reset_count(), 1);
    assert!(matches!(
        engine.finish_stream(),
        Err(AcousticModelError::StreamNotActive)
    ));
    Ok(())
}

#[test]
fn acoustic_asr_engine_splits_recursively_above_max_samples_down_to_4000()
-> Result<(), AcousticModelError> {
    let mut weights = AcousticModelWeights::default_english();
    weights.max_audio_samples = 4_096;
    let mut engine = AcousticAsrEngine::new(weights)?;

    // 16_384 samples (> 4_096): splits 16384 -> 8192+8192 -> 4x 4096.
    let mut pcm = tone_500hz(6_000);
    pcm.extend(vec![0.0_f32; 4_384]);
    pcm.extend(tone_4000hz(6_000));
    assert_eq!(pcm.len(), 16_384);

    let text = engine.transcribe_offline(&pcm, &TranscriptionOptions::default())?;
    assert!(engine.split_count() >= 3);
    let words: Vec<&str> = text.split_whitespace().collect();
    assert_eq!(words.len(), 2);
    assert_ne!(words[0], words[1]);

    // When max_audio_samples < MIN_SPLIT_AUDIO_SAMPLES (4_000), recursion stops cleanly at <= 4_000.
    let mut small_cap = AcousticModelWeights::default_english();
    small_cap.max_audio_samples = 1_000;
    assert_eq!(MIN_SPLIT_AUDIO_SAMPLES, 4_000);
    let mut bounded_engine = AcousticAsrEngine::new(small_cap)?;
    let text_bounded =
        bounded_engine.transcribe_offline(&tone_500hz(7_000), &TranscriptionOptions::default())?;
    assert_ne!(text_bounded, "");
    assert_eq!(bounded_engine.split_count(), 1);
    Ok(())
}

#[test]
fn acoustic_model_weights_load_json_and_binary_lcasr_from_disk() -> Result<(), AcousticModelError> {
    let mut custom = AcousticModelWeights::default_english();
    custom.model_id = "custom-disk-model".to_owned();
    custom.language = "en-US".to_owned();

    let json_path = temp_file_path("json", "lcasr");
    let bin_path = temp_file_path("bin", "lcasr");
    fs::write(&json_path, custom.to_json_bytes()?).map_err(|e| AcousticModelError::Io {
        message: e.to_string(),
    })?;
    fs::write(&bin_path, custom.to_binary_bytes()?).map_err(|e| AcousticModelError::Io {
        message: e.to_string(),
    })?;

    let loaded_json = AcousticModelWeights::load_from_file(&json_path)?;
    let loaded_bin = AcousticModelWeights::load_from_file(&bin_path)?;
    let _ = fs::remove_file(&json_path);
    let _ = fs::remove_file(&bin_path);

    assert_eq!(loaded_json, custom);
    assert_eq!(loaded_bin, custom);
    Ok(())
}
