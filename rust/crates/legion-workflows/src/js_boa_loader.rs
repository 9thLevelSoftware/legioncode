#![forbid(unsafe_code)]
//! TypeScript-stripping Boa [`ModuleLoader`] and diagnostic helpers.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use boa_engine::module::{ModuleLoader, Referrer, resolve_module_specifier};
use boa_engine::{
    Context, JsError, JsNativeError, JsResult, JsValue, Module, Source, js_error, js_string,
};
use serde_json::Value;

use crate::js_boa::BoaCmd;
use crate::js_specs::JsWorkflowSpec;
use crate::ts_strip::{ExternalImport, ExternalImportKind, strip_typescript};
use crate::types::WorkflowDefinitionProblem;

pub(crate) struct TsModuleLoader {
    root: PathBuf,
    cache: RefCell<HashMap<PathBuf, Module>>,
    external: RefCell<Option<ExternalImport>>,
}

impl TsModuleLoader {
    pub(crate) fn new(root: &Path) -> Self {
        Self {
            root: canonical_module_path(root),
            cache: RefCell::new(HashMap::new()),
            external: RefCell::new(None),
        }
    }

    pub(crate) fn take_external(&self) -> Option<ExternalImport> {
        self.external.borrow_mut().take()
    }
}

pub(crate) fn canonical_module_path(path: &Path) -> PathBuf {
    if let Ok(canon) = path.canonicalize() {
        return canon;
    }
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name())
        && let Ok(canon_parent) = parent.canonicalize()
    {
        return canon_parent.join(name);
    }
    path.to_path_buf()
}

impl ModuleLoader for TsModuleLoader {
    fn load_imported_module(
        self: Rc<Self>,
        referrer: Referrer,
        request: boa_engine::module::ModuleRequest,
        context: &RefCell<&mut Context>,
    ) -> impl Future<Output = JsResult<Module>> {
        let spec_str = request.specifier().to_std_string_escaped();
        let res = (|| {
            if !spec_str.starts_with("./") && !spec_str.starts_with("../") {
                *self.external.borrow_mut() = Some(ExternalImport {
                    specifier: spec_str.clone(),
                    line: 1,
                    kind: ExternalImportKind::Static,
                });
                return Err(
                    js_error!(TypeError: "External package import requires JS runtime worker"),
                );
            }
            let resolved = resolve_module_specifier(
                Some(&self.root),
                request.specifier(),
                referrer.path(),
                &mut context.borrow_mut(),
            )?;
            let actual = resolve_ts_candidate(&resolved)
                .ok_or_else(|| js_error!(TypeError: "Cannot resolve relative module specifier"))?;
            if let Some(cached) = self.cache.borrow().get(&actual) {
                return Ok(cached.clone());
            }
            let raw = std::fs::read_to_string(&actual).map_err(|e| {
                JsError::from(
                    JsNativeError::error()
                        .with_message(format!("Failed to read module {}: {e}", actual.display())),
                )
            })?;
            let stripped = strip_typescript(&raw);
            if let Some(ext) = stripped.external_imports.first() {
                *self.external.borrow_mut() = Some(ext.clone());
                return Err(
                    js_error!(TypeError: "Transitive external import requires JS runtime worker"),
                );
            }
            let reader = Cursor::new(stripped.code.as_bytes());
            let src = Source::from_reader(reader, Some(actual.as_path()));
            let module = Module::parse(src, None, &mut context.borrow_mut())?;
            self.cache.borrow_mut().insert(actual, module.clone());
            Ok(module)
        })();
        async move { res }
    }
}

fn resolve_ts_candidate(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    if path.extension().and_then(|e| e.to_str()) == Some("js") {
        for ext in ["ts", "mts"] {
            let p = path.with_extension(ext);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    for ext in ["ts", "mts", "js", "mjs"] {
        let mut p = path.as_os_str().to_os_string();
        p.push(".");
        p.push(ext);
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    None
}

pub(crate) fn boa_problem(file_name: &str, message: String) -> WorkflowDefinitionProblem {
    WorkflowDefinitionProblem {
        message,
        file: Some(file_name.to_owned()),
        line: None,
    }
}

pub(crate) fn boa_js_err_problem(file_name: &str, err: &JsError) -> WorkflowDefinitionProblem {
    let msg = err.to_string();
    WorkflowDefinitionProblem {
        line: parse_line_number(&msg),
        message: msg,
        file: Some(file_name.to_owned()),
    }
}

fn parse_line_number(msg: &str) -> Option<i64> {
    for pat in ["line ", ":"] {
        if let Some(pos) = msg.find(pat) {
            let tail = &msg[pos + pat.len()..];
            let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
            if !digits.is_empty()
                && let Ok(n) = digits.parse::<i64>()
                && n > 0
            {
                return Some(n);
            }
        }
    }
    None
}

pub(crate) fn dispatch_boa_cmd(ctx: &mut Context, cmd: BoaCmd) {
    match cmd {
        BoaCmd::DrivePass {
            wf_key,
            input,
            journal,
            target_step,
            step_ops,
            run_id,
            reply,
        } => {
            let target = target_step.map_or(Value::Null, Value::String);
            let res = call_global_json(
                ctx,
                "__legionDrivePass",
                &[
                    Value::String(wf_key),
                    input,
                    journal,
                    target,
                    step_ops,
                    Value::String(run_id),
                ],
            )
            .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()));
            let _ = reply.send(res);
        }
        BoaCmd::DriveOnEnd {
            wf_key,
            input,
            end_info,
            step_ops,
            reply,
        } => {
            let res = call_global_json(
                ctx,
                "__legionDriveOnEnd",
                &[Value::String(wf_key), input, end_info, step_ops],
            )
            .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()));
            let _ = reply.send(res);
        }
        BoaCmd::CallWebhookKey { idx, req, reply } => {
            let res = call_global_json(ctx, "__legionCallWebhookKey", &[Value::from(idx), req])
                .map(|v| v.as_str().map(str::to_owned));
            let _ = reply.send(res);
        }
        BoaCmd::CheckWaitCodec { idx, val, reply } => {
            let res = call_global_json(ctx, "__legionCheckWaitCodec", &[Value::from(idx), val])
                .and_then(|v| {
                    if v.get("ok").and_then(Value::as_bool) == Some(true) {
                        Ok(v.get("value").cloned().unwrap_or(Value::Null))
                    } else {
                        Err(v
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("Codec validation failed")
                            .to_owned())
                    }
                });
            let _ = reply.send(res);
        }
    }
}

pub(crate) fn extract_specs(ctx: &mut Context) -> Result<Vec<JsWorkflowSpec>, String> {
    let val = call_global_json(ctx, "__legionListWorkflowSpecs", &[])?;
    serde_json::from_value(val).map_err(|e| format!("Failed to decode workflow specs: {e}"))
}

fn call_global_json(ctx: &mut Context, fn_name: &str, args: &[Value]) -> Result<Value, String> {
    let global = ctx.global_object();
    let func_val = global
        .get(js_string!(fn_name), ctx)
        .map_err(|e| e.to_string())?;
    let func = func_val
        .as_callable()
        .ok_or_else(|| format!("Missing global helper '{fn_name}'"))?;
    let mut js_args = Vec::with_capacity(args.len());
    for arg in args {
        let jv = JsValue::from_json(arg, ctx).map_err(|e| e.to_string())?;
        js_args.push(jv);
    }
    let out = func
        .call(&JsValue::undefined(), &js_args, ctx)
        .map_err(|e| e.to_string())?;
    Ok(out
        .to_json(ctx)
        .map_err(|e| e.to_string())?
        .unwrap_or(Value::Null))
}
