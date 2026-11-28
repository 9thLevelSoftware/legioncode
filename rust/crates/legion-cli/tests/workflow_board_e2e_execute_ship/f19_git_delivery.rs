//! F19: Individually selected Git delivery actions (5 Tier 1 + 5 Tier 2).

use std::fs;
use std::path::Path;

use serde_json::json;

use super::support::{CrashableHarness, init_bare_remote, next_id, run_git};
use super::support_board::{
    assert_m3_execute_ship, assert_m3_git_delivery, create_project, execute_and_review_for_ship,
    expect_rpc_err_tag, get_card, require_m3_execute_ship, require_m3_git_delivery,
    run_stacked_git_action, setup_approved_plan_card,
};
use crate::workflow_board_e2e::TestResult;
use crate::workflow_board_e2e::git_fixtures::{commit_file, create_worktree};

fn create_fake_gh_bin(bin_dir: &Path) -> TestResult<()> {
    fs::create_dir_all(bin_dir)?;
    let json_out = json!({
        "number": 42,
        "title": "E2E PR",
        "url": "https://github.com/org/repo/pull/42",
        "baseRefName": "main",
        "headRefName": "feat/f19-pr",
        "state": "OPEN"
    })
    .to_string();
    fs::write(
        bin_dir.join("gh.cmd"),
        format!("@echo off\r\necho {}\r\n", json_out.replace('"', "\\\"")),
    )?;
    let sh_path = bin_dir.join("gh");
    fs::write(&sh_path, format!("#!/bin/sh\necho '{json_out}'\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&sh_path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f19_t1_1_git_commit_action_commits_without_push_or_pr() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let wt = h.temp_root().join("wt-f19-commit");
    create_worktree(h.workspace_dir(), &wt, "feat/f19-commit")?;
    fs::write(wt.join("deliverable.txt"), "ship change\n")?;

    let tok = h.pair_client("f19-t1-1", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    let chunks = run_stacked_git_action(
        &mut ws,
        json!({
            "actionId": next_id("act"),
            "cwd": wt.to_string_lossy(),
            "action": "commit",
            "commitMessage": "feat: individual commit",
        }),
    )
    .await?;

    let finished = chunks
        .iter()
        .find(|c| c["kind"] == "action_finished")
        .ok_or("expected action_finished chunk")?;
    assert_eq!(finished["result"]["commit"]["status"], "created");
    assert_eq!(
        finished["result"]["push"]["status"],
        "skipped_not_requested"
    );
    assert_eq!(finished["result"]["pr"]["status"], "skipped_not_requested");
    let head = run_git(&wt, &["rev-parse", "HEAD"])?;
    assert_eq!(finished["result"]["commit"]["commitSha"], head);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f19_t1_2_git_push_action_pushes_to_bare_remote_without_commit_or_pr() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert_m3_git_delivery(
        &h,
        "f19_t1_2_git_push_action_pushes_to_bare_remote_without_commit_or_pr",
    )
    .await?;
    let bare = h.temp_root().join("origin.git");
    init_bare_remote(&bare)?;
    let wt = h.temp_root().join("wt-f19-push");
    create_worktree(h.workspace_dir(), &wt, "feat/f19-push")?;
    run_git(&wt, &["remote", "add", "origin", &bare.to_string_lossy()])?;
    let sha = commit_file(&wt, "push.txt", "push\n", "commit before push")?;

    let tok = h.pair_client("f19-t1-2", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    let chunks = run_stacked_git_action(
        &mut ws,
        json!({
            "actionId": next_id("act"),
            "cwd": wt.to_string_lossy(),
            "action": "push",
        }),
    )
    .await?;
    let finished = chunks
        .iter()
        .find(|c| c["kind"] == "action_finished")
        .ok_or("expected action_finished")?;
    assert_eq!(
        finished["result"]["commit"]["status"],
        "skipped_not_requested"
    );
    assert_eq!(finished["result"]["push"]["status"], "pushed");
    assert_eq!(finished["result"]["pr"]["status"], "skipped_not_requested");
    let remote_sha = run_git(&bare, &["rev-parse", "refs/heads/feat/f19-push"])?;
    assert_eq!(remote_sha, sha);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f19_t1_3_git_create_pr_action_isolated_from_commit_and_push() -> TestResult<()> {
    let gh_dir = tempfile::tempdir()?;
    create_fake_gh_bin(gh_dir.path())?;
    let mut paths = vec![gh_dir.path().to_path_buf()];
    if let Some(orig) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&orig));
    }
    let path_str = std::env::join_paths(paths)?.to_string_lossy().into_owned();
    let h = CrashableHarness::start_with_env(true, &[("PATH", path_str.as_str())])?;
    assert_m3_git_delivery(
        &h,
        "f19_t1_3_git_create_pr_action_isolated_from_commit_and_push",
    )
    .await?;
    let bare = h.temp_root().join("origin-pr.git");
    init_bare_remote(&bare)?;
    let wt = h.temp_root().join("wt-f19-pr");
    create_worktree(h.workspace_dir(), &wt, "feat/f19-pr")?;
    run_git(&wt, &["remote", "add", "origin", &bare.to_string_lossy()])?;
    let before = commit_file(&wt, "pr.txt", "pr\n", "commit before pr")?;
    run_git(&wt, &["push", "-u", "origin", "feat/f19-pr"])?;
    let tok = h.pair_client("f19-t1-3", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    let chunks = run_stacked_git_action(
        &mut ws,
        json!({
            "actionId": next_id("act"),
            "cwd": wt.to_string_lossy(),
            "action": "create_pr",
        }),
    )
    .await?;
    let finished = chunks
        .iter()
        .find(|c| c["kind"] == "action_finished")
        .ok_or("expected action_finished chunk")?;
    assert_eq!(
        finished["result"]["commit"]["status"],
        "skipped_not_requested"
    );
    assert_eq!(
        finished["result"]["push"]["status"],
        "skipped_not_requested"
    );
    assert_eq!(finished["result"]["pr"]["status"], "created");
    assert_eq!(run_git(&wt, &["rev-parse", "HEAD"])?, before);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f19_t1_4_spec_plan_ship_never_auto_commits_deliverables() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert_m3_execute_ship(
        &h,
        "f19_t1_4_spec_plan_ship_never_auto_commits_deliverables",
    )
    .await?;
    let wt = h.temp_root().join("wt-f19-noauto");
    create_worktree(h.workspace_dir(), &wt, "feat/f19-noauto")?;
    let before = run_git(&wt, &["rev-parse", "HEAD"])?;
    let tok = h.pair_client("f19-t1-4", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f19-4", "F19 NoAuto", h.workspace_dir()).await?;
    let _ = setup_approved_plan_card(&mut ws, "proj-f19-4", &wt, "NoAuto Card", None).await?;
    assert_eq!(run_git(&wt, &["rev-parse", "HEAD"])?, before);
    assert!(run_git(&wt, &["status", "--porcelain"])?.contains(".legion/"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f19_t1_5_git_commit_with_file_paths_commits_only_selected_files() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert_m3_git_delivery(
        &h,
        "f19_t1_5_git_commit_with_file_paths_commits_only_selected_files",
    )
    .await?;
    let wt = h.temp_root().join("wt-f19-paths");
    create_worktree(h.workspace_dir(), &wt, "feat/f19-paths")?;
    fs::write(wt.join("kept.txt"), "keep\n")?;
    fs::write(wt.join("excluded.txt"), "exclude\n")?;

    let tok = h.pair_client("f19-t1-5", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    let chunks = run_stacked_git_action(
        &mut ws,
        json!({
            "actionId": next_id("act"),
            "cwd": wt.to_string_lossy(),
            "action": "commit",
            "commitMessage": "feat: selected only",
            "filePaths": ["kept.txt"],
        }),
    )
    .await?;
    assert!(chunks.iter().any(|c| c["kind"] == "action_finished"));
    let status = run_git(&wt, &["status", "--porcelain"])?;
    assert!(status.contains("excluded.txt"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f19_t2_1_clean_tree_commit_emits_action_failed_without_empty_commit() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let wt = h.temp_root().join("wt-f19-clean");
    create_worktree(h.workspace_dir(), &wt, "feat/f19-clean")?;
    let before = run_git(&wt, &["rev-parse", "HEAD"])?;

    let tok = h.pair_client("f19-t2-1", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    let chunks = run_stacked_git_action(
        &mut ws,
        json!({
            "actionId": next_id("act"),
            "cwd": wt.to_string_lossy(),
            "action": "commit",
            "commitMessage": "nothing to commit",
        }),
    )
    .await?;
    assert!(chunks.iter().any(|chunk| chunk["kind"] == "action_finished"
        && chunk["result"]["commit"]["status"] == "skipped_no_changes"));
    assert_eq!(run_git(&wt, &["rev-parse", "HEAD"])?, before);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f19_t2_2_nonexistent_cwd_emits_action_failed_cleanly() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let tok = h.pair_client("f19-t2-2", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    let missing = h.temp_root().join("missing-cwd-19");

    let chunks = run_stacked_git_action(
        &mut ws,
        json!({
            "actionId": next_id("act"),
            "cwd": missing.to_string_lossy(),
            "action": "commit",
        }),
    )
    .await?;
    assert!(chunks.iter().any(|c| c["kind"] == "action_failed"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f19_t2_3_readonly_token_rejected_on_git_run_stacked_action() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    let ro_tok = h
        .pair_client("f19-t2-3-ro", Some(&["orchestration:read"]))
        .await?;
    let mut ro_ws = h.connect_ws(&ro_tok).await?;

    let err = expect_rpc_err_tag(
        &mut ro_ws,
        "git.runStackedAction",
        json!({
            "actionId": next_id("act"),
            "cwd": h.workspace_dir().to_string_lossy(),
            "action": "commit",
        }),
        "EnvironmentAuthorizationError",
    )
    .await?;
    assert_eq!(err["requiredPermission"], "source-control:write");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f19_t2_4_push_to_unreachable_remote_fails_without_secret_leakage() -> TestResult<()> {
    let h = CrashableHarness::start(true)?;
    assert!(
        require_m3_git_delivery(
            &h,
            "f19_t2_4_push_to_unreachable_remote_fails_without_secret_leakage",
        )
        .await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f19-badrem");
    create_worktree(h.workspace_dir(), &wt, "feat/f19-badrem")?;
    let secret = "ghp_E2eSecretToken999";
    let remote_url = format!("https://x-access-token:{secret}@127.0.0.1:1/org/repo.git");
    run_git(&wt, &["remote", "add", "origin", &remote_url])?;
    let _ = commit_file(&wt, "a.txt", "a\n", "commit")?;
    let tok = h.pair_client("f19-t2-4", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    let chunks = run_stacked_git_action(
        &mut ws,
        json!({
            "actionId": next_id("act"),
            "cwd": wt.to_string_lossy(),
            "action": "push",
        }),
    )
    .await?;
    let failed = chunks
        .iter()
        .find(|c| c["kind"] == "action_failed")
        .ok_or("expected action_failed chunk")?;
    assert!(!failed.to_string().contains(secret));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f19_t2_5_create_pr_failure_does_not_auto_commit_or_mutate_receipt() -> TestResult<()> {
    let gh_dir = tempfile::tempdir()?;
    create_fake_gh_bin(gh_dir.path())?;
    let mut paths = vec![gh_dir.path().to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    let path = std::env::join_paths(paths)?.to_string_lossy().into_owned();
    let h = CrashableHarness::start_with_env(true, &[("PATH", &path)])?;
    let name = "f19_t2_5_create_pr_failure_does_not_auto_commit_or_mutate_receipt";
    assert!(
        require_m3_git_delivery(&h, name).await? && require_m3_execute_ship(&h, name).await?,
        "required workflow board capability"
    );
    let wt = h.temp_root().join("wt-f19-prfail");
    create_worktree(h.workspace_dir(), &wt, "feat/f19-prfail")?;
    let tok = h.pair_client("f19-t2-5", None).await?;
    let mut ws = h.connect_ws(&tok).await?;
    create_project(&mut ws, "proj-f19-5", "F19 PRFail", h.workspace_dir()).await?;
    let (card_id, rev) =
        setup_approved_plan_card(&mut ws, "proj-f19-5", &wt, "F19 PRFail Card", None).await?;
    fs::write(wt.join("dirty-pr.txt"), "uncommitted\n")?;
    let rev_ready = execute_and_review_for_ship(&mut ws, &card_id, rev).await?;
    let obs = ws
        .call_ok(
            &next_id("obs"),
            "workflowBoard.recordManualAcceptance",
            json!({ "cardId": card_id, "expectedRevision": rev_ready, "criterionId": "AC-2", "verdict": "pass", "note": "OK" }),
        )
        .await?;
    let _ = ws
        .call_ok(
            &next_id("ship"),
            "workflowBoard.approveDelivery",
            json!({ "cardId": card_id, "expectedRevision": obs["revision"] }),
        )
        .await?;
    let receipt_before = get_card(&mut ws, &card_id).await?["deliveryReceipt"].clone();
    assert_eq!(receipt_before["status"], "shipped");
    let before = run_git(&wt, &["rev-parse", "HEAD"])?;
    let chunks = run_stacked_git_action(
        &mut ws,
        json!({ "actionId": next_id("act"), "cwd": wt.to_string_lossy(), "action": "create_pr" }),
    )
    .await?;
    assert!(chunks.iter().any(|c| c["kind"] == "action_failed"));
    assert_eq!(run_git(&wt, &["rev-parse", "HEAD"])?, before);
    assert!(run_git(&wt, &["status", "--porcelain"])?.contains("dirty-pr.txt"));
    let card = get_card(&mut ws, &card_id).await?;
    assert_eq!(card["deliveryReceipt"], receipt_before);

    fs::write(wt.join("README.md"), "# Changed after approval\n")?;
    let after_drift = get_card(&mut ws, &card_id).await?;
    let mut stale_receipt = receipt_before;
    stale_receipt["status"] = json!("stale");
    assert_eq!(after_drift["deliveryReceipt"], stale_receipt);
    Ok(())
}
