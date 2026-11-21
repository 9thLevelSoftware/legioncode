//! Property-based and core behavioral tests for `t3-shared`.

use proptest::prelude::*;

#[cfg(feature = "dpop")]
use t3_shared::dpop::{
    DPOP_MAX_CLOCK_SKEW_SECONDS, DpopKeyPair, DpopVerificationFailureCode, SignDpopProofInput,
    VerifyDpopProofInput, compute_dpop_jwk_thumbprint, normalize_dpop_htu, sign_dpop_proof,
    verify_dpop_proof,
};
use t3_shared::frame_parts::{
    FrameOutput, FramePartAssembler, PART_PREFIX, PART_RECEIVED_FRAME, split_socket_frame_utf16,
    split_socket_frame_with_limit, utf16_wire_utf8_len,
};
use t3_shared::search::{FuzzyPaletteIndex, SearchOptions};
use t3_shared::utf16::{
    Utf16OffsetError, fold_in_place, utf8_byte_to_utf16_offset, utf8_len_to_utf16,
    utf16_offset_to_utf8_byte, utf16_slice_exact, utf16_slice_safe,
};

proptest! {
    #[test]
    fn prop_utf16_roundtrip_and_fold_preserves_length(s in "\\PC{0,64}") {
        let total_u16 = utf8_len_to_utf16(&s);
        prop_assert_eq!(total_u16, s.encode_utf16().count());

        let folded = fold_in_place(&s);
        prop_assert_eq!(utf8_len_to_utf16(&folded), total_u16);

        for (byte_idx, ch) in s.char_indices() {
            let u16_off = utf8_byte_to_utf16_offset(&s, byte_idx).expect("valid char boundary");
            prop_assert_eq!(utf16_offset_to_utf8_byte(&s, u16_off), Ok(byte_idx));
            if ch.len_utf16() == 2 {
                prop_assert_eq!(
                    utf16_offset_to_utf8_byte(&s, u16_off + 1),
                    Err(Utf16OffsetError::SurrogateSplit { offset: u16_off + 1 })
                );
            }
        }
        prop_assert_eq!(utf16_offset_to_utf8_byte(&s, total_u16), Ok(s.len()));
        prop_assert_eq!(
            utf16_offset_to_utf8_byte(&s, total_u16 + 1),
            Err(Utf16OffsetError::OutOfBounds { offset: total_u16 + 1, utf16_len: total_u16 })
        );
    }

    #[test]
    fn prop_utf16_slice_safe_never_panics_or_exceeds_bounds(s in "\\PC{0,40}", a in 0usize..50, b in 0usize..50) {
        let sliced = utf16_slice_safe(&s, a, b);
        let sliced_u16 = utf8_len_to_utf16(sliced);
        if a >= b {
            prop_assert!(sliced.is_empty());
        } else {
            prop_assert!(sliced_u16 <= b - a);
        }
    }

    #[test]
    fn prop_frame_split_and_reassemble_utf8(
        s in "\\PC{0,200}",
        limit in 5usize..40,
    ) {
        let parts = split_socket_frame_with_limit(&s, limit).expect("limit >= 5");
        prop_assert!(!parts.is_empty());
        for (idx, part) in parts.iter().enumerate() {
            prop_assert!(part.len() <= limit);
            if idx + 1 < parts.len() {
                prop_assert!(part.starts_with(PART_PREFIX));
            } else {
                prop_assert!(!part.starts_with(PART_PREFIX) || s.starts_with(PART_PREFIX));
            }
        }

        let mut assembler = FramePartAssembler::new();
        let mut out = None;
        for (idx, part) in parts.iter().enumerate() {
            match assembler.push(part) {
                FrameOutput::IntermediateAck(ack) => {
                    prop_assert_eq!(ack, PART_RECEIVED_FRAME);
                    prop_assert!(idx + 1 < parts.len());
                }
                FrameOutput::Complete(whole) => {
                    prop_assert_eq!(idx + 1, parts.len());
                    out = Some(whole.into_owned());
                }
            }
        }
        prop_assert_eq!(out.as_deref(), Some(s.as_str()));
        prop_assert!(!assembler.is_pending());
    }

    #[test]
    fn prop_frame_split_utf16_unpaired_surrogates_never_exceed_limit(
        units in prop::collection::vec(
            prop_oneof![
                Just(0xD800u16),
                Just(0xDBFFu16),
                Just(0xDC00u16),
                Just(0x4E2Du16),
                Just(0x0041u16),
                Just(0xD83Du16),
                Just(0xDE00u16),
            ],
            0..80,
        ),
        limit in 5usize..30,
    ) {
        let parts = split_socket_frame_utf16(&units, limit).expect("limit >= 5");
        let total_payload: usize = parts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if i + 1 < parts.len() {
                    p.len() - PART_PREFIX.len_utf8()
                } else {
                    p.len()
                }
            })
            .sum();
        prop_assert!(parts.iter().all(|p| p.len() <= limit));
        prop_assert_eq!(total_payload, utf16_wire_utf8_len(&units));
    }

    #[test]
    fn prop_fuzzy_palette_highlights_are_valid_utf16_slices(
        titles in prop::collection::vec("[A-Za-z0-9 _/\\-éİßﬁ😀]{1,32}", 1..16),
        query in "[A-Za-z0-9 é]{1,12}",
    ) {
        let index = FuzzyPaletteIndex::new(&titles);
        let hits = index.search(&query);
        for hit in hits {
            let title = &titles[hit.item_index];
            for window in hit.ranges.windows(2) {
                prop_assert!(window[0].end < window[1].start);
            }
            for range in &hit.ranges {
                let slice = utf16_slice_exact(title, range.start, range.end);
                prop_assert!(slice.is_ok(), "invalid UTF-16 range {range:?} on {title:?}");
            }
        }
    }
}

