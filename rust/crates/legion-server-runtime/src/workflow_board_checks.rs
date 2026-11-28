#![forbid(unsafe_code)]
//! `argv`-only planned check runner with scrubbed child environment, credential-scrubbed logs,
//! snapshot digest binding, and `record_check_receipt` persistence.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;

use legion_contracts::workflow_board::{
    WorkflowBoardCliImportProvenance, WorkflowBoardRequestError, WorkflowBoardRequestErrorCode,
    is_valid_argv_command,
};
use legion_orchestrator::Orchestrator;
use legion_persistence::workflow_board_repo::types_m3::{
    CardExecutionDetail, RecordCheckReceiptInput,
};
use legion_persistence::workflow_board_repo::{
    get_card_detail, get_card_planning_detail, get_cli_import_by_card, record_check_receipt,
};
use legion_scm::live_git_cli::collect_safe_base_env;
use legion_workflows::scrub::scrub_feedback_text;
use tokio::process::Command;
use uuid::Uuid;

use crate::outbox_state::utc_now_iso;
use crate::workflow_board_cli_render::render_imported_card_json;
use crate::workflow_board_fs::{
    ensure_confined_card_dir, resolve_card_workspace_root, validate_safe_card_id,
};
use crate::workflow_board_journal::{
    JournaledWriteSpec, ensure_confined_deliverable_target, finalize_staged_files,
    stage_and_write_temp_files,
};
use crate::workflow_board_locks::acquire_card_lock;
use crate::workflow_board_render::{parse_stored_spec_fields, render_card_json};
use crate::workflow_board_snapshot::{
    compute_product_snapshot_digest, reconcile_card_and_snapshot, resolve_head_commit_sha,
};
use crate::workflow_board_types::{db_lock_err, internal, map_repo_err};

const MAX_EXCERPT_CHARS: usize = 4096;

/// Executes a planned `argv`-only check directly in the card worktree (zero shell, scrubbed env)
/// and records a snapshot-bound `CheckReceipt`.
pub(crate) async fn run_planned_check_impl(
    orchestrator: &Arc<Orchestrator>,
    card_id_raw: &str,
    check_id_raw: &str,
    expected_revision: Option<u64>,
    notify_project: impl Fn(&str),
) -> Result<CardExecutionDetail, WorkflowBoardRequestError> {
    let card_id = validate_safe_card_id(card_id_raw)?;
    let check_id = check_id_raw.trim();
    if check_id.is_empty() {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InvalidInput,
            "checkId cannot be empty",
        ));
    }
    let _card_guard = acquire_card_lock(orchestrator, card_id).await;
    let now_iso = utc_now_iso();

    let PreparedCheckRun {
        ws_root,
        card_dir,
        argv,
        card_revision,
        project_id,
        prepared_snapshot,
    } = prepare_planned_check_run(
        orchestrator,
        card_id,
        check_id,
        expected_revision,
        &now_iso,
        &notify_project,
    )?;

    let launched_at = Instant::now();
    let (exec_out, launch_failure) = match execute_argv_scrubbed(&ws_root, &argv).await {
        Ok(output) => (output, None),
        Err(error) if error.code == WorkflowBoardRequestErrorCode::LaunchFailed => {
            let stderr_scrubbed = scrub_feedback_text(&error.message, &[], 16_384).text;
            let output = ScrubbedExecOutput {
                exit_code: None,
                stdout_scrubbed: String::new(),
                stderr_excerpt: bound_excerpt(&stderr_scrubbed),
                stderr_scrubbed,
                stdout_excerpt: None,
                duration_ms: i64::try_from(launched_at.elapsed().as_millis()).unwrap_or(i64::MAX),
            };
            (output, Some(error))
        }
        Err(error) => return Err(error),
    };
    let snapshot_digest = compute_product_snapshot_digest(&ws_root);
    if let Some(ref launch_failure) = launch_failure
        && snapshot_digest != prepared_snapshot
    {
        return Err(launch_failure.clone());
    }
    let finished_iso = utc_now_iso();
    let receipt_id = format!("rcpt-{}", Uuid::new_v4().simple());
    let log_rel = write_scrubbed_check_log(&card_dir, check_id, &receipt_id, &exec_out)?;
    let commit_sha = resolve_head_commit_sha(&ws_root);
    let status = if exec_out.exit_code == Some(0) {
        "passed"
    } else {
        "failed"
    };

    let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
    let rev_id = format!("rev-{}", Uuid::new_v4().simple());
    let detail = record_check_receipt(
        &conn,
        &RecordCheckReceiptInput {
            card_id,
            revision_id: &rev_id,
            receipt_id: &receipt_id,
            expected_revision: card_revision,
            check_id,
            argv: &argv,
            status,
            exit_code: exec_out.exit_code,
            log_ref: Some(&log_rel),
            stdout_excerpt: exec_out.stdout_excerpt.as_deref(),
            stderr_excerpt: exec_out.stderr_excerpt.as_deref(),
            duration_ms: Some(exec_out.duration_ms),
            snapshot_digest: &snapshot_digest,
            commit_sha: commit_sha.as_deref(),
        },
        &finished_iso,
    )
    .map_err(map_repo_err)?;
    sync_card_json_file(&card_dir, &detail.planning.card_detail.card)?;
    notify_project(&project_id);
    match launch_failure {
        Some(error) => Err(error),
        None => Ok(detail),
    }
}

