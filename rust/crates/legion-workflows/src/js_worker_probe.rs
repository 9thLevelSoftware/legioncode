#![forbid(unsafe_code)]
//! External JS runtime detection (`node --experimental-strip-types`, `deno`, `bun`) and bootstrap.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use crate::js_prelude::PRELUDE_CORE_JS;
use crate::js_prelude_api::PRELUDE_API_JS;
use crate::js_prelude_driver::PRELUDE_DRIVER_JS;
use crate::js_prelude_schemas::PRELUDE_SCHEMAS_JS;
use crate::js_specs::JsWorkflowSpec;
use crate::types::WorkflowDefinitionProblem;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const WORKER_DEADLINE: Duration = Duration::from_secs(20);

pub(crate) const BOOTSTRAP_MJS: &str = r#"
import path from "node:path";
import vm from "node:vm";
import { pathToFileURL } from "node:url";
import readline from "node:readline";

function send(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
}

function parseLineFromStack(err) {
  const stack = err && typeof err.stack === "string" ? err.stack : "";
  const m = /:(\d+):(\d+)/.exec(stack);
  return m ? Number(m[1]) : null;
}

const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on("line", async (line) => {
  if (!line.trim()) return;
  let req;
  try { req = JSON.parse(line); } catch { return; }
  const id = req.id;
  try {
    if (req.method === "init") {
      for (const code of req.preludes || []) {
        vm.runInThisContext(code);
      }
      const abs = path.resolve(req.entryPath);
      const url = pathToFileURL(abs).href + "?v=" + Date.now();
      await import(url);
      const specs = globalThis.__legionListWorkflowSpecs();
      send({ id, ok: true, result: specs });
    } else if (req.method === "drivePass") {
      const p = req.params;
      const out = globalThis.__legionDrivePass(p.wfKey, p.input, p.journal, p.targetStep, p.stepOps, p.runId);
      send({ id, ok: true, result: out });
    } else if (req.method === "driveOnEnd") {
      const p = req.params;
      const out = globalThis.__legionDriveOnEnd(p.wfKey, p.input, p.endInfo, p.stepOps);
      send({ id, ok: true, result: out });
    } else if (req.method === "callWebhookKey") {
      const p = req.params;
      send({ id, ok: true, result: globalThis.__legionCallWebhookKey(p.idx, p.req) });
    } else if (req.method === "checkWaitCodec") {
      const p = req.params;
      send({ id, ok: true, result: globalThis.__legionCheckWaitCodec(p.idx, p.val) });
    } else {
      send({ id, ok: false, error: "Unknown worker RPC method: " + req.method });
    }
  } catch (err) {
    send({
      id,
      ok: false,
      error: err instanceof Error ? err.message : String(err),
      line: parseLineFromStack(err)
    });
  }
});
"#;

/// Detected external JS runtime binary capable of running ES modules + TypeScript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalJsRuntime {
    Node,
    Deno,
    Bun,
}

impl ExternalJsRuntime {
    pub(crate) fn program(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Deno => "deno",
            Self::Bun => "bun",
        }
    }
}

/// Probes `PATH` for Node (>= 22.6 with `--experimental-strip-types`), Deno, or Bun.
pub fn detect_external_js_runtime() -> Option<ExternalJsRuntime> {
    if node_supports_strip_types() {
        return Some(ExternalJsRuntime::Node);
    }
    if command_succeeds("deno", &["--version"]) {
        return Some(ExternalJsRuntime::Deno);
    }
    if command_succeeds("bun", &["--version"]) {
        return Some(ExternalJsRuntime::Bun);
    }
    None
}

fn node_supports_strip_types() -> bool {
    let mut cmd = Command::new("node");
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    configure_headless(&mut cmd);
    let Ok(out) = cmd.output() else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    let ver = String::from_utf8_lossy(&out.stdout);
    let trimmed = ver.trim().trim_start_matches('v');
    let mut parts = trimmed.split('.');
    let major = parts
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    let minor = parts
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    major > 22 || (major == 22 && minor >= 6)
}

