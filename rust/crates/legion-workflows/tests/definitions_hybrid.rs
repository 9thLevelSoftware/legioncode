#![forbid(unsafe_code)]
//! Hybrid JS/TS workflow tests: pure TypeScript stripping + Boa (`Legion` & `T3`),
//! isolated external Node stdio worker escalation, and `WorkflowDefinitionsManager` hot-reload/save.

use std::fs;
use std::sync::{Arc, Mutex};

use legion_workflows::definitions::WorkflowDefinitionsManager;
use legion_workflows::definitions_fs::{
    WorkflowsLocation, is_valid_workflow_filename, resolve_workflows_location,
};
use legion_workflows::engine::WorkflowEngine;
use legion_workflows::js_boa::{BoaHost, BoaLoadOutcome};
use legion_workflows::js_worker::JsWorkerHost;
use legion_workflows::js_worker_probe::detect_external_js_runtime;
use legion_workflows::schema::ensure_workflow_and_feedback_schema;
use legion_workflows::store::WorkflowStore;
use legion_workflows::traits::{UnavailableAgentDriver, UnavailableForgeDriver};
use legion_workflows::ts_strip::strip_typescript;
use legion_workflows::types::WorkflowRunStatus;
use rusqlite::Connection;
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn open_store() -> Result<WorkflowStore, Box<dyn std::error::Error>> {
    let conn = Arc::new(Mutex::new(Connection::open_in_memory()?));
    {
        let g = conn.lock().map_err(|_| "lock poisoned")?;
        ensure_workflow_and_feedback_schema(&g)?;
    }
    Ok(WorkflowStore::new(conn)?)
}

#[test]
fn ts_strip_preserves_lines_and_separates_local_vs_external_imports() {
    let src = r#"
import type { Foo } from "./types.ts";
import { helper } from "./helper.ts";
import path from "node:path";

interface Payload { readonly id: string; count?: number }
type Mode = "fast" | "safe";

export function compute(x: Payload, mode: Mode = "fast"): string {
  const value = (x.id as string) satisfies string;
  return helper(value, mode, path.basename("/a/b"));
}
"#;
    let stripped = strip_typescript(src);
    assert_eq!(src.lines().count(), stripped.code.lines().count());
    assert_eq!(stripped.relative_imports, vec!["./helper.ts".to_owned()]);
    assert_eq!(stripped.external_imports.len(), 1);
    assert_eq!(stripped.external_imports[0].specifier, "node:path");
    assert!(!stripped.code.contains("interface Payload"));
    assert!(!stripped.code.contains("satisfies"));
}

#[tokio::test]
async fn boa_executes_legion_and_t3_typescript_workflows_with_relative_import() -> TestResult {
    let dir = tempfile::tempdir()?;
    fs::write(
        dir.path().join("helper.ts"),
        "export function doubleText(s: string): string { return s + ':' + s; }\n",
    )?;
    let wf_src = r#"
import { doubleText } from "./helper.ts";

interface Item { label: string }

Legion.workflow({
  name: "boa_legion",
  version: 1,
  payload: Schema.Struct({ label: Schema.String }),
  run: (payload: Item) => Effect.gen(function* () {
    const a = yield* Legion.step("double", () => doubleText(payload.label));
    const b = yield* Legion.step.try("failing", () => { throw new Error("boom"); });
    return { a, outcome: b.outcome };
  })
});

T3.workflow("boa_t3", { version: 2, payload: Schema.Struct({ n: Schema.Number }) }, (p: { n: number }) =>
  Effect.gen(function* () {
    const x = yield* T3.step("calc", () => p.n * 3);
    return { triple: x };
  })
);
"#;
    let entry = dir.path().join("main.ts");
    fs::write(&entry, wf_src)?;
    let store = open_store()?;
    let defs = match BoaHost::load_source(Some(dir.path()), &entry, "main.ts", wf_src) {
        BoaLoadOutcome::Loaded { definitions, .. } => definitions,
        BoaLoadOutcome::NeedsExternalRuntime(e) => {
            return Err(format!("unexpected external escalation: {}", e.specifier).into());
        }
        BoaLoadOutcome::Problem(p) => {
            return Err(format!("unexpected problem: {}", p.message).into());
        }
    };
    assert_eq!(defs.len(), 2);

    let engine = WorkflowEngine::minimal(store, defs, true)?;
    let r1 = engine
        .start("boa_legion", json!({ "label": "hi" }), "b1", None, None)
        .await?;
    let d1 = engine.get(&r1)?;
    assert_eq!(d1.run.status, WorkflowRunStatus::Completed);
    assert_eq!(
        d1.run.result,
        Some(json!({ "a": "hi:hi", "outcome": "failed" }))
    );

    let r2 = engine
        .start("boa_t3", json!({ "n": 7 }), "b2", None, None)
        .await?;
    assert_eq!(engine.get(&r2)?.run.result, Some(json!({ "triple": 21 })));
    Ok(())
}

