//! Shared opaque-box helpers for E2E-M4 F22 & F24 Rust integration tests.

use std::fs;
use std::path::Path;

use legion_protocol::rpc_codec::{RpcCauseReason, RpcExit};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

use super::workflow_board_e2e::TestResult;
use super::workflow_board_e2e::harness::OpaqueBoxHarness;
use super::workflow_board_e2e::ws_rpc::OpaqueWsClient;

pub(crate) async fn board_capability_enabled(harness: &OpaqueBoxHarness) -> TestResult<bool> {
    let env_desc = harness.probe_environment().await?;
    Ok(env_desc["capabilities"]["projectWorkflowBoard"].as_bool() == Some(true))
}

pub(crate) async fn mint_scoped_session(
    harness: &OpaqueBoxHarness,
    admin_token: &str,
    subject: &str,
    scopes: &[&str],
) -> TestResult<String> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/auth/sessions", harness.origin()?))
        .header(AUTHORIZATION, format!("Bearer {admin_token}"))
        .header(CONTENT_TYPE, "application/json")
        .json(&json!({
            "subject": subject,
            "label": format!("e2e-{subject}"),
            "scopes": scopes,
        }))
        .send()
        .await?;
    if resp.status() != 201 {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("POST /api/auth/sessions returned {status}: {text}").into());
    }
    let body: Value = resp.json().await?;
    Ok(body["token"]
        .as_str()
        .ok_or("missing token in /api/auth/sessions response")?
        .to_owned())
}

pub(crate) fn assert_auth_denied(
    exit: &RpcExit,
    expected_scope: &str,
    expected_perm: &str,
    context: &str,
) -> TestResult<()> {
    match exit {
        RpcExit::Failure { cause } => {
            let Some(RpcCauseReason::Fail { error }) = cause.first() else {
                return Err(format!("{context}: expected Fail cause, got {cause:?}").into());
            };
            if error["_tag"] != "EnvironmentAuthorizationError" {
                return Err(format!(
                    "{context}: expected EnvironmentAuthorizationError, got {error}"
                )
                .into());
            }
            if error["requiredScope"] != expected_scope {
                return Err(format!(
                    "{context}: expected requiredScope={expected_scope}, got {}",
                    error["requiredScope"]
                )
                .into());
            }
            if error["requiredPermission"] != expected_perm {
                return Err(format!(
                    "{context}: expected requiredPermission={expected_perm}, got {}",
                    error["requiredPermission"]
                )
                .into());
            }
            Ok(())
        }
        RpcExit::Success { value } => Err(format!(
            "{context}: expected EnvironmentAuthorizationError failure, got Success({value})"
        )
        .into()),
    }
}

pub(crate) fn assert_not_auth_denied(exit: &RpcExit, context: &str) -> TestResult<()> {
    if let RpcExit::Failure { cause } = exit
        && let Some(RpcCauseReason::Fail { error }) = cause.first()
        && error["_tag"] == "EnvironmentAuthorizationError"
    {
        return Err(format!("{context}: unexpected EnvironmentAuthorizationError: {error}").into());
    }
    Ok(())
}

pub(crate) fn assert_domain_failure(
    exit: &RpcExit,
    expected_keywords: &[&str],
    context: &str,
) -> TestResult<()> {
    match exit {
        RpcExit::Failure { cause } => {
            let Some(RpcCauseReason::Fail { error }) = cause.first() else {
                return Err(format!("{context}: expected Fail cause, got {cause:?}").into());
            };
            let tag = error["_tag"].as_str().unwrap_or("");
            if tag == "UnsupportedRpcMethod" || tag == "ParseError" {
                return Err(format!(
                    "{context}: unexpected {tag} failure instead of domain error: {error}"
                )
                .into());
            }
            let text = error.to_string().to_ascii_lowercase();
            if !expected_keywords.is_empty()
                && !expected_keywords.iter().any(|kw| text.contains(kw))
            {
                return Err(format!(
                    "{context}: expected error mentioning one of {expected_keywords:?}, got {error}"
                )
                .into());
            }
            Ok(())
        }
        RpcExit::Success { value } => {
            Err(format!("{context}: expected domain failure, got Success({value})").into())
        }
    }
}

pub(crate) async fn write_ws_masked_text(stream: &mut TcpStream, text: &str) -> TestResult<()> {
    let payload = text.as_bytes();
    let mask = [0x21_u8, 0x43, 0x65, 0x87];
    let mut frame = Vec::with_capacity(payload.len() + 14);
    frame.push(0x81);
    let len = payload.len();
    if len <= 125 {
        frame.push(0x80 | u8::try_from(len)?);
    } else if len <= 65_535 {
        frame.push(0x80 | 0x7e);
        frame.extend_from_slice(&u16::try_from(len)?.to_be_bytes());
    } else {
        frame.push(0x80 | 0x7f);
        frame.extend_from_slice(&u64::try_from(len)?.to_be_bytes());
    }
    frame.extend_from_slice(&mask);
    for (idx, byte) in payload.iter().enumerate() {
        frame.push(byte ^ mask[idx % 4]);
    }
    stream.write_all(&frame).await?;
    Ok(())
}

