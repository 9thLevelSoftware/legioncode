#![forbid(unsafe_code)]
//! Path normalization, glob matching, and git dirty-map scanning for workflow-board diffs.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use legion_workflows::sha256_prefixed;

use crate::git_checkpoints_cli::run_git;
use crate::workflow_board_fs::is_symlink_or_reparse_point;
use crate::workflow_board_snapshot::is_board_metadata_relpath;

pub(crate) fn normalize_rel(raw: &str) -> String {
    let norm = raw.trim().replace('\\', "/");
    norm.strip_prefix("./").unwrap_or(&norm).to_owned()
}

pub(crate) fn path_matches_pattern(path: &str, pattern: &str) -> bool {
    let p = normalize_rel(path);
    let pat = normalize_rel(pattern);
    if pat.is_empty() {
        return false;
    }
    if p == pat {
        return true;
    }
    if let Some(prefix) = pat.strip_suffix("/**") {
        return p == prefix || p.starts_with(&format!("{prefix}/"));
    }
    if pat.ends_with('/') {
        return p.starts_with(&pat);
    }
    if pat.contains('*') {
        let p_segs: Vec<&str> = p.split('/').collect();
        let pat_segs: Vec<&str> = pat.split('/').collect();
        return glob_segments_match(&p_segs, &pat_segs);
    }
    false
}

fn glob_segments_match(path: &[&str], pat: &[&str]) -> bool {
    if pat.is_empty() {
        return path.is_empty();
    }
    if pat[0] == "**" {
        return glob_segments_match(path, &pat[1..])
            || (!path.is_empty() && glob_segments_match(&path[1..], pat));
    }
    if path.is_empty() {
        return false;
    }
    glob_segment_match(path[0], pat[0]) && glob_segments_match(&path[1..], &pat[1..])
}

fn glob_segment_match(text: &str, pat: &str) -> bool {
    let tb = text.as_bytes();
    let pb = pat.as_bytes();
    let (mut ti, mut pi, mut star, mut match_i) = (0usize, 0usize, None, 0usize);
    while ti < tb.len() {
        if pi < pb.len() && (pb[pi] == b'?' || pb[pi] == tb[ti]) {
            ti += 1;
            pi += 1;
        } else if pi < pb.len() && pb[pi] == b'*' {
            star = Some(pi);
            match_i = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            match_i += 1;
            ti = match_i;
        } else {
            return false;
        }
    }
    while pi < pb.len() && pb[pi] == b'*' {
        pi += 1;
    }
    pi == pb.len()
}

pub(crate) fn scan_dirty_worktree_map(
    ws_root: &Path,
    head_oid: &str,
) -> BTreeMap<String, Option<String>> {
    let mut paths = BTreeSet::new();
    if head_oid != "UNBORN"
        && let Ok(raw) = run_git(
            ws_root,
            &["diff", "--no-renames", "-z", "--name-status", "HEAD"],
            &[],
        )
    {
        for (_, p) in parse_name_status_z(&raw) {
            if !is_board_metadata_relpath(&p) {
                paths.insert(normalize_rel(&p));
            }
        }
    }
    if let Ok(raw) = run_git(
        ws_root,
        &["diff", "--cached", "--no-renames", "-z", "--name-status"],
        &[],
    ) {
        for (_, p) in parse_name_status_z(&raw) {
            if !is_board_metadata_relpath(&p) {
                paths.insert(normalize_rel(&p));
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
                paths.insert(normalize_rel(p));
            }
        }
    }
    let mut out = BTreeMap::new();
    for rel in paths {
        out.insert(rel.clone(), read_worktree_file_digest(ws_root, &rel));
    }
    out
}

pub(crate) fn read_worktree_file_digest(ws_root: &Path, rel: &str) -> Option<String> {
    let full = ws_root.join(rel);
    let meta = fs::symlink_metadata(&full).ok()?;
    if is_symlink_or_reparse_point(&meta) || !meta.is_file() {
        return None;
    }
    fs::read(full).ok().map(|b| sha256_prefixed(&b))
}

pub(crate) fn parse_name_status_z(raw: &[u8]) -> Vec<(char, String)> {
    let tokens: Vec<&[u8]> = raw.split(|&b| b == 0).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < tokens.len() {
        if !tokens[i].is_empty()
            && !tokens[i + 1].is_empty()
            && let Ok(p) = std::str::from_utf8(tokens[i + 1])
        {
            out.push((char::from(tokens[i][0]), p.to_owned()));
        }
        i += 2;
    }
    out
}
