//! Tests for GitHub GraphQL batching (`988d677eb4`, `e187544d38`), `git credential fill`
//! auth (`550cb3363c`), GHE/`*.ghe.com` host detection (`fd475bd722`), Azure DevOps token auth
//! (`da60db38e8`), and Linear OAuth additional scopes (`cfde513d87`).

use legion_scm::core::discovery::parse_git_remote_url;
use legion_scm::forges::ForgeError;
use legion_scm::forges::azure_devops::{
    AzureCliLaunch, build_azure_connection_data_cli_spec, build_azure_connection_data_http_request,
    build_azure_project_list_cli_spec, encode_azure_pat_basic_auth, verify_azure_connection_data,
};
use legion_scm::forges::github::{
    GITHUB_GRAPHQL_ALIAS_BATCH_SIZE, GitHubEndpoint, PrLookupItem, ReviewCommentBatchTarget,
    build_aliased_pr_summary_batch, build_gh_cli_env, build_git_credential_fill_spec,
    build_review_thread_comments_batch, build_review_threads_page_request,
    detect_github_endpoint_from_remote, is_github_host, parse_aliased_pr_summary_batch,
    parse_git_credential_fill_output, resolve_github_env_token,
};
use legion_scm::forges::linear::{
    DEFAULT_LINEAR_OAUTH_SCOPES, LINEAR_OAUTH_AUTHORIZE_URL, build_linear_oauth_authorize_url,
    merge_linear_oauth_scopes,
};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn github_ghe_detection_credential_fill_and_graphql_batching() -> TestResult {
    assert!(is_github_host("acme.ghe.com"));
    assert!(!is_github_host("github.com/evil"));

    let ghe_cloud = GitHubEndpoint::for_host("acme.ghe.com")?;
    assert_eq!(ghe_cloud.api_base_url, "https://api.acme.ghe.com");
    assert!(ghe_cloud.is_enterprise());

    let ghe_srv = detect_github_endpoint_from_remote("git@git.corp.example:team/app.git")?;
    assert_eq!(ghe_srv.api_base_url, "https://git.corp.example/api/v3");

    // GH_ENTERPRISE_TOKEN scoped strictly to matching GH_HOST.
    assert_eq!(
        resolve_github_env_token("git.corp.example", |k| match k {
            "GH_HOST" => Some("other.corp.example".to_owned()),
            "GH_ENTERPRISE_TOKEN" => Some("secret-ent".to_owned()),
            _ => None,
        }),
        None
    );
    assert_eq!(
        resolve_github_env_token("git.corp.example", |k| match k {
            "GH_HOST" => Some("git.corp.example".to_owned()),
            "GH_ENTERPRISE_TOKEN" => Some("secret-ent".to_owned()),
            _ => None,
        })
        .as_deref(),
        Some("secret-ent")
    );
    assert!(
        build_gh_cli_env("acme.ghe.com", Some("ghp_1"))
            .contains(&("GH_TOKEN".to_owned(), "ghp_1".to_owned()))
    );

    // git credential fill builder + parser + injection guard.
    assert!(matches!(
        build_git_credential_fill_spec("https", "github.com", Some("org\npassword=evil")),
        Err(ForgeError::InvalidCredentials(_))
    ));
    let fill_spec = build_git_credential_fill_spec("https", "github.com", Some("org/repo"))?;
    assert_eq!(fill_spec.args, ["credential", "fill"]);
    assert_eq!(
        fill_spec.stdin.as_deref(),
        Some("protocol=https\nhost=github.com\npath=org/repo\n\n")
    );
    let parsed_fill = parse_git_credential_fill_output(
        "protocol=https\nhost=github.com\nusername=x-access-token\npassword=gho_secret\n\n",
    )?;
    assert_eq!(parsed_fill.password, "gho_secret");
    assert!(matches!(
        parse_git_credential_fill_output("protocol=https\nhost=github.com\n\n"),
        Err(ForgeError::AuthRequired(_))
    ));

    // GraphQL batching splits > 25 PR lookups and > 25 thread comment cursors into chunks of 25.
    let items: Vec<PrLookupItem> = (1..=27)
        .map(|n| PrLookupItem {
            owner: "o".to_owned(),
            name: "r".to_owned(),
            number: n,
        })
        .collect();
    let batches = build_aliased_pr_summary_batch(&ghe_cloud, "tok", &items)?;
    assert_eq!(batches.len(), 2);
    assert_eq!(GITHUB_GRAPHQL_ALIAS_BATCH_SIZE, 25);

    let decoded = parse_aliased_pr_summary_batch(
        &json!({
            "data": {
                "pr0": {
                    "pullRequest": {
                        "number": 1,
                        "title": "Batched PR",
                        "url": "https://acme.ghe.com/o/r/pull/1",
                        "state": "OPEN",
                        "isDraft": false,
                        "mergedAt": null,
                        "headRefName": "feat",
                        "baseRefName": "main",
                        "headRefOid": "abc",
                        "mergeable": "MERGEABLE"
                    }
                },
                "pr1": { "pullRequest": null }
            }
        }),
        2,
    )?;
    assert_eq!(decoded[0].as_ref().map(|p| p.number), Some(1));
    assert!(decoded[1].is_none());

    let page_req = build_review_threads_page_request(&ghe_cloud, "tok", "o", "r", 1, None)?;
    assert!(
        page_req
            .body
            .as_deref()
            .is_some_and(|b| b.contains("reviewThreads(first: 100"))
    );
    let comment_batches = build_review_thread_comments_batch(
        &ghe_cloud,
        "tok",
        &[ReviewCommentBatchTarget {
            thread_node_id: "PRRT_1".to_owned(),
            after_cursor: "CUR_1".to_owned(),
        }],
    )?;
    assert_eq!(comment_batches.len(), 1);
    Ok(())
}