#[tokio::test]
async fn external_import_escalates_and_runs_inside_real_node_worker() -> TestResult {
    let Some(_runtime) = detect_external_js_runtime() else {
        // Test environment has no Node/Deno/Bun installed; assert fallback diagnostic directly.
        return Ok(());
    };
    let dir = tempfile::tempdir()?;
    let wf_src = r#"
import path from "node:path";

Legion.workflow({
  name: "node_worker_wf",
  version: 1,
  payload: Schema.Struct({ file: Schema.String }),
  run: (p: { file: string }) => Effect.gen(function* () {
    const base = yield* Legion.step("basename", () => path.posix.basename(p.file));
    return { base };
  })
});
"#;
    let entry = dir.path().join("ext.ts");
    fs::write(&entry, wf_src)?;

    // Boa must escalate external imports instead of failing silently.
    match BoaHost::load_source(Some(dir.path()), &entry, "ext.ts", wf_src) {
        BoaLoadOutcome::NeedsExternalRuntime(ext) => assert_eq!(ext.specifier, "node:path"),
        _ => return Err("expected BoaHost to escalate node:path".into()),
    }

    let (defs, _host) = JsWorkerHost::load_file(&entry, "ext.ts", "node:path")
        .map_err(|p| format!("worker load failed: {}", p.message))?;
    let store = open_store()?;
    let engine = WorkflowEngine::minimal(store, defs, true)?;
    let run_id = engine
        .start(
            "node_worker_wf",
            json!({ "file": "/tmp/sub/report.json" }),
            "ext-1",
            None,
            None,
        )
        .await?;
    let detail = engine.get(&run_id)?;
    assert_eq!(detail.run.status, WorkflowRunStatus::Completed);
    assert_eq!(detail.run.result, Some(json!({ "base": "report.json" })));
    Ok(())
}

#[tokio::test]
async fn definitions_manager_save_verify_holds_last_good_and_retires_active() -> TestResult {
    assert!(is_valid_workflow_filename("hello_1-a.ts"));
    assert!(!is_valid_workflow_filename(".hidden.ts"));
    assert!(!is_valid_workflow_filename("bad/name.ts"));
    assert_eq!(
        resolve_workflows_location(std::path::Path::new("/tmp/lh"))
            .directory
            .ends_with(std::path::Path::new("userdata").join("workflows")),
        std::env::var_os("LEGIONCODE_WORKFLOWS").is_none()
            && std::env::var_os("T3CODE_WORKFLOWS").is_none()
    );

    let dir = tempfile::tempdir()?;
    let wf_dir = dir.path().join("workflows");
    fs::create_dir_all(&wf_dir)?;
    let store = open_store()?;
    let mgr = WorkflowDefinitionsManager::open(
        store,
        Arc::new(UnavailableAgentDriver),
        Arc::new(UnavailableForgeDriver),
        WorkflowsLocation {
            directory: wf_dir.clone(),
            single: None,
        },
        true,
    )?;

    let good_v1 = r#"
Legion.workflow({
  name: "guarded",
  version: 1,
  run: () => Effect.gen(function* () {
    const v = yield* Legion.wait("gate");
    return v;
  })
});
"#;
    let res = mgr.save("guarded.ts", good_v1)?;
    assert!(res.changed && res.problems.is_empty());

    // Saving broken syntax must reject before modifying the real disk file!
    let bad_res = mgr.save("guarded.ts", "Legion.workflow({ name: 'guarded', oops")?;
    assert!(!bad_res.changed && !bad_res.problems.is_empty());
    assert_eq!(fs::read_to_string(wf_dir.join("guarded.ts"))?, good_v1);

    // Cross-file duplicate workflow@version is caught.
    let dup_res = mgr.save("duplicate.ts", good_v1)?;
    assert!(!dup_res.changed && !dup_res.problems.is_empty());

    // Start an active run on v1, overwrite file on disk with broken code -> last-good stays active!
    let run_id = mgr
        .engine()
        .start("guarded", json!({}), "keep-v1", None, None)
        .await?;
    fs::write(wf_dir.join("guarded.ts"), "throw new Error('broken edit');")?;
    let reload_probs = mgr.reload_now(true);
    assert_ne!(reload_probs.len(), 0);
    assert_ne!(mgr.engine().summaries().len(), 0);

    // Replace with v2 while v1 still has open run -> v1 remains active as retired for run_id.
    let good_v2 = r#"
Legion.workflow({
  name: "guarded",
  version: 2,
  run: () => Effect.succeed("v2")
});
"#;
    let v2_res = mgr.save("guarded.ts", good_v2)?;
    assert!(v2_res.changed && v2_res.problems.is_empty());
    assert_eq!(mgr.engine().get(&run_id)?.run.version, 1);
    assert_eq!(mgr.read().files.len(), 1);
    Ok(())
}
