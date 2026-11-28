use super::mcp_m3_seed::seed_execute_card;
use legion_cli::serve::{RunningServer, ServeConfig, ServeMode, spawn_server};
use legion_persistence::auth_mutations::{IssueSessionInput, issue_scoped_session};
use legion_persistence::project_store::{
    CreateProjectInput, create_project, ensure_project_store_schema,
};
use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::types::{
    LinkStageRunThreadInput, UpsertStageRunInput,
};
use legion_persistence::workflow_board_repo::{
    get_card_execution_detail, link_stage_run_thread, upsert_or_get_stage_run_by_launch_key,
};
use reqwest::Client;
use rusqlite::Connection;
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;
const TS: &str = "2026-11-24T12:00:00Z";

async fn boot(base: &std::path::Path) -> Result<RunningServer, Box<dyn std::error::Error>> {
    spawn_server(ServeConfig {
        base_dir: base.to_path_buf(),
        base_dir_is_explicit: true,
        port: 0,
        host: Some("127.0.0.1".to_owned()),
        mode: ServeMode::Web,
        no_browser: true,
        lan: false,
        static_dir: None,
        cwd: base.to_path_buf(),
    })
    .await
    .map_err(Into::into)
}

async fn mcp_propose(
    http: &Client,
    origin: &str,
    tok: &str,
    id: u64,
    arguments: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let body = json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": "legion_workflow_board_propose", "arguments": arguments }
    });
    let resp = http
        .post(format!("{origin}/mcp"))
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .bearer_auth(tok)
        .json(&body)
        .send()
        .await?;
    Ok(resp.json().await?)
}

fn issue_tok(conn: &Connection, subject: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(issue_scoped_session(
        conn,
        IssueSessionInput {
            subject: Some(subject.to_owned()),
            scopes: vec!["orchestration:operate".to_owned()],
            ..IssueSessionInput::default()
        },
    )?
    .token)
}

