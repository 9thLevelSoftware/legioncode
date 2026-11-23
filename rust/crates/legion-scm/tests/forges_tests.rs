//! Integration tests covering `GitHub`, `GitLab`, `Azure` `DevOps`, and `Bitbucket`.

use http::{HeaderMap, Method, StatusCode};
use legion_contracts::pull_requests::{PullRequestMergeability, PullRequestState};
use legion_scm::core::discovery::parse_git_remote_url;
use legion_scm::core::rate_limit::QuotaSnapshot;
use legion_scm::forges::ForgeError;
use legion_scm::forges::azure_devops::{
    azure_text_option, build_azure_pr_create_command, normalize_azure_pr_json,
    resolve_azure_cli_launch, scope_azure_cli_args,
};
use legion_scm::forges::bitbucket::{
    BITBUCKET_CONCURRENCY, BitbucketAuth, build_bitbucket_request, normalize_bitbucket_pr_json,
};
use legion_scm::forges::github::{
    GITHUB_API_CONCURRENCY, GitHubEndpoint, GitHubRateLimitSignal, build_gh_cli_env,
    build_github_graphql_request, build_github_rest_request, check_github_quota,
    classify_github_response, normalize_github_pr_json,
};
use legion_scm::forges::gitlab::{
    GitLabCliBuilder, GitLabErrorKind, classify_gitlab_cli_error, normalize_gitlab_mr_json,
};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn github_adapter_quota_enterprise_and_pr_normalization() -> TestResult {
    assert_eq!(GITHUB_API_CONCURRENCY, 8);
    let ghe = GitHubEndpoint::for_host("github.corp.local")?;
    assert_eq!(ghe.api_base_url, "https://github.corp.local/api/v3");

    let req = build_github_rest_request(&ghe, "tok_abc", Method::GET, "/user", None)?;
    assert!(
        req.headers
            .contains(&("x-github-api-version".into(), "2022-11-28".into()))
    );

    let gql = build_github_graphql_request(&ghe, "tok_abc", "query { viewer { login } }", None)?;
    assert_eq!(gql.url, "https://github.corp.local/api/graphql");

    // GH_ENTERPRISE_TOKEN is emitted ONLY when hostname is not github.com.
    let cloud_env = build_gh_cli_env("github.com", Some("ghp_cloud"));
    assert!(!cloud_env.iter().any(|(k, _)| k == "GH_ENTERPRISE_TOKEN"));
    let ghe_env = build_gh_cli_env("github.corp.local", Some("ghe_secret"));
    assert!(ghe_env.contains(&("GH_HOST".into(), "github.corp.local".into())));
    assert!(ghe_env.contains(&("GH_ENTERPRISE_TOKEN".into(), "ghe_secret".into())));

    let low = QuotaSnapshot {
        limit: 1000,
        remaining: 99,
        reset_epoch_secs: None,
    };
    assert!(matches!(
        check_github_quota(&low, false),
        GitHubRateLimitSignal::QuotaReserveReached {
            remaining: 99,
            limit: 1000
        }
    ));
    assert_eq!(check_github_quota(&low, true), GitHubRateLimitSignal::Clear);

    let gql_rl = classify_github_response(
        StatusCode::OK,
        &HeaderMap::new(),
        r#"{"errors":[{"type":"RATE_LIMITED"}]}"#,
    );
    assert!(matches!(gql_rl, GitHubRateLimitSignal::RateLimited { .. }));

    let pr = normalize_github_pr_json(&json!({
        "number": 101,
        "title": "Enable Rust SCM",
        "html_url": "https://github.com/9thlevelsoftware/legioncode/pull/101",
        "state": "open",
        "draft": true,
        "head": { "ref": "feat/scm", "sha": "deadbeef" },
        "base": { "ref": "main" },
        "mergeable": true
    }))?;
    assert_eq!(pr.number, 101);
    assert!(pr.is_draft);
    assert_eq!(pr.mergeability, PullRequestMergeability::Mergeable);
    Ok(())
}

