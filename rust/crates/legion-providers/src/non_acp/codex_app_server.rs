//! Codex app-server JSON-RPC 2.0 request/response builders and line parser.
//!
//! Mirrors `packages/effect-codex-app-server/src/protocol.ts` (omitting `"jsonrpc"`
//! on outbound Codex app-server lines while accepting either bare or `"2.0"`
//! inbound frames) and `CodexAdapterV2.ts` notifications.

use legion_contracts::orchestration::ProviderApprovalDecision;
use legion_protocol::provider_env::NdjsonLineCodec;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use super::codex_parser::parse_codex_app_server_line;
use super::{NonAcpParseError, NonAcpTurnUpdate};

/// Codex approval reply decision serialized in `execCommandApproval` / `applyPatchApproval` responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexApprovalDecision {
    /// Approve this command or patch once.
    Approved,
    /// Approve and remember for the session.
    ApprovedForSession,
    /// Reject the invocation without aborting the session.
    Denied,
    /// Cancel/abort execution.
    Abort,
}

/// Converts a canonical [`ProviderApprovalDecision`] into a [`CodexApprovalDecision`].
#[must_use]
pub const fn codex_decision_from_approval(
    decision: ProviderApprovalDecision,
) -> CodexApprovalDecision {
    match decision {
        ProviderApprovalDecision::Accept => CodexApprovalDecision::Approved,
        ProviderApprovalDecision::AcceptAlways | ProviderApprovalDecision::AcceptForSession => {
            CodexApprovalDecision::ApprovedForSession
        }
        ProviderApprovalDecision::Decline => CodexApprovalDecision::Denied,
        ProviderApprovalDecision::Cancel => CodexApprovalDecision::Abort,
    }
}

/// Parameters for Codex `thread/start` JSON-RPC calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexThreadStartParams {
    /// Working directory for the Codex thread.
    pub cwd: String,
    /// Optional model slug override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Approval policy (`"untrusted"`, `"on-failure"`, `"on-request"`, `"never"`).
    pub approval_policy: String,
    /// Sandbox mode (`"read-only"`, `"workspace-write"`, `"danger-full-access"`).
    pub sandbox: String,
    /// Whether raw experimental events are requested.
    #[serde(default)]
    pub experimental_raw_events: bool,
}

/// Parameters for Codex `turn/start` JSON-RPC calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexTurnStartParams {
    /// Target Codex thread ID.
    pub thread_id: String,
    /// Plain user prompt text wrapped into Codex `[{"type":"text","text":...}]`.
    pub text: String,
    /// Optional reasoning effort override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// Stateful stdio NDJSON codec for Codex app-server requests, responses, and events.
///
/// # Examples
///
/// ```
/// use legion_providers::non_acp::{CodexAppServerCodec, CodexThreadStartParams, NonAcpTurnUpdate};
///
/// let mut codec = CodexAppServerCodec::new();
/// let (id, init_line) = codec.encode_initialize("legioncode", "LegionCode", "0.1.0", true)?;
/// assert_eq!(id, 1);
/// assert!(init_line.ends_with('\n'));
///
/// let updates = codec.push_bytes(
///     br#"{"method":"item/agentMessage/delta","params":{"delta":"hi","itemId":"i1","turnId":"t1"}}"#
/// )?;
/// assert!(updates.is_empty());
/// let flushed = codec.finish()?;
/// assert_eq!(flushed, vec![NonAcpTurnUpdate::AssistantTextDelta { text: "hi".to_owned() }]);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct CodexAppServerCodec {
    ndjson: NdjsonLineCodec,
    next_id: u64,
}

