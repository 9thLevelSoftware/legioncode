//! Deterministic M2 plan payloads and M3 `claudeAgent` mock providers / HTTP gate for E2E-M3.

use std::fs;
use std::path::{Path, PathBuf};
use std::process;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use super::support::next_id;
use crate::workflow_board_e2e::TestResult;
use crate::workflow_board_e2e::ws_rpc::OpaqueWsClient;

pub fn valid_plan_lenses() -> Value {
    json!([
        { "lens": "ux", "applicable": true, "summary": "Card split layout." },
        { "lens": "functional", "applicable": true, "summary": "Task DAG execution." },
        { "lens": "architecture", "applicable": true, "summary": "Worktree-isolated turns." },
        { "lens": "quality", "applicable": true, "summary": "Planned argv checks and review." },
        { "lens": "infrastructure", "applicable": false, "reason": "Local workspace only." },
        { "lens": "delivery", "applicable": true, "summary": "Discrete commit/push/PR." }
    ])
}

pub fn valid_plan_alternatives() -> Value {
    json!([
        {
            "id": "alt-1",
            "title": "Sequential DAG",
            "summary": "Separate core and dependent tasks.",
            "tradeoffs": "Clear contract isolation."
        },
        {
            "id": "alt-2",
            "title": "Single task",
            "summary": "Combine changes into one turn.",
            "tradeoffs": "Coarser diff."
        }
    ])
}

pub fn valid_plan_tasks() -> Value {
    json!([
        {
            "taskId": "TSK-1",
            "title": "Implement core task",
            "dependsOn": [],
            "filesAllowed": ["src/ok.ts"],
            "filesForbidden": ["src/secret.ts"],
            "testingMethod": "regression-first"
        },
        {
            "taskId": "TSK-2",
            "title": "Dependent follow-up task",
            "dependsOn": ["TSK-1"],
            "filesAllowed": ["src/followup.ts"],
            "filesForbidden": ["src/secret.ts"],
            "testingMethod": "regression-first"
        }
    ])
}

pub fn valid_single_ready_tasks() -> Value {
    json!([
        {
            "taskId": "TSK-1",
            "title": "Ready 1",
            "dependsOn": [],
            "filesAllowed": ["src/a.ts"],
            "filesForbidden": ["src/secret.ts"],
            "testingMethod": "regression-first"
        },
        {
            "taskId": "TSK-2",
            "title": "Ready 2",
            "dependsOn": [],
            "filesAllowed": ["src/b.ts"],
            "filesForbidden": ["src/secret.ts"],
            "testingMethod": "regression-first"
        }
    ])
}

pub fn valid_plan_update_payload(
    card_id: &str,
    expected_revision: i64,
    tasks: Value,
    checks: Value,
    ac_mappings: Value,
) -> Value {
    let mut obj = json!({
        "cardId": card_id,
        "expectedRevision": expected_revision,
        "lenses": valid_plan_lenses(),
        "alternatives": valid_plan_alternatives(),
        "selectedAlternativeId": "alt-1",
        "selectionRationale": "Two sequential tasks isolate file contracts cleanly.",
        "strategy": "outcomes",
        "granularity": "balanced",
    });
    obj["tasks"] = tasks;
    obj["checks"] = checks;
    obj["acMappings"] = ac_mappings;
    obj
}

