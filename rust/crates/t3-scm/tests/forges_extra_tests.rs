//! Integration tests covering `Forgejo`, `GitCafe`, `Cursor` `Origin`, and `Linear` GraphQL adapters.

use http::{HeaderMap, HeaderValue, Method};
use serde_json::json;
use t3_contracts::pull_requests::PullRequestMergeability;
use t3_scm::core::pr_helpers::{CheckEntry, RawCheckState};
use t3_scm::forges::ForgeError;
use t3_scm::forges::cursor_origin::{
    decode_i64_flexible, is_cursor_origin_target, normalize_cursor_origin_pr_json,
    prepared_mergeability,
};
use t3_scm::forges::forgejo::{
    TargetOs, build_forgejo_http_request, build_tea_api_command, forgejo_head_branch,
    forgejo_keys_json_path, is_forgejo_wip_title, mark_required_forgejo_checks,
    normalize_forgejo_pr_json, parse_tea_http_status,
};
use t3_scm::forges::gitcafe::{
    GitCafeReviewDraft, GitCafeRevisionFence, ReviewSide, build_gitcafe_review_draft_request,
    is_allowed_gitcafe_host, normalize_gitcafe_pr_json, resolve_gitcafe_origin,
    validate_gitcafe_api_url, verify_gitcafe_revision_fence,
};
use t3_scm::forges::linear::{
    LINEAR_GRAPHQL_URL, LinearAuth, LinearErrorCode, build_linear_api_key_request,
    build_linear_graphql_request, classify_linear_graphql_errors, parse_linear_issue_url,
    parse_linear_rate_limit_headers,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn forgejo_adapter_keys_paths_tea_wip_and_globs() -> TestResult {
    assert_eq!(
        forgejo_keys_json_path(
            TargetOs::Windows,
            r"C:\Users\dev",
            Some(r"C:\Users\dev\AppData\Roaming"),
            None
        ),
        r"C:\Users\dev\AppData\Roaming\forgejo-cli\keys.json"
    );
    let fj_req =
        build_forgejo_http_request("codeberg.org", "tok_fj", Method::GET, "/repos/a/b", None)?;
    assert!(
        fj_req
            .headers
            .contains(&("authorization".into(), "token tok_fj".into()))
    );
    let tea = build_tea_api_command(None, &Method::GET, "/repos/a/b", None);
    assert_eq!(tea.program, "tea");
    assert_eq!(parse_tea_http_status("HTTP/2 404 Not Found\n"), Some(404));
    assert_eq!(
        forgejo_head_branch(8, Some("fork:branch"), Some(1)),
        "refs/pull/8/head"
    );
    assert!(is_forgejo_wip_title("[WIP] Draft PR"));
    let mut fj_checks = vec![CheckEntry {
        name: "ci/unit-tests".to_owned(),
        context: None,
        state: RawCheckState::Success,
        required: false,
        updated_at: None,
        details_url: None,
    }];
    assert!(mark_required_forgejo_checks(&mut fj_checks, &["ci/*"]));
    assert!(fj_checks[0].required);
    assert!(!mark_required_forgejo_checks(
        &mut fj_checks,
        &["ci/[a-z]*"]
    ));
    let fj_pr = normalize_forgejo_pr_json(&json!({
        "number": 8,
        "title": "WIP: draft",
        "state": "open",
        "head": { "ref": "feat", "sha": "111" },
        "base": { "ref": "main" }
    }))?;
    assert!(fj_pr.is_draft);
    Ok(())
}

#[test]
fn gitcafe_adapter_allowlist_same_origin_and_revision_fence() -> TestResult {
    assert!(is_allowed_gitcafe_host("staging.git.cafe"));
    assert!(!is_allowed_gitcafe_host("git.cafe.evil.com"));
    assert_eq!(resolve_gitcafe_origin("git.cafe")?, "https://git.cafe");
    assert!(matches!(
        validate_gitcafe_api_url("https://git.cafe", "https://git.cafe/not-api"),
        Err(ForgeError::UntrustedUrl(_))
    ));
    let draft = GitCafeReviewDraft {
        path: "src/lib.rs".to_owned(),
        line: 4,
        side: ReviewSide::Right,
        body: "Looks good".to_owned(),
        fence: GitCafeRevisionFence {
            expected_version: 3,
            commit_oid: "sha1".to_owned(),
            request_id: "req-1".to_owned(),
        },
    };
    assert!(matches!(
        verify_gitcafe_revision_fence(&draft.fence, 4, "sha1"),
        Err(ForgeError::RevisionConflict { .. })
    ));
    let draft_req = build_gitcafe_review_draft_request(
        "git.cafe",
        "tok_gc",
        "/api/v1/review",
        &draft,
        3,
        "sha1",
    )?;
    assert_eq!(draft_req.url, "https://git.cafe/api/v1/review");
    let gc_pr = normalize_gitcafe_pr_json(&json!({
        "number": 3,
        "title": "Cafe fix",
        "state": "open",
        "headRef": "fix",
        "baseRef": "main"
    }))?;
    assert_eq!(gc_pr.number, 3);
    Ok(())
}

#[test]
fn cursor_origin_int64_string_and_sha_pinned_mergeability() -> TestResult {
    assert!(is_cursor_origin_target(
        "https://origin.cursor.com/org/repo"
    ));
    assert_eq!(
        decode_i64_flexible(&json!("9007199254740995")),
        Some(9_007_199_254_740_995)
    );
    assert_eq!(
        prepared_mergeability(Some("sha-old"), Some("sha-new"), Some(true)),
        PullRequestMergeability::Unknown
    );
    let co_pr = normalize_cursor_origin_pr_json(&json!({
        "number": "77",
        "title": "Origin change",
        "state": "open",
        "headBranch": "feat",
        "baseBranch": "main",
        "headSha": "abc123",
        "preparedMergeability": { "versionHeadSha": "abc123", "mergeable": true }
    }))?;
    assert_eq!(co_pr.number, 77);
    assert_eq!(co_pr.mergeability, PullRequestMergeability::Mergeable);
    Ok(())
}

#[test]
fn linear_raw_authorization_without_bearer_and_ratelimit_headers() -> TestResult {
    // CRITICAL: Linear API key MUST NOT include a `Bearer ` prefix!
    let api_req = build_linear_api_key_request("lin_api_deadbeef", "{ viewer { id } }", None)?;
    assert_eq!(api_req.url, LINEAR_GRAPHQL_URL);
    assert!(
        api_req
            .headers
            .contains(&("authorization".into(), "lin_api_deadbeef".into()))
    );
    assert!(
        !api_req
            .headers
            .iter()
            .any(|(_, v)| v.starts_with("Bearer "))
    );

    let oauth_req = build_linear_graphql_request(
        &LinearAuth::OAuthBearer("oauth_tok".into()),
        "{ viewer { id } }",
        None,
    )?;
    assert!(
        oauth_req
            .headers
            .contains(&("authorization".into(), "Bearer oauth_tok".into()))
    );

    let mut rl_headers = HeaderMap::new();
    rl_headers.insert(
        "x-ratelimit-requests-remaining",
        HeaderValue::from_static("1400"),
    );
    rl_headers.insert(
        "x-ratelimit-requests-reset",
        HeaderValue::from_static("1700000123000"),
    );
    rl_headers.insert(
        "x-ratelimit-complexity-remaining",
        HeaderValue::from_static("240000"),
    );
    rl_headers.insert(
        "x-ratelimit-complexity-reset",
        HeaderValue::from_static("1700000124000"),
    );
    let parsed_rl = parse_linear_rate_limit_headers(&rl_headers);
    assert_eq!(parsed_rl.requests_remaining, Some(1400));
    assert_eq!(parsed_rl.requests_reset_ms, Some(1_700_000_123_000));

    assert_eq!(
        classify_linear_graphql_errors(&json!({"errors":[{"extensions":{"code":"RATELIMITED"}}]})),
        Some(LinearErrorCode::RateLimited)
    );
    assert_eq!(
        classify_linear_graphql_errors(
            &json!({"errors":[{"extensions":{"code":"AUTHENTICATION_ERROR"}}]})
        ),
        Some(LinearErrorCode::AuthenticationError)
    );
    let issue = parse_linear_issue_url("https://linear.app/t3/issue/eng-123/implement-rust")
        .ok_or("expected Linear issue ref")?;
    assert_eq!(issue.identifier, "ENG-123");
    assert_eq!(issue.number, 123);
    Ok(())
}
