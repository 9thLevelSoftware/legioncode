#![forbid(unsafe_code)]
//! Embedded `boa_engine` host for pure TypeScript/JavaScript workflows.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use boa_engine::builtins::promise::PromiseState;
use boa_engine::vm::RuntimeLimits;
use boa_engine::{Context, JsError, Module, Source, context::ContextBuilder};
use serde_json::Value;

use crate::engine::WorkflowDefinition;
use crate::js_boa_loader::{
    TsModuleLoader, boa_js_err_problem, boa_problem, canonical_module_path, dispatch_boa_cmd,
    extract_specs,
};
use crate::js_prelude::PRELUDE_CORE_JS;
use crate::js_prelude_api::PRELUDE_API_JS;
use crate::js_prelude_driver::PRELUDE_DRIVER_JS;
use crate::js_prelude_schemas::PRELUDE_SCHEMAS_JS;
use crate::js_specs::{JsDriveResult, JsPassBackend, JsWorkflowSpec, specs_to_definitions};
use crate::traits::BoxFuture;
use crate::ts_strip::{ExternalImport, strip_typescript};
use crate::types::WorkflowDefinitionProblem;

const BOA_LOOP_LIMIT: u64 = 10_000_000;
const BOA_CALL_TIMEOUT: Duration = Duration::from_secs(20);

/// Result of attempting to load a workflow file via [`BoaHost`].
pub enum BoaLoadOutcome {
    /// Successfully loaded definitions into an isolated Boa host thread.
    Loaded {
        definitions: Vec<WorkflowDefinition>,
        backend: Arc<BoaHost>,
    },
    /// File or transitive local module imports external packages requiring `JsWorkerHost`.
    NeedsExternalRuntime(ExternalImport),
    /// Parse, linking, runtime, or schema error while loading the module.
    Problem(WorkflowDefinitionProblem),
}

pub(crate) enum BoaCmd {
    DrivePass {
        wf_key: String,
        input: Value,
        journal: Value,
        target_step: Option<String>,
        step_ops: Value,
        run_id: String,
        reply: tokio::sync::oneshot::Sender<Result<JsDriveResult, String>>,
    },
    DriveOnEnd {
        wf_key: String,
        input: Value,
        end_info: Value,
        step_ops: Value,
        reply: tokio::sync::oneshot::Sender<Result<JsDriveResult, String>>,
    },
    CallWebhookKey {
        idx: usize,
        req: Value,
        reply: mpsc::SyncSender<Result<Option<String>, String>>,
    },
    CheckWaitCodec {
        idx: usize,
        val: Value,
        reply: mpsc::SyncSender<Result<Value, String>>,
    },
}

/// Thread-confined `boa_engine` instance serving one loaded workflow file revision.
pub struct BoaHost {
    tx: mpsc::Sender<BoaCmd>,
}

