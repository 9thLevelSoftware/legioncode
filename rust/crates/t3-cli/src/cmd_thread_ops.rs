//! `t3 thread send|wait` handlers and shared JSON output helpers.

use std::io::{self, Read as _};
use std::path::Path;

use serde_json::json;
use t3_persistence::auth_mutations::{
    IssueSessionInput, IssuedSession, issue_scoped_session, revoke_session,
};
use t3_persistence::sqlite::{SqliteOpenOptions, SqliteStore};
use t3_remote::thread_client::{
    THREAD_WAIT_SLICE_MS, ThreadClientError, ThreadMcpClient, ThreadReadRequest, ThreadSendMode,
    ThreadSendRequest, ThreadWaitOutcome, ThreadWaitRequest, format_thread_send_ack,
};

use crate::cmd_pair::exchange_local_bearer;
use crate::fallback::write_stdout_line;

pub(super) async fn acquire_local_thread_auth(
    db_path: &Path,
    origin: &str,
) -> Result<(String, Option<String>), String> {
    match exchange_local_bearer(db_path, origin, "t3 thread cli").await {
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
        .with_label("t3 thread cli")
        .with_ttl_ms(300_000);
    let issued = issue_scoped_session(store.connection(), input).map_err(|err| err.to_string())?;
    let _ = store.close();
    Ok(issued)
}

pub(super) fn revoke_fallback_thread_session(
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
    wait: bool,
    json: bool,
) -> Result<i32, ThreadClientError> {
    let message =
        read_message_text(raw_message).map_err(|detail| ThreadClientError::ToolFailed {
            tool: "t3_thread_send".to_owned(),
            detail,
        })?;
    let sent = client
        .send_message(&ThreadSendRequest {
            thread_id: thread_id.to_owned(),
            message,
            mode: Some(mode),
        })
        .await?;
    if !wait {
        return Ok(if json {
            print_json_value(&sent)
        } else {
            write_stdout_line(format_thread_send_ack(sent.delivery), "thread send")
        });
    }

    finish_wait(client, thread_id, &sent.run_id, None, json).await
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
    finish_wait(client, thread_id, &target_run, timeout_ms, json).await
}

async fn finish_wait(
    client: &ThreadMcpClient,
    thread_id: &str,
    run_id: &str,
    timeout_ms: Option<u64>,
    json: bool,
) -> Result<i32, ThreadClientError> {
    if let Some(bounded_ms) = timeout_ms {
        let waited = client
            .wait_thread(&ThreadWaitRequest {
                thread_id: thread_id.to_owned(),
                run_id: Some(run_id.to_owned()),
                timeout_ms: Some(bounded_ms),
                until_delegated_work_settled: None,
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
        .finish_run_error_check(thread_id, run_id, THREAD_WAIT_SLICE_MS, None)
        .await;
    match outcome {
        Ok(ThreadWaitOutcome::Ended { status, .. }) => Ok(if json {
            print_json_value(&json!({ "threadId": thread_id, "runId": run_id, "status": status }))
        } else {
            write_stdout_line(&status, "thread wait")
        }),
        Ok(ThreadWaitOutcome::NeedsInput) => Err(ThreadClientError::NeedsInput {
            thread_id: thread_id.to_owned(),
        }),
        Err(err) => Err(err),
    }
}

fn read_message_text(arg: &str) -> Result<String, String> {
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

pub(super) fn print_json_value<T: serde::Serialize>(val: &T) -> i32 {
    match serde_json::to_string(val) {
        Ok(s) => write_stdout_line(&s, "thread json"),
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}
