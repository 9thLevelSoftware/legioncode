//! VCS/git/review, scheduledTasks, mcpApps, agentSessions, preview, device, peerLinks,
//! secrets, guestShare, and operational server.* parity tests.

use serde_json::json;

use super::dispatch;
use super::tests_core::{assert_err_tag, assert_ok, build_test_state, init_git_repo};

#[tokio::test]
async fn vcs_git_and_review_operations_work_against_real_repo() {
    let (tmp, state) = build_test_state();
    let repo_dir = tmp.path().join("vcs-repo");
    std::fs::create_dir_all(&repo_dir).expect("mkdir");
    init_git_repo(&repo_dir);
    let repo_str = repo_dir.to_string_lossy().into_owned();

    let status = assert_ok(
        dispatch(&state, "vcs.refreshStatus", json!({ "cwd": repo_str })).await,
        "vcs.refreshStatus",
    );
    assert_eq!(status["isRepo"], true);

    let refs = assert_ok(
        dispatch(&state, "vcs.listRefs", json!({ "cwd": repo_str })).await,
        "vcs.listRefs",
    );
    assert_ne!(refs["branches"].as_array().map(Vec::len), Some(0));

    let wt = assert_ok(
        dispatch(
            &state,
            "vcs.createWorktree",
            json!({ "cwd": repo_str, "branch": "feature-test" }),
        )
        .await,
        "vcs.createWorktree",
    );
    let wt_path = wt["worktree"]["path"].as_str().expect("wt path").to_owned();
    assert!(
        wt["worktree"]["branch"]
            .as_str()
            .expect("branch")
            .starts_with("legion/")
    );

    let _ = assert_ok(
        dispatch(
            &state,
            "vcs.removeWorktree",
            json!({ "cwd": repo_str, "path": wt_path, "force": true }),
        )
        .await,
        "vcs.removeWorktree",
    );

    let commits = assert_ok(
        dispatch(
            &state,
            "review.getBranchCommits",
            json!({ "cwd": repo_str }),
        )
        .await,
        "review.getBranchCommits",
    );
    assert_eq!(commits["commits"].as_array().expect("commits").len(), 1);

    std::fs::write(repo_dir.join("README.md"), "# Updated\n").expect("modify readme");
    let preview = assert_ok(
        dispatch(&state, "review.getDiffPreview", json!({ "cwd": repo_str })).await,
        "review.getDiffPreview",
    );
    assert_ne!(preview["sources"].as_array().map(Vec::len), Some(0));

    let contents = assert_ok(
        dispatch(
            &state,
            "review.getDiffFileContents",
            json!({ "cwd": repo_str, "path": "README.md" }),
        )
        .await,
        "review.getDiffFileContents",
    );
    assert!(contents["oldContents"].is_string());
    assert_eq!(contents["newContents"], "# Updated\n");
}

