//! F13: Worktree-aware thread launch & stable launch identity (5 Tier 1 + 5 Tier 2).

use std::fs;

use serde_json::{Value, json};

use super::support::{CrashableHarness, assert_zero_threads, next_id};
use super::support_board::{
    assert_m3_execute_ship, await_thread_settled, create_project, expect_rpc_err_tag,
    launch_replay_wt, launch_wt_payload, require_m3_execute_ship, setup_approved_plan_card,
};
use crate::workflow_board_e2e::TestResult;
use crate::workflow_board_e2e::git_fixtures::create_worktree;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f13_t1_1_launch_thread_worktree_strategy_creates_isolated_worktree() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let tok = h.pair_client("f13-t1-1", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f13-1", "F13 WT", h.workspace_dir()).await?;

    let launched = ws
        .call_ok(
            &next_id("launch"),
            "orchestration.launchThread",
            json!({
                "commandId": next_id("cmd"),
                "projectId": "proj-f13-1",
                "title": "F13 Worktree Strategy",
                "modelSelection": { "instanceId": "devReplay", "model": "dev-replay" },
                "runtimeMode": "approval-required",
                "interactionMode": "default",
                "workspaceStrategy": { "type": "worktree", "baseRef": "HEAD", "branch": "feat/f13-rust-wt" },
                "initialMessage": { "text": "Plan isolated task", "attachments": [] },
            }),
        )
        .await?;

    let tid = launched["threadId"].as_str().ok_or("missing threadId")?;
    let wt_path = launched["projection"]["thread"]["worktreePath"]
        .as_str()
        .ok_or("missing worktreePath")?;
    assert_ne!(wt_path, h.workspace_dir().to_string_lossy().as_ref());
    assert!(std::path::Path::new(wt_path).is_dir());
    assert_eq!(
        launched["projection"]["thread"]["runtimeMode"],
        "approval-required"
    );
    let settled = await_thread_settled(&mut ws, tid).await?;
    assert!(settled["runs"].as_array().is_some_and(|r| !r.is_empty()));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f13_t1_2_existing_worktree_binds_exact_path_and_commits_launch_row() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let wt = h.temp_root().join("wt-existing-13");
    create_worktree(h.workspace_dir(), &wt, "feat/f13-existing")?;

    let tok = h.pair_client("f13-t1-2", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f13-2", "F13 Existing", h.workspace_dir()).await?;

    let launched = launch_replay_wt(
        &mut ws,
        "proj-f13-2",
        "Existing Worktree",
        &wt,
        Some("feat/f13-existing"),
        Some("f13-existing-key"),
        false,
    )
    .await?;

    assert_eq!(
        launched["projection"]["thread"]["worktreePath"].as_str(),
        Some(wt.to_string_lossy().as_ref())
    );
    assert_eq!(
        launched["projection"]["thread"]["branch"].as_str(),
        Some("feat/f13-existing")
    );
    let committed: i64 = h.open_db_readonly()?.query_row(
        "SELECT thread_committed FROM orchestration_v2_thread_launch_workflows WHERE command_id = 'f13-existing-key'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(committed, 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f13_t1_3_stable_launch_key_resumes_across_server_restart() -> TestResult<()> {
    let mut h = CrashableHarness::start(true)?;
    let wt = h.temp_root().join("wt-restart-13");
    create_worktree(h.workspace_dir(), &wt, "feat/f13-restart")?;
    let tok1 = h.pair_client("f13-t1-3a", None).await?;
    let mut ws1 = h.connect_ws(&tok1).await?;
    create_project(&mut ws1, "proj-f13-3", "F13 Restart", h.workspace_dir()).await?;

    let first = launch_replay_wt(
        &mut ws1,
        "proj-f13-3",
        "Restart Idempotent",
        &wt,
        None,
        Some("card-13:plan:1"),
        true,
    )
    .await?;
    assert_eq!(first["resumed"], false);
    let tid = first["threadId"].as_str().ok_or("missing threadId")?;
    drop(ws1);

    h.restart_server()?;
    let tok2 = h.pair_client("f13-t1-3b", None).await?;
    let mut ws2 = h.connect_ws(&tok2).await?;
    let second = launch_replay_wt(
        &mut ws2,
        "proj-f13-3",
        "Restart Idempotent",
        &wt,
        None,
        Some("card-13:plan:1"),
        true,
    )
    .await?;
    assert_eq!(second["resumed"], true);
    assert_eq!(second["threadId"], tid);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f13_t1_4_mcp_thread_launch_uses_worktree_and_reuses_stable_key() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let wt = h.temp_root().join("wt-mcp-13");
    create_worktree(h.workspace_dir(), &wt, "feat/f13-mcp")?;
    let tok = h.pair_client("f13-t1-4", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f13-mcp", "F13 MCP", h.workspace_dir()).await?;

    let args = json!({
        "projectId": "proj-f13-mcp",
        "title": "MCP Worktree Thread",
        "modelSelection": { "instanceId": "devReplay", "model": "dev-replay" },
        "runtimeMode": "auto-accept-edits",
        "workspaceStrategy": { "type": "existing_worktree", "worktreePath": wt.to_string_lossy() },
        "stableLaunchKey": "mcp-stable-13",
        "reuseExistingThread": true,
    });
    let call = |id: u64| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": "legion_thread_launch", "arguments": args },
        })
    };
    let r1: Value = h.post_mcp(Some(&tok), &[], &call(1)).await?.json().await?;
    let r2: Value = h.post_mcp(Some(&tok), &[], &call(2)).await?.json().await?;
    assert_eq!(r1["result"]["isError"], false);
    assert_eq!(r2["result"]["isError"], false);
    assert_eq!(
        r1["result"]["structuredContent"]["threadId"],
        r2["result"]["structuredContent"]["threadId"]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f13_t1_5_board_start_stage_binds_card_worktree_and_selection() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(
        &h,
        "f13_t1_5_board_start_stage_binds_card_worktree_and_selection",
    )
    .await?;
    let wt = h.temp_root().join("wt-board-13");
    create_worktree(h.workspace_dir(), &wt, "feat/f13-board")?;
    let tok = h.pair_client("f13-t1-5", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f13-b", "F13 Board", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f13-b", &wt, "F13 Board Card", None).await?;
    let started = ws
        .call_ok(
            &next_id("start"),
            "workflowBoard.startStage",
            json!({ "cardId": card_id, "expectedRevision": rev, "stage": "execute" }),
        )
        .await?;
    let tid = started["threadId"].as_str().ok_or("missing threadId")?;
    let proj = ws
        .call_ok(
            &next_id("proj"),
            "orchestration.getThreadProjection",
            json!({ "threadId": tid }),
        )
        .await?;
    assert_eq!(
        proj["thread"]["worktreePath"].as_str(),
        Some(wt.to_string_lossy().as_ref())
    );
    Ok(())
}

async fn expect_launch_root_err(
    ws: &mut crate::workflow_board_e2e::ws_rpc::OpaqueWsClient,
    project_id: &str,
    title: &str,
    model: &str,
    tag: &str,
) -> TestResult<()> {
    let _ = expect_rpc_err_tag(
        ws,
        "orchestration.launchThread",
        json!({
            "commandId": next_id("cmd"),
            "projectId": project_id,
            "title": title,
            "modelSelection": { "instanceId": "devReplay", "model": model },
            "runtimeMode": "approval-required",
            "interactionMode": "default",
            "workspaceStrategy": { "type": "root" },
        }),
        tag,
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f13_t2_1_unknown_project_id_never_falls_back_to_first_active_project() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let tok = h.pair_client("f13-t2-1", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-active-13", "Active", h.workspace_dir()).await?;
    expect_launch_root_err(
        &mut ws,
        "proj-nonexistent-13",
        "No Fallback",
        "dev-replay",
        "OrchestrationV2ThreadLaunchError",
    )
    .await?;
    assert_zero_threads(&h)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f13_t2_2_missing_model_or_runtime_mode_rejects_without_silent_defaults() -> TestResult<()>
{
    let h = CrashableHarness::start(true)?;
    let tok = h.pair_client("f13-t2-2", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f13-strict", "Strict", h.workspace_dir()).await?;
    expect_launch_root_err(&mut ws, "proj-f13-strict", "Empty Model", "", "ParseError").await?;

    let mcp_call = json!({
        "jsonrpc": "2.0", "id": 99, "method": "tools/call",
        "params": { "name": "legion_thread_launch", "arguments": { "projectId": "proj-f13-strict", "title": "No Defaults" } },
    });
    let mcp_resp: Value = h.post_mcp(Some(&tok), &[], &mcp_call).await?.json().await?;
    assert_eq!(mcp_resp["result"]["isError"], true);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f13_t2_3_nonexistent_existing_worktree_creates_zero_threads() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let tok = h.pair_client("f13-t2-3", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f13-nowt", "NoWT", h.workspace_dir()).await?;

    let missing_wt = h.temp_root().join("missing-wt-dir");
    let payload = launch_wt_payload(
        "proj-f13-nowt",
        "Missing WT",
        &missing_wt,
        None,
        None,
        false,
    );
    let _ = expect_rpc_err_tag(
        &mut ws,
        "orchestration.launchThread",
        payload,
        "OrchestrationV2ThreadLaunchError",
    )
    .await?;
    assert_zero_threads(&h)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f13_t2_4_stable_launch_key_conflict_rejects_mismatched_project_or_wt() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let (wt_a, wt_b) = (
        h.temp_root().join("wt-conf-a"),
        h.temp_root().join("wt-conf-b"),
    );
    create_worktree(h.workspace_dir(), &wt_a, "feat/conf-a")?;
    create_worktree(h.workspace_dir(), &wt_b, "feat/conf-b")?;

    let tok = h.pair_client("f13-t2-4", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-conf-a", "ConfA", h.workspace_dir()).await?;
    create_project(&mut ws, "proj-conf-b", "ConfB", &wt_b).await?;

    let _ = launch_replay_wt(
        &mut ws,
        "proj-conf-a",
        "Conflict Base",
        &wt_a,
        None,
        Some("key-conf-13"),
        true,
    )
    .await?;
    let retry_payload = launch_wt_payload(
        "proj-conf-b",
        "Conflict Retry",
        &wt_a,
        None,
        Some("key-conf-13"),
        true,
    );
    let err = expect_rpc_err_tag(
        &mut ws,
        "orchestration.launchThread",
        retry_payload,
        "OrchestrationV2ThreadLaunchError",
    )
    .await?;
    let msg = err["message"].as_str().unwrap_or("");
    assert!(msg.contains("conflict") || msg.contains("belongs to project"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f13_t2_5_crash_recovery_between_setup_and_thread_commit() -> TestResult<()> {
    let mut h = CrashableHarness::start(true)?;
    assert!(
        require_m3_execute_ship(
            &h,
            "f13_t2_5_crash_recovery_between_setup_and_thread_commit",
        )
        .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-crash-13");
    create_worktree(h.workspace_dir(), &wt, "feat/f13-crash")?;
    let tok = h.pair_client("f13-t2-5", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f13-c", "F13 Crash", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f13-c", &wt, "F13 Crash Card", None).await?;
    drop(ws);

    let launch_key = format!("board:{card_id}:execute:crash-test");
    let thr_crash = next_id("thr-crash");
    h.stop_server();
    {
        let conn = h.open_db_rw_while_stopped()?;
        fs::create_dir_all(&wt)?;
        conn.execute(
            "INSERT OR IGNORE INTO orchestration_v2_thread_launch_workflows
             (command_id, thread_id, project_id, status, title, worktree_path, branch, setup_committed, thread_committed, message_committed, last_error, created_at, updated_at)
             VALUES (?1, ?2, 'proj-f13-c', 'preparing', 'Crash', ?3, 'feat/f13-crash', 1, 0, 0, NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            rusqlite::params![launch_key, thr_crash, wt.to_string_lossy()],
        )?;
    }
    h.restart_server()?;
    let tok2 = h.pair_client("f13-t2-5b", None).await?;
    let mut ws2 = h.connect_ws(&tok2).await?;
    let res = ws2
        .call_ok(
            &next_id("retry"),
            "workflowBoard.startStage",
            json!({ "cardId": card_id, "expectedRevision": rev, "stage": "execute", "stableLaunchKey": launch_key }),
        )
        .await?;
    assert_eq!(res["threadId"], thr_crash);
    let cnt: i64 = h.open_db_readonly()?.query_row(
        "SELECT COUNT(*) FROM orchestration_v2_projection_threads",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(cnt, 1);
    Ok(())
}
