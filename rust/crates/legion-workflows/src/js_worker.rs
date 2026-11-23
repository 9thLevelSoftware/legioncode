#![forbid(unsafe_code)]
//! Isolated bounded-deadline (`20s`) external JS runtime JSON-RPC stdio worker.

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use serde_json::Value;
use tempfile::NamedTempFile;

use crate::engine::WorkflowDefinition;
use crate::js_specs::{JsDriveResult, JsPassBackend, JsWorkflowSpec, specs_to_definitions};
use crate::js_worker_probe::{
    BOOTSTRAP_MJS, ExternalJsRuntime, RpcRequest, RpcResponse, WORKER_DEADLINE, configure_headless,
    detect_external_js_runtime, initialize_worker, send_rpc, spawn_stdout_reader, worker_prob,
};
use crate::traits::BoxFuture;
use crate::types::WorkflowDefinitionProblem;

enum WorkerCmd {
    CallAsync {
        method: &'static str,
        params: Value,
        reply: tokio::sync::oneshot::Sender<Result<Value, String>>,
    },
    CallSync {
        method: &'static str,
        params: Value,
        reply: mpsc::SyncSender<Result<Value, String>>,
    },
}

/// Persistent stdio JSON-RPC worker for one workflow file with external package imports.
pub struct JsWorkerHost {
    tx: mpsc::Sender<WorkerCmd>,
}

impl JsWorkerHost {
    /// Spawns a bounded-deadline (`20s`) Node/Deno/Bun JSON-RPC worker and loads `entry_path`.
    pub fn load_file(
        entry_path: &Path,
        file_name: &str,
        import_hint: &str,
    ) -> Result<(Vec<WorkflowDefinition>, Arc<Self>), WorkflowDefinitionProblem> {
        let Some(runtime) = detect_external_js_runtime() else {
            return Err(worker_prob(
                file_name,
                &format!(
                    "Workflow imports external module '{import_hint}', but no compatible external JavaScript runtime (Node >= 22.6 with --experimental-strip-types, Deno, or Bun) is installed."
                ),
                None,
            ));
        };

        let mut script_tmp = tempfile::Builder::new()
            .prefix("legion-wf-worker-")
            .suffix(".mjs")
            .tempfile()
            .map_err(|e| {
                worker_prob(
                    file_name,
                    &format!("Failed to create worker bootstrap file: {e}"),
                    None,
                )
            })?;
        script_tmp
            .write_all(BOOTSTRAP_MJS.as_bytes())
            .and_then(|()| script_tmp.flush())
            .map_err(|e| {
                worker_prob(
                    file_name,
                    &format!("Failed to write worker bootstrap script: {e}"),
                    None,
                )
            })?;

        let mut cmd = Command::new(runtime.program());
        match runtime {
            ExternalJsRuntime::Node => {
                cmd.arg("--experimental-strip-types").arg(script_tmp.path());
            }
            ExternalJsRuntime::Deno => {
                cmd.args(["run", "--quiet", "--allow-read", "--allow-env"])
                    .arg(script_tmp.path());
            }
            ExternalJsRuntime::Bun => {
                cmd.arg("run").arg(script_tmp.path());
            }
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        configure_headless(&mut cmd);

        let child = cmd.spawn().map_err(|e| {
            worker_prob(
                file_name,
                &format!(
                    "Failed to launch external JS worker ({}): {e}",
                    runtime.program()
                ),
                None,
            )
        })?;

        let (init_tx, init_rx) =
            mpsc::sync_channel::<Result<Vec<JsWorkflowSpec>, WorkflowDefinitionProblem>>(1);
        let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCmd>();
        let entry_owned = entry_path.to_string_lossy().into_owned();
        let fname_owned = file_name.to_owned();

        std::thread::Builder::new()
            .name(format!("legion-jsw-{file_name}"))
            .spawn(move || {
                run_worker_supervisor(
                    child,
                    &script_tmp,
                    &entry_owned,
                    &fname_owned,
                    &init_tx,
                    &cmd_rx,
                );
            })
            .map_err(|e| {
                worker_prob(
                    file_name,
                    &format!("Failed to spawn worker supervisor thread: {e}"),
                    None,
                )
            })?;

        let specs = init_rx
            .recv_timeout(WORKER_DEADLINE + Duration::from_secs(2))
            .map_err(|_| {
                worker_prob(
                    file_name,
                    "Timed out initializing external workflow runtime worker.",
                    None,
                )
            })??;

        let host = Arc::new(Self { tx: cmd_tx });
        let backend: Arc<dyn JsPassBackend> = host.clone();
        let defs = specs_to_definitions(specs, &backend, file_name)?;
        Ok((defs, host))
    }

    fn call_async(
        &self,
        method: &'static str,
        params: Value,
    ) -> BoxFuture<'_, Result<Value, String>> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let send_res = self.tx.send(WorkerCmd::CallAsync {
            method,
            params,
            reply,
        });
        Box::pin(async move {
            send_res.map_err(|e| format!("External JS worker stopped: {e}"))?;
            tokio::time::timeout(WORKER_DEADLINE + Duration::from_secs(1), rx)
                .await
                .map_err(|_| "External JS worker call timed out (20s limit)".to_owned())?
                .map_err(|_| "External JS worker dropped reply channel".to_owned())?
        })
    }

    fn call_sync(&self, method: &'static str, params: Value) -> Result<Value, String> {
        let (reply, rx) = mpsc::sync_channel(1);
        self.tx
            .send(WorkerCmd::CallSync {
                method,
                params,
                reply,
            })
            .map_err(|e| format!("External JS worker stopped: {e}"))?;
        rx.recv_timeout(WORKER_DEADLINE + Duration::from_secs(1))
            .map_err(|e| format!("External JS worker callback timed out: {e}"))?
    }
}

