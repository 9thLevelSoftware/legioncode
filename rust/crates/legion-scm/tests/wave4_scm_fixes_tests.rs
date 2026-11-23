//! Wave 4 upstream alignment tests covering multi-account `Linear`, `GitHub` link metadata &
//! oversized GraphQL skip, superseded CI checks, `Azure`, `Forgejo`, `GitLab` forks, and cross-refs.

use std::collections::HashSet;

use legion_contracts::pull_requests::PullRequestChecksState;
use legion_scm::core::discovery::parse_git_remote_url;
use legion_scm::core::pr_helpers::RawCheckState;
use legion_scm::forges::azure_devops::{
    azure_work_item_body_field, build_azure_pr_create_args, parse_azure_change_request_url,
};
use legion_scm::forges::forgejo::{
    FORGEJO_TEMPLATE_MAX_BYTES, bound_forgejo_pr_template, forgejo_template_paths,
    parse_forgejo_issue_url, parse_forgejo_ls_tree_templates,
};
use legion_scm::forges::github::{
    CHECKS_ALREADY_PASSED_WATCH_NEXT, GitHubEndpoint, GitHubWorkflowRunRef, RawGitHubCheckContext,
    RepositoryIssueBindingCache, build_github_graphql_request,
    build_github_issue_link_metadata_request, normalize_github_check_contexts,
    parse_github_cross_referenced_source, parse_github_issue_link_metadata,
    resolve_git_host_issue_bindings, resolve_watch_pull_request_next,
};
use legion_scm::forges::gitlab::{
    format_gitlab_draft_mr_title, gitlab_head_branch_probe_selectors,
    is_gitlab_missing_note_award_emoji_error, parse_gitlab_mention_reference,
    parse_gitlab_mr_with_fork_source, with_source_project_path,
};
use legion_scm::forges::issue_links::IssueReferenceKind;
use legion_scm::forges::linear::{
    BoundIssueProjectScope, LinearAccountBinding, are_all_host_accounts_unusable,
    issue_tracker_secret_name, linear_list_cursor_key, linear_viewer_key,
    matches_linear_workspace_url, resolve_bound_issue_scope, select_standalone_url_account_order,
};
use legion_scm::live_client::LiveForgeClient;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[test]
fn linear_multi_account_team_filing_and_bindings_work() {
    assert_eq!(
        issue_tracker_secret_name("linear", None),
        "issueTracker:linear"
    );
    assert_eq!(
        issue_tracker_secret_name("linear", Some("acc-2")),
        "issueTracker:linear:acc-2"
    );
    assert_eq!(
        linear_list_cursor_key("linear.app", "ENG", None),
        "linear.app eng"
    );
    assert_eq!(
        linear_list_cursor_key("linear.app", "ENG", Some("acc-2")),
        "linear.app acc-2/eng"
    );
    assert_eq!(
        linear_viewer_key("linear.app", Some("acc-2")),
        "linear.app acc-2"
    );

    let host_accs: HashSet<String> = ["linear.app", "linear.app acc-2"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let unusable_one: HashSet<String> = ["linear.app acc-2"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    assert!(!are_all_host_accounts_unusable(
        &host_accs,
        &unusable_one,
        false
    ));
    assert!(are_all_host_accounts_unusable(
        &host_accs, &host_accs, false
    ));

    let order = select_standalone_url_account_order(
        &[
            LinearAccountBinding {
                project_id: "p1".into(),
                tracker: "linear".into(),
                host: "linear.app".into(),
                scope: "ENG".into(),
                account_id: None,
            },
            LinearAccountBinding {
                project_id: "p2".into(),
                tracker: "linear".into(),
                host: "linear.app".into(),
                scope: "ENG".into(),
                account_id: Some("acc-2".into()),
            },
        ],
        Some("p2"),
        "eng",
    );
    assert_eq!(order, vec![Some("acc-2".to_owned()), None]);
    assert!(matches_linear_workspace_url(
        "https://linear.app/acme/issue/ENG-42/fix",
        "ACME"
    ));

    let bound = [
        BoundIssueProjectScope {
            project_id: "p1".into(),
            tracker: "linear".into(),
            host: "linear.app".into(),
            scope: "ENG".into(),
        },
        BoundIssueProjectScope {
            project_id: "p1".into(),
            tracker: "linear".into(),
            host: "linear.app".into(),
            scope: "OPS".into(),
        },
    ];
    assert_eq!(
        resolve_bound_issue_scope(&bound, Some("ops"), None)
            .expect("explicit scope")
            .scope,
        "OPS"
    );
    let err = resolve_bound_issue_scope(&bound, Some("DESIGN"), None)
        .expect_err("unknown scope rejected");
    assert!(err.to_string().contains("ENG, OPS"));

    let mut cache = RepositoryIssueBindingCache::new();
    let first = cache.get_or_compute("id-1", "github.com", "fork/repo", Some("up/repo"));
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].scope, "up/repo");
    assert_eq!(first[0].scope_alias.as_deref(), Some("fork/repo"));
    assert_eq!(
        resolve_git_host_issue_bindings("github.com", "same/repo", Some("same/repo")).len(),
        1
    );
}