#[test]
fn bug21_unpaired_high_surrogate_followed_by_cjk_respects_wire_limit() {
    // Unpaired high surrogate (0xD800 -> U+FFFD, 3 bytes) + '中' (0x4E2D, 3 bytes) = 6 UTF-8 bytes.
    // Legacy JS counted 4 bytes; safe Rust splits into 2 parts within a 5-byte limit.
    let units = [0xD800u16, 0x4E2Du16];
    assert_eq!(utf16_wire_utf8_len(&units), 6);

    let parts = split_socket_frame_utf16(&units, 5).expect("limit 5 >= minimum");
    assert_eq!(parts.len(), 2);
    assert!(parts.iter().all(|part| part.len() <= 5));
    assert_eq!(parts[0], format!("{PART_PREFIX}\u{FFFD}"));
    assert_eq!(parts[1], "中");
}

#[cfg(feature = "dpop")]
#[test]
fn dpop_sign_verify_thumbprint_and_replay_bounds() {
    let key_pair = DpopKeyPair::generate();
    let thumbprint = key_pair.thumbprint();
    assert_eq!(
        thumbprint,
        compute_dpop_jwk_thumbprint(&key_pair.public_jwk())
    );

    let proof = sign_dpop_proof(
        &key_pair,
        &SignDpopProofInput {
            method: "POST",
            url: "https://example.com:443/v1/environments/env/connect?foo=bar#frag",
            iat_epoch_seconds: 1_000,
            jti: Some("jti-1"),
            access_token: Some("access-token-abc"),
        },
    )
    .expect("sign proof");

    let normalized =
        normalize_dpop_htu("https://example.com:443/v1/environments/env/connect?foo=bar#frag");
    assert_eq!(
        normalized.as_deref(),
        Some("https://example.com/v1/environments/env/connect")
    );

    for offset in [-DPOP_MAX_CLOCK_SKEW_SECONDS, 0, DPOP_MAX_CLOCK_SKEW_SECONDS] {
        let verified = verify_dpop_proof(&VerifyDpopProofInput {
            proof: Some(&proof),
            method: "post",
            url: "https://example.com/v1/environments/env/connect?other=1#h",
            now_epoch_seconds: 1_000 + offset,
            expected_thumbprint: Some(&thumbprint),
            expected_access_token: Some("access-token-abc"),
            max_age_seconds: None,
        })
        .expect("within +/-300s skew");
        assert_eq!(
            (verified.thumbprint, verified.jti, verified.iat),
            (thumbprint.clone(), "jti-1".to_owned(), 1_000)
        );
    }

    for bad_offset in [
        -DPOP_MAX_CLOCK_SKEW_SECONDS - 1,
        DPOP_MAX_CLOCK_SKEW_SECONDS + 1,
    ] {
        let err = verify_dpop_proof(&VerifyDpopProofInput {
            proof: Some(&proof),
            method: "POST",
            url: "https://example.com/v1/environments/env/connect",
            now_epoch_seconds: 1_000 + bad_offset,
            expected_thumbprint: Some(&thumbprint),
            expected_access_token: Some("access-token-abc"),
            max_age_seconds: None,
        })
        .expect_err("outside 300s skew");
        assert_eq!(err.code, DpopVerificationFailureCode::TimeWindow);
    }
}

#[cfg(feature = "concurrency")]
mod concurrency_props {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::{Mutex, Notify, oneshot};

    use t3_shared::concurrency::{DrainableWorker, KeyedLock};

