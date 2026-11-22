//! Workspace entry listing (`listEntries`, `listIgnoredFiles`) and confined file read/write.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;
use std::process::Stdio;

use legion_protocol::rpc_codec::RpcExit;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;

use super::workspace_walk::collect_workspace_entries;
use super::{parse_payload, rpc_fail_obj};

#[path = "projects_file_paths.rs"]
mod file_paths;
use file_paths::{file_op_err, path_err};
pub(crate) use file_paths::{is_git_segment, resolve_confined_path_for};

const MAX_READ_FILE_BYTES: usize = 1024 * 1024;

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
            "_tag": "ProjectListEntriesError",
            "cwd": req.cwd,
            "failure": "workspace_root_not_found",
            "normalizedCwd": req.cwd,
            "message": format!("Workspace root does not exist: {}", req.cwd),
        }));
    }
    let (entries, truncated, _skipped) = collect_workspace_entries(root).await;
    RpcExit::Success {
        value: json!({
            "entries": entries,
            "truncated": truncated,
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
            value: json!({ "files": [], "truncated": false }),
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
            value: json!({ "files": [], "truncated": false }),
        };
    };
    if !output.status.success() {
        return RpcExit::Success {
            value: json!({ "files": [], "truncated": false }),
        };
    }
    let mut files = Vec::new();
    let mut truncated = false;
    for chunk in output.stdout.split(|&b| b == 0).filter(|c| !c.is_empty()) {
        let rel = String::from_utf8_lossy(chunk).into_owned();
        if let Some(ref q) = needle
            && !rel.to_ascii_lowercase().contains(q)
        {
            continue;
        }
        if files.len() >= limit {
            truncated = true;
            break;
        }
        let byte_length = fs::metadata(root.join(&rel)).map_or(0, |m| m.len());
        files.push(json!({ "path": rel, "byteLength": byte_length }));
    }
    RpcExit::Success {
        value: json!({ "files": files, "truncated": truncated }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadFilePayload {
    cwd: String,
    relative_path: String,
    #[serde(default)]
    allow_missing: Option<bool>,
}

pub(crate) fn read_file(payload: Value) -> RpcExit {
    let req: ReadFilePayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let target =
        match resolve_confined_path_for("ProjectReadFileError", &req.cwd, &req.relative_path) {
            Ok(p) => p,
            Err(e) => return e,
        };
    let rel_norm = req.relative_path.replace('\\', "/");
    let meta = match fs::metadata(&target) {
        Ok(m) => m,
        Err(err)
            if err.kind() == std::io::ErrorKind::NotFound && req.allow_missing == Some(true) =>
        {
            return RpcExit::Success {
                value: json!({
                    "relativePath": rel_norm,
                    "contents": "",
                    "byteLength": 0,
                    "truncated": false,
                    "exists": false,
                }),
            };
        }
        Err(err) => {
            return file_op_err(
                "ProjectReadFileError",
                &req.cwd,
                &req.relative_path,
                &target,
                &target,
                "stat",
                &err,
            );
        }
    };
    if !meta.is_file() {
        return path_err(
            "ProjectReadFileError",
            &req.cwd,
            &req.relative_path,
            "path_not_file",
            &format!("Workspace path '{}' is not a file", req.relative_path),
        );
    }
    let bytes = match fs::read(&target) {
        Ok(b) => b,
        Err(err) => {
            return file_op_err(
                "ProjectReadFileError",
                &req.cwd,
                &req.relative_path,
                &target,
                &target,
                "read",
                &err,
            );
        }
    };
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return path_err(
            "ProjectReadFileError",
            &req.cwd,
            &req.relative_path,
            "binary_file",
            &format!("File '{}' is binary", req.relative_path),
        );
    }
    let truncated = bytes.len() > MAX_READ_FILE_BYTES;
    let slice = if truncated {
        &bytes[..MAX_READ_FILE_BYTES]
    } else {
        &bytes[..]
    };
    let text = String::from_utf8_lossy(slice);
    let cleaned = text.strip_prefix('\u{FEFF}').unwrap_or(&text);
    RpcExit::Success {
        value: json!({
            "relativePath": rel_norm,
            "contents": cleaned,
            "byteLength": meta.len(),
            "truncated": truncated,
            "exists": true,
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
    let target =
        match resolve_confined_path_for("ProjectWriteFileError", &req.cwd, &req.relative_path) {
            Ok(p) => p,
            Err(e) => return e,
        };
    if let Some(parent) = target.parent()
        && let Err(err) = fs::create_dir_all(parent)
    {
        return file_op_err(
            "ProjectWriteFileError",
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
                "ProjectWriteFileError",
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
            "ProjectWriteFileError",
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
