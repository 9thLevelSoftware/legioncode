//! F6-T2 through F12-T1 Rust persistence, restart, and write-journal E2E tests.

use std::fs;

use serde_json::json;

use crate::workflow_board_e2e::TestResult;

use super::helpers::{
    PlanningEnv, assert_board_capable, assert_git_zero_autocommits, call_expect_failure, card_dir,
    check_board_capable, create_and_approve_spec, git_head_sha, sha256_hex, valid_ac_mappings_json,
    valid_alternatives_json, valid_lenses_json, valid_tasks_json,
};

pub async fn f7_t2_04_external_spec_edit_invalidates_challenge_binding() -> TestResult<()> {
    let mut env = PlanningEnv::start(true).await?;
    assert!(check_board_capable(&env, "F7-T2-04"), "required workflow board capability");
    let created = env
        .ws
        .call_ok(
            "c-f7",
            "workflowBoard.createCard",
            json!({
                "projectId": env.project_id,
                "title": "External Edit Challenge",
                "brief": "Initial brief",
                "sourceType": "idea",
                "assistanceMode": "balanced",
            }),
        )
        .await?;
    let card_id = created["cardId"].as_str().ok_or("missing cardId")?;
    let rev1 = created["revision"].as_i64().ok_or("missing rev1")?;
    let spec_updated = env
        .ws
        .call_ok(
            "u-f7",
            "workflowBoard.updateSpec",
            json!({
                "cardId": card_id,
                "expectedRevision": rev1,
                "acceptanceCriteria": [{ "id": "AC-01", "text": "Challenge binding detects edit." }],
                "constraints": ["Zero auto-commits"],
                "sourceRefs": ["README.md"],
                "wireframesIndex": serde_json::Value::Null,
            }),
        )
        .await?;
    let rev2 = spec_updated["revision"].as_i64().ok_or("missing rev2")?;
    let challenged = env
        .ws
        .call_ok(
            "ch-f7",
            "workflowBoard.challengeStep",
            json!({
                "cardId": card_id,
                "expectedRevision": rev2,
                "action": "complete",
                "concerns": [],
            }),
        )
        .await?;
    let rev3 = challenged["revision"].as_i64().ok_or("missing rev3")?;

    let spec_file = card_dir(env.harness.workspace_dir(), card_id).join("spec.md");
    assert!(
        spec_file.is_file(),
        "expected spec.md on disk at {}",
        spec_file.display()
    );
    let mut content = fs::read_to_string(&spec_file)?;
    content.push_str("\n<!-- external edit after challenge -->\n");
    fs::write(&spec_file, content)?;

    let err = call_expect_failure(
        &mut env.ws,
        "ap-f7",
        "workflowBoard.approveSpec",
        json!({ "cardId": card_id, "expectedRevision": rev3 }),
    )
    .await?;
    assert_ne!(err.to_string(), "");

    let after = env
        .ws
        .call_ok(
            "g-f7-after",
            "workflowBoard.getCard",
            json!({ "cardId": card_id }),
        )
        .await?;
    assert_ne!(after["specApproval"]["status"], json!("valid"));
    Ok(())
}

pub async fn f8_t1_04_spec_approval_and_files_survive_restart() -> TestResult<()> {
    let mut env = PlanningEnv::start(true).await?;
    assert_board_capable(&env, "F8-T1-04");
    let head_before = git_head_sha(env.harness.workspace_dir())?;
    let (card_id, _rev) =
        create_and_approve_spec(&mut env.ws, &env.project_id, "Spec Restart", false).await?;
    let before = env
        .ws
        .call_ok(
            "g-pre",
            "workflowBoard.getCard",
            json!({ "cardId": card_id }),
        )
        .await?;
    let before_digest = before["specApproval"]["specDigest"]
        .as_str()
        .ok_or("missing specDigest")?
        .to_owned();

    env.harness.restart_server()?;
    env.reconnect_ws("e2e-f8-post", None).await?;
    let after = env
        .ws
        .call_ok(
            "g-post",
            "workflowBoard.getCard",
            json!({ "cardId": card_id }),
        )
        .await?;
    assert_eq!(after["specApproval"]["status"].as_str(), Some("valid"));
    assert_eq!(
        after["specApproval"]["specDigest"].as_str(),
        Some(before_digest.as_str())
    );
    assert_git_zero_autocommits(env.harness.workspace_dir(), &head_before)?;
    Ok(())
}

