//! Unit tests for `t3_speech::wav` (`PCM16LE` & `IEEE-float32` encode/decode, stereo downmix,
//! `RIFF` odd-chunk alignment, and linear resampling to `16 kHz`).

use t3_speech::wav::{
    WavError, decode_wav, decode_wav_to_16khz_mono, encode_wav_f32le, encode_wav_pcm16le,
    resample_linear,
};

#[test]
fn wav_pcm16le_and_f32le_mono_and_stereo_round_trips() -> Result<(), WavError> {
    let mono = vec![-1.0_f32, -0.5, 0.0, 0.25, 0.75, 1.0];
    let pcm16_bytes = encode_wav_pcm16le(&mono, 16_000, 1)?;
    let decoded_16 = decode_wav(&pcm16_bytes)?;
    assert_eq!(decoded_16.sample_rate_hz, 16_000);
    assert_eq!(decoded_16.channels, 1);
    assert_eq!(decoded_16.mono_samples.len(), mono.len());
    for (&got, &expected) in decoded_16.mono_samples.iter().zip(&mono) {
        assert!((got - expected).abs() < 1e-3, "got {got} vs {expected}");
    }

    let f32_bytes = encode_wav_f32le(&mono, 44_100, 1)?;
    let decoded_f32 = decode_wav(&f32_bytes)?;
    assert_eq!(decoded_f32.sample_rate_hz, 44_100);
    assert_eq!(decoded_f32.mono_samples, mono);

    // Stereo downmix averages left and right channels per frame.
    let stereo = vec![0.8_f32, 0.2_f32, -0.6_f32, 0.6_f32, -1.0_f32, 0.0_f32];
    let stereo_bytes = encode_wav_f32le(&stereo, 32_000, 2)?;
    let decoded_stereo = decode_wav(&stereo_bytes)?;
    assert_eq!(decoded_stereo.channels, 2);
    assert_eq!(decoded_stereo.mono_samples.len(), 3);
    assert!((decoded_stereo.mono_samples[0] - 0.5).abs() < 1e-6);
    assert!(decoded_stereo.mono_samples[1].abs() < 1e-6);
    assert!((decoded_stereo.mono_samples[2] - (-0.5)).abs() < 1e-6);
    Ok(())
}

#[test]
fn wav_handles_odd_sized_junk_subchunks_and_resamples_to_16khz() -> Result<(), WavError> {
    // 48 kHz -> 16 kHz exact 3:1 decimation.
    let input_48k = vec![0.1_f32, 0.9, 0.9, -0.4, 0.8, 0.8, 0.25, 0.7, 0.7];
    let resampled_down = resample_linear(&input_48k, 48_000, 16_000)?;
    assert_eq!(resampled_down, vec![0.1_f32, -0.4, 0.25]);

    // 8 kHz -> 16 kHz midpoint linear interpolation.
    let upsample_src = vec![0.0_f32, 0.8];
    let resampled_up = resample_linear(&upsample_src, 8_000, 16_000)?;
    assert_eq!(resampled_up.len(), 4);
    assert!((resampled_up[0] - 0.0).abs() < 1e-5);
    assert!((resampled_up[1] - 0.4).abs() < 1e-5);
    assert!((resampled_up[2] - 0.8).abs() < 1e-5);
    assert!((resampled_up[3] - 0.8).abs() < 1e-5);

    // Inject an odd-sized JUNK subchunk (3 bytes + 1 pad byte) before data.
    let base = encode_wav_f32le(&[0.5_f32, 0.5, -0.5, -0.5, 0.2, 0.2], 32_000, 2)?;
    let mut with_junk = Vec::new();
    with_junk.extend_from_slice(&base[..36]);
    with_junk.extend_from_slice(b"JUNK");
    with_junk.extend_from_slice(&3_u32.to_le_bytes());
    with_junk.extend_from_slice(b"abc");
    with_junk.push(0);
    with_junk.extend_from_slice(&base[36..]);

    let mono_16k = decode_wav_to_16khz_mono(&with_junk)?;
    assert_eq!(mono_16k.len(), 1);
    assert!((mono_16k[0] - 0.5).abs() < 1e-5);
    Ok(())
}

#[test]
fn wav_rejects_corrupt_headers_unsupported_formats_and_bad_geometry() {
    assert!(matches!(
        decode_wav(b"NOTARIFF"),
        Err(WavError::InvalidHeader { .. })
    ));

    // Minimal header claiming 8-bit PCM (unsupported).
    let mut pcm8 = encode_wav_pcm16le(&[0.0_f32], 16_000, 1).unwrap_or_default();
    pcm8[34] = 8;
    pcm8[35] = 0;
    assert!(matches!(
        decode_wav(&pcm8),
        Err(WavError::UnsupportedFormat {
            bits_per_sample: 8,
            ..
        })
    ));

    // NaN sample inside IEEE-float32 data chunk.
    let mut nan_wav = encode_wav_f32le(&[0.0_f32], 16_000, 1).unwrap_or_default();
    let len = nan_wav.len();
    nan_wav[len - 4..].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(matches!(
        decode_wav(&nan_wav),
        Err(WavError::InvalidSamples { .. })
    ));

    assert!(matches!(
        resample_linear(&[0.1], 0, 16_000),
        Err(WavError::InvalidGeometry { .. })
    ));
    assert!(matches!(
        encode_wav_pcm16le(&[0.1, 0.2, 0.3], 16_000, 2),
        Err(WavError::InvalidGeometry { .. })
    ));
}
