//! Integration and property tests for `t3_scm::core`.

use http::{HeaderMap, HeaderValue, StatusCode};
use proptest::prelude::*;
use t3_contracts::pull_requests::PullRequestChecksState;
use t3_scm::core::CoreError;
use t3_scm::core::discovery::{RemoteTransport, is_windows_drive_path, parse_git_remote_url};
use t3_scm::core::pr_helpers::{
    CheckEntry, MergeMessageInput, RawCheckState, build_merge_commit_message, dedupe_checks,
    remove_agent_credits, rollup_check_statuses, sort_checks,
};
use t3_scm::core::rate_limit::{
    QuotaSnapshot, RateLimitDecision, SourceControlRateLimit, parse_retry_after,
};
use t3_scm::core::revisions::{
    DiffFileRevisionTracker, RevisionMatch, cut_line_note, quote_git_patch_path, sha256_hex,
    unquote_git_patch_path,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn discovery_parses_all_remote_formats_and_rejects_local_paths() -> TestResult {
    let https = parse_git_remote_url("https://github.com/t3dotgg/t3code.git")?;
    assert_eq!(https.transport, RemoteTransport::Https);
    assert_eq!(https.full_name, "t3dotgg/t3code");
    assert_eq!(
        https
            .provider_hint
            .as_ref()
            .map(t3_contracts::ids::SourceControlProviderKind::as_str),
        Some("github")
    );

    let ssh = parse_git_remote_url("ssh://git@gitlab.corp.internal:2222/team/sub/app.git")?;
    assert_eq!(ssh.transport, RemoteTransport::Ssh);
    assert_eq!(ssh.host, "gitlab.corp.internal");
    assert_eq!(ssh.port, Some(2222));
    assert_eq!(ssh.owner, "team/sub");
    assert_eq!(ssh.repo, "app");

    let scp = parse_git_remote_url("git@bitbucket.org:workspace/service.git")?;
    assert_eq!(scp.transport, RemoteTransport::Scp);
    assert_eq!(scp.host, "bitbucket.org");
    assert_eq!(scp.owner, "workspace");
    assert_eq!(scp.repo, "service");

    let az_https = parse_git_remote_url("https://dev.azure.com/contoso/Core%20App/_git/web")?;
    assert_eq!(
        az_https.azure.as_ref().map(|a| a.organization.as_str()),
        Some("contoso")
    );
    assert_eq!(
        az_https.azure.as_ref().map(|a| a.repository.as_str()),
        Some("web")
    );

    let az_ssh = parse_git_remote_url("git@ssh.dev.azure.com:v3/contoso/proj/api")?;
    assert_eq!(az_ssh.owner, "contoso/proj");
    assert_eq!(az_ssh.repo, "api");

    assert!(is_windows_drive_path(r"C:\dev\t3code"));
    for bad in [
        "",
        r"C:\dev\t3code",
        "D:/repo.git",
        "/tmp/local-repo",
        "file:///tmp/x",
    ] {
        assert!(
            matches!(
                parse_git_remote_url(bad),
                Err(CoreError::InvalidRemote { .. })
            ),
            "expected InvalidRemote for `{bad}`"
        );
    }
    Ok(())
}

#[test]
fn rate_limit_concurrency_retry_after_and_reserve() -> TestResult {
    let limiter = SourceControlRateLimit::new(2, 10);
    let now = 1_700_000_000;

    // RFC 7231 IMF-fixdate parsing: Wed, 21 Oct 2015 07:28:00 GMT == 1_445_412_480
    assert_eq!(parse_retry_after("45", now), Some(45));
    assert_eq!(
        parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT", 1_445_412_420),
        Some(60)
    );

    let p1 = limiter.acquire("api.github.com", false, now)?;
    let p2 = limiter.acquire("api.github.com", false, now)?;
    assert_eq!(limiter.active_count("api.github.com"), 2);
    assert!(matches!(
        limiter.check_admission("api.github.com", false, now)?,
        RateLimitDecision::ConcurrencyFull {
            active: 2,
            max_concurrency: 2
        }
    ));
    drop(p1);
    drop(p2);
    assert_eq!(limiter.active_count("api.github.com"), 0);

    let mut headers = HeaderMap::new();
    headers.insert("retry-after", HeaderValue::from_static("30"));
    headers.insert("x-ratelimit-limit", HeaderValue::from_static("5000"));
    headers.insert("x-ratelimit-remaining", HeaderValue::from_static("499"));
    let update = limiter.record_response(
        "api.github.com",
        StatusCode::TOO_MANY_REQUESTS,
        &headers,
        now,
    )?;
    assert_eq!(update.retry_after_secs, Some(30));
    assert!(matches!(
        limiter.check_admission("api.github.com", false, now + 10)?,
        RateLimitDecision::BlockedUntil { .. }
    ));

    // After backoff expires, 499/5000 hits 10% quota reserve unless `allow_reserve` is true.
    assert!(matches!(
        limiter.check_admission("api.github.com", false, now + 31)?,
        RateLimitDecision::RejectedQuotaReserve {
            remaining: 499,
            limit: 5000,
            reserve_threshold: 500
        }
    ));
    assert_eq!(
        limiter.check_admission("api.github.com", true, now + 31)?,
        RateLimitDecision::Admitted
    );
    Ok(())
}

#[test]
fn merge_message_strips_agent_credits_and_preserves_humans() {
    let raw = "Refactor parser.\n\nGenerated with Claude Code\n\nCo-authored-by: Claude <noreply@anthropic.com>\nCo-authored-by: Codex <noreply@openai.com>\nCo-authored-by: Jane Doe <jane@example.com>";
    assert_eq!(
        remove_agent_credits(raw),
        "Refactor parser.\n\nCo-authored-by: Jane Doe <jane@example.com>"
    );

    let msg = build_merge_commit_message(&MergeMessageInput {
        title: "feat:  add   rate limiter".to_owned(),
        number: Some(42),
        body: Some("Summary line.\nCo-authored-by: T3 Code <bot@t3code>".to_owned()),
        include_body: true,
        strip_agent_credits: true,
        co_authors: vec!["Alice <alice@example.com>".to_owned()],
    });
    assert_eq!(msg.subject, "feat: add rate limiter (#42)");
    assert_eq!(
        msg.full_message,
        "feat: add rate limiter (#42)\n\nSummary line.\n\nCo-authored-by: Alice <alice@example.com>"
    );
}

#[test]
fn pr_checks_rollup_sort_and_dedupe() {
    let checks = vec![
        CheckEntry {
            name: "build".to_owned(),
            context: Some("ci".to_owned()),
            state: RawCheckState::Pending,
            required: false,
            updated_at: Some("2026-11-20T01:00:00Z".to_owned()),
            details_url: None,
        },
        CheckEntry {
            name: "build".to_owned(),
            context: Some("ci".to_owned()),
            state: RawCheckState::Success,
            required: true,
            updated_at: Some("2026-11-20T02:00:00Z".to_owned()),
            details_url: None,
        },
        CheckEntry {
            name: "lint".to_owned(),
            context: None,
            state: RawCheckState::Failure,
            required: true,
            updated_at: Some("2026-11-20T02:00:00Z".to_owned()),
            details_url: None,
        },
    ];

    let mut deduped = dedupe_checks(checks);
    assert_eq!(deduped.len(), 2);
    sort_checks(&mut deduped);
    assert_eq!(deduped[0].name, "lint");
    assert_eq!(deduped[1].state, RawCheckState::Success);
    assert_eq!(
        rollup_check_statuses(&deduped),
        PullRequestChecksState::Failing
    );
    assert_eq!(
        rollup_check_statuses(&deduped[1..]),
        PullRequestChecksState::Passing
    );
}

#[test]
fn revision_tracker_and_git_patch_path_roundtrip() -> TestResult {
    let mut tracker = DiffFileRevisionTracker::new();
    let v1 = b"fn hi() {}\n";
    let fp1 = tracker.record_file("src/a.rs", v1, Some(10)).clone();
    assert_eq!(fp1.sha256, sha256_hex(v1));
    assert_eq!(
        cut_line_note(fp1.cut_line).as_deref(),
        Some("Diff truncated at line 10")
    );

    let viewed_sha = tracker.mark_viewed("src/a.rs")?;
    assert!(tracker.is_viewed_at_current_revision("src/a.rs"));
    assert_eq!(
        tracker.compare_revision("src/a.rs", &viewed_sha),
        RevisionMatch::Current
    );

    tracker.record_file("src/a.rs", b"fn hi() { 2; }\n", None);
    assert!(!tracker.is_viewed_at_current_revision("src/a.rs"));
    assert_eq!(tracker.stale_viewed_files(), vec!["src/a.rs"]);

    for sample in [
        "  leading space/file .rs ",
        "dir\\sub\"quote\"\t\némoji🦀.txt",
    ] {
        let quoted = quote_git_patch_path(sample);
        let roundtripped = unquote_git_patch_path(&quoted)?;
        assert_eq!(roundtripped, sample);
    }
    Ok(())
}

proptest! {
    #[test]
    fn prop_git_patch_path_roundtrips_without_losing_spaces(path in ".*") {
        let quoted = quote_git_patch_path(&path);
        let unquoted = unquote_git_patch_path(&quoted).map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(unquoted, path);
    }

    #[test]
    fn prop_quota_ten_percent_reserve_matches_exact_ratio(
        limit in 1u64..100_000u64,
        remaining in 0u64..100_000u64,
    ) {
        let snap = QuotaSnapshot {
            limit,
            remaining,
            reset_epoch_secs: None,
        };
        let expected_background = remaining > 0 && remaining * 10 >= limit;
        prop_assert_eq!(snap.has_headroom(10, false), expected_background);
        prop_assert_eq!(snap.has_headroom(10, true), remaining > 0);
    }
}