fn wf_check(
    name: &str,
    run_number: u64,
    conclusion: &str,
    required: bool,
) -> RawGitHubCheckContext {
    RawGitHubCheckContext {
        name: Some(name.into()),
        context: None,
        status: Some("COMPLETED".into()),
        conclusion: Some(conclusion.into()),
        is_required: required,
        updated_at: None,
        details_url: None,
        workflow_run: Some(GitHubWorkflowRunRef {
            event: "pull_request".into(),
            run_number,
            workflow_id: "WF_1".into(),
        }),
    }
}

#[test]
fn github_metadata_checks_cross_refs_azure_forgejo_and_gitlab_work() {
    let ep = GitHubEndpoint::for_host("github.com").expect("github endpoint");
    let link_req =
        build_github_issue_link_metadata_request(&ep, "tok", "acme", "app", 99).expect("req");
    assert!(
        link_req
            .body
            .as_deref()
            .is_some_and(|b| b.contains("IssueLinkMetadata"))
    );
    let meta = parse_github_issue_link_metadata(&json!({
        "data": {"repository": {"issue": {"number": 99, "title": "Fix", "url": "https://github.com/acme/app/issues/99"}}}
    }))
    .expect("parse meta")
    .expect("some meta");
    assert_eq!(meta.display_key, "#99");

    let (detail, reference) = parse_github_cross_referenced_source(&json!({
        "__typename": "PullRequest",
        "number": 12,
        "url": "https://github.com/acme/app/pull/12",
        "repository": {"nameWithOwner": "acme/app"}
    }))
    .expect("github cross ref");
    assert_eq!(detail, "acme/app#12");
    assert_eq!(
        reference.expect("ref").kind,
        IssueReferenceKind::PullRequest
    );

    let gl_ref = parse_gitlab_mention_reference(
        "mentioned in merge request sub-proj!44",
        "https://git.corp.example/subpath/group/main-proj/-/issues/9",
        "group/main-proj",
    )
    .expect("gitlab mention ref");
    assert_eq!(gl_ref.repository, "group/sub-proj");
    assert_eq!(
        gl_ref.url,
        "https://git.corp.example/subpath/group/sub-proj/-/merge_requests/44"
    );
    let gl_same = parse_gitlab_mention_reference(
        "issue #19",
        "https://gitlab.com/group/main-proj/-/issues/9",
        "group/main-proj",
    )
    .expect("same-project gitlab issue ref");
    assert_eq!(gl_same.kind, IssueReferenceKind::Issue);
    assert_eq!(gl_same.repository, "group/main-proj");
    assert!(
        parse_gitlab_mention_reference(
            "commit abcdef12",
            "https://gitlab.com/g/p/-/issues/1",
            "g/p"
        )
        .is_none()
    );

    let checks = normalize_github_check_contexts(&[
        wf_check("matrix-old", 4, "CANCELLED", false),
        wf_check("req-check", 4, "CANCELLED", true),
        wf_check("matrix-new", 5, "SUCCESS", false),
    ]);
    assert_eq!(checks.len(), 2);
    assert_eq!(checks[0].name, "req-check");
    assert_eq!(checks[0].state, RawCheckState::Cancelled);
    assert_eq!(
        resolve_watch_pull_request_next(None, PullRequestChecksState::Passing).as_deref(),
        Some(CHECKS_ALREADY_PASSED_WATCH_NEXT)
    );

    let az =
        parse_azure_change_request_url("https://dev.azure.com/myorg/_git/myrepo/pullrequest/77")
            .expect("project-less azure URL");
    assert_eq!(az.organization, "myorg");
    assert_eq!(az.project, None);
    assert_eq!(az.pull_request_id, 77);
    assert!(
        parse_azure_change_request_url("https://myorg.visualstudio.com/_git/r/pullrequest/5")
            .is_some()
    );
    assert_eq!(
        azure_work_item_body_field(Some("Bug"), Some(" "), Some("1. Open")),
        "Microsoft.VSTS.TCM.ReproSteps"
    );
    let az_remote = parse_git_remote_url("https://dev.azure.com/o/p/_git/r").expect("remote");
    let (az_args, _) =
        build_azure_pr_create_args(&az_remote, "feat", "main", "Title", "body.md", true);
    assert!(az_args.windows(2).any(|w| w == ["--draft", "true"]));
}

