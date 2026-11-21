//! Unit and invariant tests for `t3_speech::vad` (`PCM16LE` normalization, 512-sample framing,
//! deterministic energy + zero-crossing `VAD`, and `BUG-10` disposal/rebuild isolation).

use t3_speech::pcm16_to_f32;
use t3_speech::vad::{
    SileroStyleVad, VAD_FRAME_SAMPLES, VadConfig, VadError, VadFrame, VadFrameAccumulator,
    VadFrameEvent, VadSessionState,
};

/// Exact 32-sample cycle (`500 Hz` at `16 kHz`, amplitude ~`0.305`).
const WAVE_500HZ_PCM16: [i16; 32] = [
    0, 1951, 3827, 5556, 7071, 8315, 9239, 9808, 10000, 9808, 9239, 8315, 7071, 5556, 3827, 1951,
    0, -1951, -3827, -5556, -7071, -8315, -9239, -9808, -10000, -9808, -9239, -8315, -7071, -5556,
    -3827, -1951,
];

fn tone_pcm16le_bytes(sample_count: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(sample_count * 2);
    for idx in 0..sample_count {
        let sample = WAVE_500HZ_PCM16[idx % WAVE_500HZ_PCM16.len()];
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

fn tone_f32(sample_count: usize) -> Vec<f32> {
    (0..sample_count)
        .map(|idx| pcm16_to_f32(WAVE_500HZ_PCM16[idx % WAVE_500HZ_PCM16.len()]))
        .collect()
}

#[test]
fn pcm16le_normalization_and_split_chunk_parity() -> Result<(), VadError> {
    let mut raw_bytes = Vec::new();
    raw_bytes.extend_from_slice(&i16::MIN.to_le_bytes());
    raw_bytes.extend_from_slice(&0_i16.to_le_bytes());
    raw_bytes.extend_from_slice(&i16::MAX.to_le_bytes());
    raw_bytes.extend_from_slice(&tone_pcm16le_bytes(VAD_FRAME_SAMPLES * 2));

    let mut whole = VadFrameAccumulator::new(VAD_FRAME_SAMPLES);
    let mut whole_frames = whole.push_pcm16le(&raw_bytes)?;
    if let Some(tail) = whole.flush()? {
        whole_frames.push(tail);
    }

    let mut split = VadFrameAccumulator::new(VAD_FRAME_SAMPLES);
    let mut split_frames = Vec::new();
    let chunk_sizes = [1usize, 2, 3, 17, 63, 255, 511, 1025];
    let mut pos = 0usize;
    let mut step = 0usize;
    while pos < raw_bytes.len() {
        let size = chunk_sizes[step % chunk_sizes.len()];
        let end = (pos + size).min(raw_bytes.len());
        split_frames.extend(split.push_pcm16le(&raw_bytes[pos..end])?);
        pos = end;
        step += 1;
    }
    if let Some(tail) = split.flush()? {
        split_frames.push(tail);
    }

    assert_eq!(whole_frames, split_frames);
    assert_eq!(whole_frames[0].samples[0], -1.0_f32);
    assert_eq!(whole_frames[0].samples[1], 0.0_f32);
    assert!((whole_frames[0].samples[2] - (32_767.0_f32 / 32_768.0_f32)).abs() < 1e-6);
    assert_eq!(whole_frames.last().map(|f| f.valid_samples), Some(3));
    assert_eq!(whole_frames.last().map(|f| f.samples.len()), Some(512));
    Ok(())
}

#[test]
fn accumulator_rejects_unaligned_trailing_byte_and_invalid_f32() -> Result<(), VadError> {
    let mut acc = VadFrameAccumulator::new(VAD_FRAME_SAMPLES);
    assert_eq!(acc.push_pcm16le(&[0x01])?, [] as [VadFrame; 0]);
    assert!(acc.has_pending_byte());
    assert_eq!(
        acc.push_f32(&[0.1]),
        Err(VadError::UnalignedTrailingPcm16Byte)
    );
    assert_eq!(acc.flush(), Err(VadError::UnalignedTrailingPcm16Byte));
    assert!(!acc.has_pending_byte());
    assert_eq!(acc.pending_samples(), 0);

    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.01, -1.01] {
        assert!(acc.push_f32(&[0.0, bad]).is_err());
    }
    Ok(())
}

#[test]
fn silero_style_vad_distinguishes_speech_silence_dc_and_hiss() -> Result<(), VadError> {
    let mut vad = SileroStyleVad::new(VadConfig::default())?;

    let silence = vec![0.0_f32; VAD_FRAME_SAMPLES];
    let d_silence = vad.process_frame(&silence)?;
    assert_eq!(d_silence.probability, 0.0);
    assert_eq!(d_silence.event, VadFrameEvent::Silence);
    assert!(!d_silence.is_speech);

    // Pure DC offset has zero crossings and must not trigger speech onset.
    let dc = vec![0.6_f32; VAD_FRAME_SAMPLES];
    let d_dc = vad.process_frame(&dc)?;
    assert!(!d_dc.is_speech);
    assert_eq!(d_dc.event, VadFrameEvent::Silence);

    // Nyquist alternating static exceeds max_zcr and must not trigger speech onset.
    let nyquist: Vec<f32> = (0..VAD_FRAME_SAMPLES)
        .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
        .collect();
    let d_nyquist = vad.process_frame(&nyquist)?;
    assert!(!d_nyquist.is_speech);
    assert_eq!(d_nyquist.event, VadFrameEvent::Silence);

    // Voiced frames trigger OnsetPending -> SpeechStart -> SpeechContinue -> SpeechHangover -> SpeechEnd.
    let voiced = tone_f32(VAD_FRAME_SAMPLES);
    let f1 = vad.process_frame(&voiced)?;
    assert_eq!(f1.event, VadFrameEvent::OnsetPending);
    assert!(!f1.is_speech);

    let f2 = vad.process_frame(&voiced)?;
    assert_eq!(f2.event, VadFrameEvent::SpeechStart);
    assert!(f2.is_speech);
    assert!(vad.is_in_speech());

    let f3 = vad.process_frame(&voiced)?;
    assert_eq!(f3.event, VadFrameEvent::SpeechContinue);
    assert!(f3.is_speech);

    for i in 0..VadConfig::default().hangover_frames {
        let hang = vad.process_frame(&silence)?;
        assert_eq!(hang.event, VadFrameEvent::SpeechHangover, "frame {i}");
        assert!(hang.is_speech);
        assert!(hang.in_hangover);
    }

    let end = vad.process_frame(&silence)?;
    assert_eq!(end.event, VadFrameEvent::SpeechEnd);
    assert!(!end.is_speech);
    assert!(!vad.is_in_speech());
    assert_eq!(vad.segments_completed(), 1);

    vad.reset();
    assert_eq!(vad.frame_index(), 0);
    assert_eq!(vad.segments_completed(), 0);
    assert_eq!(vad.active_speech_frames(), 0);
    assert!(!vad.is_in_speech());
    Ok(())
}

#[test]
fn vad_session_preroll_short_utterance_and_disposal_fence() -> Result<(), VadError> {
    let config = VadConfig {
        prefill_frames: 2,
        onset_frames: 2,
        hangover_frames: 1,
        ..VadConfig::default()
    };
    let mut session = VadSessionState::new(config)?;
    assert_eq!(session.generation_id(), 1);

    // 4 frames silence + 2 frames voiced + 2 frames silence.
    let silence = vec![0.0_f32; VAD_FRAME_SAMPLES * 4];
    let voiced = tone_f32(VAD_FRAME_SAMPLES * 2);
    let tail_silence = vec![0.0_f32; VAD_FRAME_SAMPLES * 2];

    session.feed_f32(&silence, true)?;
    assert_eq!(session.retained_pcm(), [0.0_f32; 0]);
    assert_eq!(session.prefill_len(), 2);

    session.feed_f32(&voiced, true)?;
    // OnsetPending (frame 4) sits in prefill_ring alongside 1 silence pre-roll frame (frame 3),
    // then SpeechStart (frame 5) flushes those 2 prefill frames + frame 5 = 3 frames (1536 samples).
    assert_eq!(session.retained_pcm().len(), VAD_FRAME_SAMPLES * 3);

    session.feed_f32(&tail_silence, true)?;
    // + 1 hangover frame = 4 frames total (2048 samples); SpeechEnd frame is not appended.
    assert_eq!(session.retained_pcm().len(), VAD_FRAME_SAMPLES * 4);

    // Dispose and rebuild increments generation_id and clears residual buffers.
    let mut old = session.clone();
    let next = session.rebuild_next()?;
    assert!(session.is_disposed());
    assert_eq!(next.generation_id(), 2);
    assert_eq!(next.retained_pcm(), [0.0_f32; 0]);
    assert_eq!(
        session.feed_f32(&voiced, true),
        Err(VadError::SessionDisposed { generation_id: 1 })
    );
    old.dispose();
    assert_eq!(old.retained_pcm(), [0.0_f32; 0]);
    assert_eq!(
        old.flush(true),
        Err(VadError::SessionDisposed { generation_id: 1 })
    );

    // Single short onset utterance (< onset_frames) is still preserved on flush.
    let mut short = VadSessionState::new(config)?;
    let one_voiced = tone_f32(300);
    short.feed_f32(&one_voiced, true)?;
    short.flush(true)?;
    assert_eq!(short.retained_pcm().len(), 300);
    Ok(())
}

#[test]
fn vad_deterministic_random_split_sweep_matches_unsplit_decisions() -> Result<(), VadError> {
    let mut pcm16 = Vec::new();
    pcm16.extend(std::iter::repeat_n(0_u8, VAD_FRAME_SAMPLES * 3 * 2));
    pcm16.extend_from_slice(&tone_pcm16le_bytes(VAD_FRAME_SAMPLES * 4 + 173));
    pcm16.extend(std::iter::repeat_n(0_u8, VAD_FRAME_SAMPLES * 20 * 2));

    let mut base = VadSessionState::new(VadConfig::default())?;
    let mut expected_decisions = base.feed_pcm16le(&pcm16, true)?;
    expected_decisions.extend(base.flush(true)?);
    let expected_pcm = base.take_retained_pcm();

    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    for _ in 0..40 {
        let mut chunked = VadSessionState::new(VadConfig::default())?;
        let mut got_decisions = Vec::new();
        let mut pos = 0usize;
        while pos < pcm16.len() {
            seed = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = seed;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^= z >> 31;
            let len = usize::try_from((z % 997) + 1).unwrap_or(1);
            let end = (pos + len).min(pcm16.len());
            got_decisions.extend(chunked.feed_pcm16le(&pcm16[pos..end], true)?);
            pos = end;
        }
        got_decisions.extend(chunked.flush(true)?);
        let got_pcm = chunked.take_retained_pcm();
        assert_eq!(got_decisions, expected_decisions);
        assert_eq!(got_pcm, expected_pcm);
    }
    Ok(())
}
