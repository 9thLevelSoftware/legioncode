//! Integration tests for `mobile_review` features (`#40295`, `#26759`, `#36868`, `#24841`,
//! `#31257`, `#40297`).

use t3_contracts::ids::{ThreadId, TurnItemId};
use t3_contracts::orchestration::SecretRequestField;
use t3_contracts::secrets::{SecretRequestAnswer, SecretRequestAnswerInput};
use t3_surfaces::mobile_review::{
    ChangedFileKind, GitSectionLoadState, LinkedLineRange, PullRequestReviewEntry,
    PullRequestReviewState, ReviewApprovalLabels, ReviewDiffSource, ReviewFileEntry,
    ReviewGitSectionKind, ReviewSectionKind, SUPPORTED_REVIEW_LOCALES, SecretCardState,
    TextSelectionMenuLabels, build_review_section_items, classify_changed_file,
    decline_secret_request_input, evaluate_pull_request_reviews, filter_review_files,
    secret_request_privacy_note, selection_marker_width_dp, selection_marker_width_px,
    shown_patch_indices, validate_review_source_selectors,
};

#[test]
fn linked_line_range_and_selection_marker_width_handle_source_viewer() {
    let range = LinkedLineRange::new(42, 18);
    assert_eq!(range.start_line, 18);
    assert_eq!(range.end_line, 42);
    assert_eq!(range.line_count(), 25);
    assert!(range.contains(18));
    assert!(range.contains(42));
    assert!(!range.contains(43));

    assert!((selection_marker_width_dp(0.0) - 4.0).abs() < f32::EPSILON);
    assert!((selection_marker_width_dp(6.0) - 6.0).abs() < f32::EPSILON);
    assert!((selection_marker_width_px(0.0, 3.0) - 12.0).abs() < f32::EPSILON);
    assert!((selection_marker_width_px(18.0, 3.0) - 18.0).abs() < f32::EPSILON);
}

#[test]
fn test_file_filter_and_shown_patch_indices_skip_tests_unless_included() {
    assert_eq!(
        classify_changed_file("Cargo.lock"),
        ChangedFileKind::Generated
    );
    assert_eq!(
        classify_changed_file("src/__mocks__/client.ts"),
        ChangedFileKind::Mocks
    );
    assert_eq!(
        classify_changed_file("src/AppTest.java"),
        ChangedFileKind::Tests
    );
    assert_eq!(
        classify_changed_file("src/Latest.java"),
        ChangedFileKind::Code
    );
    assert_eq!(classify_changed_file("README.md"), ChangedFileKind::Docs);
    assert_eq!(
        classify_changed_file("requirements.txt"),
        ChangedFileKind::Code
    );

    let entries = vec![
        ReviewFileEntry {
            path: "a.test.ts".to_owned(),
            additions: 10,
            deletions: 2,
        },
        ReviewFileEntry {
            path: "src/main.rs".to_owned(),
            additions: 5,
            deletions: 1,
        },
        ReviewFileEntry {
            path: "src/__fixtures__/data.json".to_owned(),
            additions: 20,
            deletions: 0,
        },
    ];
    let filtered = filter_review_files(&entries, true, Some("a.test.ts"));
    assert_eq!(filtered.file_count, 2);
    assert_eq!(filtered.hidden_test_count, 1);
    assert_eq!(filtered.additions, 15);
    assert_eq!(filtered.deletions, 3);

    let paths = [
        "a.test.ts",
        "b.test.ts",
        "c.test.ts",
        "d.ts",
        "e.test.ts",
        "f.ts",
        "g.ts",
    ];
    assert_eq!(shown_patch_indices(&paths, true, None), vec![3, 5, 6]);
    assert_eq!(shown_patch_indices(&paths, true, Some(5)), vec![5, 6]);
    assert_eq!(shown_patch_indices(&paths, false, None), vec![0, 1, 2]);
}

#[test]
fn outdated_pr_approval_detection_marks_stale_approvals_after_new_commits() {
    let entries = vec![
        PullRequestReviewEntry {
            id: "r1".to_owned(),
            author_login: Some("alice".to_owned()),
            raw_state: Some("approved".to_owned()),
            created_at_ms: 1_000,
        },
        PullRequestReviewEntry {
            id: "r2".to_owned(),
            author_login: Some("bob".to_owned()),
            raw_state: Some("changes-requested".to_owned()),
            created_at_ms: 2_500,
        },
        PullRequestReviewEntry {
            id: "r3".to_owned(),
            author_login: Some("alice".to_owned()),
            raw_state: Some("COMMENTED".to_owned()),
            created_at_ms: 3_000,
        },
        PullRequestReviewEntry {
            id: "r4".to_owned(),
            author_login: Some("carol".to_owned()),
            raw_state: Some("APPROVED".to_owned()),
            created_at_ms: 4_000,
        },
    ];

    let summary = evaluate_pull_request_reviews(&entries, &[2_000, 3_500]);
    assert_eq!(summary.current_approval_count, 1);
    assert_eq!(summary.outdated_approval_count, 1);
    assert_eq!(summary.changes_requested_count, 1);

    let alice = summary
        .reviews
        .iter()
        .find(|r| r.author_login.as_deref() == Some("alice"))
        .expect("alice review kept");
    assert_eq!(alice.state, PullRequestReviewState::Approved);
    assert!(alice.outdated);
    assert!(!alice.is_current_approval());

    let carol = summary
        .reviews
        .iter()
        .find(|r| r.author_login.as_deref() == Some("carol"))
        .expect("carol review kept");
    assert!(!carol.outdated);
    assert!(carol.is_current_approval());
}

