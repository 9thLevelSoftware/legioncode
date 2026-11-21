//! Live HTTP integration tests (`LiveForgeClient` vs local `tokio::net::TcpListener` server).

use std::fmt::Write as _;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use http::Method;
use serde_json::{Value, json};
use t3_contracts::pull_requests::{PullRequestMergeability, PullRequestState};
use t3_scm::core::CoreError;
use t3_scm::core::rate_limit::{RateLimitDecision, SourceControlRateLimit};
use t3_scm::forges::github::GitHubEndpoint;
use t3_scm::forges::{ForgeError, HttpRequestSpec};
use t3_scm::live_client::{
    CreatePullRequestParams, ForgeClientTarget, LiveForgeClient, MergeMethod,
    MergePullRequestParams,
};
use t3_scm::live_http::{LiveClientError, current_epoch_secs};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct RecordedHttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

struct CannedHttpResponse {
    status_line: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl CannedHttpResponse {
    fn json(status_line: &str, extra_headers: Vec<(&str, &str)>, body: &Value) -> Self {
        let mut headers: Vec<(String, String)> = extra_headers
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
        Self {
            status_line: status_line.to_owned(),
            headers,
            body: body.to_string(),
        }
    }
}

async fn spawn_mock_http_server<F>(
    handler: F,
) -> Result<(String, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>>
where
    F: Fn(RecordedHttpRequest) -> CannedHttpResponse + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let handler = Arc::new(handler);
    let handle = tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                break;
            };
            let handler = Arc::clone(&handler);
            tokio::spawn(async move {
                serve_connection(socket, handler).await;
            });
        }
    });
    Ok((format!("127.0.0.1:{}", addr.port()), handle))
}

async fn serve_connection<F>(socket: tokio::net::TcpStream, handler: Arc<F>)
where
    F: Fn(RecordedHttpRequest) -> CannedHttpResponse + Send + Sync,
{
    let mut reader = BufReader::new(socket);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).await.unwrap_or(0) == 0 {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let path = parts.next().unwrap_or("").to_owned();
    let mut headers = Vec::new();
    let mut content_len = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            let key_lower = k.trim().to_ascii_lowercase();
            let val_trimmed = v.trim().to_owned();
            if key_lower == "content-length" {
                content_len = val_trimmed.parse::<usize>().unwrap_or(0);
            }
            headers.push((key_lower, val_trimmed));
        }
    }
    let mut body_buf = vec![0u8; content_len];
    if content_len > 0 && reader.read_exact(&mut body_buf).await.is_err() {
        return;
    }
    let resp = handler(RecordedHttpRequest {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body_buf).into_owned(),
    });
    let mut wire = format!(
        "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        resp.status_line,
        resp.body.len()
    );
    for (k, v) in resp.headers {
        let _ = write!(wire, "{k}: {v}\r\n");
    }
    wire.push_str("\r\n");
    wire.push_str(&resp.body);
    let mut stream = reader.into_inner();
    let _ = stream.write_all(wire.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[tokio::test]
async fn live_client_github_fetch_list_create_and_merge() -> TestResult {
    let (host_port, server) =
        spawn_mock_http_server(|req| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/repos/t3dotgg/t3code/pulls/42") => CannedHttpResponse::json(
                "200 OK",
                vec![
                    ("x-ratelimit-limit", "5000"),
                    ("x-ratelimit-remaining", "4999"),
                ],
                &json!({
                    "number": 42, "title": "Live Rust SCM", "state": "open", "draft": false,
                    "html_url": "https://github.com/t3dotgg/t3code/pull/42",
                    "head": { "ref": "feat/live", "sha": "abc123" },
                    "base": { "ref": "main" }, "mergeable": true
                }),
            ),
            ("GET", "/repos/t3dotgg/t3code/pulls?state=open") => CannedHttpResponse::json(
                "200 OK",
                vec![],
                &json!([{"number": 42, "title": "Live Rust SCM", "state": "open",
                    "head": { "ref": "feat/live" }, "base": { "ref": "main" }}]),
            ),
            ("POST", "/repos/t3dotgg/t3code/pulls") => {
                let parsed: Value = serde_json::from_str(&req.body).unwrap_or(Value::Null);
                assert_eq!(parsed["title"], "Open PR");
                CannedHttpResponse::json(
                    "201 Created",
                    vec![],
                    &json!({ "number": 43, "title": "Open PR", "state": "open", "draft": true,
                        "head": { "ref": "feat/new" }, "base": { "ref": "main" }}),
                )
            }
            ("PUT", "/repos/t3dotgg/t3code/pulls/42/merge") => {
                let parsed: Value = serde_json::from_str(&req.body).unwrap_or(Value::Null);
                let msg = parsed["commit_message"].as_str().unwrap_or("");
                assert!(!msg.contains("Claude"));
                assert!(msg.contains("Human line"));
                CannedHttpResponse::json(
                    "200 OK",
                    vec![],
                    &json!({ "merged": true, "sha": "deadbeef", "message": "Merged" }),
                )
            }
            _ => CannedHttpResponse::json("404 Not Found", vec![], &json!({ "message": "404" })),
        })
        .await?;

    let client = LiveForgeClient::new()?;
    let target = ForgeClientTarget::GitHub {
        endpoint: GitHubEndpoint {
            hostname: "127.0.0.1".to_owned(),
            api_base_url: format!("http://{host_port}"),
            graphql_url: format!("http://{host_port}/graphql"),
        },
        token: "ghp_mock".to_owned(),
    };

    let pr = client
        .fetch_pull_request(&target, "t3dotgg/t3code", 42)
        .await?;
    assert_eq!(pr.number, 42);
    assert_eq!(pr.mergeability, PullRequestMergeability::Mergeable);
    assert_eq!(
        client
            .list_pull_requests(&target, "t3dotgg/t3code", Some("open"))
            .await?
            .len(),
        1
    );

    let created = client
        .create_pull_request(
            &target,
            &CreatePullRequestParams {
                repo: "t3dotgg/t3code".to_owned(),
                title: "Open PR".to_owned(),
                body: "Body".to_owned(),
                head: "feat/new".to_owned(),
                base: "main".to_owned(),
                draft: true,
            },
        )
        .await?;
    assert_eq!(created.number, 43);
    assert_eq!(created.state, PullRequestState::Open);

    let merged = client
        .merge_pull_request(
            &target,
            &MergePullRequestParams {
                repo: "t3dotgg/t3code".to_owned(),
                number: 42,
                commit_title: Some("Ship it".to_owned()),
                commit_message: Some(
                    "Human line\n\nCo-authored-by: Claude <noreply@anthropic.com>".to_owned(),
                ),
                method: MergeMethod::Squash,
                strip_agent_credits: true,
            },
        )
        .await?;
    assert!(merged.merged);
    assert_eq!(merged.sha.as_deref(), Some("deadbeef"));

    server.abort();
    Ok(())
}