#[tokio::test]
async fn scheduled_tasks_mcp_sessions_preview_and_misc_rpcs_work() {
    let (tmp, state) = build_test_state();
    let repo_dir = tmp.path().join("sched-repo");
    std::fs::create_dir_all(&repo_dir).expect("mkdir");
    init_git_repo(&repo_dir);
    let repo_str = repo_dir.to_string_lossy().into_owned();

    let _ = assert_ok(
        dispatch(
            &state,
            "projects.mutate",
            json!({ "type": "create", "projectId": "proj-s", "workspaceRoot": repo_str }),
        )
        .await,
        "create project",
    );

    let upserted = assert_ok(
        dispatch(
            &state,
            "scheduledTasks.upsert",
            json!({
                "taskId": "task-1",
                "title": "Hourly Check",
                "prompt": "Check build status",
                "projectId": "proj-s",
                "schedule": { "type": "interval", "everyMs": 3_600_000 },
                "modelSelection": { "instanceId": "dev-replay", "model": "dev-replay" }
            }),
        )
        .await,
        "scheduledTasks.upsert",
    );
    assert_eq!(upserted["task"]["id"], "task-1");

    let rotated = assert_ok(
        dispatch(
            &state,
            "scheduledTasks.rotateWebhookToken",
            json!({ "taskId": "task-1" }),
        )
        .await,
        "rotateWebhookToken",
    );
    assert!(rotated["task"]["webhookToken"].is_string());

    let disabled = assert_ok(
        dispatch(
            &state,
            "scheduledTasks.setEnabled",
            json!({ "taskId": "task-1", "enabled": false }),
        )
        .await,
        "setEnabled",
    );
    assert_eq!(disabled["task"]["enabled"], false);

    let ran = assert_ok(
        dispatch(
            &state,
            "scheduledTasks.runNow",
            json!({ "taskId": "task-1" }),
        )
        .await,
        "runNow",
    );
    assert!(ran["threadId"].is_string());

    let deliveries = assert_ok(
        dispatch(
            &state,
            "scheduledTasks.listWebhookDeliveries",
            json!({ "taskId": "task-1" }),
        )
        .await,
        "listWebhookDeliveries",
    );
    assert!(deliveries["deliveries"].is_array());

    let deleted = assert_ok(
        dispatch(
            &state,
            "scheduledTasks.delete",
            json!({ "taskId": "task-1" }),
        )
        .await,
        "scheduledTasks.delete",
    );
    assert_eq!(deleted["deleted"], true);

    let _ = assert_ok(
        dispatch(
            &state,
            "mcpApps.updateModelContext",
            json!({
                "threadId": "thread-1",
                "itemId": "item-1",
                "server": "local",
                "tool": "view",
                "text": "context"
            }),
        )
        .await,
        "mcpApps.updateModelContext",
    );
}

#[tokio::test]
async fn server_ops_preview_device_peers_secrets_and_guest_share_work() {
    let (_tmp, state) = build_test_state();

    let scanned = assert_ok(
        dispatch(&state, "agentSessions.scan", json!({})).await,
        "agentSessions.scan",
    );
    assert!(scanned["projects"].is_array());

    let _ = assert_ok(
        dispatch(&state, "server.getStorageCleanupReport", json!({})).await,
        "getStorageCleanupReport",
    );
    let db_storage = assert_ok(
        dispatch(&state, "server.getDatabaseStorage", json!({})).await,
        "getDatabaseStorage",
    );
    assert!(db_storage["databaseBytes"].is_number());
    let _ = assert_ok(
        dispatch(&state, "server.reclaimDatabaseSpace", json!({})).await,
        "reclaimDatabaseSpace",
    );
    let _ = assert_ok(
        dispatch(&state, "server.getUsageSummary", json!({})).await,
        "getUsageSummary",
    );
    let _ = assert_ok(
        dispatch(&state, "cloud.getRelayClientStatus", json!({})).await,
        "getRelayClientStatus",
    );
    let preview = assert_ok(
        dispatch(&state, "preview.list", json!({})).await,
        "preview.list",
    );
    assert!(preview["sessions"].is_array());
    let device = assert_ok(
        dispatch(&state, "device.list", json!({})).await,
        "device.list",
    );
    assert!(device["hosts"].is_array());
    let _ = assert_ok(
        dispatch(&state, "peerLinks.list", json!({})).await,
        "peerLinks.list",
    );
    let _ = assert_ok(
        dispatch(
            &state,
            "secrets.answerRequest",
            json!({
                "threadId": "thread-1",
                "turnItemId": "item-1",
                "answer": { "type": "save", "secret": "s3cr3t" }
            }),
        )
        .await,
        "secrets.answerRequest",
    );
    let _ = assert_err_tag(
        dispatch(
            &state,
            "guestShare.create",
            json!({ "threadId": "nonexistent-thread" }),
        )
        .await,
        "GuestShareCreateError",
        "guestShare missing thread",
    );
}