pub async fn f8_t1_05_write_journal_and_disk_sha256_consistency() -> TestResult<()> {
    let mut env = PlanningEnv::start(true).await?;
    assert_board_capable(&env, "F8-T1-05");
    let (card_id, _rev) =
        create_and_approve_spec(&mut env.ws, &env.project_id, "Journal Check", false).await?;

    env.harness.restart_server()?;
    env.reconnect_ws("e2e-f8-journal", None).await?;
    let detail = env
        .ws
        .call_ok("g-j", "workflowBoard.getCard", json!({ "cardId": card_id }))
        .await?;
    let spec_bytes = fs::read(card_dir(env.harness.workspace_dir(), &card_id).join("spec.md"))?;
    let disk_sha = sha256_hex(&spec_bytes);
    let digest = detail["specApproval"]["specDigest"]
        .as_str()
        .ok_or("missing specApproval.specDigest")?;
    assert_eq!(digest.trim_start_matches("sha256:"), disk_sha);

    let db = env.harness.open_db_readonly()?;
    let pending: i64 = db.query_row(
        "SELECT COUNT(*) FROM workflow_board_write_journal WHERE status != 'committed'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(pending, 0, "no unreconciled write-journal rows may remain");
    Ok(())
}

pub async fn f8_t2_04_external_disk_edit_invalidates_spec_and_plan_approvals() -> TestResult<()> {
    let mut env = PlanningEnv::start(true).await?;
    assert!(check_board_capable(&env, "F8-T2-04"), "required workflow board capability");
    let (card_id, rev) =
        create_and_approve_spec(&mut env.ws, &env.project_id, "Ext Edit Spec", false).await?;
    let plan_upd = env
        .ws
        .call_ok(
            "u-p8",
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
    let rev_plan = plan_upd["revision"].as_i64().ok_or("missing rev_plan")?;
    let _ = env
        .ws
        .call_ok(
            "a-p8",
            "workflowBoard.approvePlan",
            json!({ "cardId": card_id, "expectedRevision": rev_plan }),
        )
        .await?;

    let spec_path = card_dir(env.harness.workspace_dir(), &card_id).join("spec.md");
    let mut text = fs::read_to_string(&spec_path)?;
    text.push_str("\n<!-- external edit -->\n");
    fs::write(&spec_path, text)?;

    let after = env
        .ws
        .call_ok(
            "g-ext",
            "workflowBoard.getCard",
            json!({ "cardId": card_id }),
        )
        .await?;
    assert_eq!(after["specApproval"]["status"].as_str(), Some("stale"));
    assert_eq!(after["planApproval"]["status"].as_str(), Some("stale"));
    Ok(())
}

pub async fn f9_t1_04_six_lenses_and_rationales_survive_restart() -> TestResult<()> {
    let mut env = PlanningEnv::start(true).await?;
    assert_board_capable(&env, "F9-T1-04");
    let (card_id, rev) =
        create_and_approve_spec(&mut env.ws, &env.project_id, "Lenses Restart", false).await?;
    env.ws
        .call_ok(
            "u-f9",
            "workflowBoard.updatePlan",
            json!({
                "cardId": card_id,
                "expectedRevision": rev,
                "lenses": valid_lenses_json(),
            }),
        )
        .await?;

    env.harness.restart_server()?;
    env.reconnect_ws("e2e-f9-post", None).await?;
    let detail = env
        .ws
        .call_ok(
            "g-f9",
            "workflowBoard.getCard",
            json!({ "cardId": card_id }),
        )
        .await?;
    assert_eq!(detail["lenses"].as_array().map(Vec::len), Some(6));
    assert!(
        card_dir(env.harness.workspace_dir(), &card_id)
            .join("design.md")
            .is_file()
    );
    Ok(())
}

pub async fn f10_t1_04_comparison_and_selection_survive_restart() -> TestResult<()> {
    let mut env = PlanningEnv::start(true).await?;
    assert_board_capable(&env, "F10-T1-04");
    let (card_id, rev) =
        create_and_approve_spec(&mut env.ws, &env.project_id, "Compare Restart", false).await?;
    let rationale = "Alt A guarantees atomic SQLite receipts.";
    env.ws
        .call_ok(
            "u-f10",
            "workflowBoard.updatePlan",
            json!({
                "cardId": card_id,
                "expectedRevision": rev,
                "lenses": valid_lenses_json(),
                "alternatives": valid_alternatives_json(),
                "selectedAlternativeId": "alt-a",
                "selectionRationale": rationale,
            }),
        )
        .await?;

    env.harness.restart_server()?;
    env.reconnect_ws("e2e-f10-post", None).await?;
    let detail = env
        .ws
        .call_ok(
            "g-f10",
            "workflowBoard.getCard",
            json!({ "cardId": card_id }),
        )
        .await?;
    assert_eq!(detail["selectedAlternativeId"].as_str(), Some("alt-a"));
    assert_eq!(detail["selectionRationale"].as_str(), Some(rationale));
    Ok(())
}

pub async fn f11_t1_04_task_contracts_written_to_disk_uncommitted() -> TestResult<()> {
    let mut env = PlanningEnv::start(true).await?;
    assert_board_capable(&env, "F11-T1-04");
    let head_before = git_head_sha(env.harness.workspace_dir())?;
    let (card_id, rev) =
        create_and_approve_spec(&mut env.ws, &env.project_id, "Task Files", false).await?;
    env.ws
        .call_ok(
            "u-f11",
            "workflowBoard.updatePlan",
            json!({
                "cardId": card_id,
                "expectedRevision": rev,
                "lenses": valid_lenses_json(),
                "alternatives": valid_alternatives_json(),
                "selectedAlternativeId": "alt-a",
                "selectionRationale": "Disk task contract check",
                "strategy": "outcomes",
                "granularity": "balanced",
                "tasks": valid_tasks_json(),
            }),
        )
        .await?;
    let tasks_dir = card_dir(env.harness.workspace_dir(), &card_id).join("tasks");
    assert!(tasks_dir.join("TSK-0001.md").is_file());
    assert!(tasks_dir.join("TSK-0002.md").is_file());
    assert_git_zero_autocommits(env.harness.workspace_dir(), &head_before)?;
    Ok(())
}

pub async fn f12_t1_04_plan_approval_and_deliverables_survive_restart() -> TestResult<()> {
    let mut env = PlanningEnv::start(true).await?;
    assert_board_capable(&env, "F12-T1-04");
    let head_before = git_head_sha(env.harness.workspace_dir())?;
    let (card_id, rev) =
        create_and_approve_spec(&mut env.ws, &env.project_id, "Plan Restart", false).await?;
    let updated = env
        .ws
        .call_ok(
            "u-f12",
            "workflowBoard.updatePlan",
            json!({
                "cardId": card_id,
                "expectedRevision": rev,
                "lenses": valid_lenses_json(),
                "alternatives": valid_alternatives_json(),
                "selectedAlternativeId": "alt-a",
                "selectionRationale": "Full plan durability",
                "strategy": "outcomes",
                "granularity": "balanced",
                "tasks": valid_tasks_json(),
                "acMappings": valid_ac_mappings_json(),
            }),
        )
        .await?;
    let rev_plan = updated["revision"].as_i64().ok_or("missing rev_plan")?;
    env.ws
        .call_ok(
            "a-f12",
            "workflowBoard.approvePlan",
            json!({ "cardId": card_id, "expectedRevision": rev_plan }),
        )
        .await?;

    env.harness.restart_server()?;
    env.reconnect_ws("e2e-f12-post", None).await?;
    let detail = env
        .ws
        .call_ok(
            "g-f12",
            "workflowBoard.getCard",
            json!({ "cardId": card_id }),
        )
        .await?;
    assert_eq!(detail["specApproval"]["status"].as_str(), Some("valid"));
    assert_eq!(detail["planApproval"]["status"].as_str(), Some("valid"));
    assert!(
        card_dir(env.harness.workspace_dir(), &card_id)
            .join("plan.md")
            .is_file()
    );
    assert_git_zero_autocommits(env.harness.workspace_dir(), &head_before)?;
    Ok(())
}
