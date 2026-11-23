//! Orchestration v2 parity tests (`dispatchCommand`, `launchThread`, `searchThreads`, `searchThread`,
//! `getTurnDiff`, `getShellSnapshot`, `getArchivedShellSnapshot`, `getThreadFamily`, `forceStopThread`, etc.).

use std::sync::Arc;

use serde_json::json;
use tempfile::TempDir;

use super::super::AppState;
use super::dispatch;
use super::tests_core::{assert_err_tag, assert_ok, build_test_state, init_git_repo};

async fn seed_launched_thread() -> (TempDir, Arc<AppState>, String) {
    let (tmp, state) = build_test_state();
    let repo_dir = tmp.path().join("orch-repo");
    std::fs::create_dir_all(&repo_dir).expect("mkdir");
    init_git_repo(&repo_dir);
    let repo_str = repo_dir.to_string_lossy().into_owned();

    let proj = assert_ok(
        dispatch(
            &state,
            "orchestration.dispatchCommand",
            json!({
                "type": "project.create",
                "commandId": "cmd-proj-1",
                "projectId": "project-1",
                "title": "OrchRepo",
                "workspaceRoot": repo_str,
                "createdAt": "2026-11-22T00:00:00.000Z"
            }),
        )
        .await,
        "project.create",
    );
    assert!(proj.get("sequence").is_some());

    let launch = assert_ok(
        dispatch(
            &state,
            "orchestration.v2.launchThread",
            json!({
                "commandId": "cmd-launch-1",
                "projectId": "project-1",
                "threadId": "thread-launch-1",
                "title": "Investigate trigram search",
                "modelSelection": { "provider": "devReplay", "model": "dev-replay" },
                "runtimeMode": "approval-required",
                "interactionMode": "default",
                "workspaceStrategy": { "type": "root" },
                "initialMessage": {
                    "messageId": "msg-launch-1",
                    "text": "Investigating orchestration trigram search performance across threads",
                    "attachments": []
                }
            }),
        )
        .await,
        "orchestration.v2.launchThread",
    );
    assert_eq!(launch["threadId"], "thread-launch-1");
    assert_eq!(launch["resumed"], false);
    (tmp, state, repo_str)
}

#[tokio::test]
async fn orchestration_v2_shell_and_projection_queries_work() {
    let (_tmp, state, repo_str) = seed_launched_thread().await;

    let shell = assert_ok(
        dispatch(&state, "orchestration.getShellSnapshot", json!({})).await,
        "orchestration.getShellSnapshot",
    );
    assert!(
        shell["threads"]
            .as_array()
            .expect("threads")
            .iter()
            .any(|t| t["id"] == "thread-launch-1")
    );

    let archived = assert_ok(
        dispatch(&state, "orchestration.getArchivedShellSnapshot", json!({})).await,
        "orchestration.getArchivedShellSnapshot",
    );
    assert!(archived["threads"].is_array());

    let proj_res = assert_ok(
        dispatch(
            &state,
            "orchestration.getThreadProjection",
            json!({ "threadId": "thread-launch-1" }),
        )
        .await,
        "orchestration.getThreadProjection",
    );
    assert_eq!(proj_res["thread"]["id"], "thread-launch-1");

    let titles = assert_ok(
        dispatch(
            &state,
            "orchestration.getThreadTitles",
            json!({ "threadIds": ["thread-launch-1", "thread-missing"] }),
        )
        .await,
        "orchestration.getThreadTitles",
    );
    assert_eq!(titles["thread-launch-1"], "Investigate trigram search");
    assert!(titles.get("thread-missing").is_none());

    let links = assert_ok(
        dispatch(
            &state,
            "orchestration.getThreadLinkMetadata",
            json!({ "threadIds": ["thread-launch-1"] }),
        )
        .await,
        "orchestration.getThreadLinkMetadata",
    );
    assert_eq!(links["thread-launch-1"]["projectId"], "project-1");

    let family = assert_ok(
        dispatch(
            &state,
            "orchestration.getThreadFamily",
            json!({ "threadId": "thread-launch-1" }),
        )
        .await,
        "orchestration.getThreadFamily",
    );
    assert_eq!(family["threads"].as_array().expect("family").len(), 1);

    let wt_ids = assert_ok(
        dispatch(
            &state,
            "orchestration.getWorktreeThreadIds",
            json!({ "worktreePath": repo_str }),
        )
        .await,
        "orchestration.getWorktreeThreadIds",
    );
    assert!(wt_ids["threadIds"].is_array());
}