#[test]
fn failed_branch_comparison_keeps_working_tree_uncommitted_section_visible() {
    let working_tree = ReviewDiffSource {
        kind: ReviewGitSectionKind::WorkingTree,
        title: "Uncommitted".to_owned(),
        subtitle: None,
        diff: "diff --git a/a.ts b/a.ts\n".to_owned(),
        truncated: false,
    };

    let items = build_review_section_items(
        &[],
        &[working_tree],
        GitSectionLoadState::Idle,
        GitSectionLoadState::Failed,
    );
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "git:working-tree");
    assert_eq!(items[0].kind, ReviewSectionKind::WorkingTree);
    assert!(items[0].diff.is_some());
    assert!(!items[0].is_loading);
    assert_eq!(items[1].id, "git:branch-range");
    assert_eq!(items[1].kind, ReviewSectionKind::BranchRange);
    assert!(items[1].diff.is_none());
    assert!(!items[1].is_loading);
    assert_eq!(items[1].load_state, GitSectionLoadState::Failed);

    assert_eq!(
        validate_review_source_selectors(
            Some(ReviewGitSectionKind::WorkingTree),
            Some(ReviewGitSectionKind::BranchRange)
        ),
        Err("Review source selectors must agree.")
    );
    assert_eq!(
        validate_review_source_selectors(None, Some(ReviewGitSectionKind::WorkingTree)),
        Ok(Some(ReviewGitSectionKind::WorkingTree))
    );
}

#[test]
fn secret_card_state_handles_multi_value_fields_show_hide_and_contracts() {
    let fields = vec![
        SecretRequestField {
            name: "email".to_owned(),
            label: "Email".to_owned(),
            placeholder: Some("you@example.com".to_owned()),
            secret: Some(false),
            revealable: false,
        },
        SecretRequestField {
            name: "password".to_owned(),
            label: "Password".to_owned(),
            placeholder: None,
            secret: Some(true),
            revealable: true,
        },
    ];
    let mut card = SecretCardState::new("Deploy login", None, Some(&fields));
    assert!(card.is_revealed("email"));
    assert!(!card.is_revealed("password"));
    assert!(card.toggle_reveal("password"));
    assert!(card.is_revealed("password"));
    assert!(!card.toggle_reveal("password"));
    assert!(!card.is_revealed("password"));

    card.set_value("email", " dev@example.com ");
    assert!(!card.is_ready());
    assert_eq!(card.to_save_answer(), None);
    card.set_value("password", " hunter2 ");
    assert!(card.is_ready());

    let thread_id = ThreadId::new("thread-1").expect("valid thread id");
    let item_id = TurnItemId::new("item-1").expect("valid item id");
    let answer = card.to_answer_input(thread_id.clone(), item_id.clone());
    assert!(matches!(
        answer,
        Some(SecretRequestAnswerInput {
            answer: SecretRequestAnswer::Save {
                secret: None,
                values: Some(ref map),
            },
            ..
        }) if map.get("email").map(String::as_str) == Some("dev@example.com")
            && map.get("password").map(String::as_str) == Some("hunter2")
    ));
    assert_eq!(
        decline_secret_request_input(thread_id, item_id).answer,
        SecretRequestAnswer::Decline
    );
    assert!(secret_request_privacy_note(true).contains("24 hours"));
    assert!(secret_request_privacy_note(false).contains("never shown"));
}

#[test]
fn selection_menu_and_approval_labels_avoid_english_leakage_across_all_locales() {
    let en_menu = TextSelectionMenuLabels::for_locale("en", &[]);
    let en_approval = ReviewApprovalLabels::for_locale("en", &[]);
    assert_eq!(en_menu.format_copy_image_urls(1), "Copy Image URL");
    assert_eq!(en_menu.format_copy_image_urls(3), "Copy 3 Image URLs");

    for locale in SUPPORTED_REVIEW_LOCALES {
        let menu = TextSelectionMenuLabels::for_locale(locale, &[]);
        let approval = ReviewApprovalLabels::for_locale(locale, &[]);
        assert_eq!(menu.locale, locale);
        assert_eq!(approval.locale, locale);
        assert!(menu.copy_image_urls_template.contains("{count}"));
        if locale != "en" {
            assert_ne!(menu.copy, en_menu.copy, "leak in copy for {locale}");
            assert_ne!(
                menu.copy_as_markdown, en_menu.copy_as_markdown,
                "leak in copy_as_markdown for {locale}"
            );
            assert_ne!(
                menu.copy_image_url, en_menu.copy_image_url,
                "leak in copy_image_url for {locale}"
            );
            assert_ne!(
                approval.approved_before_latest_commit, en_approval.approved_before_latest_commit,
                "leak in approved_before_latest_commit for {locale}"
            );
            assert_ne!(
                approval.outdated, en_approval.outdated,
                "leak in outdated for {locale}"
            );
            assert_ne!(
                approval.unknown_reviewer, en_approval.unknown_reviewer,
                "leak in unknown_reviewer for {locale}"
            );
        }
    }

    let zh_fallback = TextSelectionMenuLabels::for_locale("system", &["zh-Hans-CN"]);
    assert_eq!(zh_fallback.locale, "zh");
    assert_eq!(zh_fallback.copy, "复制");
}