    #[tokio::test]
    async fn keyed_lock_preserves_fifo_order_and_cleans_idle_entries() {
        let lock = KeyedLock::new();
        let events = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let first_entered = Arc::new(Notify::new());
        let (release_tx, release_rx) = oneshot::channel::<()>();

        let first = tokio::spawn({
            let lock = lock.clone();
            let events = Arc::clone(&events);
            let first_entered = Arc::clone(&first_entered);
            async move {
                lock.with_lock("key", async {
                    events.lock().await.push("first:start");
                    first_entered.notify_one();
                    let _ = release_rx.await;
                    events.lock().await.push("first:end");
                })
                .await;
            }
        });
        first_entered.notified().await;

        let spawn_waiter = |label: &'static str| {
            tokio::spawn({
                let lock = lock.clone();
                let events = Arc::clone(&events);
                async move {
                    lock.with_lock("key", async { events.lock().await.push(label) })
                        .await;
                }
            })
        };
        let second = spawn_waiter("second");
        tokio::task::yield_now().await;
        let third = spawn_waiter("third");
        tokio::task::yield_now().await;
        assert_eq!(lock.active_keys(), vec!["key"]);
        assert_eq!(*events.lock().await, vec!["first:start"]);

        let _ = release_tx.send(());
        let _ = tokio::join!(first, second, third);
        assert_eq!(
            *events.lock().await,
            vec!["first:start", "first:end", "second", "third"]
        );
        assert_eq!(lock.active_keys(), Vec::<&str>::new());
    }

    #[tokio::test]
    async fn drainable_worker_tracks_mid_flight_enqueues_and_drains_deterministically() {
        struct Signals {
            processed: Mutex<Vec<&'static str>>,
            first_started: Notify,
            second_started: Notify,
            release_first: Notify,
            release_second: Notify,
        }
        let sig = Arc::new(Signals {
            processed: Mutex::new(Vec::new()),
            first_started: Notify::new(),
            second_started: Notify::new(),
            release_first: Notify::new(),
            release_second: Notify::new(),
        });
        let worker = DrainableWorker::new({
            let sig = Arc::clone(&sig);
            move |item: &'static str| {
                let sig = Arc::clone(&sig);
                async move {
                    if item == "first" {
                        sig.first_started.notify_one();
                        sig.release_first.notified().await;
                    } else if item == "second" {
                        sig.second_started.notify_one();
                        sig.release_second.notified().await;
                    }
                    sig.processed.lock().await.push(item);
                }
            }
        });
        assert!(worker.enqueue("first"));
        sig.first_started.notified().await;
        let drain_done = Arc::new(AtomicUsize::new(0));
        let drain_task = tokio::spawn({
            let worker = worker.clone();
            let drain_done = Arc::clone(&drain_done);
            async move {
                worker.drain().await;
                drain_done.store(1, Ordering::SeqCst);
            }
        });
        assert!(worker.enqueue("second"));
        assert_eq!(worker.stats().outstanding, 2);
        sig.release_first.notify_one();
        sig.second_started.notified().await;
        tokio::task::yield_now().await;
        assert_eq!(drain_done.load(Ordering::SeqCst), 0);
        sig.release_second.notify_one();
        let _ = drain_task.await;
        assert_eq!(drain_done.load(Ordering::SeqCst), 1);
        assert_eq!(*sig.processed.lock().await, vec!["first", "second"]);
        assert_eq!(worker.stats().outstanding, 0);
    }
}

#[test]
fn fuzzy_palette_index_ranks_whole_words_prefixes_typos_and_unicode() {
    let titles = [
        "Fix oauth redirect",
        "Authentication cleanup",
        "Share auth across worktrees",
        "Café İstanbul ß ﬁ",
    ];
    let index = FuzzyPaletteIndex::new(titles);

    let auth_hits = index.search("auth");
    assert_eq!(
        auth_hits.iter().map(|h| h.item_index).collect::<Vec<_>>(),
        vec![2, 1, 0]
    );
    assert_eq!(auth_hits[0].ranges.len(), 1);
    let r = &auth_hits[0].ranges[0];
    assert_eq!(utf16_slice_exact(titles[2], r.start, r.end), Ok("auth"));

    let hits = index.search("cafe istanbul ß ﬁ");
    assert_eq!((hits.len(), hits[0].item_index), (1, 3));
    let slices: Vec<&str> = hits[0]
        .ranges
        .iter()
        .map(|r| utf16_slice_exact(titles[3], r.start, r.end).expect("valid UTF-16 highlight"))
        .collect();
    assert_eq!(slices, vec!["Café", "İstanbul", "ß", "ﬁ"]);

    let typo_hits = index.search_with_options("rediect", SearchOptions::default());
    assert_eq!((typo_hits.len(), typo_hits[0].item_index), (1, 0));
}
