//! Shared helpers, WS/CLI setup, and artifact/git assertions for E2E-M2 Rust tests.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::process::Command;

use legion_protocol::rpc_codec::RpcExit;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::workflow_board_e2e::TestResult;
use crate::workflow_board_e2e::harness::OpaqueBoxHarness;
use crate::workflow_board_e2e::ws_rpc::OpaqueWsClient;

pub struct PlanningEnv {
    pub ws: OpaqueWsClient,
    pub harness: OpaqueBoxHarness,
    pub project_id: String,
    pub board_supported: bool,
}

impl PlanningEnv {
    pub async fn start(seed_git: bool) -> TestResult<Self> {
        let harness = OpaqueBoxHarness::start(seed_git)?;
        let env_desc = harness.probe_environment().await?;
        let board_supported = env_desc["capabilities"]["projectWorkflowBoard"]
            .as_bool()
            .unwrap_or(false);
        let token = harness.pair_client("e2e-planning-admin", None).await?;
        let auth_header = format!("Bearer {token}");
        let mut ws = OpaqueWsClient::connect(
            harness.addr()?,
            "/ws?orchestrationProtocol=2&frameParts=1",
            &[("Authorization", &auth_header)],
        )
        .await?;

        let project_id = "proj-rust-e2e".to_owned();
        let ws_root = harness.workspace_dir().to_string_lossy().into_owned();
        ws.call_ok(
            "create-proj",
            "orchestration.v2.dispatchCommand",
            json!({
                "type": "project.create",
                "commandId": "cmd-proj-rust-e2e",
                "projectId": project_id,
                "title": "Rust Planning E2E",
                "workspaceRoot": ws_root,
            }),
        )
        .await?;

        Ok(Self {
            ws,
            harness,
            project_id,
            board_supported,
        })
    }

    pub async fn reconnect_ws(&mut self, label: &str, scopes: Option<&[&str]>) -> TestResult<()> {
        let token = self.harness.pair_client(label, scopes).await?;
        let auth_header = format!("Bearer {token}");
        self.ws = OpaqueWsClient::connect(
            self.harness.addr()?,
            "/ws?orchestrationProtocol=2&frameParts=1",
            &[("Authorization", &auth_header)],
        )
        .await?;
        Ok(())
    }
}

pub fn check_board_capable(env: &PlanningEnv, test_id: &str) -> bool {
    if env.board_supported {
        return true;
    }
    eprintln!("SKIPPED {test_id}: capabilities.projectWorkflowBoard is not true");
    false
}

pub fn assert_board_capable(env: &PlanningEnv, test_id: &str) {
    assert!(
        env.board_supported,
        "{test_id}: capabilities.projectWorkflowBoard must be true"
    );
}

