//! Stage 8 tests for non-ACP subprocesses (`CodexAppServer`, `ClaudeStreamJson`) and HTTP/SSE (`OpenCodeHttp`, `GrokHttp`).

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use legion_contracts::orchestration::RunStatus;
use legion_providers::drivers::ProviderLaunchSpec;
use legion_server_runtime::ProviderDriverMode;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::Notify;

use super::{
    TestResult, compile_mock_rust_bin, dispatch_thread_turn, has_assistant_message_containing,
    make_orchestrator, worker_opts,
};

const MOCK_NON_ACP_SRC: &str = r##"
use std::env;
use std::io::{self, BufRead, Write};

fn emit(line: &str) {
    let mut out = io::stdout().lock();
    let _ = out.write_all(line.as_bytes());
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

fn json_id(line: &str) -> String {
    if let Some(pos) = line.find("\"id\":") {
        let rest = line[pos + 5..].trim_start();
        let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        if end > 0 {
            return rest[..end].to_owned();
        }
    }
    "1".to_owned()
}

fn run_codex() {
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines().map_while(Result::ok);
    while let Some(line) = lines.next() {
        let id = json_id(&line);
        if line.contains("\"method\":\"initialize\"") {
            emit(&format!(r#"{{"id":{id},"result":{{"userAgent":"mock-codex"}}}}"#));
        } else if line.contains("\"method\":\"thread/start\"") {
            emit(&format!(r#"{{"id":{id},"result":{{"thread":{{"id":"codex-thr-1"}},"model":"gpt-5"}}}}"#));
        } else if line.contains("\"method\":\"turn/start\"") {
            emit(&format!(r#"{{"id":{id},"result":{{"turn":{{"id":"turn-1","status":"inProgress"}}}}}}"#));
            emit(r#"{"method":"item/reasoning/textDelta","params":{"delta":"Thinking via Codex "}}"#);
            emit(r#"{"method":"item/agentMessage/delta","params":{"delta":"Codex step 1 ","itemId":"i1","turnId":"turn-1"}}"#);
            emit(r#"{"id":901,"method":"item/commandExecution/requestApproval","params":{"itemId":"cmd-1","command":"echo hi"}}"#);
            let approval = lines.next().unwrap_or_default();
            let accepted = approval.contains("\"approved\"");
            let tail = if accepted { "approved!" } else { "declined!" };
            emit(&format!(r#"{{"method":"item/agentMessage/delta","params":{{"delta":"{tail}","itemId":"i1","turnId":"turn-1"}}}}"#));
            emit(r#"{"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"last":{"inputTokens":9,"outputTokens":4}}}}"#);
            emit(r#"{"method":"turn/completed","params":{"turn":{"id":"turn-1","status":"completed"}}}"#);
            break;
        }
    }
}

fn run_claude() {
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines().map_while(Result::ok);
    if let Some(_prompt) = lines.next() {
        emit(r#"{"type":"system","subtype":"init","session_id":"claude-sess-1","model":"claude-sonnet-4"}"#);
        emit(r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"Claude think "},{"type":"tool_use","id":"tu-1","name":"Bash","input":{"cmd":"pwd"}},{"type":"text","text":"Claude streamed ok."}]}}"#);
        emit(r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu-1","content":"ok"}]}}"#);
        emit(r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"Bash"}}"#);
        let reply = lines.next().unwrap_or_default();
        let is_err = !reply.contains("\"allow\"");
        emit(&format!(r#"{{"type":"result","subtype":"success","is_error":{is_err},"result":"done","usage":{{"input_tokens":12,"output_tokens":7}}}}"#));
    }
}

fn main() {
    if env::args().any(|a| a == "--claude") {
        run_claude();
    } else {
        run_codex();
    }
}
"##;

pub async fn run_subprocess_tests() -> TestResult {
    let fx = make_orchestrator()?;
    let bin = compile_mock_rust_bin(fx.temp_dir.path(), "mock_non_acp", MOCK_NON_ACP_SRC)?;
    let exe = bin.to_string_lossy().into_owned();
    let ws_str = fx.workspace_dir.to_string_lossy().into_owned();

    fx.executor.register_driver(
        "codex",
        ProviderDriverMode::CodexAppServer(ProviderLaunchSpec {
            executable: exe.clone(),
            args: vec!["--codex".to_owned()],
            cwd: Some(ws_str.clone()),
            env: BTreeMap::new(),
            removed_env_keys: Vec::new(),
            cancel_behavior: "interrupt",
        }),
    );
    fx.executor.register_driver(
        "claude",
        ProviderDriverMode::ClaudeStreamJson(ProviderLaunchSpec {
            executable: exe,
            args: vec!["--claude".to_owned()],
            cwd: Some(ws_str),
            env: BTreeMap::new(),
            removed_env_keys: Vec::new(),
            cancel_behavior: "interrupt",
        }),
    );

    let codex_thr = dispatch_thread_turn(
        &fx.orchestrator,
        "codex",
        "codex",
        "gpt-5",
        "Run codex check",
    )
    .await?;
    let claude_thr = dispatch_thread_turn(
        &fx.orchestrator,
        "claude",
        "claude",
        "claude-sonnet-4",
        "Run claude check",
    )
    .await?;

    let worker = fx
        .orchestrator
        .manual_outbox_worker(fx.executor, worker_opts());
    let stats = worker.drain().await?;
    assert!(stats.completed >= 4);

    let codex_state = fx.orchestrator.load_thread_state(&codex_thr)?;
    assert_eq!(codex_state.runs[0].status, RunStatus::Completed);
    assert!(has_assistant_message_containing(
        &fx.db,
        &codex_thr,
        "Codex step 1 approved!"
    )?);

    let claude_state = fx.orchestrator.load_thread_state(&claude_thr)?;
    assert_eq!(claude_state.runs[0].status, RunStatus::Completed);
    assert!(has_assistant_message_containing(
        &fx.db,
        &claude_thr,
        "Claude streamed ok."
    )?);
    Ok(())
}

#[derive(Clone)]
struct MockHttpSseState {
    prompt_notify: Arc<Notify>,
}

async fn spawn_mock_http_sse_server()
-> Result<(String, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
    let state = Arc::new(MockHttpSseState {
        prompt_notify: Arc::new(Notify::new()),
    });
    let app = Router::new()
        .route(
            "/session",
            post(|| async { Json(json!({ "id": "oc-sess-1" })) }),
        )
        .route(
            "/session/{id}/message",
            post(|State(st): State<Arc<MockHttpSseState>>| async move {
                st.prompt_notify.notify_one();
                Json(json!({ "ok": true }))
            }),
        )
        .route(
            "/event",
            get(|State(st): State<Arc<MockHttpSseState>>| async move {
                let stream = futures_util::stream::once(async move {
                    st.prompt_notify.notified().await;
                    let body = concat!(
                        "data: {\"type\":\"session.text.delta\",\"data\":{\"delta\":\"OpenCode SSE ok.\"}}\n\n",
                        "data: {\"type\":\"session.step.ended\",\"data\":{\"tokens\":{\"input\":5,\"output\":3}}}\n\n",
                        "data: {\"type\":\"session.execution.succeeded\",\"data\":{}}\n\n"
                    );
                    Ok::<_, std::convert::Infallible>(axum::body::Bytes::from_static(body.as_bytes()))
                });
                ([(CONTENT_TYPE, "text/event-stream")], axum::body::Body::from_stream(stream))
                    .into_response()
            }),
        )
        .route(
            "/v1/chat/completions",
            post(|Json(body): Json<Value>| async move {
                let is_stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
                if !is_stream {
                    return Json(json!({
                        "choices": [{ "message": { "content": "Grok Generated Title" } }]
                    }))
                    .into_response();
                }
                let sse = concat!(
                    "data: {\"id\":\"g1\",\"model\":\"grok-4\",\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"Grok SSE reply.\"}}]}\n\n",
                    "data: {\"id\":\"g1\",\"model\":\"grok-4\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":6,\"completion_tokens\":4}}\n\n",
                    "data: [DONE]\n\n"
                );
                ([(CONTENT_TYPE, "text/event-stream")], sse).into_response()
            }),
        )
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((format!("http://{addr}"), handle))
}

pub async fn run_http_sse_and_title_tests() -> TestResult {
    let (base_url, srv) = spawn_mock_http_sse_server().await?;
    let fx = make_orchestrator()?;

    fx.executor.register_driver(
        "opencode",
        ProviderDriverMode::OpenCodeHttp {
            base_url: base_url.clone(),
            auth_header: None,
            model: Some("opencode-1".to_owned()),
        },
    );
    fx.executor.register_driver(
        "grok",
        ProviderDriverMode::GrokHttp {
            endpoint_url: base_url.clone(),
            api_key: "xai-test-key".to_owned(),
            model: Some("grok-4".to_owned()),
        },
    );
    fx.executor
        .set_title_provider(ProviderDriverMode::GrokHttp {
            endpoint_url: base_url,
            api_key: "xai-test-key".to_owned(),
            model: Some("grok-4".to_owned()),
        });

    let oc_thr = dispatch_thread_turn(
        &fx.orchestrator,
        "opencode",
        "opencode",
        "opencode-1",
        "Hi OpenCode",
    )
    .await?;
    let grok_thr = dispatch_thread_turn(
        &fx.orchestrator,
        "grok",
        "grok",
        "grok-4",
        "Summarize API architecture",
    )
    .await?;

    let worker = fx
        .orchestrator
        .manual_outbox_worker(fx.executor, worker_opts());
    let _ = worker.drain().await?;
    srv.abort();

    let oc_state = fx.orchestrator.load_thread_state(&oc_thr)?;
    assert_eq!(oc_state.runs[0].status, RunStatus::Completed);
    assert!(has_assistant_message_containing(
        &fx.db,
        &oc_thr,
        "OpenCode SSE ok."
    )?);

    let grok_state = fx.orchestrator.load_thread_state(&grok_thr)?;
    assert_eq!(grok_state.runs[0].status, RunStatus::Completed);
    assert!(has_assistant_message_containing(
        &fx.db,
        &grok_thr,
        "Grok SSE reply."
    )?);
    assert_eq!(
        grok_state.thread.ok_or("grok thread")?.title,
        "Grok Generated Title"
    );
    Ok(())
}
