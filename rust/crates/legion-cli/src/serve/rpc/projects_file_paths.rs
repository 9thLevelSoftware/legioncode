//! Workspace root confinement and error builders for project file RPCs.

use std::path::{Component, Path, PathBuf};

use legion_protocol::rpc_codec::RpcExit;
use serde_json::json;

use super::super::rpc_fail_obj;

pub(crate) fn is_git_segment(seg: &str) -> bool {
    let head = seg.split(':').next().unwrap_or("");
    let cleaned = head.trim_end_matches(['.', ' ']).to_ascii_uppercase();
    cleaned == ".GIT" || cleaned == "GIT~1"
}

pub(super) fn path_err(tag: &str, cwd: &str, rel: &str, failure: &str, msg: &str) -> RpcExit {
    rpc_fail_obj(json!({
        "_tag": tag,
        "cwd": cwd,
        "relativePath": rel,
        "failure": failure,
        "message": msg,
    }))
}

pub(crate) fn resolve_confined_path_for(
    err_tag: &str,
    cwd: &str,
    relative_path: &str,
) -> Result<PathBuf, RpcExit> {
    let root = Path::new(cwd);
    if !root.exists() {
        return Err(path_err(
            err_tag,
            cwd,
            relative_path,
            "operation_failed",
            &format!("Workspace root does not exist: {cwd}"),
        ));
    }
    if !root.is_dir() {
        return Err(path_err(
            err_tag,
            cwd,
            relative_path,
            "operation_failed",
            &format!("Workspace root is not a directory: {cwd}"),
        ));
    }
    let rel = Path::new(relative_path);
    if rel.is_absolute() {
        return Err(path_err(
            err_tag,
            cwd,
            relative_path,
            "workspace_path_outside_root",
            &format!("Workspace file path must be relative to the project root: {relative_path}"),
        ));
    }
    let mut joined = PathBuf::new();
    for comp in rel.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !joined.pop() {
                    return Err(path_err(
                        err_tag,
                        cwd,
                        relative_path,
                        "resolved_path_outside_root",
                        &format!(
                            "Workspace file '{relative_path}' resolves outside workspace root '{cwd}'"
                        ),
                    ));
                }
            }
            Component::Normal(part) => {
                let text = part.to_string_lossy();
                if is_git_segment(&text) {
                    return Err(path_err(
                        err_tag,
                        cwd,
                        relative_path,
                        "resolved_path_outside_root",
                        &format!("Workspace file '{relative_path}' resolves inside .git metadata"),
                    ));
                }
                joined.push(part);
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(path_err(
                    err_tag,
                    cwd,
                    relative_path,
                    "workspace_path_outside_root",
                    &format!("Workspace file path must be relative: {relative_path}"),
                ));
            }
        }
    }
    Ok(root.join(joined))
}

pub(super) fn file_op_err(
    tag: &str,
    root: &str,
    rel: &str,
    target: &Path,
    op_path: &Path,
    op: &str,
    err: &std::io::Error,
) -> RpcExit {
    rpc_fail_obj(json!({
        "_tag": tag,
        "cwd": root,
        "relativePath": rel,
        "failure": "operation_failed",
        "resolvedPath": target.to_string_lossy(),
        "operationPath": op_path.to_string_lossy(),
        "operation": op,
        "message": err.to_string(),
    }))
}
