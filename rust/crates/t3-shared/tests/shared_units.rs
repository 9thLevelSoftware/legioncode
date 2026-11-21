//! Detailed behavioral and edge-case unit tests mirroring TypeScript shared test suites.

#[cfg(feature = "dpop")]
use base64::Engine as _;
#[cfg(feature = "dpop")]
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
#[cfg(feature = "dpop")]
use t3_shared::dpop::{
    DpopKeyPair, DpopPublicJwk, DpopVerificationFailureCode, SignDpopProofInput,
    VerifyDpopProofInput, compute_dpop_access_token_hash, compute_dpop_jwk_thumbprint,
    sign_dpop_proof, verify_dpop_proof,
};
use t3_shared::frame_parts::{
    FrameOutput, FramePartAssembler, FramePartsError, PART_RECEIVED_FRAME, split_socket_frame,
    split_socket_frame_with_limit,
};
use t3_shared::search::{
    FieldMatchMode, FieldOptions, FuzzyPaletteIndex, PaletteEntry, PaletteField, SearchRange,
    assistant_citation_label, extract_assistant_citation_hrefs, fold_search_query,
    strip_thread_query_prefixes, strip_worktree_branch_prefix,
};
use t3_shared::utf16::{
    Utf16OffsetError, fold_char_in_place, fold_in_place, splits_surrogate_pair,
    utf16_offset_to_utf8_byte, utf16_slice_exact, utf16_slice_safe,
};

#[test]
fn utf16_folding_and_surrogate_edge_cases() {
    assert_eq!(fold_char_in_place('ß'), 'ß');
    assert_eq!(fold_char_in_place('ẞ'), 'ß');
    assert_eq!(fold_char_in_place('İ'), 'i');
    assert_eq!(fold_char_in_place('ﬁ'), 'ﬁ');
    assert_eq!(fold_char_in_place('ς'), 'σ');
    assert_eq!(fold_char_in_place('‘'), '\'');
    assert_eq!(fold_char_in_place('’'), '\'');
    assert_eq!(fold_in_place("İstanbul ß ﬁ ς ’"), "istanbul ß ﬁ σ '");

    let units: Vec<u16> = "a😀b".encode_utf16().collect();
    assert!(!splits_surrogate_pair(&units, 0));
    assert!(!splits_surrogate_pair(&units, 1));
    assert!(splits_surrogate_pair(&units, 2));
    assert!(!splits_surrogate_pair(&units, 3));
    assert!(!splits_surrogate_pair(&units, 4));

    assert_eq!(utf16_slice_safe("a😀b", 0, 2), "a");
    assert_eq!(utf16_slice_safe("a😀b", 2, 4), "b");
    assert_eq!(utf16_slice_safe("a😀b", 1, 2), "");
    assert_eq!(utf16_slice_exact("a😀b", 1, 3), Ok("😀"));
    assert_eq!(
        utf16_offset_to_utf8_byte("a😀b", 2),
        Err(Utf16OffsetError::SurrogateSplit { offset: 2 })
    );
}

#[test]
fn frame_parts_rejects_limit_too_small_for_multibyte_scalar() {
    let err =
        split_socket_frame_with_limit("😀😀", 4).expect_err("4 bytes < prefix + 4-byte emoji");
    assert_eq!(
        err,
        FramePartsError::PartLimitTooSmall {
            part_bytes: 4,
            minimum: 5,
        }
    );

    let mut assembler = FramePartAssembler::new();
    assert_eq!(
        assembler.push("plain"),
        FrameOutput::Complete(std::borrow::Cow::Borrowed("plain"))
    );

    let payload = "🚀".repeat(15_000);
    let parts = split_socket_frame(&payload);
    assert_eq!(parts.len(), 2);
    assert!(parts.iter().all(|part| part.len() <= 48 * 1024));
    assert_eq!(
        assembler.push(&parts[0]),
        FrameOutput::IntermediateAck(PART_RECEIVED_FRAME)
    );
    assert!(assembler.is_pending());
    let FrameOutput::Complete(whole) = assembler.push(&parts[1]) else {
        panic!("expected final frame to complete assembly");
    };
    assert_eq!(whole.as_ref(), payload);
    assert!(!assembler.is_pending());
}