impl BoaHost {
    /// Loads and evaluates TypeScript/JavaScript source from `file_path` (or synthetic file name).
    pub fn load_source(
        root_dir: Option<&Path>,
        file_path: &Path,
        file_name: &str,
        source: &str,
    ) -> BoaLoadOutcome {
        let stripped = strip_typescript(source);
        if let Some(ext) = stripped.external_imports.first() {
            return BoaLoadOutcome::NeedsExternalRuntime(ext.clone());
        }
        let (init_tx, init_rx) = mpsc::sync_channel::<Result<Vec<JsWorkflowSpec>, BoaInitFail>>(1);
        let (cmd_tx, cmd_rx) = mpsc::channel::<BoaCmd>();
        let root_buf = canonical_module_path(
            &root_dir
                .map(Path::to_path_buf)
                .or_else(|| file_path.parent().map(Path::to_path_buf))
                .unwrap_or_else(|| PathBuf::from(".")),
        );
        let entry_path = canonical_module_path(file_path);
        let code = stripped.code;
        let name_owned = file_name.to_owned();

        let spawned = std::thread::Builder::new()
            .name(format!("legion-boa-{file_name}"))
            .spawn(move || {
                run_boa_actor(
                    &root_buf,
                    &entry_path,
                    &name_owned,
                    &code,
                    &init_tx,
                    &cmd_rx,
                );
            });
        if let Err(err) = spawned {
            return BoaLoadOutcome::Problem(boa_problem(
                file_name,
                format!("Failed to start Boa JS runtime thread: {err}"),
            ));
        }
        let specs = match init_rx.recv_timeout(BOA_CALL_TIMEOUT) {
            Ok(Ok(s)) => s,
            Ok(Err(BoaInitFail::External(ext))) => {
                return BoaLoadOutcome::NeedsExternalRuntime(ext);
            }
            Ok(Err(BoaInitFail::Problem(p))) => return BoaLoadOutcome::Problem(p),
            Err(err) => {
                return BoaLoadOutcome::Problem(boa_problem(
                    file_name,
                    format!("Timed out evaluating workflow file: {err}"),
                ));
            }
        };
        let host = Arc::new(Self { tx: cmd_tx });
        let backend: Arc<dyn JsPassBackend> = host.clone();
        match specs_to_definitions(specs, &backend, file_name) {
            Ok(definitions) => BoaLoadOutcome::Loaded {
                definitions,
                backend: host,
            },
            Err(problem) => BoaLoadOutcome::Problem(problem),
        }
    }
}

impl JsPassBackend for BoaHost {
    fn drive_pass(
        &self,
        wf_key: &str,
        input: &Value,
        journal: &Value,
        target_step: Option<&str>,
        step_ops: &Value,
        run_id: &str,
    ) -> BoxFuture<'_, Result<JsDriveResult, String>> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let cmd = BoaCmd::DrivePass {
            wf_key: wf_key.to_owned(),
            input: input.clone(),
            journal: journal.clone(),
            target_step: target_step.map(str::to_owned),
            step_ops: step_ops.clone(),
            run_id: run_id.to_owned(),
            reply,
        };
        let send_res = self.tx.send(cmd);
        Box::pin(async move {
            send_res.map_err(|e| format!("Boa runtime stopped: {e}"))?;
            tokio::time::timeout(BOA_CALL_TIMEOUT, rx)
                .await
                .map_err(|_| "Workflow pass exceeded Boa evaluation timeout".to_owned())?
                .map_err(|_| "Boa worker dropped reply channel".to_owned())?
        })
    }

    fn drive_on_end(
        &self,
        wf_key: &str,
        input: &Value,
        end_info: &Value,
        step_ops: &Value,
    ) -> BoxFuture<'_, Result<JsDriveResult, String>> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let cmd = BoaCmd::DriveOnEnd {
            wf_key: wf_key.to_owned(),
            input: input.clone(),
            end_info: end_info.clone(),
            step_ops: step_ops.clone(),
            reply,
        };
        let send_res = self.tx.send(cmd);
        Box::pin(async move {
            send_res.map_err(|e| format!("Boa runtime stopped: {e}"))?;
            tokio::time::timeout(BOA_CALL_TIMEOUT, rx)
                .await
                .map_err(|_| "Workflow onEnd exceeded Boa evaluation timeout".to_owned())?
                .map_err(|_| "Boa worker dropped reply channel".to_owned())?
        })
    }

    fn call_webhook_key(&self, idx: usize, req: &Value) -> Result<Option<String>, String> {
        let (reply, rx) = mpsc::sync_channel(1);
        self.tx
            .send(BoaCmd::CallWebhookKey {
                idx,
                req: req.clone(),
                reply,
            })
            .map_err(|e| format!("Boa runtime stopped: {e}"))?;
        rx.recv_timeout(BOA_CALL_TIMEOUT)
            .map_err(|e| format!("Webhook key callback failed: {e}"))?
    }

    fn check_wait_codec(&self, idx: usize, val: &Value) -> Result<Value, String> {
        let (reply, rx) = mpsc::sync_channel(1);
        self.tx
            .send(BoaCmd::CheckWaitCodec {
                idx,
                val: val.clone(),
                reply,
            })
            .map_err(|e| format!("Boa runtime stopped: {e}"))?;
        rx.recv_timeout(BOA_CALL_TIMEOUT)
            .map_err(|e| format!("Wait codec callback failed: {e}"))?
    }
}

