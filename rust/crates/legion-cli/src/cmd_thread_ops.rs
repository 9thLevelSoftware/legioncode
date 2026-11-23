//! `legioncode thread send|wait` handlers and shared JSON output helpers.

use std::io::{self, Read as _};
use std::path::Path;

use legion_persistence::auth_mutations::{
    IssueSessionInput, IssuedSession, issue_scoped_session, revoke_session,
};
use legion_persistence::sqlite::{SqliteOpenOptions, SqliteStore};
use legion_remote::thread_client::{
    THREAD_WAIT_SLICE_MS, ThreadClientError, ThreadMcpClient, ThreadReadRequest, ThreadSendMode,
    ThreadSendRequest, ThreadWaitOutcome, ThreadWaitRequest, format_thread_send_ack,
};
use serde_json::json;

use crate::cmd_pair::exchange_local_bearer;
use crate::fallback::write_stdout_line;

pub(crate) async fn acquire_local_thread_auth(
    db_path: &Path,
    origin: &str,
) -> Result<(String, Option<String>), String> {
    match exchange_local_bearer(db_path, origin, "legioncode thread cli").await {
        Ok(bearer) => Ok((bearer, None)),
        Err(exchange_err) => match issue_fallback_thread_session(db_path) {
            Ok(issued) => Ok((issued.token, Some(issued.session_id))),
            Err(_) => Err(exchange_err),
        },
    }
}

fn issue_fallback_thread_session(db_path: &Path) -> Result<IssuedSession, String> {
    let store = SqliteStore::open_with_options(
        db_path,
        SqliteOpenOptions {
            ensure_core_schema: false,
        },
    )
    .map_err(|err| err.to_string())?;
    let input = IssueSessionInput::default()
        .with_label("legioncode thread cli")
        .with_ttl_ms(300_000);
    let issued = issue_scoped_session(store.connection(), input).map_err(|err| err.to_string())?;
    let _ = store.close();
    Ok(issued)
}

pub(crate) fn revoke_fallback_thread_session(
    local_db_session: Option<&(std::path::PathBuf, String)>,
) {
    let Some((db_path, session_id)) = local_db_session else {
        return;
    };
    if let Ok(store) = SqliteStore::open_with_options(
        db_path,
        SqliteOpenOptions {
            ensure_core_schema: false,
        },
    ) {
        let _ = revoke_session(store.connection(), session_id);
        let _ = store.close();
    }
}

pub(super) async fn run_send(
    client: &ThreadMcpClient,
    thread_id: &str,
    raw_message: &str,
    mode: ThreadSendMode,
    output_schema_path: Option<&str>,
    wait: bool,
    json: bool,
) -> Result<i32, ThreadClientError> {
    let message =
        read_message_text(raw_message).map_err(|detail| ThreadClientError::ToolFailed {
            tool: "legion_thread_send".to_owned(),
            detail,
        })?;
    let schema = super::start::read_output_schema_file(output_schema_path)?;
    let has_output = schema.is_some();
    let sent = client
        .send_message(&ThreadSendRequest {
            thread_id: thread_id.to_owned(),
            message,
            mode: Some(mode),
            output_schema: schema,
        })
        .await?;
    if !wait {
        return Ok(if json {
            print_json_value(&sent)
        } else {
            write_stdout_line(format_thread_send_ack(sent.delivery), "thread send")
        });
    }

    finish_wait(client, thread_id, &sent.run_id, None, json, has_output).await
}

pub(super) async fn run_wait(
    client: &ThreadMcpClient,
    thread_id: &str,
    run_id_arg: Option<&str>,
    timeout_ms: Option<u64>,
    json: bool,
) -> Result<i32, ThreadClientError> {
    let target_run = if let Some(r) = run_id_arg {
        r.to_owned()
    } else {
        let snap = client
            .read_thread(&ThreadReadRequest {
                thread_id: thread_id.to_owned(),
                limit: Some(1),
                run_limit: Some(1),
                ..ThreadReadRequest::default()
            })
            .await?;
        let Some(latest) = snap.recent_runs.first() else {
            return Ok(if json {
                print_json_value(
                    &json!({ "threadId": thread_id, "status": snap.thread.status, "timedOut": false }),
                )
            } else {
                write_stdout_line(&snap.thread.status, "thread wait")
            });
        };
        latest.run_id.clone()
    };
    finish_wait(client, thread_id, &target_run, timeout_ms, json, false).await
}