struct ScrubbedExecOutput {
    exit_code: Option<i64>,
    stdout_scrubbed: String,
    stderr_scrubbed: String,
    stdout_excerpt: Option<String>,
    stderr_excerpt: Option<String>,
    duration_ms: i64,
}

async fn execute_argv_scrubbed(
    ws_root: &Path,
    argv: &[String],
) -> Result<ScrubbedExecOutput, WorkflowBoardRequestError> {
    if !is_valid_argv_command(argv) {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InvalidInput,
            "Planned check command must be a valid non-shell argv list",
        ));
    }
    let started = Instant::now();
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .current_dir(ws_root)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in collect_safe_base_env() {
        cmd.env(k, v);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0");

    let output = cmd.output().await.map_err(|e| {
        WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::LaunchFailed,
            format!("Failed to execute planned check `{}`: {e}", argv[0]),
        )
    })?;
    let duration_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
    let stdout_scrubbed =
        scrub_feedback_text(&String::from_utf8_lossy(&output.stdout), &[], 16_384).text;
    let stderr_scrubbed =
        scrub_feedback_text(&String::from_utf8_lossy(&output.stderr), &[], 16_384).text;
    let stdout_excerpt = bound_excerpt(&stdout_scrubbed);
    let stderr_excerpt = bound_excerpt(&stderr_scrubbed);

    Ok(ScrubbedExecOutput {
        exit_code: output.status.code().map(i64::from),
        stdout_scrubbed,
        stderr_scrubbed,
        stdout_excerpt,
        stderr_excerpt,
        duration_ms,
    })
}

fn bound_excerpt(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut out: String = trimmed.chars().take(MAX_EXCERPT_CHARS).collect();
    if trimmed.chars().count() > MAX_EXCERPT_CHARS {
        out.push_str("\n...[truncated]");
    }
    Some(out)
}

fn write_scrubbed_check_log(
    card_dir: &Path,
    check_id: &str,
    receipt_id: &str,
    exec_out: &ScrubbedExecOutput,
) -> Result<String, WorkflowBoardRequestError> {
    let safe_check = check_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    let rel = format!("logs/{safe_check}-{receipt_id}.log");
    let target = card_dir
        .join("logs")
        .join(format!("{safe_check}-{receipt_id}.log"));
    let safe_target = ensure_confined_deliverable_target(card_dir, &target, true)?;
    let content = format!(
        "exit_code: {:?}\nduration_ms: {}\n\n=== STDOUT ===\n{}\n\n=== STDERR ===\n{}\n",
        exec_out.exit_code,
        exec_out.duration_ms,
        exec_out.stdout_scrubbed,
        exec_out.stderr_scrubbed
    );
    fs::write(&safe_target, content)
        .map_err(|e| internal(format!("Failed writing check log `{rel}`: {e}")))?;
    Ok(rel)
}

pub(crate) fn sync_card_json_file(
    card_dir: &Path,
    card: &legion_persistence::workflow_board_repo::types::CardRow,
) -> Result<(), WorkflowBoardRequestError> {
    let target = card_dir.join("card.json");
    let safe = ensure_confined_deliverable_target(card_dir, &target, true)?;
    let tmp = card_dir.join(format!("card.json.sync-{}.tmp", Uuid::new_v4().simple()));
    let safe_tmp = ensure_confined_deliverable_target(card_dir, &tmp, true)?;
    let fields = parse_stored_spec_fields(&card.spec_json);
    let rendered = match card
        .import_provenance_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<WorkflowBoardCliImportProvenance>(raw).ok())
    {
        Some(prov) => render_imported_card_json(card, &fields, &prov)?,
        None => render_card_json(card, &fields)?,
    };
    fs::write(&safe_tmp, rendered.as_bytes())
        .map_err(|e| internal(format!("Failed writing temp card.json: {e}")))?;
    fs::rename(&safe_tmp, &safe).map_err(|e| internal(format!("Failed syncing card.json: {e}")))
}

