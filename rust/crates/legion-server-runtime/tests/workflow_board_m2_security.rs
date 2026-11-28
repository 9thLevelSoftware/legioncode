#![forbid(unsafe_code)]
//! Iteration 2 security regression tests (Findings 3 & 4: junction write escape and secret intake bypass).

#[path = "workflow_board_m2_support/plan_helpers.rs"]
mod plan_helpers;
#[path = "workflow_board_m2_support/mod.rs"]
mod workflow_board_m2_support;

use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;

use legion_contracts::ids::ProjectId;
use legion_contracts::orchestration::PatchValue;
use legion_contracts::workflow_board::{
    WorkflowBoardApplyCliImportInput, WorkflowBoardApproveSpecInput, WorkflowBoardArtifactKind,
    WorkflowBoardCreateCardInput, WorkflowBoardGetArtifactInput, WorkflowBoardGetCardInput,
    WorkflowBoardPreviewCliImportInput, WorkflowBoardRequestErrorCode, WorkflowBoardSpecSourceType,
    WorkflowBoardUpdateSpecInput,
};
use legion_persistence::workflow_board_repo::stage_write;
use legion_persistence::workflow_board_repo::types::StageWriteInput;
use legion_server_runtime::reconcile_incomplete_write_journal;
use legion_workflows::sha256_prefixed;
use plan_helpers::{make_plan_input, make_task, setup_spec_approved_card};
use workflow_board_m2_support::{TestResult, setup_m2_board};

