//! Live server runtime discovery and typed `/mcp` client for `t3 thread` operations.
//!
//! Mirrors `apps/server/src/cli/thread.ts`.

#[path = "thread_client_format.rs"]
mod format;
#[path = "thread_client_ops.rs"]
mod ops;
#[path = "thread_client_types.rs"]
mod types;

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde_json::Value;
use t3_persistence::ownership::{PersistedServerRuntimeState, read_live_server_runtime_state};

use crate::acp_mcp_bridge::McpHttpClient;

pub use format::{
    THREAD_READ_ITEM_MAX_CHARS, THREAD_READ_PAGE_LIMIT, THREAD_TITLE_MAX_CHARS,
    THREAD_WAIT_SLICE_MS, ThreadClientError, format_thread_list_line, format_thread_send_ack,
    format_title_from_message,
};
pub use types::{
    ThreadDetail, ThreadLaunchRequest, ThreadLaunchResult, ThreadListItem, ThreadListRequest,
    ThreadListResult, ThreadReadRequest, ThreadReadResult, ThreadRunSummary, ThreadSendDelivery,
    ThreadSendMode, ThreadSendRequest, ThreadSendResult, ThreadTimelineItem, ThreadWaitRequest,
    ThreadWaitResult, ThreadWaitResultRun,
};

/// Live T3 Code server discovered from `<base_dir>/{userdata,dev}/server-runtime.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredServerRuntime {
    /// State directory (`<base_dir>/userdata` or `<base_dir>/dev`).
    pub state_dir: PathBuf,
    /// Path to `server-runtime.json` that produced the live record.
    pub runtime_state_path: PathBuf,
    /// Live runtime descriptor read from disk (PID confirmed running).
    pub runtime: PersistedServerRuntimeState,
    /// Resolved `/mcp` HTTP endpoint URL (`<origin>/mcp`).
    pub mcp_endpoint: String,
}

/// Discovers a live server runtime under `<base_dir>/userdata` first, then `<base_dir>/dev`.
///
/// Mirrors `readLiveServerRuntimeState` + `ThreadServerNotRunningError` (`thread.ts:56-64,226-229`).
///
/// ```
/// use t3_remote::thread_client::{ThreadClientError, discover_live_server_runtime};
///
/// let dir = tempfile::tempdir()?;
/// let err = discover_live_server_runtime(dir.path()).unwrap_err();
/// assert_eq!(err.exit_code(), 1);
/// assert!(matches!(err, ThreadClientError::ServerNotRunning { .. }));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn discover_live_server_runtime(
    base_dir: &Path,
) -> Result<DiscoveredServerRuntime, ThreadClientError> {
    for sub in ["userdata", "dev"] {
        let state_dir = base_dir.join(sub);
        let runtime_path = state_dir.join("server-runtime.json");
        if let Some(runtime) = read_live_server_runtime_state(&runtime_path) {
            let trimmed_origin = runtime.origin.trim_end_matches('/');
            return Ok(DiscoveredServerRuntime {
                state_dir,
                runtime_state_path: runtime_path,
                mcp_endpoint: format!("{trimmed_origin}/mcp"),
                runtime,
            });
        }
    }
    Err(ThreadClientError::ServerNotRunning {
        base_dir: base_dir.display().to_string(),
    })
}

/// Result of awaiting a run (`wait_for_run`, `thread.ts:514-520`).
#[derive(Debug, Clone, PartialEq)]
pub enum ThreadWaitOutcome {
    /// Run settled at a terminal status.
    Ended {
        /// Settled or updated run ID.
        run_id: Option<String>,
        /// Terminal status string.
        status: String,
        /// Optional typed JSON output submitted against `outputSchema`.
        output: Option<Value>,
    },
    /// Run is blocked on user input (`ThreadNeedsInputError`, exit code `2`).
    NeedsInput,
}

/// Typed client calling `t3_thread_{list,read,send,wait}` over an initialized [`McpHttpClient`].
#[derive(Debug, Clone)]
pub struct ThreadMcpClient {
    origin: String,
    mcp: McpHttpClient,
}

