//! Workspace entry listing (`listEntries`, `listIgnoredFiles`) and confined file read/write.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

use legion_protocol::rpc_codec::RpcExit;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;

use super::workspace_walk::collect_workspace_entries;
use super::{parse_payload, rpc_fail_obj};

const MAX_READ_FILE_BYTES: usize = 1024 * 1024;

pub(crate) fn is_git_segment(seg: &str) -> bool {
    let head = seg.split(':').next().unwrap_or("");
    let cleaned = head.trim_end_matches(['.', ' ']).to_ascii_uppercase();
    cleaned == ".GIT" || cleaned == "GIT~1"
}

pub(crate) fn resolve_confined_path(cwd: &str, relative_path: &str) -> Result<PathBuf, RpcExit> {
    let root = Path::new(cwd);
    if !root.exists() {
        return Err(rpc_fail_obj(json!({
            "_tag": "WorkspaceRootNotExistsError",
            "workspaceRoot": cwd,
            "normalizedWorkspaceRoot": cwd,
            "message": format!("Workspace root does not exist: {cwd}"),
        })));
    }
    if !root.is_dir() {
        return Err(rpc_fail_obj(json!({
            "_tag": "WorkspaceRootNotDirectoryError",
            "workspaceRoot": cwd,
            "normalizedWorkspaceRoot": cwd,
            "message": format!("Workspace root is not a directory: {cwd}"),
        })));
    }
    let rel = Path::new(relative_path);
    if rel.is_absolute() {
        return Err(rpc_fail_obj(json!({
            "_tag": "WorkspacePathOutsideRootError",
            "workspaceRoot": cwd,
            "relativePath": relative_path,
            "message": format!("Workspace file path must be relative to the project root: {relative_path}"),
        })));
    }
    let mut joined = PathBuf::new();
    for comp in rel.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !joined.pop() {
                    return Err(rpc_fail_obj(json!({
                        "_tag": "WorkspaceFilePathEscapeError",
                        "workspaceRoot": cwd,
                        "relativePath": relative_path,
                        "resolvedWorkspaceRoot": cwd,
                        "resolvedPath": relative_path,
                        "message": format!("Workspace file '{relative_path}' resolves outside workspace root '{cwd}'"),
                    })));
                }
            }
            Component::Normal(part) => {
                let text = part.to_string_lossy();
                if is_git_segment(&text) {
                    return Err(rpc_fail_obj(json!({
                        "_tag": "WorkspaceFilePathEscapeError",
                        "workspaceRoot": cwd,
                        "relativePath": relative_path,
                        "resolvedWorkspaceRoot": cwd,
                        "resolvedPath": relative_path,
                        "message": format!("Workspace file '{relative_path}' resolves inside .git metadata"),
                    })));
                }
                joined.push(part);
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(rpc_fail_obj(json!({
                    "_tag": "WorkspacePathOutsideRootError",
                    "workspaceRoot": cwd,
                    "relativePath": relative_path,
                    "message": format!("Workspace file path must be relative: {relative_path}"),
                })));
            }
        }
    }
    Ok(root.join(joined))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CwdOnlyPayload {
    cwd: String,
}