async fn ensure_ws_bytes(stream: &mut TcpStream, buf: &mut Vec<u8>, need: usize) -> TestResult<()> {
    let mut tmp = [0_u8; 4096];
    while buf.len() < need {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Err("unexpected EOF reading WebSocket frame".into());
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    Ok(())
}

pub(crate) async fn read_ws_text_frame(
    stream: &mut TcpStream,
    buf: &mut Vec<u8>,
) -> TestResult<String> {
    loop {
        ensure_ws_bytes(stream, buf, 2).await?;
        let (b0, b1) = (buf[0], buf[1]);
        buf.drain(..2);
        let opcode = b0 & 0x0f;
        let len = match b1 & 0x7f {
            n @ 0..=125 => usize::from(n),
            126 => {
                ensure_ws_bytes(stream, buf, 2).await?;
                let n = u16::from_be_bytes([buf[0], buf[1]]);
                buf.drain(..2);
                usize::from(n)
            }
            _ => {
                ensure_ws_bytes(stream, buf, 8).await?;
                let bytes: [u8; 8] = buf[..8].try_into()?;
                buf.drain(..8);
                usize::try_from(u64::from_be_bytes(bytes))?
            }
        };
        ensure_ws_bytes(stream, buf, len).await?;
        let payload: Vec<u8> = buf.drain(..len).collect();
        match opcode {
            0x1 => return Ok(String::from_utf8(payload)?),
            0x9 => {
                let n = payload.len().min(125);
                let mut pong = vec![0x8A, 0x80 | u8::try_from(n)?, 0, 0, 0, 0];
                pong.extend_from_slice(&payload[..n]);
                stream.write_all(&pong).await?;
            }
            0xA => {}
            other => return Err(format!("unexpected WebSocket opcode 0x{other:x}").into()),
        }
    }
}

pub(crate) async fn ensure_project_added(
    harness: &OpaqueBoxHarness,
    ws: &mut OpaqueWsClient,
) -> TestResult<String> {
    let ws_str = harness.workspace_dir().to_string_lossy().into_owned();
    let base_str = harness.home_dir().to_string_lossy().into_owned();
    let out = harness.run_cli(&[
        "project",
        "add",
        &ws_str,
        "--title",
        "Rust E2E M4 Workspace",
        "--base-dir",
        &base_str,
    ])?;
    if !out.status.success() {
        return Err(format!(
            "legioncode project add failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    let shell = ws
        .call_ok("shell-snap", "orchestration.v2.getShellSnapshot", json!({}))
        .await?;
    let project_id = shell["projects"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|p| p.get("id"))
        .and_then(Value::as_str)
        .ok_or("expected project id in shell snapshot")?
        .to_owned();
    ws.call_ok(
        "e2e-enable-wf-defaults",
        "server.updateSettings",
        json!({
            "patch": {
                "workflowsEnabled": true,
                "defaultModelSelection": { "instanceId": "devReplay", "model": "dev-replay" }
            }
        }),
    )
    .await?;
    Ok(project_id)
}

pub(crate) fn extract_card_id(value: &Value) -> TestResult<String> {
    value
        .get("cardId")
        .or_else(|| value.get("id"))
        .or_else(|| value.pointer("/card/cardId"))
        .or_else(|| value.pointer("/card/id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("missing cardId in response: {value}").into())
}

pub(crate) fn extract_thread_id(value: &Value) -> TestResult<String> {
    value
        .get("threadId")
        .or_else(|| value.pointer("/launch/threadId"))
        .or_else(|| value.pointer("/card/activeThreadId"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("missing threadId in response: {value}").into())
}

pub(crate) async fn post_mcp_raw_accept(
    origin: &str,
    accept_header: &str,
    body: &Value,
) -> TestResult<reqwest::Response> {
    let client = reqwest::Client::new();
    Ok(client
        .post(format!("{origin}/mcp"))
        .header(ACCEPT, accept_header)
        .header(CONTENT_TYPE, "application/json")
        .json(body)
        .send()
        .await?)
}

pub(crate) async fn call_mcp_tool(
    harness: &OpaqueBoxHarness,
    token: &str,
    thread_id: &str,
    id: usize,
    tool_name: &str,
    arguments: Value,
) -> TestResult<Value> {
    let resp = harness
        .post_mcp(
            Some(token),
            &[("x-legioncode-thread-id", thread_id)],
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": { "name": tool_name, "arguments": arguments }
            }),
        )
        .await?;
    Ok(resp.json().await?)
}

pub(crate) fn count_files_under(dir: &Path) -> TestResult<usize> {
    if !dir.exists() {
        return Ok(0);
    }
    let mut count = 0;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            count += count_files_under(&entry.path())?;
        } else {
            count += 1;
        }
    }
    Ok(count)
}