impl Default for CodexAppServerCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexAppServerCodec {
    /// Creates a new Codex app-server codec starting request IDs at `1`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ndjson: NdjsonLineCodec::default(),
            next_id: 1,
        }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    fn encode_request_value(
        &mut self,
        method: &str,
        params: &Value,
    ) -> Result<(u64, String), NonAcpParseError> {
        let id = self.alloc_id();
        let frame = json!({
            "id": id,
            "method": method,
            "params": params,
        });
        let line = self.ndjson.encode_line(&frame)?;
        Ok((id, line))
    }

    /// Encodes the Codex `initialize` request frame.
    pub fn encode_initialize(
        &mut self,
        name: &str,
        title: &str,
        version: &str,
        experimental_api: bool,
    ) -> Result<(u64, String), NonAcpParseError> {
        self.encode_request_value(
            "initialize",
            &json!({
                "clientInfo": { "name": name, "title": title, "version": version },
                "capabilities": {
                    "experimentalApi": experimental_api,
                    "optOutNotificationMethods": Value::Null,
                }
            }),
        )
    }

    /// Encodes the follow-up `initialized` notification line (`{"method":"initialized"}\n`).
    pub fn encode_initialized(&self) -> Result<String, NonAcpParseError> {
        Ok(self
            .ndjson
            .encode_line(&json!({ "method": "initialized" }))?)
    }

    /// Encodes a Codex `thread/start` request line.
    pub fn encode_thread_start(
        &mut self,
        params: &CodexThreadStartParams,
    ) -> Result<(u64, String), NonAcpParseError> {
        let val = serde_json::to_value(params)?;
        self.encode_request_value("thread/start", &val)
    }

    /// Encodes a Codex `turn/start` request line.
    pub fn encode_turn_start(
        &mut self,
        params: &CodexTurnStartParams,
    ) -> Result<(u64, String), NonAcpParseError> {
        let mut obj = json!({
            "threadId": params.thread_id,
            "input": [{ "type": "text", "text": params.text }],
        });
        if let Some(effort) = &params.effort
            && let Some(map) = obj.as_object_mut()
        {
            map.insert("effort".to_owned(), Value::String(effort.clone()));
        }
        self.encode_request_value("turn/start", &obj)
    }

    /// Encodes a Codex `turn/steer` request line injecting mid-turn user steering input.
    pub fn encode_turn_steer(
        &mut self,
        thread_id: &str,
        expected_turn_id: &str,
        text: &str,
    ) -> Result<(u64, String), NonAcpParseError> {
        self.encode_request_value(
            "turn/steer",
            &json!({
                "threadId": thread_id,
                "expectedTurnId": expected_turn_id,
                "input": [{ "type": "text", "text": text }],
            }),
        )
    }

    /// Encodes a Codex `turn/interrupt` request line cancelling the active turn.
    pub fn encode_turn_interrupt(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<(u64, String), NonAcpParseError> {
        self.encode_request_value(
            "turn/interrupt",
            &json!({
                "threadId": thread_id,
                "turnId": turn_id,
            }),
        )
    }

    /// Feeds raw stdout bytes and translates all complete NDJSON frames.
    pub fn push_bytes(&mut self, bytes: &[u8]) -> Result<Vec<NonAcpTurnUpdate>, NonAcpParseError> {
        let lines = self.ndjson.push_bytes(bytes)?;
        let mut updates = Vec::new();
        for line in lines {
            updates.extend(parse_codex_app_server_line(&line)?);
        }
        Ok(updates)
    }

    /// Flushes any trailing unterminated NDJSON line at EOF.
    pub fn finish(&mut self) -> Result<Vec<NonAcpTurnUpdate>, NonAcpParseError> {
        let Some(line) = self.ndjson.finish()? else {
            return Ok(Vec::new());
        };
        parse_codex_app_server_line(&line)
    }
}

/// Encodes an `execCommandApproval` (or `item/commandExecution/requestApproval`) response line.
pub fn encode_exec_command_approval_response(
    request_id: &Value,
    decision: CodexApprovalDecision,
) -> Result<String, NonAcpParseError> {
    encode_approval_response(request_id, decision)
}

/// Encodes an `applyPatchApproval` (or `item/fileChange/requestApproval`) response line.
pub fn encode_apply_patch_approval_response(
    request_id: &Value,
    decision: CodexApprovalDecision,
) -> Result<String, NonAcpParseError> {
    encode_approval_response(request_id, decision)
}

fn encode_approval_response(
    request_id: &Value,
    decision: CodexApprovalDecision,
) -> Result<String, NonAcpParseError> {
    let payload = json!({
        "id": request_id,
        "result": { "decision": decision },
    });
    Ok(NdjsonLineCodec::default().encode_line(&payload)?)
}