impl ThreadMcpClient {
    /// Wraps an already-connected [`McpHttpClient`].
    #[must_use]
    pub fn from_http_client(origin: impl Into<String>, mcp: McpHttpClient) -> Self {
        Self {
            origin: origin.into(),
            mcp,
        }
    }

    /// Discovers a live server in `base_dir` and opens an MCP HTTP session (`t3-thread-cli`).
    pub async fn connect_base_dir(
        base_dir: &Path,
        bearer_token: &str,
    ) -> Result<Self, ThreadClientError> {
        let discovered = discover_live_server_runtime(base_dir)?;
        let auth = if bearer_token.starts_with("Bearer ") {
            bearer_token.to_owned()
        } else {
            format!("Bearer {bearer_token}")
        };
        let mcp = McpHttpClient::connect(&discovered.mcp_endpoint, auth, "t3-thread-cli")
            .await
            .map_err(|err| ThreadClientError::ServerUnreachable {
                origin: discovered.runtime.origin.clone(),
                detail: format!(" {err}"),
            })?;
        Ok(Self {
            origin: discovered.runtime.origin,
            mcp,
        })
    }

    /// Sends `DELETE` to terminate the underlying `/mcp` session (`BUG-06`).
    pub async fn close(self) {
        self.mcp.close().await;
    }

    /// Calls `t3_thread_list` for one page.
    pub async fn list_threads(
        &self,
        req: &ThreadListRequest,
    ) -> Result<ThreadListResult, ThreadClientError> {
        let args = serialize_args("t3_thread_list", req)?;
        self.call_typed_tool("t3_thread_list", &args).await
    }

    /// Calls `t3_thread_read` for one page of thread metadata and timeline items.
    pub async fn read_thread(
        &self,
        req: &ThreadReadRequest,
    ) -> Result<ThreadReadResult, ThreadClientError> {
        let args = serialize_args("t3_thread_read", req)?;
        self.call_typed_tool("t3_thread_read", &args).await
    }

    /// Calls `t3_thread_launch`.
    pub async fn launch_thread(
        &self,
        req: &ThreadLaunchRequest,
    ) -> Result<ThreadLaunchResult, ThreadClientError> {
        let args = serialize_args("t3_thread_launch", req)?;
        self.call_typed_tool("t3_thread_launch", &args).await
    }

    /// Calls `t3_thread_send`.
    pub async fn send_message(
        &self,
        req: &ThreadSendRequest,
    ) -> Result<ThreadSendResult, ThreadClientError> {
        let args = serialize_args("t3_thread_send", req)?;
        self.call_typed_tool("t3_thread_send", &args).await
    }

    /// Calls `t3_thread_wait` for one bounded slice.
    pub async fn wait_thread(
        &self,
        req: &ThreadWaitRequest,
    ) -> Result<ThreadWaitResult, ThreadClientError> {
        let args = serialize_args("t3_thread_wait", req)?;
        self.call_typed_tool("t3_thread_wait", &args).await
    }

    /// Polls `t3_thread_wait` in bounded `THREAD_WAIT_SLICE_MS` slices, returning [`ThreadWaitOutcome::NeedsInput`]
    /// when `pending_request_count > 0` (`thread.ts:571-609`).
    pub async fn wait_for_run(
        &self,
        thread_id: &str,
        run_id: &str,
        slice_ms: u64,
    ) -> Result<ThreadWaitOutcome, ThreadClientError> {
        self.wait_for_run_with_output(thread_id, run_id, slice_ms, false)
            .await
    }

