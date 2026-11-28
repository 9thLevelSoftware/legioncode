use legion_cli::serve::{RunningServer, ServeConfig, ServeMode, spawn_server};
use legion_persistence::auth_mutations::{IssueSessionInput, issue_scoped_session};
use legion_persistence::project_store::{
    CreateProjectInput, create_project, ensure_project_store_schema,
};
use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::types::{
    CreateCardInput, LinkStageRunThreadInput, UpsertStageRunInput,
};
use legion_persistence::workflow_board_repo::{
    create_card, link_stage_run_thread, upsert_or_get_stage_run_by_launch_key,
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

async fn mcp_post(
    http: &Client,
    origin: &str,
    tok: &str,
    extra_thread_header: Option<&str>,
    body: Value,
) -> Result<(u16, Value), Box<dyn std::error::Error>> {
    let mut req = http
        .post(format!("{origin}/mcp"))
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .bearer_auth(tok)
        .json(&body);
    if let Some(tid) = extra_thread_header {
        req = req.header("x-legioncode-thread-id", tid);
    }
    let resp = req.send().await?;
    let status = resp.status().as_u16();
    let val: Value = resp.json().await?;
    Ok((status, val))
}

fn seed_card_stage_thread(
    conn: &Connection,
    card_id: &str,
    sr_id: &str,
    stage: &str,
    thr_id: &str,
) -> TestResult {
    create_card(
        conn,
        &CreateCardInput {
            card_id,
            project_id: "proj-mcp-wb",
            revision_id: &format!("rev-{card_id}"),
            title: &format!("MCP {card_id}"),
            brief: "Brief",
            source_type: "idea",
            assistance_mode: "balanced",
            intake_goal: None,
            brief_path: None,
            brief_digest: None,
            input_paths_json: None,
            wireframes_digest: None,
            spec_json: "{\"acceptanceCriteria\":[{\"id\":\"AC-01\",\"text\":\"Initial\"}]}",
            spec_digest: Some("sha256:init"),
            workspace_strategy_json: "{\"type\":\"root\"}",
            worktree_path: None,
            branch: None,
            model_selection_json: None,
            runtime_mode: None,
            interaction_mode: None,
        },
        TS,
    )?;
    upsert_or_get_stage_run_by_launch_key(
        conn,
        &UpsertStageRunInput {
            id: sr_id,
            card_id,
            stage,
            task_id: None,
            attempt: 1,
            stable_launch_key: &format!("wb:{card_id}:{stage}:1"),
            card_revision: 1,
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
        conn,
        &LinkStageRunThreadInput {
            id: sr_id,
            status: "running",
            thread_id: Some(thr_id),
            workflow_run_id: None,
            worktree_path: None,
            branch: None,
            last_error: None,
            ended_at: None,
        },
        TS,
    )?;
    Ok(())
}

fn issue_mcp_tok(conn: &Connection, subject: &str) -> Result<String, Box<dyn std::error::Error>> {
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

async fn verify_tool_visibility_and_guardrails(
    http: &Client,
    origin: &str,
    non_thread_tok: &str,
    spec_tok: &str,
    ordinary_tok: &str,
) -> TestResult {
    let (_s, list_outside) = mcp_post(
        http,
        origin,
        non_thread_tok,
        None,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }),
    )
    .await?;
    let outside_names: Vec<&str> = list_outside["result"]["tools"]
        .as_array()
        .ok_or("tools")?
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(!outside_names.iter().any(|n| n.contains("workflow_board")));

    let (_s, list_thread) = mcp_post(
        http,
        origin,
        spec_tok,
        None,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} }),
    )
    .await?;
    let thread_names: Vec<&str> = list_thread["result"]["tools"]
        .as_array()
        .ok_or("tools")?
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(thread_names.contains(&"legion_workflow_board_read_brief"));
    assert!(thread_names.contains(&"legion_workflow_board_propose"));

    let (mismatch_status, _) = mcp_post(
        http,
        origin,
        spec_tok,
        Some("thr-mcp-plan"),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {} }),
    )
    .await?;
    assert_eq!(mismatch_status, 401);

    let (_s, non_stage_res) = mcp_post(http, origin, ordinary_tok, None, json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "legion_workflow_board_read_brief", "arguments": {} } })).await?;
    assert_eq!(non_stage_res["result"]["isError"], true);

    let (_s, cross_res) = mcp_post(http, origin, spec_tok, Some("thr-mcp-spec"), json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": { "name": "t3_workflow_board_read_brief", "arguments": { "cardId": "card-mcp-2" } } })).await?;
    assert_eq!(cross_res["result"]["isError"], true);
    Ok(())
}