fn create_dir_link(link: &Path, target: &Path) -> std::io::Result<()> {
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)?;
    }
    #[cfg(windows)]
    {
        let link_win = link.to_string_lossy().replace('/', "\\");
        let target_win = target.to_string_lossy().replace('/', "\\");
        let out = StdCommand::new("cmd")
            .args(["/C", "mklink", "/J", &link_win, &target_win])
            .output()?;
        if !out.status.success() {
            return Err(std::io::Error::other(format!(
                "mklink /J failed: {}",
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        Ok(())
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
}

#[test]
fn cli_import_rejects_linked_source_ancestor_without_creating_card() -> TestResult {
    let (_tmp, _orch, board, repo_str) = setup_m2_board()?;
    let repo = Path::new(&repo_str);
    let outside = tempfile::tempdir()?;
    let spec = outside.path().join("source/specs/spec-fixture");
    fs::create_dir_all(&spec)?;
    fs::write(
        spec.join("SPEC.md"),
        "---\ntitle: Import confinement fixture\n---\nFixture specification\n",
    )?;
    create_dir_link(&repo.join("linked"), outside.path())?;

    let project_id = ProjectId::new("proj-board-1")?;
    let cli_dir = "linked/source".to_owned();
    let preview = board
        .preview_cli_import(&WorkflowBoardPreviewCliImportInput {
            project_id: project_id.clone(),
            cli_dir: Some(cli_dir.clone()),
        })
        .expect_err("CLI import must reject linked ancestors");
    let apply = board
        .apply_cli_import(&WorkflowBoardApplyCliImportInput {
            project_id: project_id.clone(),
            cli_dir: Some(cli_dir),
            acknowledge_unsupported_policies: None,
            acknowledged_policies: None,
            model_selection: None,
            runtime_mode: None,
            interaction_mode: None,
            workspace_strategy: None,
        })
        .expect_err("CLI import must reject linked ancestors before writing");
    assert_eq!(preview.code, WorkflowBoardRequestErrorCode::InvalidInput);
    assert_eq!(apply.code, preview.code);
    assert!(board.get_board_snapshot(&project_id)?.cards.is_empty());
    assert!(!repo.join(".legion/workflows").exists());
    Ok(())
}

#[tokio::test]
async fn junction_secret_intake_bypass_is_blocked() -> TestResult {
    let (_tmp, _orch, board, repo_str) = setup_m2_board()?;
    let repo_dir = Path::new(&repo_str);
    fs::write(repo_dir.join(".env"), "SECRET=topsecret\n")?;
    create_dir_link(&repo_dir.join("docs/git_dir_link"), &repo_dir.join(".git"))?;

    let err_brief = board
        .create_card(&WorkflowBoardCreateCardInput {
            project_id: ProjectId::new("proj-board-1")?,
            title: "Junction Git Brief".to_owned(),
            brief: None,
            brief_path: Some("docs/git_dir_link/HEAD".to_owned()),
            input_paths: None,
            intake_goal: None,
            wireframes_enabled: None,
            wireframes_index: PatchValue::Absent,
            source_type: Some(WorkflowBoardSpecSourceType::File),
            assistance_mode: None,
            workspace_strategy: None,
            model_selection: None,
            runtime_mode: None,
            interaction_mode: None,
        })
        .expect_err("junction to .git/HEAD must be rejected");
    assert_eq!(
        err_brief.code,
        WorkflowBoardRequestErrorCode::InvalidArtifactKey
    );

    let err_inputs = board
        .create_card(&WorkflowBoardCreateCardInput {
            project_id: ProjectId::new("proj-board-1")?,
            title: "Junction Git InputPaths".to_owned(),
            brief: Some("Normal brief".to_owned()),
            brief_path: None,
            input_paths: Some(vec!["docs/git_dir_link/HEAD".to_owned()]),
            intake_goal: None,
            wireframes_enabled: None,
            wireframes_index: PatchValue::Absent,
            source_type: Some(WorkflowBoardSpecSourceType::Idea),
            assistance_mode: None,
            workspace_strategy: None,
            model_selection: None,
            runtime_mode: None,
            interaction_mode: None,
        })
        .expect_err("junction in inputPaths must be rejected");
    assert_eq!(
        err_inputs.code,
        WorkflowBoardRequestErrorCode::InvalidArtifactKey
    );

    let (cid, rev) = setup_spec_approved_card(&board, PatchValue::Null).await?;
    let err_update_inputs = board
        .update_spec(&WorkflowBoardUpdateSpecInput {
            card_id: cid,
            expected_revision: rev,
            title: None,
            brief: None,
            intake_goal: None,
            input_paths: Some(vec!["docs/git_dir_link/HEAD".to_owned()]),
            acceptance_criteria: None,
            constraints: None,
            source_refs: None,
            wireframes_index: PatchValue::Absent,
        })
        .await
        .expect_err("junction in updateSpec inputPaths must be rejected");
    assert_eq!(
        err_update_inputs.code,
        WorkflowBoardRequestErrorCode::InvalidArtifactKey
    );
    Ok(())
}

#[tokio::test]
async fn junction_deliverable_write_escape_is_blocked() -> TestResult {
    let (tmp, _orch, board, repo_str) = setup_m2_board()?;
    let repo_dir = Path::new(&repo_str);
    let outside_dir = tmp.path().join("outside-target");
    fs::create_dir_all(&outside_dir)?;

    let (cid, rev) = setup_spec_approved_card(&board, PatchValue::Null).await?;
    let card_dir = repo_dir.join(".legion/workflows").join(cid.as_str());
    create_dir_link(&card_dir.join("wireframes"), &outside_dir)?;
    let wf_err = board
        .update_spec(&WorkflowBoardUpdateSpecInput {
            card_id: cid.clone(),
            expected_revision: rev,
            title: None,
            brief: None,
            intake_goal: None,
            input_paths: None,
            acceptance_criteria: None,
            constraints: None,
            source_refs: None,
            wireframes_index: PatchValue::Set("wireframes/index.html".to_owned()),
        })
        .await
        .expect_err("wireframes junction escape must be rejected");
    assert_eq!(
        wf_err.code,
        WorkflowBoardRequestErrorCode::InvalidArtifactKey
    );
    assert!(!outside_dir.join("index.html").exists());

    create_dir_link(&card_dir.join("tasks"), &outside_dir)?;
    let plan_err = board
        .update_plan(&make_plan_input(
            cid,
            rev,
            vec![make_task("TSK-1", "src/a.rs")],
        ))
        .await
        .expect_err("tasks junction escape must be rejected");
    assert_eq!(
        plan_err.code,
        WorkflowBoardRequestErrorCode::InvalidArtifactKey
    );
    assert!(!outside_dir.join("TSK-1.md").exists());
    Ok(())
}

fn stage_junction_row(
    conn: &rusqlite::Connection,
    id: &str,
    card_id: &str,
    target: &Path,
    staging: &Path,
    expected_digest: Option<&str>,
    rev: i64,
) -> TestResult {
    stage_write(
        conn,
        &StageWriteInput {
            id,
            card_id,
            target_path: &target.to_string_lossy(),
            staging_path: Some(&staging.to_string_lossy()),
            operation: "write",
            expected_digest,
            previous_digest: None,
            revision: Some(rev),
        },
        "2026-11-24T12:00:00Z",
    )?;
    Ok(())
}

#[tokio::test]
async fn reconcile_and_journal_reject_card_dir_junction_bypass() -> TestResult {
    let (tmp, orch, board, repo_str) = setup_m2_board()?;
    let repo_dir = Path::new(&repo_str);
    let outside_card = tmp.path().join("outside-card-copy");
    fs::create_dir_all(&outside_card)?;

    let (cid, rev) = setup_spec_approved_card(&board, PatchValue::Null).await?;
    let card_dir = repo_dir.join(".legion/workflows").join(cid.as_str());
    let spec_bytes = fs::read(card_dir.join("spec.md"))?;
    let spec_digest = sha256_prefixed(&spec_bytes);
    fs::write(outside_card.join("spec.md"), &spec_bytes)?;
    fs::write(outside_card.join("keep.tmp"), b"outside temp")?;
    fs::write(outside_card.join("staged_committed.tmp"), &spec_bytes)?;
    fs::write(
        outside_card.join("victim_uncommitted.tmp"),
        b"uncommitted staging victim",
    )?;
    fs::remove_dir_all(&card_dir)?;
    create_dir_link(&card_dir, &outside_card)?;

    let detail_err = board
        .get_card_detail(&WorkflowBoardGetCardInput {
            card_id: cid.clone(),
            project_id: None,
        })
        .expect_err("getCardDetail must reject card_dir junction");
    assert_eq!(
        detail_err.code,
        WorkflowBoardRequestErrorCode::InvalidArtifactKey
    );

    let artifact_err = board
        .get_artifact(&WorkflowBoardGetArtifactInput {
            card_id: cid.clone(),
            key: None,
            kind: Some(WorkflowBoardArtifactKind::Spec),
        })
        .expect_err("getArtifact must reject card_dir junction");
    assert_eq!(
        artifact_err.code,
        WorkflowBoardRequestErrorCode::InvalidArtifactKey
    );

    let approve_err = board
        .approve_spec(
            &WorkflowBoardApproveSpecInput {
                card_id: cid.clone(),
                expected_revision: rev,
                expected_digest: None,
            },
            "client",
        )
        .await
        .expect_err("approveSpec must reject card_dir junction");
    assert_eq!(
        approve_err.code,
        WorkflowBoardRequestErrorCode::InvalidArtifactKey
    );

    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        let rev_i64 = i64::try_from(rev)?;
        let spec_target = card_dir.join("spec.md");
        stage_junction_row(
            &conn,
            "wj-junc-committed",
            cid.as_str(),
            &spec_target,
            &card_dir.join("staged_committed.tmp"),
            Some(&spec_digest),
            rev_i64,
        )?;
        stage_junction_row(
            &conn,
            "wj-junc-uncommitted",
            cid.as_str(),
            &spec_target,
            &card_dir.join("victim_uncommitted.tmp"),
            None,
            rev_i64,
        )?;
        let report = reconcile_incomplete_write_journal(&conn, "2026-11-24T12:00:01Z")?;
        assert_eq!(report.committed, 0);
        assert_eq!(report.rolled_back, 2);
        assert_eq!(report.temp_files_cleaned, 0);
        for jid in ["wj-junc-committed", "wj-junc-uncommitted"] {
            let status: String = conn.query_row(
                "SELECT status FROM workflow_board_write_journal WHERE id = ?1",
                [jid],
                |r| r.get(0),
            )?;
            assert_eq!(status, "rolled_back");
        }
    }
    assert!(outside_card.join("keep.tmp").exists());
    assert!(outside_card.join("staged_committed.tmp").exists());
    assert!(outside_card.join("victim_uncommitted.tmp").exists());
    Ok(())
}

#[tokio::test]
async fn pre_approval_leaf_spec_junction_is_rejected_by_update_spec() -> TestResult {
    let (tmp, _orch, board, repo_str) = setup_m2_board()?;
    let repo_dir = Path::new(&repo_str);
    let outside_dir = tmp.path().join("outside-spec-target");
    fs::create_dir_all(&outside_dir)?;

    let created = board.create_card(&WorkflowBoardCreateCardInput {
        project_id: ProjectId::new("proj-board-1")?,
        title: "Leaf Spec Junction".to_owned(),
        brief: Some("Initial brief".to_owned()),
        brief_path: None,
        input_paths: None,
        intake_goal: None,
        wireframes_enabled: None,
        wireframes_index: PatchValue::Absent,
        source_type: Some(WorkflowBoardSpecSourceType::Idea),
        assistance_mode: None,
        workspace_strategy: None,
        model_selection: None,
        runtime_mode: None,
        interaction_mode: None,
    })?;
    let spec_path = repo_dir
        .join(".legion/workflows")
        .join(created.summary.card_id.as_str())
        .join("spec.md");
    fs::remove_file(&spec_path)?;
    create_dir_link(&spec_path, &outside_dir)?;

    let err = board
        .update_spec(&WorkflowBoardUpdateSpecInput {
            card_id: created.summary.card_id,
            expected_revision: created.summary.revision,
            title: Some("Updated Title".to_owned()),
            brief: None,
            intake_goal: None,
            input_paths: None,
            acceptance_criteria: None,
            constraints: None,
            source_refs: None,
            wireframes_index: PatchValue::Absent,
        })
        .await
        .expect_err("updateSpec must reject leaf spec.md junction before spec approval");
    assert_eq!(err.code, WorkflowBoardRequestErrorCode::InvalidArtifactKey);
    Ok(())
}
