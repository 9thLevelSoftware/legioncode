#![forbid(unsafe_code)]
//! Deterministic product snapshot digest computation and automatic evidence staleness binding.
//!
//! Excludes `.legion/workflows/` metadata/briefs/logs so board metadata writes never invalidate
//! product check receipts, review runs, or acceptance observations.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use legion_contracts::workflow_board::WorkflowBoardRequestError;
use legion_persistence::workflow_board_repo::{
    get_card_detail, get_card_planning_detail, invalidate_snapshot_bound_evidence,
    update_card_external_edit_state, update_card_product_snapshot_digest,
};
use rusqlite::Connection;
use sha2::{Digest, Sha256};

use crate::git_checkpoints_cli::{is_git_workspace, resolve_commit_oid, run_git};
use crate::workflow_board_fs::{
    is_git_segment, is_symlink_or_reparse_point, resolve_card_workspace_root,
    resolve_confined_card_dir_if_exists,
};
use crate::workflow_board_journal::read_confined_digest_if_file;
use crate::workflow_board_recovery::{reconcile_card_external_edits, verify_disk_task_contracts};
use crate::workflow_board_render::parse_stored_spec_fields;
use crate::workflow_board_types::map_repo_err;

/// Returns `true` when `rel_path` belongs to `.legion/workflows/` or `.git/` board metadata.
pub(crate) fn is_board_metadata_relpath(rel_path: &str) -> bool {
    let norm = rel_path.trim().replace('\\', "/");
    let clean = norm.strip_prefix("./").unwrap_or(&norm);
    clean == ".legion/workflows"
        || clean.starts_with(".legion/workflows/")
        || clean.split('/').any(is_git_segment)
}

/// Resolves the current git `HEAD` commit SHA when `ws_root` is a git worktree with a commit.
pub(crate) fn resolve_head_commit_sha(ws_root: &Path) -> Option<String> {
    if !ws_root.is_dir() || !is_git_workspace(ws_root) {
        return None;
    }
    resolve_commit_oid(ws_root, "HEAD")
}

/// Computes a deterministic `sha256:...` product snapshot digest for `ws_root`,
/// hashing `HEAD` OID, tracked changes, and untracked files **excluding** `.legion/workflows/`.
pub(crate) fn compute_product_snapshot_digest(ws_root: &Path) -> String {
    if !ws_root.is_dir() || !is_git_workspace(ws_root) {
        return compute_non_git_snapshot_digest(ws_root);
    }

    let mut hasher = Sha256::new();
    let head_oid = resolve_commit_oid(ws_root, "HEAD").unwrap_or_else(|| "UNBORN".to_owned());
    hasher.update(b"HEAD\0");
    hasher.update(head_oid.as_bytes());
    hasher.update(b"\0");

    let mut paths = BTreeSet::new();
    if head_oid != "UNBORN"
        && let Ok(raw) = run_git(
            ws_root,
            &["diff", "--no-renames", "-z", "--name-status", "HEAD"],
            &[],
        )
    {
        for p in parse_name_status_z_paths(&raw) {
            if !is_board_metadata_relpath(&p) {
                paths.insert(p);
            }
        }
    }
    if let Ok(raw) = run_git(
        ws_root,
        &["diff", "--cached", "--no-renames", "-z", "--name-status"],
        &[],
    ) {
        for p in parse_name_status_z_paths(&raw) {
            if !is_board_metadata_relpath(&p) {
                paths.insert(p);
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
                paths.insert(p.to_owned());
            }
        }
    }

    for rel in paths {
        hasher.update(b"FILE\0");
        hasher.update(rel.as_bytes());
        hasher.update(b"\0");
        let full = ws_root.join(&rel);
        match fs::symlink_metadata(&full) {
            Ok(meta) if !is_symlink_or_reparse_point(&meta) && meta.is_file() => {
                if let Ok(bytes) = fs::read(&full) {
                    hasher.update(b"PRESENT\0");
                    hasher.update((bytes.len() as u64).to_le_bytes());
                    hasher.update(&bytes);
                } else {
                    hasher.update(b"UNREADABLE\0");
                }
            }
            _ => {
                hasher.update(b"DELETED\0");
            }
        }
    }

    let digest = hasher.finalize();
    format_sha256_prefixed(&digest)
}

