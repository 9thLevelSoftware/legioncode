//! Crashable server wrapper, M2/M3 card builders, thread-settlement, and CLI-import helpers.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::{self, JoinHandle};

use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::workflow_board_e2e::git_fixtures::{create_worktree, init_git_repo};
use crate::workflow_board_e2e::ws_rpc::OpaqueWsClient;
use crate::workflow_board_e2e::{TestResult, apply_isolated_env};

use super::helpers::{
    connect_ws_at, create_and_approve_m1_spec, create_project, next_id, probe_m3_execute_ship,
    start_m1_ws, ws_ok,
};
use crate::workflow_board_e2e::harness::OpaqueBoxHarness;

struct SpawnedProc {
    child: Child,
    origin: String,
    addr: SocketAddr,
    stdout_drain: Option<JoinHandle<String>>,
    stderr_drain: Option<JoinHandle<String>>,
}

impl SpawnedProc {
    fn spawn(
        home_dir: &Path,
        workspace_dir: &Path,
        static_dir: &Path,
        extra_env: &[(String, String)],
    ) -> TestResult<Self> {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_legioncode"));
        cmd.args([
            "serve",
            "--port",
            "0",
            "--host",
            "127.0.0.1",
            "--base-dir",
            &home_dir.to_string_lossy(),
            "--no-browser",
            "--static-dir",
            &static_dir.to_string_lossy(),
        ])
        .current_dir(workspace_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        apply_isolated_env(&mut cmd, home_dir);
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().ok_or("missing child stdout")?;
        let stderr = child.stderr.take().ok_or("missing child stderr")?;
        let stderr_drain = Some(thread::spawn(move || {
            let mut buf = String::new();
            let _ = BufReader::new(stderr).read_to_string(&mut buf);
            buf
        }));
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let mut origin = None;
        let mut has_pair = false;
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                break;
            }
            let trimmed = line.trim();
            if origin.is_none()
                && let Some(rest) = trimmed.strip_prefix("LegionCode server listening on ")
            {
                origin = Some(rest.trim().to_owned());
            }
            if trimmed.starts_with("pairingUrl:") {
                has_pair = true;
            }
            if origin.is_some() && has_pair {
                break;
            }
        }
        let Some(origin) = origin else {
            let _ = child.kill();
            let _ = child.wait();
            let err_tail = stderr_drain.and_then(|h| h.join().ok()).unwrap_or_default();
            return Err(format!("legioncode serve exited early; stderr:\n{err_tail}").into());
        };
        let addr_str = origin
            .strip_prefix("http://")
            .or_else(|| origin.strip_prefix("https://"))
            .ok_or_else(|| format!("unexpected origin format: {origin}"))?;
        let addr: SocketAddr = addr_str.parse()?;
        let stdout_drain = Some(thread::spawn(move || {
            let mut rest = String::new();
            let _ = reader.read_to_string(&mut rest);
            rest
        }));
        Ok(Self {
            child,
            origin,
            addr,
            stdout_drain,
            stderr_drain,
        })
    }
}

impl Drop for SpawnedProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(h) = self.stdout_drain.take() {
            let _ = h.join();
        }
        if let Some(h) = self.stderr_drain.take() {
            let _ = h.join();
        }
    }
}

/// Crash-capable server wrapper (`server` declared before `temp_dir` for Windows SQLite locks).
pub(super) struct CrashServer {
    server: Option<SpawnedProc>,
    home_dir: PathBuf,
    pub(super) workspace_dir: PathBuf,
    static_dir: PathBuf,
    extra_env: Vec<(String, String)>,
    pub(super) temp_dir: TempDir,
}

impl CrashServer {
    pub(super) fn start() -> TestResult<Self> {
        Self::start_with_env(&[])
    }

    pub(super) fn start_with_env(extra: &[(&str, &str)]) -> TestResult<Self> {
        let temp_dir = tempfile::tempdir()?;
        let (home_dir, workspace_dir, static_dir) = (
            temp_dir.path().join("home"),
            temp_dir.path().join("workspace"),
            temp_dir.path().join("static"),
        );
        fs::create_dir_all(&home_dir)?;
        fs::create_dir_all(&workspace_dir)?;
        fs::create_dir_all(&static_dir)?;
        init_git_repo(&workspace_dir)?;
        let extra_env: Vec<(String, String)> = extra
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        let server = SpawnedProc::spawn(&home_dir, &workspace_dir, &static_dir, &extra_env)?;
        Ok(Self {
            server: Some(server),
            home_dir,
            workspace_dir,
            static_dir,
            extra_env,
            temp_dir,
        })
    }