enum BoaInitFail {
    External(ExternalImport),
    Problem(WorkflowDefinitionProblem),
}

fn run_boa_actor(
    root: &Path,
    entry_path: &Path,
    file_name: &str,
    code: &str,
    init_tx: &mpsc::SyncSender<Result<Vec<JsWorkflowSpec>, BoaInitFail>>,
    cmd_rx: &mpsc::Receiver<BoaCmd>,
) {
    let mut ctx = match init_boa_context(root, entry_path, file_name, code) {
        Ok(c) => c,
        Err(fail) => {
            let _ = init_tx.send(Err(fail));
            return;
        }
    };
    let specs = match extract_specs(&mut ctx) {
        Ok(s) => s,
        Err(msg) => {
            let _ = init_tx.send(Err(BoaInitFail::Problem(boa_problem(file_name, msg))));
            return;
        }
    };
    let _ = init_tx.send(Ok(specs));
    while let Ok(cmd) = cmd_rx.recv() {
        dispatch_boa_cmd(&mut ctx, cmd);
    }
}

fn init_boa_context(
    root: &Path,
    entry_path: &Path,
    file_name: &str,
    code: &str,
) -> Result<Context, BoaInitFail> {
    let loader = Rc::new(TsModuleLoader::new(root));
    let mut ctx = ContextBuilder::new()
        .module_loader(loader.clone())
        .build()
        .map_err(|err| {
            BoaInitFail::Problem(boa_problem(
                file_name,
                format!("Boa context init failed: {err}"),
            ))
        })?;
    let mut limits = RuntimeLimits::default();
    limits.set_loop_iteration_limit(BOA_LOOP_LIMIT);
    limits.set_recursion_limit(256);
    ctx.set_runtime_limits(limits);

    for prelude in [
        PRELUDE_CORE_JS,
        PRELUDE_SCHEMAS_JS,
        PRELUDE_API_JS,
        PRELUDE_DRIVER_JS,
    ] {
        ctx.eval(Source::from_bytes(prelude)).map_err(|err| {
            BoaInitFail::Problem(boa_problem(
                file_name,
                format!("Workflow prelude error: {err}"),
            ))
        })?;
    }

    let reader = Cursor::new(code.as_bytes());
    let src = Source::from_reader(reader, Some(entry_path));
    let module = Module::parse(src, None, &mut ctx)
        .map_err(|err| BoaInitFail::Problem(boa_js_err_problem(file_name, &err)))?;
    let promise = module.load_link_evaluate(&mut ctx);
    if let Err(err) = ctx.run_jobs() {
        return Err(loader.take_external().map_or_else(
            || BoaInitFail::Problem(boa_js_err_problem(file_name, &err)),
            BoaInitFail::External,
        ));
    }
    if let Some(ext) = loader.take_external() {
        return Err(BoaInitFail::External(ext));
    }
    match promise.state() {
        PromiseState::Fulfilled(_) => Ok(ctx),
        PromiseState::Rejected(err) => {
            let je = JsError::from_opaque(err);
            Err(BoaInitFail::Problem(boa_js_err_problem(file_name, &je)))
        }
        PromiseState::Pending => Err(BoaInitFail::Problem(boa_problem(
            file_name,
            "Workflow module did not settle synchronously (unawaited top-level promise)."
                .to_owned(),
        ))),
    }
}