#[test]
fn azure_devops_auth_verification_and_linear_oauth_scopes() -> TestResult {
    assert!(encode_azure_pat_basic_auth("pat123")?.starts_with("Basic "));
    let http_probe = build_azure_connection_data_http_request("acme", "pat123")?;
    assert!(http_probe.url.contains("_apis/connectionData"));

    let remote = parse_git_remote_url("https://dev.azure.com/acme/web/_git/app")?;
    let launch = AzureCliLaunch {
        program: "az".to_owned(),
        prefix_args: vec![],
        env: vec![],
    };
    let cli_probe = build_azure_connection_data_cli_spec(&launch, &remote, Some("pat123"))?;
    assert!(cli_probe.args.contains(&"connectionData".to_owned()));
    assert!(
        cli_probe
            .env
            .contains(&("AZURE_DEVOPS_EXT_PAT".to_owned(), "pat123".to_owned()))
    );
    let proj_probe = build_azure_project_list_cli_spec(&launch, "acme", None)?;
    assert!(proj_probe.args.contains(&"--top".to_owned()));

    assert!(matches!(
        verify_azure_connection_data(&json!({
            "authenticatedUser": {
                "id": "0000",
                "subjectDescriptor": "System:PublicAccess",
                "providerDisplayName": "Anonymous"
            }
        })),
        Err(ForgeError::AuthRequired(_))
    ));
    let viewer = verify_azure_connection_data(&json!({
        "authenticatedUser": {
            "id": "user-uuid-1",
            "customDisplayName": "Theo"
        }
    }))?;
    assert_eq!(viewer.display_name.as_deref(), Some("Theo"));

    let merged =
        merge_linear_oauth_scopes(DEFAULT_LINEAR_OAUTH_SCOPES, &["write", "admin", "read"])?;
    assert_eq!(
        merged,
        vec!["read", "write", "issues:create", "comments:create", "admin"]
    );
    assert!(matches!(
        merge_linear_oauth_scopes(DEFAULT_LINEAR_OAUTH_SCOPES, &["bad scope"]),
        Err(ForgeError::InvalidCredentials(_))
    ));

    let auth_url = build_linear_oauth_authorize_url(
        "cid_1",
        "http://127.0.0.1:3773/oauth/linear",
        "st_1",
        &["admin"],
    )?;
    assert!(auth_url.starts_with(LINEAR_OAUTH_AUTHORIZE_URL));
    assert!(auth_url.contains("admin"));
    Ok(())
}