fn init_m3_git_repo(repo_dir: &std::path::Path) -> TestResult {
    std::fs::create_dir_all(repo_dir.join("src"))?;
    std::fs::write(repo_dir.join("README.md"), "# M3 MCP Repo\n")?;
    std::fs::write(repo_dir.join("src/allowed.txt"), "base\n")?;
    for args in [
        vec!["init"],
        vec!["config", "user.email", "test@legion.local"],
        vec!["config", "user.name", "Legion Test"],
        vec!["add", "."],
        vec!["commit", "-m", "init"],
    ] {
        let st = std::process::Command::new("git")
            .current_dir(repo_dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(args)
            .status()?;
        assert!(st.success());
    }
    Ok(())
}

fn seed_m3_mcp_server(
    server: &RunningServer,
    repo_dir: &std::path::Path,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let store = SqliteStore::open(server.paths().db_path.clone())?;
    ensure_project_store_schema(store.connection())?;
    create_project(
        store.connection(),
        &CreateProjectInput::new(&*repo_dir.to_string_lossy())
            .with_project_id("proj-mcp-m3")
            .with_title("MCP M3"),
    )?;
    seed_execute_card(store.connection(), "card-mcp-m3")?;
    upsert_or_get_stage_run_by_launch_key(
        store.connection(),
        &UpsertStageRunInput {
            id: "sr-mcp-rev",
            card_id: "card-mcp-m3",
            stage: "review",
            task_id: None,
            attempt: 1,
            stable_launch_key: "wb:mcp-m3:rev:1",
            card_revision: 5,
            provider_instance_id: "devReplay",
            model: "dev-replay",
            model_options_json: None,
            runtime_mode: "full-access",
            interaction_mode: "default",
            workspace_strategy_json: "{\"type\":\"root\"}",
            worktree_path: None,
            branch: None,
            brief_digest: None,
        },
        TS,
    )?;
    link_stage_run_thread(
        store.connection(),
        &LinkStageRunThreadInput {
            id: "sr-mcp-rev",
            status: "running",
            thread_id: Some("thr-mcp-rev"),
            workflow_run_id: None,
            worktree_path: None,
            branch: None,
            last_error: None,
            ended_at: None,
        },
        TS,
    )?;
    Ok((
        issue_tok(store.connection(), "thread:thr-mcp-exec")?,
        issue_tok(store.connection(), "thread:thr-mcp-rev")?,
    ))
}

async fn assert_colliding_review_round(
    db_path: &std::path::Path,
    http: &Client,
    origin: &str,
    attempt: u64,
    card_revision: u64,
    findings: serde_json::Value,
    summary: &str,
) -> TestResult {
    let store = SqliteStore::open(db_path)?;
    let sr_id = format!("sr-mcp-rev-{attempt}");
    let thr_id = format!("thr-mcp-rev-{attempt}");
    let launch_key = format!("wb:mcp-m3:rev:{attempt}");
    upsert_or_get_stage_run_by_launch_key(
        store.connection(),
        &UpsertStageRunInput {
            id: &sr_id,
            card_id: "card-mcp-m3",
            stage: "review",
            task_id: None,
            attempt: i64::try_from(attempt)?,
            stable_launch_key: &launch_key,
            card_revision: i64::try_from(card_revision)?,
            provider_instance_id: "devReplay",
            model: "dev-replay",
            model_options_json: None,
            runtime_mode: "full-access",
            interaction_mode: "default",
            workspace_strategy_json: "{\"type\":\"root\"}",
            worktree_path: None,
            branch: None,
            brief_digest: None,
        },
        TS,
    )?;
    link_stage_run_thread(
        store.connection(),
        &LinkStageRunThreadInput {
            id: &sr_id,
            status: "running",
            thread_id: Some(&thr_id),
            workflow_run_id: None,
            worktree_path: None,
            branch: None,
            last_error: None,
            ended_at: None,
        },
        TS,
    )?;
    let tok = issue_tok(store.connection(), &format!("thread:{thr_id}"))?;
    let round_args = json!({
        "stage": "review",
        "kind": "review",
        "verdict": "findings",
        "summary": summary,
        "findings": findings
    });
    let outcome = mcp_propose(http, origin, &tok, attempt + 3, round_args).await?;
    assert_eq!(outcome["result"]["isError"], false, "{outcome}");
    Ok(())
}

#[tokio::test]
async fn mcp_m3_execute_check_review_and_human_gate_guards() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let repo_dir = tmp.path().join("repo-mcp-m3");
    init_m3_git_repo(&repo_dir)?;
    let server = boot(&tmp.path().join("srv")).await?;
    let (exec_tok, rev_tok) = seed_m3_mcp_server(&server, &repo_dir)?;
    let http = Client::new();
    let origin = server.origin();

    // Scoped execute check runs argv-only check and records receipt.
    let chk_args =
        json!({ "stage": "execute", "kind": "check", "checkId": "chk-ok", "expectedRevision": 5 });
    let chk_ok = mcp_propose(&http, origin, &exec_tok, 1, chk_args).await?;
    assert_eq!(chk_ok["result"]["isError"], false, "{chk_ok}");
    assert_eq!(
        chk_ok["result"]["structuredContent"]["receipt"]["status"],
        "passed"
    );

    // Human-gate bypasses are strictly blocked for execute & review agents.
    for bad_args in [
        json!({ "stage": "execute", "resolveContractViolation": true }),
        json!({ "stage": "execute", "proposeAmendment": true }),
        json!({ "stage": "execute", "reconcileInterruptedAction": true }),
        json!({ "stage": "execute", "action": "approveDelivery" }),
        json!({ "stage": "review", "kind": "review", "verdict": "findings", "findings": [{ "id": "f-1", "severity": "minor", "message": "Fix", "disposition": "fix_applied" }] }),
        json!({ "stage": "review", "kind": "review", "verdict": "findings", "findings": [{ "id": "f-1", "severity": "minor", "message": "Fix", "resolvedBy": "agent" }] }),
    ] {
        let bad = mcp_propose(&http, origin, &rev_tok, 2, bad_args).await?;
        assert_eq!(bad["result"]["isError"], true);
    }

    // Scoped execute task completion detects unexpected edit, blocks card, and preserves file on disk.
    std::fs::write(repo_dir.join("src/allowed.txt"), "edited\n")?;
    std::fs::write(repo_dir.join("src/unexpected.txt"), "keep-me\n")?;
    let exec_args = json!({ "stage": "execute", "kind": "execute", "taskId": "TSK-1", "outcomeStatus": "completed" });
    let exec_done = mcp_propose(&http, origin, &exec_tok, 3, exec_args).await?;
    assert_eq!(exec_done["result"]["isError"], false, "{exec_done}");
    assert_eq!(
        exec_done["result"]["structuredContent"]["status"],
        "blocked"
    );
    assert_eq!(
        std::fs::read_to_string(repo_dir.join("src/unexpected.txt"))?,
        "keep-me\n"
    );

    // Scoped review proposal records findings with disposition = "unresolved".
    let rev_args = json!({
        "stage": "review", "kind": "review", "verdict": "findings",
        "summary": "Found one minor gap",
        "findings": [{ "id": "f-1", "severity": "minor", "criterionId": "AC-01", "file": "src/allowed.txt", "line": 1, "message": "Missing edge test" }]
    });
    let first_review = mcp_propose(&http, origin, &rev_tok, 4, rev_args).await?;
    assert_eq!(first_review["result"]["isError"], false, "{first_review}");
    let store = SqliteStore::open(server.paths().db_path.clone())?;
    let detail = get_card_execution_detail(store.connection(), "card-mcp-m3")?;
    assert_eq!(detail.review_findings.len(), 1);
    assert_eq!(detail.review_findings[0].disposition, "unresolved");
    let base_rev = u64::try_from(detail.planning.card_detail.card.revision)?;

    // Second & third review rounds with omitted finding `id` AND colliding `"fnd-1"` must not fail PK constraint.
    assert_colliding_review_round(
        &server.paths().db_path,
        &http,
        origin,
        2,
        base_rev,
        json!([
            { "severity": "blocking", "message": "Omitted id 1" },
            { "id": "fnd-1", "severity": "warning", "message": "Colliding id" }
        ]),
        "Round 2 with omitted and colliding IDs",
    )
    .await?;
    assert_colliding_review_round(
        &server.paths().db_path,
        &http,
        origin,
        3,
        base_rev + 1,
        json!([
            { "severity": "note", "message": "Omitted id 2" },
            { "id": "fnd-1", "severity": "minor", "message": "Repeated fnd-1" }
        ]),
        "Round 3 repeating colliding and omitted IDs",
    )
    .await?;
    let after = get_card_execution_detail(store.connection(), "card-mcp-m3")?;
    assert_eq!(after.review_findings.len(), 5);
    server.shutdown().await?;
    Ok(())
}