fn format_sha256_prefixed(digest: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut hex = String::with_capacity("sha256:".len() + digest.len() * 2);
    hex.push_str("sha256:");
    for b in digest {
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

fn parse_name_status_z_paths(raw: &[u8]) -> Vec<String> {
    let tokens: Vec<&[u8]> = raw.split(|&b| b == 0).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < tokens.len() {
        if !tokens[i].is_empty()
            && !tokens[i + 1].is_empty()
            && let Ok(p) = std::str::from_utf8(tokens[i + 1])
        {
            out.push(p.to_owned());
        }
        i += 2;
    }
    out
}

fn compute_non_git_snapshot_digest(ws_root: &Path) -> String {
    let mut files = BTreeSet::new();
    collect_non_git_files(ws_root, ws_root, 0, &mut files);
    let mut hasher = Sha256::new();
    hasher.update(b"NONGIT\0");
    for rel in files {
        hasher.update(rel.as_bytes());
        hasher.update(b"\0");
        if let Ok(bytes) = fs::read(ws_root.join(&rel)) {
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(&bytes);
        }
    }
    let digest = hasher.finalize();
    format_sha256_prefixed(&digest)
}

fn collect_non_git_files(base: &Path, dir: &Path, depth: usize, out: &mut BTreeSet<String>) {
    if depth > 6 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if is_symlink_or_reparse_point(&meta) {
            continue;
        }
        let Ok(rel) = path.strip_prefix(base) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if is_board_metadata_relpath(&rel_str) {
            continue;
        }
        if meta.is_dir() {
            collect_non_git_files(base, &path, depth + 1, out);
        } else if meta.is_file() {
            out.insert(rel_str);
        }
    }
}

/// Synchronizes `product_snapshot_digest` on `card_id` and invalidates bound checks/review/AC
/// evidence whenever the workspace product snapshot has drifted.
pub(crate) fn sync_product_snapshot_and_invalidate(
    conn: &Connection,
    card_id: &str,
    ws_root: &Path,
    now_iso: &str,
) -> Result<(String, bool), WorkflowBoardRequestError> {
    let current_snap = compute_product_snapshot_digest(ws_root);
    let detail = get_card_detail(conn, card_id).map_err(map_repo_err)?;
    let changed = detail.card.product_snapshot_digest.as_deref() != Some(current_snap.as_str());
    let mut staled = false;
    if changed {
        if detail.card.product_snapshot_digest.is_some() {
            let count = invalidate_snapshot_bound_evidence(
                conn,
                card_id,
                &current_snap,
                "product_snapshot_changed",
                now_iso,
            )
            .map_err(map_repo_err)?;
            staled = count > 0;
        }
        update_card_product_snapshot_digest(conn, card_id, &current_snap, now_iso)
            .map_err(map_repo_err)?;
    }
    Ok((current_snap, staled))
}

/// Runs external deliverable edit reconciliation plus product snapshot drift detection for M3 cards.
pub(crate) fn reconcile_card_and_snapshot(
    conn: &Connection,
    card_id: &str,
    ws_root: &Path,
    now_iso: &str,
) -> Result<(bool, String), WorkflowBoardRequestError> {
    let ext_invalidated = reconcile_card_external_edits(conn, card_id, now_iso)?;
    if ext_invalidated {
        let _ = update_card_external_edit_state(conn, card_id, "deliverable_stale", None, now_iso);
    }
    let (snap, snap_staled) =
        sync_product_snapshot_and_invalidate(conn, card_id, ws_root, now_iso)?;
    Ok((ext_invalidated || snap_staled, snap))
}

/// Resets `external_edit_state` from `'deliverable_stale'` to `'clean'` once all persisted
/// spec and plan deliverables match their files on disk.
pub(crate) fn clear_deliverable_stale_if_clean(
    conn: &Connection,
    card_id: &str,
    now_iso: &str,
) -> Result<bool, WorkflowBoardRequestError> {
    let planning = get_card_planning_detail(conn, card_id).map_err(map_repo_err)?;
    let detail = &planning.card_detail;
    if detail.card.external_edit_state.as_deref() != Some("deliverable_stale") {
        return Ok(false);
    }
    let ws_root = resolve_card_workspace_root(conn, &detail.card)?;
    let Some(card_dir) = resolve_confined_card_dir_if_exists(&ws_root, card_id)? else {
        return Ok(false);
    };
    if let Some(ref sd) = detail.card.spec_digest
        && read_confined_digest_if_file(&card_dir, &card_dir.join("spec.md"))?.as_deref()
            != Some(sd.as_str())
    {
        return Ok(false);
    }
    let wf_dg = detail
        .approvals
        .iter()
        .find(|a| a.gate == "spec" && a.status == "valid")
        .and_then(|a| a.wireframes_digest.as_deref())
        .or(detail.card.wireframes_digest.as_deref());
    if let Some(dg) = wf_dg {
        let fields = parse_stored_spec_fields(&detail.card.spec_json);
        let wf_key = fields
            .wireframes_index
            .as_deref()
            .unwrap_or("wireframes/index.html");
        let wf_path = wf_key.split('/').fold(card_dir.clone(), |p, s| p.join(s));
        if read_confined_digest_if_file(&card_dir, &wf_path)
            .ok()
            .flatten()
            .as_deref()
            != Some(dg)
        {
            return Ok(false);
        }
    }
    if let Some(ref pd) = detail.card.plan_digest {
        if read_confined_digest_if_file(&card_dir, &card_dir.join("plan.md"))?.as_deref()
            != Some(pd.as_str())
        {
            return Ok(false);
        }
        if let Some(ref dd) = detail.card.design_digest
            && read_confined_digest_if_file(&card_dir, &card_dir.join("design.md"))?.as_deref()
                != Some(dd.as_str())
        {
            return Ok(false);
        }
        if !verify_disk_task_contracts(&card_dir, card_id, &planning) {
            return Ok(false);
        }
    }
    update_card_external_edit_state(conn, card_id, "clean", None, now_iso).map_err(map_repo_err)?;
    Ok(true)
}

pub(crate) fn clear_deliverable_stale_if_clean_and_reload(
    conn: &Connection,
    card_id: &str,
    current: legion_persistence::workflow_board_repo::types_m2::CardPlanningDetail,
    now_iso: &str,
) -> Result<
    legion_persistence::workflow_board_repo::types_m2::CardPlanningDetail,
    WorkflowBoardRequestError,
> {
    if clear_deliverable_stale_if_clean(conn, card_id, now_iso)? {
        get_card_planning_detail(conn, card_id).map_err(map_repo_err)
    } else {
        Ok(current)
    }
}