pub struct HttpGate {
    port: u16,
    entered_rx: Option<oneshot::Receiver<()>>,
    release_tx: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl HttpGate {
    pub async fn bind() -> TestResult<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let (entered_tx, entered_rx) = oneshot::channel::<()>();
        let (release_tx, release_rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let mut req = Vec::new();
            let mut tmp = [0_u8; 512];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                let Ok(n) = sock.read(&mut tmp).await else {
                    break;
                };
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&tmp[..n]);
            }
            let _ = entered_tx.send(());
            let _ = release_rx.await;
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await;
            let _ = sock.flush().await;
        });
        Ok(Self {
            port,
            entered_rx: Some(entered_rx),
            release_tx: Some(release_tx),
            task: Some(task),
        })
    }

    pub const fn port(&self) -> u16 {
        self.port
    }

    pub async fn wait_entered(&mut self) -> TestResult<()> {
        if let Some(rx) = self.entered_rx.take() {
            rx.await
                .map_err(|_| "HTTP gate dropped before mock-gate connected")?;
        }
        Ok(())
    }

    pub fn release(&mut self) {
        if let Some(tx) = self.release_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for HttpGate {
    fn drop(&mut self) {
        self.release();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn write_claude_shim(dir: &Path, name: &str, js_body: &str) -> TestResult<String> {
    fs::create_dir_all(dir)?;
    let script_path = dir.join(format!("{name}.mjs"));
    fs::write(&script_path, js_body)?;
    #[cfg(windows)]
    {
        let shim_path = dir.join(format!("{name}.cmd"));
        fs::write(
            &shim_path,
            format!("@echo off\r\nnode \"{}\" %*\r\n", script_path.display()),
        )?;
        Ok(shim_path.to_string_lossy().into_owned())
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let shim_path = dir.join(name);
        fs::write(
            &shim_path,
            format!(
                "#!/bin/sh\nexec node \"{}\" \"$@\"\n",
                script_path.display()
            ),
        )?;
        fs::set_permissions(&shim_path, fs::Permissions::from_mode(0o755))?;
        Ok(shim_path.to_string_lossy().into_owned())
    }
}

fn claude_stream_js(session_id: &str, msg_id: &str, text_expr: &str, pre_js: &str) -> String {
    format!(
        "process.stdin.resume();\n\
         {pre_js}\n\
         console.log(JSON.stringify({{ type: 'system', subtype: 'init', session_id: '{session_id}' }}));\n\
         console.log(JSON.stringify({{ type: 'assistant', message: {{ id: '{msg_id}', role: 'assistant', content: [{{ type: 'text', text: {text_expr} }}], usage: {{ input_tokens: 10, output_tokens: 8 }} }} }}));\n\
         console.log(JSON.stringify({{ type: 'result', subtype: 'success', is_error: false, usage: {{ input_tokens: 10, output_tokens: 8 }} }}));\n\
         process.exit(0);\n"
    )
}

pub async fn install_mock_providers(
    ws: &mut OpaqueWsClient,
    gate_port: Option<u16>,
) -> TestResult<()> {
    let dir: PathBuf = std::env::temp_dir().join(format!("legion-e2e-m3-mocks-{}", process::id()));
    let pass_bin = write_claude_shim(
        &dir,
        "mock-review-pass",
        &claude_stream_js(
            "claude-rev-pass",
            "msg-rev-pass",
            "JSON.stringify({ verdict: 'pass', summary: 'Clean independent review.', findings: [] })",
            "",
        ),
    )?;
    let findings_bin = write_claude_shim(
        &dir,
        "mock-review-findings",
        &claude_stream_js(
            "claude-rev-find",
            "msg-rev-find",
            "JSON.stringify({ verdict: 'findings', summary: 'Found 1 minor issue.', findings: [{ id: 'FND-1', severity: 'minor', criterionId: 'AC-1', file: 'src/ok.ts', line: 1, message: 'Document exported symbol.' }] })",
            "",
        ),
    )?;
    let mut instances = json!({
        "mock-review-pass": {
            "driver": "claudeAgent",
            "enabled": true,
            "config": { "binaryPath": pass_bin }
        },
        "mock-review-findings": {
            "driver": "claudeAgent",
            "enabled": true,
            "config": { "binaryPath": findings_bin }
        }
    });
    if let Some(port) = gate_port {
        let gate_pre = format!(
            "const http = await import('node:http');\n\
             await new Promise((resolve) => {{\n\
               const req = http.get('http://127.0.0.1:{port}/wait', (res) => {{ res.resume(); res.on('end', resolve); }});\n\
               req.on('error', resolve);\n\
             }});"
        );
        let gate_bin = write_claude_shim(
            &dir,
            &format!("mock-gate-{port}"),
            &claude_stream_js(
                "claude-gate",
                "msg-gate",
                "'Completed gated task.'",
                &gate_pre,
            ),
        )?;
        instances["mock-gate"] = json!({
            "driver": "claudeAgent",
            "enabled": true,
            "config": { "binaryPath": gate_bin }
        });
    }
    let _ = ws
        .call_ok(
            &next_id("cfg-mocks"),
            "server.updateSettings",
            json!({
                "patch": {
                    "workflowsEnabled": true,
                    "providerInstances": instances
                }
            }),
        )
        .await?;
    Ok(())
}
