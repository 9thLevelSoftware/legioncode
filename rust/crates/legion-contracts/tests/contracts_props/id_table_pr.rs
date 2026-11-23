use legion_contracts::id_table::{
    IdTableError, IdTableTarget, decode_with_id_table, encode_with_id_table, expand_record_ids,
    intern_record_ids,
};
use legion_contracts::ids::ProjectId;
use legion_contracts::pull_requests::{
    ThreadLinkedPullRequest, ThreadPullRequestKey, ThreadPullRequestLink,
    ThreadPullRequestLinkSource, effective_pull_requests, legacy_pull_request_key,
};
use proptest::prelude::*;
use serde_json::{Value, json};

#[test]
fn id_table_intern_and_expand_reject_bad_indices_without_mutation() {
    let mut ids = vec![json!("t-existing")];
    let original = json!({
        "thread": { "id": "t-1", "projectId": "p-1" },
        "runs": [
            { "threadId": "t-1", "runId": "r-1", "item": { "providerThreadId": "pt-1", "messageId": null } }
        ]
    });

    let interned = intern_record_ids(&original, &mut ids).expect("intern");
    assert_eq!(interned["thread"]["id"], json!(1));
    assert_eq!(interned["thread"]["projectId"], json!(2));
    assert_eq!(interned["runs"][0]["threadId"], json!(1));
    assert_eq!(interned["runs"][0]["runId"], json!(3));
    assert_eq!(interned["runs"][0]["item"]["providerThreadId"], json!(4));
    assert_eq!(interned["runs"][0]["item"]["messageId"], Value::Null);

    let expanded = expand_record_ids(&interned, &ids).expect("expand");
    assert_eq!(expanded, original);

    // BUG-20: Out-of-bounds indices MUST fail and NEVER mutate input or ids.
    let ids_before = ids.clone();
    let bad_payload = json!({ "runs": [{ "threadId": 999, "runId": "raw-not-interned" }] });
    let bad_before = bad_payload.clone();
    assert_eq!(
        expand_record_ids(&bad_payload, &ids),
        Err(IdTableError::OutOfBounds)
    );
    assert_eq!(bad_payload, bad_before);
    assert_eq!(ids, ids_before);
}

#[test]
fn encode_and_decode_with_id_table_handles_events_and_targets() {
    let raw = json!({
        "idTable": true,
        "projection": {
            "events": [
                { "id": "ev-1", "threadId": "t-1", "runId": "r-1" },
                { "id": "ev-2", "threadId": "t-1", "runId": "r-2" }
            ]
        }
    });
    let encoded = encode_with_id_table(&raw, IdTableTarget::Field("projection")).expect("encode");
    assert!(encoded.get("ids").is_some());
    assert!(encoded.get("idTable").is_none());
    assert_eq!(
        encoded["projection"]["events"][0]["threadId"],
        encoded["projection"]["events"][1]["threadId"]
    );

    let decoded =
        decode_with_id_table(&encoded, IdTableTarget::Field("projection")).expect("decode");
    assert_eq!(decoded["projection"], raw["projection"]);

    let no_ids = json!({ "projection": { "events": [{ "threadId": "t-plain" }] } });
    assert_eq!(
        decode_with_id_table(&no_ids, IdTableTarget::Field("projection")).expect("passthrough"),
        no_ids
    );
}

proptest! {
    #[test]
    fn id_table_round_trip_and_out_of_bounds_invariance(
        thread_a in "[a-z0-9_-]{1,12}",
        thread_b in "[a-z0-9_-]{1,12}",
        bad_offset in 5u64..1000u64,
    ) {
        let input = json!({
            "items": [
                { "threadId": thread_a, "messageId": "m-1", "runId": thread_b },
                { "parentItemId": "it-0", "threadId": thread_a, "runId": thread_b }
            ]
        });
        let mut table = Vec::new();
        let compact = intern_record_ids(&input, &mut table).expect("intern");
        let restored = expand_record_ids(&compact, &table).expect("expand");
        prop_assert_eq!(restored, input);

        let invalid = json!({ "items": [{ "threadId": (table.len() as u64) + bad_offset }] });
        let snapshot = invalid.clone();
        prop_assert_eq!(expand_record_ids(&invalid, &table), Err(IdTableError::OutOfBounds));
        prop_assert_eq!(invalid, snapshot);
    }
}