#[cfg(feature = "dpop")]
#[test]
fn dpop_verification_rejects_all_failure_modes_in_order() {
    let kp = DpopKeyPair::generate();
    let other_kp = DpopKeyPair::generate();
    let thumbprint = kp.thumbprint();

    let missing = verify_dpop_proof(&VerifyDpopProofInput {
        proof: Some("   "),
        method: "POST",
        url: "https://example.com/oauth/token",
        now_epoch_seconds: 100,
        expected_thumbprint: None,
        expected_access_token: None,
        max_age_seconds: None,
    })
    .expect_err("blank proof");
    assert_eq!(missing.code, DpopVerificationFailureCode::MissingProof);

    let proof = sign_dpop_proof(
        &kp,
        &SignDpopProofInput {
            method: "POST",
            url: "https://example.com/oauth/token",
            iat_epoch_seconds: 100,
            jti: Some("p-1"),
            access_token: Some("tok-1"),
        },
    )
    .expect("valid sign");

    let parts: Vec<&str> = proof.split('.').collect();
    assert_eq!(parts.len(), 3);

    // Tamper signature by signing identical header+payload with other key -> InvalidSignature beats TimeWindow!
    let other_sig_proof = sign_dpop_proof(
        &other_kp,
        &SignDpopProofInput {
            method: "POST",
            url: "https://example.com/oauth/token",
            iat_epoch_seconds: 100,
            jti: Some("p-1"),
            access_token: Some("tok-1"),
        },
    )
    .expect("sign");
    let other_sig = other_sig_proof.split('.').nth(2).expect("sig");
    let mismatched_sig = format!("{}.{}.{other_sig}", parts[0], parts[1]);
    let sig_err = verify_dpop_proof(&VerifyDpopProofInput {
        proof: Some(&mismatched_sig),
        method: "POST",
        url: "https://example.com/oauth/token",
        now_epoch_seconds: 10_000,
        expected_thumbprint: Some(&thumbprint),
        expected_access_token: Some("tok-1"),
        max_age_seconds: None,
    })
    .expect_err("bad signature checked before time_window");
    assert_eq!(sig_err.code, DpopVerificationFailureCode::InvalidSignature);

    let key_err = verify_dpop_proof(&VerifyDpopProofInput {
        proof: Some(&proof),
        method: "POST",
        url: "https://example.com/oauth/token",
        now_epoch_seconds: 101,
        expected_thumbprint: Some("wrong-thumbprint"),
        expected_access_token: Some("tok-1"),
        max_age_seconds: None,
    })
    .expect_err("thumbprint mismatch");
    assert_eq!(key_err.code, DpopVerificationFailureCode::KeyMismatch);

    let method_err = verify_dpop_proof(&VerifyDpopProofInput {
        proof: Some(&proof),
        method: "GET",
        url: "https://example.com/oauth/token",
        now_epoch_seconds: 101,
        expected_thumbprint: Some(&thumbprint),
        expected_access_token: Some("tok-1"),
        max_age_seconds: None,
    })
    .expect_err("method mismatch");
    assert_eq!(method_err.code, DpopVerificationFailureCode::MethodMismatch);

    let url_err = verify_dpop_proof(&VerifyDpopProofInput {
        proof: Some(&proof),
        method: "POST",
        url: "https://example.com/other",
        now_epoch_seconds: 101,
        expected_thumbprint: Some(&thumbprint),
        expected_access_token: Some("tok-1"),
        max_age_seconds: None,
    })
    .expect_err("url mismatch");
    assert_eq!(url_err.code, DpopVerificationFailureCode::UrlMismatch);

    let ath_err = verify_dpop_proof(&VerifyDpopProofInput {
        proof: Some(&proof),
        method: "POST",
        url: "https://example.com/oauth/token",
        now_epoch_seconds: 101,
        expected_thumbprint: Some(&thumbprint),
        expected_access_token: Some("wrong-token"),
        max_age_seconds: None,
    })
    .expect_err("ath mismatch");
    assert_eq!(
        ath_err.code,
        DpopVerificationFailureCode::AccessTokenHashMismatch
    );
}