    pub(super) fn restart(&mut self) -> TestResult<()> {
        drop(self.server.take());
        self.server = Some(SpawnedProc::spawn(
            &self.home_dir,
            &self.workspace_dir,
            &self.static_dir,
            &self.extra_env,
        )?);
        Ok(())
    }

    pub(super) fn open_db_readonly(&self) -> TestResult<Connection> {
        let db_path = self.home_dir.join("userdata").join("statev2.sqlite");
        Ok(Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?)
    }

    pub(super) async fn probe_environment(&self) -> TestResult<Value> {
        let origin = &self.server.as_ref().ok_or("server stopped")?.origin;
        let resp = reqwest::Client::new()
            .get(format!("{origin}/.well-known/legioncode/environment"))
            .send()
            .await?;
        Ok(resp.json().await?)
    }

    pub(super) fn inject_torn_journal_and_restart(
        &mut self,
        card_dir: &Path,
        cid: &str,
        status: &str,
    ) -> TestResult<()> {
        let tmp = card_dir.join("spec.md.tmp");
        fs::write(&tmp, "orphaned partial write")?;
        drop(self.server.take());
        let conn = Connection::open(self.home_dir.join("userdata").join("statev2.sqlite"))?;
        conn.execute(
            "INSERT INTO workflow_board_write_journal (id, card_id, target_path, staging_path, operation, status, expected_digest, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 'write', ?5, 'sha256:bad', '2026-11-24T00:00:00Z', '2026-11-24T00:00:00Z')",
            rusqlite::params![next_id("wj"), cid, card_dir.join("spec.md").to_string_lossy().as_ref(), tmp.to_string_lossy().as_ref(), status],
        )?;
        drop(conn);
        self.restart()?;
        assert!(!tmp.exists(), "orphan .tmp must be cleaned on startup");
        Ok(())
    }

    pub(super) fn inject_interrupted_check_and_restart(&mut self, cid: &str) -> TestResult<()> {
        drop(self.server.take());
        let conn = Connection::open(self.home_dir.join("userdata").join("statev2.sqlite"))?;
        conn.execute(
            "INSERT INTO workflow_board_external_actions (id, card_id, action_kind, idempotency_key, status, created_at, updated_at) VALUES ('interrupted-check', ?1, 'planned_check', 'interrupted-check', 'interrupted', '2026-11-24T00:00:00Z', '2026-11-24T00:00:00Z')",
            [cid],
        )?;
        drop(conn);
        self.restart()
    }

    pub(super) async fn pair_and_connect(&self, label: &str) -> TestResult<OpaqueWsClient> {
        let srv = self.server.as_ref().ok_or("server stopped")?;
        let base_str = self.home_dir.to_string_lossy().into_owned();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_legioncode"));
        cmd.args(["pair", "--base-dir", &base_str, "--json", "--label", label])
            .current_dir(&self.workspace_dir);
        apply_isolated_env(&mut cmd, &self.home_dir);
        let parsed: Value = serde_json::from_slice(&cmd.output()?.stdout)?;
        let cred = parsed["token"].as_str().ok_or("missing pair token")?;
        let body: Value = reqwest::Client::new()
            .post(format!("{}/api/auth/pair", srv.origin))
            .json(&json!({ "token": cred, "clientLabel": label }))
            .send()
            .await?
            .json()
            .await?;
        let tok = body["accessToken"].as_str().ok_or("missing accessToken")?;
        connect_ws_at(srv.addr, tok).await
    }
}

pub(crate) type M3Ws = (
    OpaqueBoxHarness,
    String,
    OpaqueWsClient,
    PathBuf,
    String,
    i64,
);

pub(crate) async fn start_m3_ws(tag: &str, strategy: &str) -> TestResult<Option<M3Ws>> {
    let Some((h, tok, mut ws)) = start_m1_ws(tag).await? else {
        return Ok(None);
    };
    if !probe_m3_execute_ship(&mut ws, tag).await? {
        return Ok(None);
    }
    let (wt, cid, plan_rev) = setup_m3_card(&h, &mut ws, tag, strategy).await?;
    Ok(Some((h, tok, ws, wt, cid, plan_rev)))
}