#[tokio::test]
async fn mcp_board_tools_enforce_thread_scope_card_binding_and_no_approval_bypasses() -> TestResult
{
    let tmp = tempfile::tempdir()?;
    let repo_dir = tmp.path().join("repo-mcp-wb");
    std::fs::create_dir_all(&repo_dir)?;
    std::fs::write(repo_dir.join("README.md"), "# MCP Board Repo\n")?;
    let server = boot(&tmp.path().join("srv")).await?;

    let (non_thread_tok, spec_tok, plan_tok, ordinary_tok) = {
        let store = SqliteStore::open(server.paths().db_path.clone())?;
        ensure_project_store_schema(store.connection())?;
        create_project(
            store.connection(),
            &CreateProjectInput::new(&*repo_dir.to_string_lossy())
                .with_project_id("proj-mcp-wb")
                .with_title("MCP WB"),
        )?;
        seed_card_stage_thread(
            store.connection(),
            "card-mcp-1",
            "sr-mcp-spec",
            "spec",
            "thr-mcp-spec",
        )?;
        seed_card_stage_thread(
            store.connection(),
            "card-mcp-2",
            "sr-mcp-plan",
            "plan",
            "thr-mcp-plan",
        )?;
        (
            issue_mcp_tok(store.connection(), "outside-client")?,
            issue_mcp_tok(store.connection(), "thread:thr-mcp-spec")?,
            issue_mcp_tok(store.connection(), "thread:thr-mcp-plan")?,
            issue_mcp_tok(store.connection(), "thread:thr-ordinary")?,
        )
    };

    let http = Client::new();
    let origin = server.origin();
    verify_tool_visibility_and_guardrails(&http, origin, &non_thread_tok, &spec_tok, &ordinary_tok)
        .await?;

    let (_s, read_ok) = mcp_post(&http, origin, &spec_tok, None, json!({ "jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": { "name": "t3_workflow_board_read_brief", "arguments": {} } })).await?;
    assert_eq!(read_ok["result"]["isError"], false);
    assert_eq!(
        read_ok["result"]["structuredContent"]["cardId"],
        "card-mcp-1"
    );

    let (_s, prop_ok) = mcp_post(&http, origin, &spec_tok, None, json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": { "name": "legion_workflow_board_propose", "arguments": { "stage": "spec", "markdown": "Refined spec\n- AC-01: First criterion\n- AC-02: Second criterion" } }
    })).await?;
    assert_eq!(prop_ok["result"]["isError"], false);
    assert!(prop_ok["result"]["structuredContent"]["card"]["specApproval"].is_null());

    for bad_args in [
        json!({ "stage": "ship", "markdown": "bypass" }),
        json!({ "stage": "spec", "approve": true }),
        json!({ "stage": "spec", "action": "approveSpec" }),
        json!({ "stage": "spec", "approvePlan": true }),
        json!({ "stage": "spec", "recordManualAcceptance": true }),
        json!({ "stage": "spec", "resolveFinding": true }),
        json!({ "stage": "spec", "returnForRevision": true }),
        json!({ "stage": "spec", "kind": "challenge", "action": "complete" }),
        json!({ "stage": "spec", "kind": "challenge", "action": "resolve", "concerns": [] }),
        json!({ "stage": "spec", "kind": "challenge", "action": "manualFallback", "acknowledgementToken": "I acknowledge", "answers": ["a", "b", "c"] }),
    ] {
        let (_s, bad_res) = mcp_post(&http, origin, &spec_tok, None, json!({ "jsonrpc": "2.0", "id": 8, "method": "tools/call", "params": { "name": "legion_workflow_board_propose", "arguments": bad_args } })).await?;
        assert_eq!(bad_res["result"]["isError"], true);
    }

    let (_s, spec_exp) = mcp_post(&http, origin, &spec_tok, None, json!({
        "jsonrpc": "2.0", "id": 9, "method": "tools/call",
        "params": { "name": "t3_workflow_board_propose", "arguments": { "stage": "spec", "kind": "explore", "action": "step" } }
    })).await?;
    assert_eq!(spec_exp["result"]["isError"], false);

    let (_s, spec_chal) = mcp_post(&http, origin, &spec_tok, None, json!({
        "jsonrpc": "2.0", "id": 10, "method": "tools/call",
        "params": { "name": "legion_workflow_board_propose", "arguments": {
            "stage": "spec", "kind": "challenge", "action": "start",
            "concerns": [{ "id": "CON-1", "summary": "Verify spec edge case", "disposition": "unresolved" }]
        } }
    })).await?;
    assert_eq!(spec_chal["result"]["isError"], false);

    let (_s, plan_exp_err) = mcp_post(&http, origin, &plan_tok, None, json!({
        "jsonrpc": "2.0", "id": 11, "method": "tools/call",
        "params": { "name": "t3_workflow_board_propose", "arguments": { "stage": "plan", "kind": "explore", "action": "step" } }
    })).await?;
    assert_eq!(plan_exp_err["result"]["isError"], true);

    let (_s, accepted_plan) = mcp_post(&http, origin, &plan_tok, None, json!({
        "jsonrpc": "2.0", "id": 12, "method": "tools/call",
        "params": { "name": "t3_workflow_board_propose", "arguments": { "stage": "plan", "kind": "plan", "granularity": "balanced" } }
    })).await?;
    assert_eq!(accepted_plan["result"]["isError"], false);

    let (_s, plan_bad) = mcp_post(&http, origin, &plan_tok, None, json!({
        "jsonrpc": "2.0", "id": 13, "method": "tools/call",
        "params": { "name": "t3_workflow_board_propose", "arguments": { "stage": "plan", "kind": "challenge", "action": "manualFallback", "acknowledgementToken": "I acknowledge", "answers": ["a", "b", "c"] } }
    })).await?;
    assert_eq!(plan_bad["result"]["isError"], true);

    server.shutdown().await?;
    Ok(())
}