fn command_succeeds(bin: &str, args: &[&str]) -> bool {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_headless(&mut cmd);
    cmd.status().is_ok_and(|s| s.success())
}

#[cfg(windows)]
pub(crate) fn configure_headless(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub(crate) fn configure_headless(_cmd: &mut Command) {}

#[derive(Serialize)]
pub(crate) struct RpcRequest {
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preludes: Option<Vec<&'static str>>,
    #[serde(rename = "entryPath", skip_serializing_if = "Option::is_none")]
    pub entry_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Deserialize)]
pub(crate) struct RpcResponse {
    pub id: u64,
    pub ok: bool,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub line: Option<usize>,
}

pub(crate) fn send_rpc(
    stdin: &mut std::process::ChildStdin,
    line_rx: &mpsc::Receiver<Result<RpcResponse, String>>,
    req: &RpcRequest,
) -> Result<Value, (String, Option<usize>)> {
    let encoded = serde_json::to_string(req).map_err(|e| (e.to_string(), None))?;
    writeln!(stdin, "{encoded}")
        .and_then(|()| stdin.flush())
        .map_err(|e| (format!("Failed writing to external JS worker: {e}"), None))?;

    loop {
        match line_rx.recv_timeout(WORKER_DEADLINE) {
            Ok(Ok(resp)) if resp.id == req.id => {
                if resp.ok {
                    return Ok(resp.result.unwrap_or(Value::Null));
                }
                return Err((
                    resp.error
                        .unwrap_or_else(|| "External JS worker error".to_owned()),
                    resp.line,
                ));
            }
            Ok(Ok(_)) => {}
            Ok(Err(err)) => return Err((err, None)),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err((
                    "External JS worker evaluation timed out after 20s.".to_owned(),
                    None,
                ));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err((
                    "External JS worker process exited unexpectedly.".to_owned(),
                    None,
                ));
            }
        }
    }
}

pub(crate) fn worker_prob(
    file_name: &str,
    msg: &str,
    line: Option<usize>,
) -> WorkflowDefinitionProblem {
    WorkflowDefinitionProblem {
        message: msg.to_owned(),
        file: Some(file_name.to_owned()),
        line: line.and_then(|n| i64::try_from(n).ok()),
    }
}

pub(crate) fn spawn_stdout_reader(
    stdout: std::process::ChildStdout,
    line_tx: mpsc::Sender<Result<RpcResponse, String>>,
) {
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(raw) if !raw.trim().is_empty() => {
                    let parsed = serde_json::from_str::<RpcResponse>(&raw)
                        .map_err(|e| format!("Invalid worker JSON-RPC response: {e}"));
                    if line_tx.send(parsed).is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(err) => {
                    let _ = line_tx.send(Err(format!("Worker stdout read error: {err}")));
                    break;
                }
            }
        }
    });
}

pub(crate) fn initialize_worker(
    stdin: &mut std::process::ChildStdin,
    line_rx: &mpsc::Receiver<Result<RpcResponse, String>>,
    next_id: &mut u64,
    entry_path: &str,
    file_name: &str,
    init_tx: &mpsc::SyncSender<Result<Vec<JsWorkflowSpec>, WorkflowDefinitionProblem>>,
) -> bool {
    let init_req = RpcRequest {
        id: *next_id,
        method: "init".to_owned(),
        preludes: Some(vec![
            PRELUDE_CORE_JS,
            PRELUDE_SCHEMAS_JS,
            PRELUDE_API_JS,
            PRELUDE_DRIVER_JS,
        ]),
        entry_path: Some(entry_path.to_owned()),
        params: None,
    };
    *next_id += 1;
    match send_rpc(stdin, line_rx, &init_req) {
        Ok(val) => match serde_json::from_value::<Vec<JsWorkflowSpec>>(val) {
            Ok(specs) => {
                let _ = init_tx.send(Ok(specs));
                true
            }
            Err(err) => {
                let _ = init_tx.send(Err(worker_prob(file_name, &err.to_string(), None)));
                false
            }
        },
        Err((msg, line)) => {
            let _ = init_tx.send(Err(worker_prob(file_name, &msg, line)));
            false
        }
    }
}