fn sample_link(number: u32, source: ThreadPullRequestLinkSource) -> ThreadPullRequestLink {
    ThreadPullRequestLink {
        host: "github.com".to_owned(),
        repository: "9thlevelsoftware/legioncode".to_owned(),
        number,
        url: format!("https://github.com/9thlevelsoftware/legioncode/pull/{number}"),
        repository_web_url: None,
        source,
        linked_at: "2026-04-01T00:00:00Z".to_owned(),
        snapshot: None,
        stack: None,
        watch: None,
        ended_watch: None,
    }
}

#[test]
fn pull_requests_bug19_distinguishes_some_empty_from_none_and_filters_dismissed() {
    let legacy = ThreadLinkedPullRequest {
        project_id: ProjectId::new("proj-1").expect("proj id"),
        repository: "9thLevelSoftware/LegionCode".to_owned(),
        number: 77,
        url: "https://GitHub.com/9thLevelSoftware/legioncode/pull/77".to_owned(),
    };

    assert_eq!(
        legacy_pull_request_key(&legacy, None),
        ThreadPullRequestKey {
            host: "github.com".to_owned(),
            repository: "9thlevelsoftware/legioncode".to_owned(),
            number: 77,
        }
    );

    // BUG-19: `Some(&[])` means explicitly unlinked — NEVER fall back to legacy.
    assert_eq!(
        effective_pull_requests(Some(&[]), Some(&legacy)),
        Vec::<ThreadPullRequestLink>::new()
    );

    // BUG-19: Only dismissed links present in `Some(...)` still stays empty (no legacy fallback).
    let dismissed_only = [sample_link(10, ThreadPullRequestLinkSource::StackDismissed)];
    assert_eq!(
        effective_pull_requests(Some(&dismissed_only), Some(&legacy)),
        Vec::<ThreadPullRequestLink>::new()
    );

    // Active links survive while `stack-dismissed` is stripped.
    let mixed = [
        sample_link(10, ThreadPullRequestLinkSource::StackDismissed),
        sample_link(11, ThreadPullRequestLinkSource::Stack),
    ];
    let visible = effective_pull_requests(Some(&mixed), Some(&legacy));
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].number, 11);

    // Only `None` synthesizes a manual link from `legacy`.
    let fallback = effective_pull_requests(None, Some(&legacy));
    assert_eq!(fallback.len(), 1);
    assert_eq!(fallback[0].number, 77);
    assert_eq!(fallback[0].source, ThreadPullRequestLinkSource::Manual);
    assert_eq!(fallback[0].repository, "9thlevelsoftware/legioncode");
}

proptest! {
    #[test]
    fn effective_pull_requests_never_includes_stack_dismissed(
        flags in prop::collection::vec(any::<bool>(), 0..10),
        has_legacy in any::<bool>(),
    ) {
        let links: Vec<ThreadPullRequestLink> = flags
            .iter()
            .enumerate()
            .map(|(i, active)| {
                let src = if *active {
                    ThreadPullRequestLinkSource::Stack
                } else {
                    ThreadPullRequestLinkSource::StackDismissed
                };
                sample_link(u32::try_from(i + 1).expect("small len"), src)
            })
            .collect();
        let legacy = has_legacy.then(|| ThreadLinkedPullRequest {
            project_id: ProjectId::new("proj-p").expect("id"),
            repository: "org/repo".to_owned(),
            number: 999,
            url: "https://github.com/org/repo/pull/999".to_owned(),
        });

        let resolved = effective_pull_requests(Some(&links), legacy.as_ref());
        prop_assert!(resolved.iter().all(|l| l.source != ThreadPullRequestLinkSource::StackDismissed));
        prop_assert_eq!(resolved.len(), flags.iter().filter(|f| **f).count());
    }
}