impl JsPassBackend for JsWorkerHost {
    fn drive_pass(
        &self,
        wf_key: &str,
        input: &Value,
        journal: &Value,
        target_step: Option<&str>,
        step_ops: &Value,
        run_id: &str,
    ) -> BoxFuture<'_, Result<JsDriveResult, String>> {
        let params = serde_json::json!({
            "wfKey": wf_key,
            "input": input,
            "journal": journal,
            "targetStep": target_step,
            "stepOps": step_ops,
            "runId": run_id,
        });
        Box::pin(async move {
            let val = self.call_async("drivePass", params).await?;
            serde_json::from_value(val).map_err(|e| e.to_string())
        })
    }

    fn drive_on_end(
        &self,
        wf_key: &str,
        input: &Value,
        end_info: &Value,
        step_ops: &Value,
    ) -> BoxFuture<'_, Result<JsDriveResult, String>> {
        let params = serde_json::json!({
            "wfKey": wf_key,
            "input": input,
            "endInfo": end_info,
            "stepOps": step_ops,
        });
        Box::pin(async move {
            let val = self.call_async("driveOnEnd", params).await?;
            serde_json::from_value(val).map_err(|e| e.to_string())
        })
    }

    fn call_webhook_key(&self, idx: usize, req: &Value) -> Result<Option<String>, String> {
        let val = self.call_sync(
            "callWebhookKey",
            serde_json::json!({ "idx": idx, "req": req }),
        )?;
        Ok(val.as_str().map(str::to_owned))
    }

    fn check_wait_codec(&self, idx: usize, val: &Value) -> Result<Value, String> {
        let out = self.call_sync(
            "checkWaitCodec",
            serde_json::json!({ "idx": idx, "val": val }),
        )?;
        if out.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(out.get("value").cloned().unwrap_or(Value::Null))
        } else {
            Err(out
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("Codec validation failed")
                .to_owned())
        }
    }
}

fn run_worker_supervisor(
    mut child: Child,
    script_tmp: &NamedTempFile,
    entry_path: &str,
    file_name: &str,
    init_tx: &mpsc::SyncSender<Result<Vec<JsWorkflowSpec>, WorkflowDefinitionProblem>>,
    cmd_rx: &mpsc::Receiver<WorkerCmd>,
) {
    let _keep_tmp = script_tmp.path();
    let Some(mut stdin) = child.stdin.take() else {
        let _ = init_tx.send(Err(worker_prob(
            file_name,
            "Missing child stdin pipe",
            None,
        )));
        let _ = child.kill();
        return;
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = init_tx.send(Err(worker_prob(
            file_name,
            "Missing child stdout pipe",
            None,
        )));
        let _ = child.kill();
        return;
    };
    let (line_tx, line_rx) = mpsc::channel::<Result<RpcResponse, String>>();
    spawn_stdout_reader(stdout, line_tx);

    let mut next_id = 1u64;
    if !initialize_worker(
        &mut stdin,
        &line_rx,
        &mut next_id,
        entry_path,
        file_name,
        init_tx,
    ) {
        let _ = child.kill();
        let _ = child.wait();
        return;
    }

    while let Ok(cmd) = cmd_rx.recv() {
        if !handle_worker_cmd(&mut stdin, &line_rx, &mut next_id, cmd) {
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn handle_worker_cmd(
    stdin: &mut std::process::ChildStdin,
    line_rx: &mpsc::Receiver<Result<RpcResponse, String>>,
    next_id: &mut u64,
    cmd: WorkerCmd,
) -> bool {
    let (method, params, async_reply, sync_reply) = match cmd {
        WorkerCmd::CallAsync {
            method,
            params,
            reply,
        } => (method, params, Some(reply), None),
        WorkerCmd::CallSync {
            method,
            params,
            reply,
        } => (method, params, None, Some(reply)),
    };
    let req = RpcRequest {
        id: *next_id,
        method: method.to_owned(),
        preludes: None,
        entry_path: None,
        params: Some(params),
    };
    *next_id += 1;
    let res = send_rpc(stdin, line_rx, &req).map_err(|(m, _)| m);
    let timed_out = res.as_ref().is_err_and(|m| m.contains("timed out"));
    if let Some(reply) = async_reply {
        let _ = reply.send(res);
    } else if let Some(reply) = sync_reply {
        let _ = reply.send(res);
    }
    !timed_out
}