    /// Polls `t3_thread_wait` in bounded slices, optionally waiting until `untilOutput` resolves.
    pub async fn wait_for_run_with_output(
        &self,
        thread_id: &str,
        run_id: &str,
        slice_ms: u64,
        until_output: bool,
    ) -> Result<ThreadWaitOutcome, ThreadClientError> {
        loop {
            let waited = self
                .wait_thread(&ThreadWaitRequest {
                    thread_id: thread_id.to_owned(),
                    run_id: Some(run_id.to_owned()),
                    timeout_ms: Some(slice_ms),
                    until_delegated_work_settled: None,
                    until_output: until_output.then_some(true),
                })
                .await?;
            if !waited.timed_out {
                return Ok(ThreadWaitOutcome::Ended {
                    run_id: waited.run_id,
                    status: waited.status,
                    output: waited.output,
                });
            }
            let snapshot = self
                .read_thread(&ThreadReadRequest {
                    thread_id: thread_id.to_owned(),
                    limit: Some(1),
                    run_limit: Some(1),
                    ..ThreadReadRequest::default()
                })
                .await?;
            if snapshot.thread.pending_request_count > 0 {
                let recheck = self
                    .wait_thread(&ThreadWaitRequest {
                        thread_id: thread_id.to_owned(),
                        run_id: Some(run_id.to_owned()),
                        timeout_ms: Some(1),
                        until_delegated_work_settled: None,
                        until_output: until_output.then_some(true),
                    })
                    .await?;
                return Ok(if recheck.timed_out {
                    ThreadWaitOutcome::NeedsInput
                } else {
                    ThreadWaitOutcome::Ended {
                        run_id: recheck.run_id,
                        status: recheck.status,
                        output: recheck.output,
                    }
                });
            }
        }
    }

    /// Invokes `wait_for_run` and converts `NeedsInput` / non-`completed` runs into typed [`ThreadClientError`] (`thread.ts:679-688`).
    pub async fn finish_run_error_check(
        &self,
        thread_id: &str,
        run_id: &str,
        slice_ms: u64,
        last_error: Option<String>,
    ) -> Result<ThreadWaitOutcome, ThreadClientError> {
        let outcome = self.wait_for_run(thread_id, run_id, slice_ms).await?;
        match &outcome {
            ThreadWaitOutcome::NeedsInput => Err(ThreadClientError::NeedsInput {
                thread_id: thread_id.to_owned(),
            }),
            ThreadWaitOutcome::Ended { status, .. } if status != "completed" => {
                Err(ThreadClientError::RunEnded {
                    thread_id: thread_id.to_owned(),
                    status: status.clone(),
                    error: last_error,
                })
            }
            ThreadWaitOutcome::Ended { .. } => Ok(outcome),
        }
    }

    async fn call_typed_tool<R: DeserializeOwned>(
        &self,
        tool: &str,
        args: &Value,
    ) -> Result<R, ThreadClientError> {
        let raw = self.mcp.call_tool(tool, args).await.map_err(|err| {
            ThreadClientError::ServerUnreachable {
                origin: self.origin.clone(),
                detail: format!(" {err}"),
            }
        })?;
        let Some(obj) = raw.as_object() else {
            return Err(unexpected_tool_result(tool));
        };
        if obj.get("isError").and_then(Value::as_bool) == Some(true) {
            return Err(ThreadClientError::ToolFailed {
                tool: tool.to_owned(),
                detail: extract_tool_failure_detail(obj),
            });
        }
        let Some(structured) = obj.get("structuredContent") else {
            return Err(unexpected_tool_result(tool));
        };
        serde_json::from_value(structured.clone()).map_err(|_| unexpected_tool_result(tool))
    }
}

fn serialize_args<T: serde::Serialize>(tool: &str, value: &T) -> Result<Value, ThreadClientError> {
    serde_json::to_value(value).map_err(|err| ThreadClientError::ToolFailed {
        tool: tool.to_owned(),
        detail: err.to_string(),
    })
}

fn unexpected_tool_result(tool: &str) -> ThreadClientError {
    ThreadClientError::ToolFailed {
        tool: tool.to_owned(),
        detail: format!(
            "The server sent an unexpected {tool} result. Update the t3 CLI to match the server."
        ),
    }
}

fn extract_tool_failure_detail(obj: &serde_json::Map<String, Value>) -> String {
    let raw_text = obj
        .get("content")
        .and_then(Value::as_array)
        .and_then(|parts| {
            parts
                .iter()
                .find_map(|p| p.get("text").and_then(Value::as_str))
        })
        .unwrap_or("");
    if let Ok(Value::Object(failure)) = serde_json::from_str::<Value>(raw_text)
        && let Some(msg) = failure.get("message").and_then(Value::as_str)
    {
        return msg.to_owned();
    }
    if raw_text.is_empty() {
        "The tool call failed.".to_owned()
    } else {
        raw_text.to_owned()
    }
}
