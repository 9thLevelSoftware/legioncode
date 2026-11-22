//! Tests and `proptest` suites for `thread_families`.

use std::collections::BTreeMap;

use legion_client_core::thread_families::{
    THREAD_FAMILY_IDLE_TTL_MS, ThreadFamilyCache, ThreadFamilyRead, ThreadFamilyRow,
    ThreadFamilySnapshot, thread_family_rows_missing_from_shell,
};
use legion_contracts::ids::{EnvironmentId, ThreadId};
use proptest::prelude::*;

fn env(id: &str) -> EnvironmentId {
    EnvironmentId::new(id).expect("valid env id")
}

fn tid(id: &str) -> ThreadId {
    ThreadId::new(id).expect("valid thread id")
}

#[test]
fn thread_families_respects_generations_ref_counts_ttl_and_removals() {
    let e = env("env-1");
    let parent = tid("parent");
    let child = tid("idle-child");
    let removed = tid("removed");

    let mut cache: ThreadFamilyCache<ThreadFamilyRow<&'static str>> = ThreadFamilyCache::default();
    let mut h1 = cache.mount_family(&e, &parent, 0);
    let mut h2 = cache.mount_family(&e, &parent, 0);
    assert_eq!(cache.mount_ref_count(&e, "family:parent"), 2);
    assert_eq!(cache.begin_fetch(&e, &parent, 0), None);
    assert_eq!(cache.read_state(&e, &parent), ThreadFamilyRead::Pending);

    // Generation 1 settles and merges behind shell rows.
    cache.set_shell_generation(&e, 1, 10);
    assert_eq!(cache.begin_fetch(&e, &parent, 10), Some(1));
    cache.complete_fetch(
        &e,
        &parent,
        1,
        Ok(ThreadFamilySnapshot {
            snapshot_sequence: 5,
            threads: vec![
                ThreadFamilyRow::new(parent.clone(), "stale-copy"),
                ThreadFamilyRow::new(child.clone(), "idle"),
                ThreadFamilyRow::new(removed.clone(), "gone"),
            ],
        }),
        true,
        20,
    );
    assert_eq!(cache.read_state(&e, &parent), ThreadFamilyRead::Settled);
    let mut removed_at = BTreeMap::new();
    removed_at.insert(removed, 7);
    let merged = cache.merged_threads(
        &e,
        &[ThreadFamilyRow::new(parent.clone(), "live-shell")],
        Some(4),
        &removed_at,
    );
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].payload, "live-shell");
    assert_eq!(merged[1].id, child);

    // Generation 2 invalidates old read immediately; failure retries on re-mount.
    cache.set_shell_generation(&e, 2, 30);
    assert_eq!(cache.read_state(&e, &parent), ThreadFamilyRead::Pending);
    assert_eq!(
        cache
            .merged_threads(
                &e,
                &[ThreadFamilyRow::new(parent.clone(), "live-shell")],
                None,
                &BTreeMap::new(),
            )
            .len(),
        1
    );
    assert_eq!(cache.begin_fetch(&e, &parent, 30), Some(2));
    cache.complete_fetch(&e, &parent, 2, Err("boom".to_owned()), true, 40);
    assert_eq!(cache.read_state(&e, &parent), ThreadFamilyRead::Failed);
    let mut h3 = cache.mount_family(&e, &parent, 50);
    assert_eq!(cache.read_state(&e, &parent), ThreadFamilyRead::Pending);

    // Idempotent release + 60s idle TTL eviction.
    assert!(cache.release(&mut h1, 60));
    assert!(!cache.release(&mut h1, 61));
    assert_eq!(cache.mount_ref_count(&e, "family:parent"), 2);
    assert!(cache.release(&mut h2, 62));
    assert!(cache.release(&mut h3, 63));
    assert_eq!(cache.mount_ref_count(&e, "family:parent"), 0);
    cache.complete_fetch(
        &e,
        &parent,
        2,
        Ok(ThreadFamilySnapshot {
            snapshot_sequence: 9,
            threads: vec![ThreadFamilyRow::new(child, "idle")],
        }),
        true,
        63,
    );
    // Generation 1 went idle at t=30; generation 2 went idle at t=63.
    assert_eq!(cache.evict_expired(30 + THREAD_FAMILY_IDLE_TTL_MS - 1), 0);
    assert_eq!(cache.evict_expired(30 + THREAD_FAMILY_IDLE_TTL_MS), 1);
    assert_eq!(cache.evict_expired(63 + THREAD_FAMILY_IDLE_TTL_MS - 1), 0);
    assert_eq!(cache.evict_expired(63 + THREAD_FAMILY_IDLE_TTL_MS), 1);
    assert_eq!(cache.cached_entry_count(), 0);
}