#[test]
fn forgejo_issue_links_templates_and_gitlab_fork_mrs_work() {
    assert!(parse_forgejo_issue_url("https://git.corp.example/o/r/issues/14", false).is_none());
    let fj = parse_forgejo_issue_url("https://git.corp.example/o/r/issues/14", true)
        .expect("self-hosted forgejo issue");
    assert_eq!(fj.scope, "o/r");
    assert_eq!(fj.number, 14);
    assert!(parse_forgejo_issue_url("https://codeberg.org/o/r/issues/2", false).is_some());

    let tpl_paths = forgejo_template_paths();
    assert_eq!(tpl_paths[0], "PULL_REQUEST_TEMPLATE.md");
    assert!(
        tpl_paths
            .iter()
            .any(|p| p == ".forgejo/pull_request_template.md")
    );
    let mut ls_tree = Vec::new();
    ls_tree.extend_from_slice(
        b"120000 blob aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\tPULL_REQUEST_TEMPLATE.md\0",
    );
    ls_tree.extend_from_slice(
        b"100644 blob bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\t.forgejo/pull_request_template.md\0",
    );
    let parsed_tree = parse_forgejo_ls_tree_templates(&ls_tree);
    assert!(!parsed_tree.contains_key("PULL_REQUEST_TEMPLATE.md"));
    assert!(parsed_tree.contains_key(".forgejo/pull_request_template.md"));
    let huge = "a".repeat(FORGEJO_TEMPLATE_MAX_BYTES + 20);
    let bounded = bound_forgejo_pr_template(huge.as_bytes()).expect("template");
    assert!(bounded.ends_with("\n\n[truncated]"));

    let fork_mr = parse_gitlab_mr_with_fork_source(&json!({
        "iid": 21,
        "title": "Fork fix",
        "web_url": "https://gitlab.com/up/app/-/merge_requests/21",
        "state": "opened",
        "source_branch": "fix",
        "target_branch": "main",
        "source_project_id": 9001,
        "target_project_id": 100
    }))
    .expect("fork mr");
    assert_eq!(fork_mr.is_cross_repository, Some(true));
    assert_eq!(fork_mr.unresolved_source_project_id, Some(9001));
    let resolved_mr = with_source_project_path(fork_mr, Some("contrib-user/app"));
    assert_eq!(resolved_mr.unresolved_source_project_id, None);
    assert_eq!(
        resolved_mr.head_repository_name_with_owner.as_deref(),
        Some("contrib-user/app")
    );
    assert_eq!(
        resolved_mr.head_repository_owner_login.as_deref(),
        Some("contrib-user")
    );
    assert_eq!(
        gitlab_head_branch_probe_selectors(&["contrib:feat", "feat"]),
        vec!["contrib:feat"]
    );
    assert!(is_gitlab_missing_note_award_emoji_error(
        "Field 'awardEmoji' doesn't exist on type 'Note'"
    ));
    assert_eq!(
        format_gitlab_draft_mr_title("Add feature"),
        "Draft: Add feature"
    );
    assert_eq!(
        format_gitlab_draft_mr_title("[Draft] Ready"),
        "[Draft] Ready"
    );
}

#[tokio::test]
async fn live_client_overlaps_linear_reads_and_skips_oversized_graphql_responses() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let big_body = json!({"data": {"big": "x".repeat(4096)}}).to_string();

    tokio::spawn(async move {
        for _ in 0..3 {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let payload_len = big_body.len();
            let big_clone = big_body.clone();
            tokio::spawn(async move {
                let mut req_buf = [0_u8; 4096];
                let n = stream.read(&mut req_buf).await.unwrap_or(0);
                let req_text = String::from_utf8_lossy(&req_buf[..n]);
                let resp_body = if req_text.contains("LinearViewer") {
                    r#"{"data":{"viewer":{"id":"usr-1","displayName":"Ada"}}}"#.to_owned()
                } else if req_text.contains("IssueById") {
                    r#"{"data":{"issue":{"id":"iss-1","identifier":"ENG-7","title":"Bug","url":"https://linear.app/acme/issue/ENG-7/bug","state":{"name":"Todo"}}}}"#.to_owned()
                } else {
                    let _ = payload_len;
                    big_clone
                };
                let http_resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp_body.len(),
                    resp_body
                );
                let _ = stream.write_all(http_resp.as_bytes()).await;
            });
        }
    });

    let client = LiveForgeClient::new()
        .expect("client")
        .with_linear_endpoint(format!("http://{addr}/linear"));
    let (issue, viewer_id) = client
        .fetch_linear_issue_with_viewer("lin_api_key", "ENG-7")
        .await
        .expect("linear issue + viewer");
    assert_eq!(issue.identifier, "ENG-7");
    assert_eq!(viewer_id.as_deref(), Some("usr-1"));

    let mut oversized_spec = build_github_graphql_request(
        &GitHubEndpoint::for_host("github.com").expect("ep"),
        "tok",
        "{ viewer { login } }",
        None,
    )
    .expect("graphql spec");
    oversized_spec.url = format!("http://{addr}/graphql");
    let err = client
        .execute_graphql_bounded(&oversized_spec, Some(512))
        .await
        .expect_err("oversized graphql rejected before decode");
    assert!(err.to_string().contains("512-byte decode limit"));
}