pub(crate) async fn complete_m2_plan(
    ws: &mut OpaqueWsClient,
    cid: &str,
    rev: i64,
    include_ux: bool,
    strategy: &str,
    check_argv: Option<Vec<&str>>,
) -> TestResult<i64> {
    let argv: Vec<&str> = check_argv.unwrap_or_else(|| vec!["git", "status", "--porcelain"]);
    let mut plan_body: Value = serde_json::from_str(
        r#"{"lenses":[{"lens":"ux","applicable":false,"reason":"Non-UI"},{"lens":"functional","applicable":true,"summary":"Transitions"},{"lens":"architecture","applicable":true,"summary":"SQLite"},{"lens":"quality","applicable":true,"summary":"E2E"},{"lens":"infrastructure","applicable":true,"summary":"Worktrees"},{"lens":"delivery","applicable":true,"summary":"Ship"}],"alternatives":[{"id":"alt-1","title":"Ledger","summary":"Ledger","tradeoffs":"Audit"},{"id":"alt-2","title":"Inline","summary":"JSON","tradeoffs":"Small"}],"selectedAlternativeId":"alt-1","selectionRationale":"Ledger preserves audit trail.","granularity":"balanced","tasks":[{"taskId":"TSK-0001","title":"Core","dependsOn":[],"filesAllowed":["src/core.rs"],"filesForbidden":["src/secret.rs"],"testingMethod":"test-first"}]}"#,
    )?;
    if include_ux {
        plan_body["lenses"][0] = json!({ "lens": "ux", "applicable": true, "summary": "Board UX" });
    }
    if strategy == "risk-first" {
        plan_body["strategyMetadata"] =
            json!({ "uncertainty": "Verify risk probe", "probeTaskId": "TSK-0001" });
    }
    plan_body["cardId"] = json!(cid);
    plan_body["expectedRevision"] = json!(rev);
    plan_body["strategy"] = json!(strategy);
    plan_body["checks"] = json!([{ "id": "CHK-1", "criterionId": "AC-01", "argv": argv }]);
    plan_body["acMappings"] = json!([
        { "criterionId": "AC-01", "method": "automated", "checkIds": ["CHK-1"], "command": argv },
        { "criterionId": "AC-02", "method": "manual", "procedure": "Manual inspect" }
    ]);
    let updated = ws_ok(ws, "workflowBoard.updatePlan", plan_body).await?;
    let plan_rev = updated["revision"].as_i64().ok_or("missing plan_rev")?;
    let approved = ws_ok(
        ws,
        "workflowBoard.approvePlan",
        json!({ "cardId": cid, "expectedRevision": plan_rev }),
    )
    .await?;
    approved["revision"]
        .as_i64()
        .ok_or_else(|| "missing approved plan rev".into())
}

pub(crate) async fn setup_m3_card(
    h: &OpaqueBoxHarness,
    ws: &mut OpaqueWsClient,
    tag: &str,
    strategy: &str,
) -> TestResult<(PathBuf, String, i64)> {
    enable_workflows(ws).await?;
    let wt_root = h
        .workspace_dir()
        .parent()
        .ok_or("missing parent")?
        .join(format!("wt-{tag}"));
    create_worktree(h.workspace_dir(), &wt_root, &format!("feat/{tag}"))?;
    create_project(ws, &format!("proj-{tag}"), tag, h.workspace_dir()).await?;
    let (cid, spec_rev, _) =
        create_and_approve_m1_spec(ws, &format!("proj-{tag}"), &wt_root, tag).await?;
    let plan_rev = complete_m2_plan(ws, &cid, spec_rev, false, strategy, None).await?;
    Ok((wt_root, cid, plan_rev))
}

pub(crate) async fn await_thread_settled(
    ws: &mut OpaqueWsClient,
    thread_id: &str,
) -> TestResult<Value> {
    let sub_id = next_id("sub-thread");
    ws.subscribe(
        &sub_id,
        "orchestration.subscribeThread",
        json!({ "threadId": thread_id, "afterSequence": 0, "requestCompletionMarker": true, "acceptBoundedSnapshot": true }),
    )
    .await?;
    loop {
        let proj = ws_ok(
            ws,
            "orchestration.getThreadProjection",
            json!({ "threadId": thread_id }),
        )
        .await?;
        let settled = proj["runs"].as_array().is_some_and(|runs| {
            runs.iter().any(|r| {
                matches!(
                    r["status"].as_str(),
                    Some("completed" | "failed" | "interrupted")
                )
            })
        });
        if settled {
            ws.interrupt(&sub_id).await?;
            return Ok(proj);
        }
        let _ = ws.next_chunk_for(&sub_id).await?;
    }
}

pub(super) async fn enable_workflows(ws: &mut OpaqueWsClient) -> TestResult<()> {
    ws_ok(
        ws,
        "server.updateSettings",
        json!({ "patch": { "workflowsEnabled": true } }),
    )
    .await?;
    Ok(())
}

pub(super) async fn start_stage(
    ws: &mut OpaqueWsClient,
    cid: &str,
    rev: &Value,
    stage: &str,
    model: Option<Value>,
) -> TestResult<Value> {
    let mut request = json!({ "cardId": cid, "expectedRevision": rev, "stage": stage });
    if let Some(model) = model {
        request["modelSelection"] = model;
    }
    ws_ok(ws, "workflowBoard.startStage", request).await
}