#[cfg(feature = "dpop")]
#[test]
fn dpop_rejects_private_jwk_and_uses_rfc7638_key_order() {
    let kp = DpopKeyPair::generate();
    let proof = sign_dpop_proof(
        &kp,
        &SignDpopProofInput {
            method: "POST",
            url: "https://example.com/oauth/token",
            iat_epoch_seconds: 100,
            jti: Some("p-1"),
            access_token: None,
        },
    )
    .expect("valid sign");
    let parts: Vec<&str> = proof.split('.').collect();
    let jwk = kp.public_jwk();
    let bad_hdr = URL_SAFE_NO_PAD.encode(format!(
        r#"{{"typ":"dpop+jwt","alg":"ES256","jwk":{{"kty":"EC","crv":"P-256","x":"{}","y":"{}","d":"secret"}}}}"#,
        jwk.x, jwk.y
    ));
    let bad_hdr_proof = format!("{bad_hdr}.{}.{}", parts[1], parts[2]);
    let hdr_err = verify_dpop_proof(&VerifyDpopProofInput {
        proof: Some(&bad_hdr_proof),
        method: "POST",
        url: "https://example.com/oauth/token",
        now_epoch_seconds: 101,
        expected_thumbprint: None,
        expected_access_token: None,
        max_age_seconds: None,
    })
    .expect_err("private d rejected");
    assert_eq!(hdr_err.code, DpopVerificationFailureCode::MalformedProof);

    // RFC 7638 canonical thumbprint uses exact ASCII key order crv,kty,x,y.
    let canonical = DpopPublicJwk {
        kty: "EC".to_owned(),
        crv: "P-256".to_owned(),
        x: "abc".to_owned(),
        y: "def".to_owned(),
    };
    let expected_hash =
        compute_dpop_access_token_hash(r#"{"crv":"P-256","kty":"EC","x":"abc","y":"def"}"#);
    assert_eq!(compute_dpop_jwk_thumbprint(&canonical), expected_hash);
}

#[test]
fn palette_search_edge_cases_and_regex_helpers() {
    let index = FuzzyPaletteIndex::from_entries([
        PaletteEntry {
            fields: vec![
                PaletteField::primary("Fix JSONSchema parsing"),
                PaletteField {
                    text: "pr",
                    options: FieldOptions {
                        weight: 0.8,
                        mode: FieldMatchMode::KeywordWholeWords,
                        context: false,
                    },
                },
            ],
            usage_boost: 1.0,
        },
        PaletteEntry {
            fields: vec![
                PaletteField::primary("VS Code integration"),
                PaletteField {
                    text: "t3code",
                    options: FieldOptions {
                        weight: 0.25,
                        mode: FieldMatchMode::WordStart,
                        context: true,
                    },
                },
            ],
            usage_boost: 2.0,
        },
    ]);

    let camel = index.search("sch");
    assert_eq!(camel.len(), 1);
    assert_eq!(camel[0].item_index, 0);
    assert_eq!(camel[0].ranges, vec![SearchRange { start: 8, end: 11 }]);

    let joined = index.search("vscode");
    assert_eq!(joined.len(), 1);
    assert_eq!(joined[0].item_index, 1);
    assert_eq!(
        joined[0].ranges,
        vec![
            SearchRange { start: 0, end: 2 },
            SearchRange { start: 3, end: 7 },
        ]
    );

    assert_eq!(
        fold_search_query("  “Fix  the Sidebar” "),
        "fix the sidebar"
    );
    assert_eq!(strip_worktree_branch_prefix("t3/cmdk-v2"), "cmdk-v2");
    assert_eq!(strip_thread_query_prefixes("pr #22889"), "22889");
    assert_eq!(strip_thread_query_prefixes("pull request 42"), "42");
    assert_eq!(
        assistant_citation_label(&format!("{}😀 more", "a".repeat(63))),
        format!("{}…", "a".repeat(63))
    );
    let md = "[Assistant quote](t3-citation://v1/e/t/m?text=a&start=0&end=1&prefix=&suffix=)";
    assert_eq!(
        extract_assistant_citation_hrefs(md),
        vec!["t3-citation://v1/e/t/m?text=a&start=0&end=1&prefix=&suffix="]
    );
}