#[tokio::test]
async fn orchestration_v2_search_diff_stop_and_workflow_work() {
    let (_tmp, state, _) = seed_launched_thread().await;

    let older = assert_ok(
        dispatch(&state, "orchestration.getOlderSettledThreads", json!({})).await,
        "orchestration.getOlderSettledThreads",
    );
    assert!(older["threads"].is_array());

    let search = assert_ok(
        dispatch(
            &state,
            "orchestration.searchThreads",
            json!({ "query": "trigram search", "limit": 10 }),
        )
        .await,
        "orchestration.searchThreads",
    );
    assert!(
        search["matches"]
            .as_array()
            .expect("matches")
            .iter()
            .any(|m| m["threadId"] == "thread-launch-1")
    );

    let in_thread = assert_ok(
        dispatch(
            &state,
            "orchestration.searchThread",
            json!({ "threadId": "thread-launch-1", "query": "trigram" }),
        )
        .await,
        "orchestration.searchThread",
    );
    assert!(in_thread["totalMatches"].as_u64().unwrap_or(0) >= 1);

    let zero_diff = assert_ok(
        dispatch(
            &state,
            "orchestration.getTurnDiff",
            json!({ "threadId": "thread-launch-1", "fromTurnCount": 0, "toTurnCount": 0 }),
        )
        .await,
        "getTurnDiff 0..0",
    );
    assert_eq!(zero_diff["diff"], "");

    let _ = assert_err_tag(
        dispatch(
            &state,
            "orchestration.stopWorkflow",
            json!({ "threadId": "thread-launch-1", "subagentId": "sub-missing" }),
        )
        .await,
        "OrchestrationV2StopWorkflowError",
        "stopWorkflow",
    );

    let forced = assert_ok(
        dispatch(
            &state,
            "orchestration.forceStopThread",
            json!({ "threadId": "thread-launch-1" }),
        )
        .await,
        "forceStopThread",
    );
    assert_eq!(forced["outcome"], "released");

    let item = assert_ok(
        dispatch(
            &state,
            "orchestration.getTurnItem",
            json!({ "threadId": "thread-launch-1", "itemId": "missing-item" }),
        )
        .await,
        "getTurnItem",
    );
    assert!(item["item"].is_null());

    let native = assert_ok(
        dispatch(
            &state,
            "orchestration.getNativeChildWork",
            json!({ "threadId": "thread-launch-1" }),
        )
        .await,
        "getNativeChildWork",
    );
    assert_eq!(native["children"], json!([]));

    let _ = assert_err_tag(
        dispatch(
            &state,
            "orchestration.getWorkflowScript",
            json!({ "threadId": "thread-launch-1", "scriptPath": "relative.js" }),
        )
        .await,
        "OrchestrationGetWorkflowScriptError",
        "getWorkflowScript relative",
    );
}

#[tokio::test]
async fn projects_create_search_entries_attachments_trackers_and_git_commit() {
    let (tmp, state) = build_test_state();
    let created = assert_ok(
        dispatch(
            &state,
            "projects.createNew",
            json!({ "name": "Demo App", "directory": tmp.path().to_string_lossy() }),
        )
        .await,
        "projects.createNew",
    );
    let root = created["workspaceRoot"]
        .as_str()
        .expect("workspaceRoot")
        .to_owned();
    let proj_id = created["projectId"]
        .as_str()
        .expect("project id")
        .to_owned();

    std::fs::write(
        std::path::Path::new(&root).join("NOTES.md"),
        "hello notes\n",
    )
    .expect("write note");
    let search_entries = assert_ok(
        dispatch(
            &state,
            "projects.searchEntries",
            json!({ "cwd": root, "query": "NOTES", "limit": 10 }),
        )
        .await,
        "projects.searchEntries",
    );
    assert_eq!(search_entries["entries"].as_array().map(Vec::len), Some(1));

    let _ = assert_ok(
        dispatch(
            &state,
            "issueTrackers.bindProject",
            json!({ "projectId": proj_id, "tracker": "github", "scope": "org/repo" }),
        )
        .await,
        "issueTrackers.bindProject",
    );
    let trackers = assert_ok(
        dispatch(&state, "issueTrackers.list", json!({})).await,
        "issueTrackers.list",
    );
    assert_eq!(
        trackers["bindings"].as_object().map(serde_json::Map::len),
        Some(1)
    );

    let persisted = assert_ok(
        dispatch(
            &state,
            "assets.persistChatAttachments",
            json!({
                "threadId": "thread-a1",
                "attachments": [{
                    "type": "image",
                    "id": "att-1.png",
                    "dataUrl": "data:image/png;base64,iVBORw0KGgo="
                }]
            }),
        )
        .await,
        "assets.persistChatAttachments",
    );
    assert_eq!(persisted["attachments"][0]["id"], "att-1.png");
    let _ = assert_ok(
        dispatch(
            &state,
            "attachments.delete",
            json!({ "attachmentId": "att-1.png" }),
        )
        .await,
        "attachments.delete",
    );

    init_git_repo(std::path::Path::new(&root));
    std::fs::write(std::path::Path::new(&root).join("CHANGELOG.md"), "v1\n").expect("write");
    let stacked = assert_ok(
        dispatch(
            &state,
            "git.runStackedAction",
            json!({ "cwd": root, "action": "commit", "commitMessage": "feat: add changelog" }),
        )
        .await,
        "git.runStackedAction",
    );
    assert_eq!(stacked["commit"]["status"], "created");
}