/// Re-journals `card.json` and `import-provenance.json` on disk after CLI policy acknowledgement.
pub(crate) fn sync_cli_policy_deliverables(
    conn: &rusqlite::Connection,
    ws_root: &Path,
    card_id: &str,
    now_iso: &str,
) -> Result<(), WorkflowBoardRequestError> {
    let Some(cli_row) = get_cli_import_by_card(conn, card_id).map_err(map_repo_err)? else {
        return Ok(());
    };
    let detail = get_card_detail(conn, card_id).map_err(map_repo_err)?;
    let provenance: WorkflowBoardCliImportProvenance =
        serde_json::from_str(&cli_row.import_provenance_json)
            .map_err(|e| internal(format!("Invalid persisted CLI provenance JSON: {e}")))?;
    let fields = parse_stored_spec_fields(&detail.card.spec_json);
    let card_json = render_imported_card_json(&detail.card, &fields, &provenance)?;
    let prov_json = serde_json::to_string_pretty(&provenance)
        .map_err(|e| internal(format!("Failed to serialize import-provenance.json: {e}")))?;
    let card_dir = ensure_confined_card_dir(ws_root, card_id)?;
    let rev = Some(detail.card.revision);
    let writes = vec![
        JournaledWriteSpec {
            key: "card.json".to_owned(),
            bytes: card_json.as_bytes().to_vec(),
            expected_digest: legion_workflows::sha256_prefixed(card_json.as_bytes()),
            revision: rev,
        },
        JournaledWriteSpec {
            key: "import-provenance.json".to_owned(),
            bytes: prov_json.as_bytes().to_vec(),
            expected_digest: legion_workflows::sha256_prefixed(prov_json.as_bytes()),
            revision: rev,
        },
    ];
    let staged = stage_and_write_temp_files(conn, card_id, &card_dir, &writes, now_iso)?;
    finalize_staged_files(conn, &staged, now_iso)
}

struct PreparedCheckRun {
    ws_root: PathBuf,
    card_dir: PathBuf,
    argv: Vec<String>,
    card_revision: i64,
    project_id: String,
    prepared_snapshot: String,
}

fn prepare_planned_check_run(
    orchestrator: &Orchestrator,
    card_id: &str,
    check_id: &str,
    expected_revision: Option<u64>,
    now_iso: &str,
    notify_project: &impl Fn(&str),
) -> Result<PreparedCheckRun, WorkflowBoardRequestError> {
    let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
    let planning = get_card_planning_detail(&conn, card_id).map_err(map_repo_err)?;
    let card = &planning.card_detail.card;
    if let Some(exp) = expected_revision
        && i64::try_from(exp).ok() != Some(card.revision)
    {
        let cur = u64::try_from(card.revision.max(1)).unwrap_or(1);
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::RevisionConflict,
            format!("Revision conflict on card `{card_id}`: expected {exp}, current {cur}"),
        )
        .with_current_revision(cur));
    }
    let ws_root = resolve_card_workspace_root(&conn, card)?;
    let (invalidated, prepared_snapshot) =
        reconcile_card_and_snapshot(&conn, card_id, &ws_root, now_iso)?;
    if invalidated {
        notify_project(&card.project_id);
    }
    let planning = get_card_planning_detail(&conn, card_id).map_err(map_repo_err)?;
    let card = &planning.card_detail.card;
    if matches!(card.status.as_str(), "paused" | "completed" | "shipped") {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InvalidStageTransition,
            format!(
                "Cannot run planned check while card status is `{}`",
                card.status
            ),
        ));
    }
    let check = planning
        .planned_checks
        .iter()
        .find(|c| c.check_id == check_id)
        .ok_or_else(|| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InvalidInput,
                format!("Planned check `{check_id}` does not exist on card `{card_id}`"),
            )
        })?;
    let argv: Vec<String> = serde_json::from_str(&check.argv_json).map_err(|e| {
        internal(format!(
            "Invalid persisted argv_json on check `{check_id}`: {e}"
        ))
    })?;
    if !is_valid_argv_command(&argv) {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InvalidInput,
            format!("Planned check `{check_id}` has unsafe or invalid argv command"),
        ));
    }
    let card_dir = ensure_confined_card_dir(&ws_root, card_id)?;
    Ok(PreparedCheckRun {
        ws_root,
        card_dir,
        argv,
        card_revision: card.revision,
        project_id: card.project_id.clone(),
        prepared_snapshot,
    })
}
