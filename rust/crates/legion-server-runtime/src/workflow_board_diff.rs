#![forbid(unsafe_code)]
//! Pre-turn baseline capture, post-turn `filesAllowed`/`filesForbidden` contract validation,
//! and `workflowBoard.getCardDiff` against the card worktree (never discarding unexpected edits).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use legion_contracts::ids::WorkflowCardId;
use legion_contracts::workflow_board::{
    WorkflowBoardDiffChangeType, WorkflowBoardDiffFileContractStatus, WorkflowBoardDiffFileEntry,
    WorkflowBoardGetCardDiffInput, WorkflowBoardGetCardDiffResult, WorkflowBoardRequestError,
};
use legion_orchestrator::Orchestrator;
use legion_persistence::workflow_board_repo::get_card_execution_detail;
use legion_persistence::workflow_board_repo::types_m2::TaskContractRow;
use legion_scm::checkpoints::diff_query::{CheckpointDiffQuery, DiffFileInput, DiffQueryOptions};
use legion_workflows::sha256_prefixed;
use serde::{Deserialize, Serialize};

use crate::git_checkpoints_cli::{is_git_workspace, resolve_commit_oid, run_git};
use crate::outbox_state::utc_now_iso;
use crate::workflow_board_diff_glob::{
    normalize_rel, parse_name_status_z, path_matches_pattern, read_worktree_file_digest,
    scan_dirty_worktree_map,
};
use crate::workflow_board_fs::{resolve_card_workspace_root, validate_safe_card_id};
use crate::workflow_board_hydration_m3::to_card_detail_from_execution;
use crate::workflow_board_journal::ensure_confined_deliverable_target;
use crate::workflow_board_snapshot::{
    compute_product_snapshot_digest, is_board_metadata_relpath, resolve_head_commit_sha,
};
use crate::workflow_board_types::{db_lock_err, internal, map_repo_err};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorktreeBaseline {
    pub head_oid: String,
    pub dirty_digests: BTreeMap<String, Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClassifiedContractDiff {
    pub changed_files: Vec<String>,
    pub unexpected_files: Vec<String>,
    pub forbidden_files: Vec<String>,
    pub diff_digest: String,
}

/// Captures a non-destructive pre-turn baseline of `ws_root` (excluding `.legion/workflows/`).
pub(crate) fn capture_worktree_baseline(ws_root: &Path) -> WorktreeBaseline {
    if !ws_root.is_dir() || !is_git_workspace(ws_root) {
        return WorktreeBaseline::default();
    }
    let head_oid = resolve_commit_oid(ws_root, "HEAD").unwrap_or_else(|| "UNBORN".to_owned());
    let dirty = scan_dirty_worktree_map(ws_root, &head_oid);
    WorktreeBaseline {
        head_oid,
        dirty_digests: dirty,
    }
}

pub(crate) fn save_task_baseline(
    card_dir: &Path,
    stage_run_id: &str,
    baseline: &WorktreeBaseline,
) -> Result<(), WorkflowBoardRequestError> {
    let target = card_dir
        .join("baselines")
        .join(format!("{stage_run_id}.json"));
    let safe = ensure_confined_deliverable_target(card_dir, &target, true)?;
    let bytes = serde_json::to_vec_pretty(baseline).map_err(|e| internal(e.to_string()))?;
    fs::write(&safe, bytes).map_err(|e| internal(format!("Failed writing task baseline: {e}")))
}

pub(crate) fn load_task_baseline(card_dir: &Path, stage_run_id: &str) -> Option<WorktreeBaseline> {
    let target = card_dir
        .join("baselines")
        .join(format!("{stage_run_id}.json"));
    let safe = ensure_confined_deliverable_target(card_dir, &target, false).ok()?;
    let bytes = fs::read(safe).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Computes files touched since `baseline` (preserving all disk edits intact).
pub(crate) fn compute_turn_changed_files(
    ws_root: &Path,
    baseline: Option<&WorktreeBaseline>,
) -> Vec<String> {
    if !ws_root.is_dir() || !is_git_workspace(ws_root) {
        return Vec::new();
    }
    let current_head = resolve_commit_oid(ws_root, "HEAD").unwrap_or_else(|| "UNBORN".to_owned());
    let current_dirty = scan_dirty_worktree_map(ws_root, &current_head);
    let Some(base) = baseline else {
        return current_dirty.into_keys().collect();
    };

    let mut changed = BTreeSet::new();
    if base.head_oid != current_head
        && base.head_oid != "UNBORN"
        && current_head != "UNBORN"
        && let Ok(raw) = run_git(
            ws_root,
            &[
                "diff",
                "--no-renames",
                "-z",
                "--name-status",
                &base.head_oid,
                &current_head,
            ],
            &[],
        )
    {
        for (_, path) in parse_name_status_z(&raw) {
            if !is_board_metadata_relpath(&path) {
                changed.insert(path);
            }
        }
    }
    for (path, cur_digest) in &current_dirty {
        match base.dirty_digests.get(path) {
            Some(prev_digest) if prev_digest == cur_digest => {}
            _ => {
                changed.insert(path.clone());
            }
        }
    }
    for prev_path in base.dirty_digests.keys() {
        if !current_dirty.contains_key(prev_path) {
            changed.insert(prev_path.clone());
        }
    }
    changed.into_iter().collect()
}

/// Classifies `changed_files` against `filesAllowed` and `filesForbidden` without modifying disk.
pub(crate) fn classify_contract_files(
    ws_root: &Path,
    changed_files: &[String],
    allowed_patterns: &[String],
    forbidden_patterns: &[String],
) -> ClassifiedContractDiff {
    let mut unexpected_files = Vec::new();
    let mut forbidden_files = Vec::new();
    let mut digest_buf = String::new();

    for path in changed_files {
        let norm = normalize_rel(path);
        let is_forbidden = forbidden_patterns
            .iter()
            .any(|pat| path_matches_pattern(&norm, pat));
        let is_allowed = allowed_patterns
            .iter()
            .any(|pat| path_matches_pattern(&norm, pat));
        if is_forbidden {
            forbidden_files.push(norm.clone());
        } else if !is_allowed {
            unexpected_files.push(norm.clone());
        }
        let file_dg =
            read_worktree_file_digest(ws_root, &norm).unwrap_or_else(|| "DELETED".to_owned());
        digest_buf.push_str(&norm);
        digest_buf.push(':');
        digest_buf.push_str(&file_dg);
        digest_buf.push('\n');
    }

    ClassifiedContractDiff {
        changed_files: changed_files.iter().map(|s| normalize_rel(s)).collect(),
        unexpected_files,
        forbidden_files,
        diff_digest: sha256_prefixed(digest_buf.as_bytes()),
    }
}

pub(crate) fn get_card_diff_impl(
    orchestrator: &Orchestrator,
    input: &WorkflowBoardGetCardDiffInput,
) -> Result<WorkflowBoardGetCardDiffResult, WorkflowBoardRequestError> {
    let card_id = validate_safe_card_id(input.card_id.as_str())?;
    let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
    let exec = get_card_execution_detail(&conn, card_id).map_err(map_repo_err)?;
    let card = &exec.planning.card_detail.card;
    let ws_root = resolve_card_workspace_root(&conn, card)?;
    let include_patches = input.include_patches.unwrap_or(true);

    let target_task_id = input
        .task_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or(card.current_task_id.as_deref());
    let task_contract: Option<&TaskContractRow> =
        target_task_id.and_then(|tid| exec.planning.tasks.iter().find(|t| t.task_id == tid));

    let allowed: Vec<String> = task_contract
        .and_then(|t| serde_json::from_str(&t.files_allowed_json).ok())
        .unwrap_or_default();
    let forbidden: Vec<String> = task_contract
        .and_then(|t| serde_json::from_str(&t.files_forbidden_json).ok())
        .unwrap_or_default();

    let head_sha = resolve_head_commit_sha(&ws_root);
    let snapshot_digest = compute_product_snapshot_digest(&ws_root);
    let mut entries = collect_worktree_diff_entries(
        &ws_root,
        head_sha.as_deref(),
        include_patches,
        task_contract.is_some(),
        &allowed,
        &forbidden,
    );
    let max_files = 250;
    let truncated = entries.len() > max_files;
    if truncated {
        entries.truncate(max_files);
    }

    let detail = to_card_detail_from_execution(&exec, &[], Vec::new())?;
    let reports = detail.file_contract_reports;
    let file_contract_report = target_task_id
        .and_then(|tid| reports.iter().find(|r| r.task_id == tid).cloned())
        .or_else(|| reports.into_iter().next());
    let diff_sig: String = entries
        .iter()
        .map(|f| format!("{}:{}:{}", f.path, f.additions, f.deletions))
        .collect::<Vec<_>>()
        .join("\n");

    Ok(WorkflowBoardGetCardDiffResult {
        card_id: WorkflowCardId::new(card_id).map_err(|e| internal(e.to_string()))?,
        worktree_path: Some(ws_root.to_string_lossy().into_owned()),
        base_ref: head_sha,
        product_snapshot_digest: Some(snapshot_digest),
        diff_digest: Some(sha256_prefixed(diff_sig.as_bytes())),
        files: entries,
        file_contract_report,
        truncated,
        generated_at: utc_now_iso(),
    })
}

fn collect_worktree_diff_entries(
    ws_root: &Path,
    head_sha: Option<&str>,
    include_patches: bool,
    has_task_scope: bool,
    allowed: &[String],
    forbidden: &[String],
) -> Vec<WorkflowBoardDiffFileEntry> {
    if !ws_root.is_dir() || !is_git_workspace(ws_root) {
        return Vec::new();
    }
    let mut status_by_path: BTreeMap<String, char> = BTreeMap::new();
    if let Some(head) = head_sha
        && let Ok(raw) = run_git(
            ws_root,
            &["diff", "--no-renames", "-z", "--name-status", head],
            &[],
        )
    {
        for (st, p) in parse_name_status_z(&raw) {
            if !is_board_metadata_relpath(&p) {
                status_by_path.insert(normalize_rel(&p), st);
            }
        }
    }
    if let Ok(raw) = run_git(
        ws_root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        &[],
    ) {
        for slice in raw.split(|&b| b == 0) {
            if !slice.is_empty()
                && let Ok(p) = std::str::from_utf8(slice)
                && !is_board_metadata_relpath(p)
            {
                status_by_path.entry(normalize_rel(p)).or_insert('A');
            }
        }
    }

    let mut out = Vec::with_capacity(status_by_path.len());
    for (path, st) in status_by_path {
        let old_bytes = if st == 'A' || head_sha.is_none() {
            Vec::new()
        } else {
            run_git(ws_root, &["show", &format!("HEAD:{path}")], &[]).unwrap_or_default()
        };
        let new_bytes = if st == 'D' {
            Vec::new()
        } else {
            fs::read(ws_root.join(&path)).unwrap_or_default()
        };
        let (old_text, new_text, is_bin) =
            match (String::from_utf8(old_bytes), String::from_utf8(new_bytes)) {
                (Ok(o), Ok(n)) if !o.contains('\0') && !n.contains('\0') => (o, n, false),
                _ => (String::new(), String::new(), true),
            };
        let computed = CheckpointDiffQuery::compute_files(
            &[DiffFileInput {
                old_path: &path,
                new_path: &path,
                old_content: &old_text,
                new_content: &new_text,
                is_binary: is_bin,
            }],
            DiffQueryOptions::default(),
        );
        let change_type = match st {
            'A' => WorkflowBoardDiffChangeType::New,
            'D' => WorkflowBoardDiffChangeType::Deleted,
            _ => WorkflowBoardDiffChangeType::Change,
        };
        let contract_status = if !has_task_scope {
            Some(WorkflowBoardDiffFileContractStatus::Unassigned)
        } else if forbidden.iter().any(|pat| path_matches_pattern(&path, pat)) {
            Some(WorkflowBoardDiffFileContractStatus::Forbidden)
        } else if allowed.iter().any(|pat| path_matches_pattern(&path, pat)) {
            Some(WorkflowBoardDiffFileContractStatus::Allowed)
        } else {
            Some(WorkflowBoardDiffFileContractStatus::Unexpected)
        };
        out.push(WorkflowBoardDiffFileEntry {
            path,
            previous_path: None,
            change_type,
            additions: u64::try_from(computed.insertions).unwrap_or(0),
            deletions: u64::try_from(computed.deletions).unwrap_or(0),
            patch: include_patches.then_some(computed.diff),
            contract_status,
        });
    }
    out
}