#[test]
fn thread_families_settles_empty_when_capability_disabled_and_filters_older_cut() {
    let e = env("env-2");
    let parent = tid("parent");
    let stale_row = tid("stale");
    let fresh_row = tid("fresh");

    let mut cache: ThreadFamilyCache<ThreadFamilyRow<&'static str>> = ThreadFamilyCache::default();
    let mut shelf = cache.mount_rows(
        &e,
        "older-settled:page",
        vec![
            ThreadFamilySnapshot {
                snapshot_sequence: 3,
                threads: vec![ThreadFamilyRow::new(stale_row, "old")],
            },
            ThreadFamilySnapshot {
                snapshot_sequence: 5,
                threads: vec![ThreadFamilyRow::new(fresh_row.clone(), "kept")],
            },
        ],
        0,
    );
    let merged = cache.merged_threads(
        &e,
        &[ThreadFamilyRow::new(parent.clone(), "shell")],
        Some(4),
        &BTreeMap::new(),
    );
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[1].id, fresh_row);
    assert!(cache.release(&mut shelf, 10));

    // When `shell_thread_families_enabled` is false, fetch settles empty.
    cache.set_shell_generation(&e, 1, 10);
    let generation = cache.begin_fetch(&e, &parent, 10).expect("generation 1");
    cache.complete_fetch(
        &e,
        &parent,
        generation,
        Ok(ThreadFamilySnapshot {
            snapshot_sequence: 8,
            threads: vec![ThreadFamilyRow::new(fresh_row, "ignored")],
        }),
        false,
        20,
    );
    assert_eq!(cache.read_state(&e, &parent), ThreadFamilyRead::Settled);
    let mut handle = cache.mount_family(&e, &parent, 25);
    assert_eq!(
        cache
            .merged_threads(
                &e,
                &[ThreadFamilyRow::new(parent, "shell")],
                None,
                &BTreeMap::new(),
            )
            .len(),
        1
    );
    assert!(cache.release(&mut handle, 30));
}

proptest! {
    #[test]
    fn prop_family_merge_shell_rows_always_win_and_no_duplicates(
        shell_count in 0_usize..8,
        fam_count in 0_usize..8,
        fam_seq in 0_u64..20,
        removed_seq in 0_u64..20,
    ) {
        let shell_rows: Vec<ThreadId> = (0..shell_count).map(|i| tid(&format!("t-{i}"))).collect();
        let fam_rows: Vec<ThreadId> = (0..fam_count).map(|i| tid(&format!("t-{i}"))).collect();
        let mut removed = BTreeMap::new();
        if let Some(first) = fam_rows.first() {
            removed.insert(first.clone(), removed_seq);
        }
        let missing = thread_family_rows_missing_from_shell(
            &shell_rows,
            &[ThreadFamilySnapshot { snapshot_sequence: fam_seq, threads: fam_rows }],
            &removed,
        );
        for item in &missing {
            prop_assert!(!shell_rows.contains(item));
            prop_assert!(removed.get(item).is_none_or(|&r| r <= fam_seq));
        }
    }
}