pub async fn call_expect_failure(
    ws: &mut OpaqueWsClient,
    id: &str,
    tag: &str,
    payload: Value,
) -> TestResult<Value> {
    match ws.call_unary(id, tag, payload).await? {
        RpcExit::Failure { cause } => Ok(serde_json::to_value(cause)?),
        RpcExit::Success { value } => {
            Err(format!("expected {tag} to fail, but succeeded with {value}").into())
        }
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub fn assert_git_zero_autocommits(repo_dir: &Path, expected_head: &str) -> TestResult<()> {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_dir)
        .output()?;
    if !out.status.success() {
        return Err("git rev-parse HEAD failed".into());
    }
    let head = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    assert_eq!(head, expected_head, "authoring must never auto-commit");
    Ok(())
}

pub fn git_head_sha(repo_dir: &Path) -> TestResult<String> {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_dir)
        .output()?;
    if !out.status.success() {
        return Err("git rev-parse HEAD failed".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

pub fn card_dir(workspace_dir: &Path, card_id: &str) -> std::path::PathBuf {
    workspace_dir
        .join(".legion")
        .join("workflows")
        .join(card_id)
}

pub fn valid_lenses_json() -> Value {
    json!([
        { "lens": "ux", "applicable": true, "summary": "Board split layout." },
        { "lens": "functional", "applicable": true, "summary": "Card lifecycle." },
        { "lens": "architecture", "applicable": true, "summary": "SQLite + journal." },
        { "lens": "quality", "applicable": true, "summary": "Category-partition tests." },
        { "lens": "infrastructure", "applicable": true, "summary": "Isolated tempdir." },
        { "lens": "delivery", "applicable": true, "summary": "Gated milestones." }
    ])
}

pub fn valid_alternatives_json() -> Value {
    json!([
        {
            "id": "alt-a",
            "title": "Transactional ledger",
            "summary": "Revision ledger in SQLite.",
            "tradeoffs": "Strong history auditability."
        },
        {
            "id": "alt-b",
            "title": "Snapshot + sidecar log",
            "summary": "Single row with external journal.",
            "tradeoffs": "Simpler reads, extra recovery step."
        }
    ])
}

pub fn valid_tasks_json() -> Value {
    json!([
        {
            "taskId": "TSK-0001",
            "title": "Core domain",
            "dependsOn": [],
            "filesAllowed": ["src/domain.rs"],
            "filesForbidden": ["src/secret.rs"],
            "testingMethod": "test-first"
        },
        {
            "taskId": "TSK-0002",
            "title": "RPC handler",
            "dependsOn": ["TSK-0001"],
            "filesAllowed": ["src/rpc.rs"],
            "filesForbidden": ["src/domain.rs"],
            "testingMethod": "regression-first"
        }
    ])
}

pub fn valid_ac_mappings_json() -> Value {
    json!([
        {
            "criterionId": "AC-01",
            "method": "automated",
            "command": ["cargo", "test", "--lib"]
        },
        {
            "criterionId": "AC-02",
            "method": "manual",
            "procedure": "Inspect card history badge in UI."
        }
    ])
}

pub async fn create_and_approve_spec(
    ws: &mut OpaqueWsClient,
    project_id: &str,
    title: &str,
    with_wireframes: bool,
) -> TestResult<(String, i64)> {
    let created = ws
        .call_ok(
            "c-card",
            "workflowBoard.createCard",
            json!({
                "projectId": project_id,
                "title": title,
                "brief": format!("Deliver {title} safely."),
                "sourceType": "idea",
                "assistanceMode": "balanced",
            }),
        )
        .await?;
    let card_id = created["cardId"]
        .as_str()
        .ok_or("missing cardId")?
        .to_owned();
    let rev1 = created["revision"].as_i64().ok_or("missing revision")?;

    let updated = ws
        .call_ok(
            "u-spec",
            "workflowBoard.updateSpec",
            json!({
                "cardId": card_id,
                "expectedRevision": rev1,
                "acceptanceCriteria": [
                    { "id": "AC-01", "text": "Domain validates cleanly." },
                    { "id": "AC-02", "text": "State survives restart." }
                ],
                "constraints": ["Zero auto-commits"],
                "sourceRefs": ["README.md"],
                "wireframesIndex": if with_wireframes { Value::String("wireframes/index.html".to_owned()) } else { Value::Null },
            }),
        )
        .await?;
    let rev2 = updated["revision"].as_i64().ok_or("missing rev2")?;

    let challenged = ws
        .call_ok(
            "c-chal",
            "workflowBoard.challengeStep",
            json!({
                "cardId": card_id,
                "expectedRevision": rev2,
                "action": "complete",
                "concerns": []
            }),
        )
        .await?;
    let rev3 = challenged["revision"].as_i64().ok_or("missing rev3")?;

    let approved = ws
        .call_ok(
            "a-spec",
            "workflowBoard.approveSpec",
            json!({
                "cardId": card_id,
                "expectedRevision": rev3
            }),
        )
        .await?;
    let rev4 = approved["revision"].as_i64().ok_or("missing rev4")?;
    Ok((card_id, rev4))
}

pub async fn populate_and_approve_plan(
    ws: &mut OpaqueWsClient,
    card_id: &str,
    rev: i64,
) -> TestResult<i64> {
    let plan_upd = ws
        .call_ok(
            "u-plan",
            "workflowBoard.updatePlan",
            json!({
                "cardId": card_id,
                "expectedRevision": rev,
                "lenses": valid_lenses_json(),
                "alternatives": valid_alternatives_json(),
                "selectedAlternativeId": "alt-a",
                "selectionRationale": "Transactional ledger",
                "strategy": "outcomes",
                "granularity": "balanced",
                "tasks": valid_tasks_json(),
                "acMappings": valid_ac_mappings_json(),
            }),
        )
        .await?;
    let rev_upd = plan_upd["revision"].as_i64().ok_or("missing rev_upd")?;
    let plan_app = ws
        .call_ok(
            "a-plan",
            "workflowBoard.approvePlan",
            json!({ "cardId": card_id, "expectedRevision": rev_upd }),
        )
        .await?;
    let rev_app = plan_app["revision"].as_i64().ok_or("missing rev_app")?;
    Ok(rev_app)
}

pub fn write_inert_file(path: &Path, content: &str) -> TestResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(())
}