pub(super) async fn settle_stage(ws: &mut OpaqueWsClient, started: &Value) -> TestResult<Value> {
    let cid = &started["cardId"];
    let run_id = started["stageRunId"].as_str().ok_or("missing stageRunId")?;
    let initial = ws_ok(ws, "workflowBoard.getCard", json!({ "cardId": cid })).await?;
    let sub = next_id("board-updates");
    ws.subscribe(
        &sub,
        "workflowBoard.subscribe",
        json!({ "projectId": initial["projectId"] }),
    )
    .await?;
    loop {
        ws.next_chunk_for(&sub).await?;
        let card = ws_ok(ws, "workflowBoard.getCard", json!({ "cardId": cid })).await?;
        let run = card["stageRuns"]
            .as_array()
            .ok_or("missing stageRuns")?
            .iter()
            .find(|run| run["id"] == run_id);
        if run.is_some_and(|run| {
            matches!(
                run["status"].as_str(),
                Some("completed" | "failed" | "cancelled")
            )
        }) {
            ws.interrupt(&sub).await?;
            return Ok(card);
        }
    }
}

pub(super) async fn start_stage_settled(
    ws: &mut OpaqueWsClient,
    cid: &str,
    rev: &Value,
    stage: &str,
    model: Option<Value>,
) -> TestResult<(Value, Value)> {
    let started = start_stage(ws, cid, rev, stage, model).await?;
    let card = settle_stage(ws, &started).await?;
    Ok((started, card))
}

pub(super) async fn run_check(ws: &mut OpaqueWsClient, card: &Value) -> TestResult<Value> {
    let response = ws_ok(
        ws,
        "workflowBoard.retryStep",
        json!({
            "cardId": card["cardId"],
            "expectedRevision": card["revision"],
            "stepKind": "check",
            "targetId": "CHK-1",
        }),
    )
    .await?;
    Ok(response["card"].clone())
}

pub(super) async fn install_provider(
    ws: &mut OpaqueWsClient,
    root: &Path,
    instance: &str,
    output: &str,
    before_result: &str,
) -> TestResult<Value> {
    fs::write(
        root.join(format!("{instance}.mjs")),
        format!(
            r#"
process.stdin.resume();
{before_result}
console.log(JSON.stringify({{ type: "system", subtype: "init", session_id: {session} }}));
console.log(JSON.stringify({{ type: "assistant", message: {{ id: "response", role: "assistant", content: [{{ type: "text", text: {output} }}], usage: {{ input_tokens: 10, output_tokens: 8 }} }} }}));
console.log(JSON.stringify({{ type: "result", subtype: "success", is_error: false, usage: {{ input_tokens: 10, output_tokens: 8 }} }}));
process.exit(0);
"#,
            session = json!(instance),
            output = json!(output),
        ),
    )?;
    #[cfg(windows)]
    let launcher = {
        let path = root.join(format!("{instance}.cmd"));
        fs::write(
            &path,
            format!("@echo off\r\nnode \"%~dp0{instance}.mjs\" %*\r\n"),
        )?;
        path
    };
    #[cfg(not(windows))]
    let launcher = {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join(instance);
        fs::write(
            &path,
            format!("#!/bin/sh\nexec node \"$(dirname \"$0\")/{instance}.mjs\" \"$@\"\n"),
        )?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        path
    };
    ws_ok(ws, "server.updateSettings", json!({
        "patch": { "providerInstances": {
            (instance): { "driver": "claudeAgent", "enabled": true, "config": { "binaryPath": launcher } }
        } }
    })).await?;
    Ok(json!({ "instanceId": instance, "model": "claude-sonnet-4-5" }))
}

pub(super) async fn install_reviewer(ws: &mut OpaqueWsClient, root: &Path) -> TestResult<Value> {
    install_provider(
        ws,
        root,
        "scenario-reviewer",
        r#"{"verdict":"pass","summary":"Clean independent review.","findings":[]}"#,
        "",
    )
    .await
}

pub(crate) async fn start_crash_m3_if_ready(
    extra: &[(&str, &str)],
    name: &str,
) -> TestResult<Option<(CrashServer, OpaqueWsClient)>> {
    let srv = CrashServer::start_with_env(extra)?;
    let env_desc = srv.probe_environment().await?;
    if env_desc["capabilities"]["projectWorkflowBoard"].as_bool() != Some(true) {
        eprintln!("SKIPPED {name}: capabilities.projectWorkflowBoard is false");
        return Ok(None);
    }
    let mut ws = srv.pair_and_connect(name).await?;
    if !probe_m3_execute_ship(&mut ws, name).await? {
        return Ok(None);
    }
    enable_workflows(&mut ws).await?;
    Ok(Some((srv, ws)))
}