#[test]
fn gitlab_adapter_host_env_snap_stdin_errors_and_mr_json() -> TestResult {
    let builder = GitLabCliBuilder::new(Some("gitlab.corp.dev"), Some("eng/legion"))?;
    let cmd = builder.mr_create("Title", "Body text", "feat", "main", true, true);
    assert!(
        cmd.env
            .contains(&("GITLAB_HOST".into(), "gitlab.corp.dev".into()))
    );
    assert!(cmd.args.contains(&"--draft".to_owned()));
    assert_eq!(cmd.stdin.as_deref(), Some("Body text"));

    assert_eq!(
        classify_gitlab_cli_error("oauth invalid_grant; run glab auth login"),
        GitLabErrorKind::AuthExpired
    );
    let mr = normalize_gitlab_mr_json(&json!({
        "iid": 55,
        "title": "WIP: Update CI",
        "web_url": "https://gitlab.corp.dev/eng/legion/-/merge_requests/55",
        "state": "opened",
        "work_in_progress": true,
        "source_branch": "ci",
        "target_branch": "main",
        "has_conflicts": true
    }))?;
    assert!(mr.is_draft);
    assert_eq!(mr.state, PullRequestState::Open);
    assert_eq!(mr.mergeability, PullRequestMergeability::Conflicting);
    Ok(())
}

#[test]
fn azure_devops_python_bypass_at_stdin_and_scoping() -> TestResult {
    let zip = resolve_azure_cli_launch(r"D:\Tools\AzureCLI\bin\az.cmd", true, true, true);
    assert_eq!(zip.program, r"D:\Tools\AzureCLI\python.exe");
    assert_eq!(zip.prefix_args, vec!["-X", "utf8", "-IBm", "azure.cli"]);
    assert_eq!(zip.env, vec![("AZ_INSTALLER".into(), "ZIP".into())]);

    let (opt_args, opt_stdin) = azure_text_option("description", "@C:\\secret.txt");
    assert_eq!(opt_args, vec!["--description=@-"]);
    assert_eq!(opt_stdin.as_deref(), Some("@C:\\secret.txt"));

    let remote = parse_git_remote_url("https://dev.azure.com/acme/Platform/_git/backend")?;
    let scoped = scope_azure_cli_args(&remote, &["repos", "pr", "list", "--detect", "true"]);
    assert!(!scoped.iter().any(|a| a.starts_with("--detect")));
    assert!(scoped.contains(&"https://dev.azure.com/acme".to_owned()));
    assert!(scoped.contains(&"Platform".to_owned()));

    let created = build_azure_pr_create_command(
        &zip,
        &remote,
        "Fix bug",
        "@leading-at",
        "refs/heads/feat/fix",
        "refs/heads/main",
        false,
    );
    assert_eq!(created.program, r"D:\Tools\AzureCLI\python.exe");
    assert_eq!(created.stdin.as_deref(), Some("@leading-at"));

    let norm = normalize_azure_pr_json(&json!({
        "pullRequestId": 9,
        "title": "Fix bug",
        "status": "completed",
        "sourceRefName": "refs/heads/feat/fix",
        "targetRefName": "refs/heads/main",
        "mergeStatus": "succeeded"
    }))?;
    assert_eq!(norm.state, PullRequestState::Merged);
    assert_eq!(norm.head_branch, "feat/fix");
    Ok(())
}

#[test]
fn bitbucket_adapter_auth_concurrency_and_origin_guard() -> TestResult {
    assert_eq!(BITBUCKET_CONCURRENCY, 4);
    let bb_auth = BitbucketAuth::Basic {
        email: "user@example.com".to_owned(),
        api_token: "app_pw_123".to_owned(),
    };
    let bb_req = build_bitbucket_request(
        &bb_auth,
        Method::GET,
        "/repositories/a/b/pullrequests",
        None,
    )?;
    assert!(
        bb_req
            .headers
            .iter()
            .any(|(k, v)| k == "authorization" && v.starts_with("Basic "))
    );
    assert!(matches!(
        build_bitbucket_request(
            &bb_auth,
            Method::GET,
            "https://evil.example.com/2.0/pr",
            None
        ),
        Err(ForgeError::UntrustedUrl(_))
    ));
    let bb_pr = normalize_bitbucket_pr_json(&json!({
        "id": 12,
        "title": "Refactor auth",
        "state": "OPEN",
        "source": { "branch": { "name": "auth" }, "commit": { "hash": "abc" } },
        "destination": { "branch": { "name": "main" } }
    }))?;
    assert_eq!(bb_pr.number, 12);
    Ok(())
}