pub(crate) async fn list_entries(payload: Value) -> RpcExit {
    let req: CwdOnlyPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let root = Path::new(&req.cwd);
    if !root.is_dir() {
        return rpc_fail_obj(json!({
            "_tag": "WorkspaceRootNotExistsError",
            "workspaceRoot": req.cwd,
            "normalizedWorkspaceRoot": req.cwd,
            "message": format!("Workspace root does not exist: {}", req.cwd),
        }));
    }
    let (entries, truncated, skipped) = collect_workspace_entries(root).await;
    RpcExit::Success {
        value: json!({
            "entries": entries,
            "truncated": truncated,
            "skippedEntryCount": skipped,
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IgnoredFilesPayload {
    cwd: String,
    query: Option<String>,
    limit: Option<usize>,
}

pub(crate) async fn list_ignored_files(payload: Value) -> RpcExit {
    let req: IgnoredFilesPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let root = Path::new(&req.cwd);
    if !root.is_dir() {
        return RpcExit::Success {
            value: json!({ "paths": [], "truncated": false }),
        };
    }
    let limit = req.limit.unwrap_or(500).clamp(1, 5000);
    let needle = req.query.as_deref().map(str::to_ascii_lowercase);
    let Ok(output) = Command::new("git")
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
    else {
        return RpcExit::Success {
            value: json!({ "paths": [], "truncated": false }),
        };
    };
    if !output.status.success() {
        return RpcExit::Success {
            value: json!({ "paths": [], "truncated": false }),
        };
    }
    let mut paths = Vec::new();
    let mut truncated = false;
    for chunk in output.stdout.split(|&b| b == 0).filter(|c| !c.is_empty()) {
        let rel = String::from_utf8_lossy(chunk).into_owned();
        if let Some(ref q) = needle
            && !rel.to_ascii_lowercase().contains(q)
        {
            continue;
        }
        if paths.len() >= limit {
            truncated = true;
            break;
        }
        paths.push(rel);
    }
    RpcExit::Success {
        value: json!({ "paths": paths, "truncated": truncated }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadFilePayload {
    cwd: String,
    relative_path: String,
}

pub(crate) fn read_file(payload: Value) -> RpcExit {
    let req: ReadFilePayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let target = match resolve_confined_path(&req.cwd, &req.relative_path) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let meta = match fs::metadata(&target) {
        Ok(m) => m,
        Err(err) => {
            return file_op_err(&req.cwd, &req.relative_path, &target, &target, "stat", &err);
        }
    };
    if !meta.is_file() {
        return rpc_fail_obj(json!({
            "_tag": "WorkspacePathNotFileError",
            "workspaceRoot": req.cwd,
            "relativePath": req.relative_path,
            "resolvedPath": target.to_string_lossy(),
            "message": format!("Workspace path '{}' is not a file", req.relative_path),
        }));
    }
    let bytes = match fs::read(&target) {
        Ok(b) => b,
        Err(err) => {
            return file_op_err(&req.cwd, &req.relative_path, &target, &target, "read", &err);
        }
    };
    if bytes.len() > MAX_READ_FILE_BYTES || bytes.iter().take(8192).any(|&b| b == 0) {
        return rpc_fail_obj(json!({
            "_tag": "WorkspaceBinaryFileError",
            "workspaceRoot": req.cwd,
            "relativePath": req.relative_path,
            "resolvedPath": target.to_string_lossy(),
            "message": format!("File '{}' is binary or exceeds size limit", req.relative_path),
        }));
    }
    let text = String::from_utf8_lossy(&bytes);
    let cleaned = text.strip_prefix('\u{FEFF}').unwrap_or(&text);
    RpcExit::Success {
        value: json!({
            "relativePath": req.relative_path.replace('\\', "/"),
            "contents": cleaned,
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WriteFilePayload {
    cwd: String,
    relative_path: String,
    contents: String,
}

pub(crate) fn write_file(payload: Value) -> RpcExit {
    let req: WriteFilePayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let target = match resolve_confined_path(&req.cwd, &req.relative_path) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Some(parent) = target.parent()
        && let Err(err) = fs::create_dir_all(parent)
    {
        return file_op_err(
            &req.cwd,
            &req.relative_path,
            &target,
            parent,
            "make-directory",
            &err,
        );
    }
    let mut file = match OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&target)
    {
        Ok(f) => f,
        Err(err) => {
            return file_op_err(
                &req.cwd,
                &req.relative_path,
                &target,
                &target,
                "write-file",
                &err,
            );
        }
    };
    if let Err(err) = file.write_all(req.contents.as_bytes()) {
        return file_op_err(
            &req.cwd,
            &req.relative_path,
            &target,
            &target,
            "write-file",
            &err,
        );
    }
    RpcExit::Success {
        value: json!({
            "relativePath": req.relative_path.replace('\\', "/"),
        }),
    }
}

fn file_op_err(
    root: &str,
    rel: &str,
    target: &Path,
    op_path: &Path,
    op: &str,
    err: &std::io::Error,
) -> RpcExit {
    rpc_fail_obj(json!({
        "_tag": "WorkspaceFileSystemOperationError",
        "workspaceRoot": root,
        "relativePath": rel,
        "resolvedPath": target.to_string_lossy(),
        "operationPath": op_path.to_string_lossy(),
        "operation": op,
        "message": err.to_string(),
    }))
}