pub(super) async fn finish_wait(
    client: &ThreadMcpClient,
    thread_id: &str,
    run_id: &str,
    timeout_ms: Option<u64>,
    json: bool,
    until_output: bool,
) -> Result<i32, ThreadClientError> {
    if let Some(bounded_ms) = timeout_ms {
        let waited = client
            .wait_thread(&ThreadWaitRequest {
                thread_id: thread_id.to_owned(),
                run_id: Some(run_id.to_owned()),
                timeout_ms: Some(bounded_ms),
                until_delegated_work_settled: None,
                until_output: until_output.then_some(true),
            })
            .await?;
        if json {
            return Ok(print_json_value(&waited));
        }
        return Ok(if waited.timed_out {
            write_stdout_line("Timed out waiting for thread run.", "thread wait");
            1
        } else {
            write_stdout_line(&waited.status, "thread wait")
        });
    }

    let outcome = client
        .wait_for_run_settled(thread_id, run_id, THREAD_WAIT_SLICE_MS, until_output)
        .await?;
    match outcome {
        ThreadWaitOutcome::Ended {
            run_id: settled_run_id,
            status,
            output,
        } => {
            if status != "completed" {
                let error = client.run_result_text(thread_id, run_id, "error").await?;
                return Err(ThreadClientError::RunEnded {
                    thread_id: thread_id.to_owned(),
                    status,
                    error,
                });
            }
            let effective_run_id = settled_run_id.unwrap_or_else(|| run_id.to_owned());
            let reply = client
                .run_result_text(thread_id, &effective_run_id, "assistant_message")
                .await?;
            Ok(if json {
                let mut payload = json!({
                    "threadId": thread_id,
                    "runId": effective_run_id,
                    "status": status,
                    "reply": reply,
                });
                if let Some(val) = output
                    && let Some(obj) = payload.as_object_mut()
                {
                    obj.insert("output".to_owned(), val);
                }
                print_json_value(&payload)
            } else if let Some(text) = reply {
                write_stdout_line(&text, "thread wait")
            } else {
                write_stdout_line(&status, "thread wait")
            })
        }
        ThreadWaitOutcome::NeedsInput => Err(ThreadClientError::NeedsInput {
            thread_id: thread_id.to_owned(),
        }),
    }
}

pub(super) async fn run_read(
    mcp: &legion_remote::acp_mcp_bridge::McpHttpClient,
    client: &ThreadMcpClient,
    thread_id: &str,
    view: Option<String>,
    limit: Option<u32>,
    after_position: Option<i64>,
    json: bool,
) -> Result<i32, ThreadClientError> {
    let mut read_args = json!({ "threadId": thread_id });
    if let Some(obj) = read_args.as_object_mut() {
        if let Some(ref v) = view {
            obj.insert("view".to_owned(), serde_json::Value::String(v.clone()));
        }
        if let Some(l) = limit {
            obj.insert("limit".to_owned(), serde_json::Value::from(l));
        }
        if let Some(after) = after_position {
            obj.insert("afterPosition".to_owned(), serde_json::Value::from(after));
        }
    }
    let raw = mcp
        .call_tool("legion_thread_read", &read_args)
        .await
        .map_err(|err| ThreadClientError::ToolFailed {
            tool: "legion_thread_read".to_owned(),
            detail: err.to_string(),
        })?;
    let structured = raw
        .get("structuredContent")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let result: legion_remote::thread_client::ThreadReadResult =
        serde_json::from_value(structured.clone()).map_err(|err| {
            ThreadClientError::ToolFailed {
                tool: "legion_thread_read".to_owned(),
                detail: err.to_string(),
            }
        })?;
    let run_error = client.latest_run_error(&result.thread).await?;
    if json {
        let mut payload = structured;
        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                "error".to_owned(),
                run_error.map_or(serde_json::Value::Null, serde_json::Value::String),
            );
        }
        return Ok(print_json_value(&payload));
    }
    let header = format!(
        "{}\n{}  {}  {}/{}",
        result.thread.title,
        result.thread.thread_id,
        result.thread.status,
        result.thread.provider_instance_id,
        result.thread.model
    );
    if write_stdout_line(&header, "thread read") != 0 {
        return Ok(1);
    }
    if let Some(ref err) = run_error
        && write_stdout_line(&format!("Error: {err}"), "thread read") != 0
    {
        return Ok(1);
    }
    for item in &result.items {
        if let Some(ref text) = item.text
            && write_stdout_line(&format!("\n[{}]\n{text}", item.item_type), "thread read") != 0
        {
            return Ok(1);
        }
    }
    Ok(0)
}

pub(super) fn read_message_text(arg: &str) -> Result<String, String> {
    let text = if arg == "-" {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("Failed reading stdin: {e}"))?;
        buf
    } else {
        arg.to_owned()
    };
    if text.trim().is_empty() {
        return Err("The message is empty.".to_owned());
    }
    Ok(text)
}

pub(crate) fn print_json_value<T: serde::Serialize>(val: &T) -> i32 {
    match serde_json::to_string(val) {
        Ok(s) => write_stdout_line(&s, "thread json"),
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

/// Unwraps `structuredContent` from an `/mcp` `tools/call` payload, propagating `isError: true`.
pub(crate) fn extract_mcp_tool_structured(
    raw: serde_json::Value,
) -> Result<serde_json::Value, String> {
    if raw.get("isError").and_then(serde_json::Value::as_bool) == Some(true) {
        let text = raw
            .get("content")
            .and_then(serde_json::Value::as_array)
            .and_then(|arr| arr.first())
            .and_then(|item| item.get("text"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("MCP tool call failed.");
        let parsed_msg = serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|v| {
                v.get("message")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            });
        return Err(parsed_msg.unwrap_or_else(|| text.to_owned()));
    }
    Ok(raw.get("structuredContent").cloned().unwrap_or(raw))
}