#[tokio::test]
async fn live_client_linear_raw_auth_429_rate_limit_and_redirects() -> TestResult {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_srv = Arc::clone(&calls);

    let (host_port, server) = spawn_mock_http_server(move |req| match req.path.as_str() {
        "/graphql" => {
            let auth = req
                .headers
                .iter()
                .find(|(k, _)| k == "authorization")
                .map_or("", |(_, v)| v.as_str());
            assert_eq!(auth, "lin_api_raw_secret");
            assert!(!auth.starts_with("Bearer "));
            CannedHttpResponse::json(
                "200 OK",
                vec![],
                &json!({"data":{"issue":{"id":"u1","identifier":"ENG-99","title":"Wire live Rust SCM","url":"https://linear.app/t3/issue/ENG-99","state":{"name":"In Progress"}}}}),
            )
        }
        "/rate-limited" => {
            calls_srv.fetch_add(1, Ordering::SeqCst);
            CannedHttpResponse::json(
                "429 Too Many Requests",
                vec![("retry-after", "45"), ("ratelimit-limit", "100"), ("ratelimit-remaining", "0")],
                &json!({ "message": "slow down" }),
            )
        }
        "/redirect-same" => CannedHttpResponse {
            status_line: "302 Found".to_owned(),
            headers: vec![("Location".to_owned(), "/redirect-final".to_owned())],
            body: String::new(),
        },
        "/redirect-final" => CannedHttpResponse::json("200 OK", vec![], &json!({ "ok": true })),
        "/redirect-cross" => CannedHttpResponse {
            status_line: "302 Found".to_owned(),
            headers: vec![("Location".to_owned(), "https://evil.example.com/steal".to_owned())],
            body: String::new(),
        },
        _ => CannedHttpResponse::json("404 Not Found", vec![], &json!({})),
    })
    .await?;

    let client = LiveForgeClient::with_rate_limit(SourceControlRateLimit::default())?
        .with_linear_endpoint(format!("http://{host_port}/graphql"));

    let issue = client
        .fetch_linear_issue("lin_api_raw_secret", "ENG-99")
        .await?;
    assert_eq!(issue.identifier, "ENG-99");
    assert_eq!(issue.state_name.as_deref(), Some("In Progress"));

    let cross = client
        .execute_http(&HttpRequestSpec {
            method: Method::GET,
            url: format!("http://{host_port}/redirect-cross"),
            headers: vec![],
            body: None,
        })
        .await;
    assert!(matches!(
        cross,
        Err(LiveClientError::Forge(ForgeError::UntrustedUrl(_)))
    ));

    let same = client
        .execute_http(&HttpRequestSpec {
            method: Method::GET,
            url: format!("http://{host_port}/redirect-same"),
            headers: vec![],
            body: None,
        })
        .await?;
    assert_eq!(same.json()?["ok"], json!(true));

    let rl_spec = HttpRequestSpec {
        method: Method::GET,
        url: format!("http://{host_port}/rate-limited"),
        headers: vec![],
        body: None,
    };
    let first = client.execute_http(&rl_spec).await?;
    assert_eq!(first.status.as_u16(), 429);
    assert!(matches!(
        client
            .rate_limit()
            .check_admission(&host_port, false, current_epoch_secs() + 5)?,
        RateLimitDecision::BlockedUntil { .. }
    ));
    let second = client.execute_http(&rl_spec).await;
    assert!(matches!(
        second,
        Err(LiveClientError::Core(CoreError::RateLimited { .. }))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    server.abort();
    Ok(())
}
