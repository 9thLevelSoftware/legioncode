//! Host directory browsing, file metadata lookup, and external editor launch.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use legion_protocol::rpc_codec::RpcExit;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{AppState, parse_payload, rpc_fail, rpc_fail_obj};

pub(crate) fn dispatch(state: &AppState, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "filesystem.browse" => browse_filesystem(payload),
        "filesystem.getMetadata" => get_metadata(state, payload),
        "shell.openInEditor" => open_in_editor(payload),
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowsePayload {
    partial_path: String,
    cwd: Option<String>,
}

fn browse_filesystem(payload: Value) -> RpcExit {
    let req: BrowsePayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let partial = req.partial_path.trim();
    let ends_sep =
        partial.ends_with('/') || partial.ends_with('\\') || partial == "~" || partial.is_empty();
    let resolved = resolve_browse_path(partial, req.cwd.as_deref());
    let parent = if ends_sep {
        resolved.clone()
    } else {
        resolved
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    };
    let prefix = if ends_sep {
        String::new()
    } else {
        resolved
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
    };
    let show_hidden = ends_sep || prefix.starts_with('.');
    let rd = match fs::read_dir(&parent) {
        Ok(rd) => rd,
        Err(err) => {
            return rpc_fail_obj(json!({
                "_tag": "FilesystemBrowseError",
                "partialPath": req.partial_path,
                "cwd": req.cwd,
                "failure": "read_directory_failed",
                "parentPath": parent.to_string_lossy(),
                "pathMissing": !parent.exists(),
                "message": err.to_string(),
            }));
        }
    };
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut symlink_stats = 0usize;
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        let is_dir = if ft.is_dir() {
            true
        } else if ft.is_symlink() && symlink_stats < 32 {
            symlink_stats += 1;
            fs::metadata(entry.path()).is_ok_and(|m| m.is_dir())
        } else {
            false
        };
        if !is_dir {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        if !name.to_ascii_lowercase().starts_with(&prefix) {
            continue;
        }
        entries.push((name, entry.path().to_string_lossy().into_owned()));
    }
    entries.sort_by_key(|a| a.0.to_ascii_lowercase());
    let json_entries: Vec<Value> = entries
        .into_iter()
        .map(|(name, full_path)| json!({ "name": name, "fullPath": full_path }))
        .collect();
    RpcExit::Success {
        value: json!({
            "parentPath": parent.to_string_lossy(),
            "entries": json_entries,
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetMetadataPayload {
    paths: Vec<String>,
}

fn get_metadata(state: &AppState, payload: Value) -> RpcExit {
    let req: GetMetadataPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let state_dir = &state.paths.state_dir;
    let entries: Vec<Value> = req
        .paths
        .iter()
        .map(|raw| {
            let p = expand_tilde(raw);
            if !p.is_absolute() || p.starts_with(state_dir) {
                return Value::Null;
            }
            let Ok(meta) = fs::metadata(&p) else {
                return Value::Null;
            };
            if meta.is_dir() {
                json!({ "kind": "directory" })
            } else if meta.is_file() {
                json!({ "kind": "file", "byteLength": meta.len() })
            } else {
                json!({ "kind": "other" })
            }
        })
        .collect();
    RpcExit::Success {
        value: json!({ "entries": entries }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenInEditorPayload {
    cwd: String,
    editor: String,
    #[serde(default)]
    reveal: Option<bool>,
}

fn open_in_editor(payload: Value) -> RpcExit {
    let req: OpenInEditorPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let target = req.cwd.trim();
    if target.is_empty()
        || target.contains('\n')
        || target.contains('\r')
        || target.contains('"')
        || target.contains('\0')
    {
        return rpc_fail_obj(json!({
            "_tag": "ExternalLauncherUnsupportedTargetError",
            "editor": req.editor,
            "message": "Target path is empty or contains invalid characters.",
        }));
    }
    if req.reveal == Some(true) && !Path::new(target).exists() {
        return rpc_fail_obj(json!({
            "_tag": "ExternalLauncherRevealTargetMissingError",
            "editor": req.editor,
            "target": target,
            "message": format!("No file or folder exists at {target}"),
        }));
    }
    let Some(cmd_name) = map_editor_command(&req.editor) else {
        return rpc_fail_obj(json!({
            "_tag": "ExternalLauncherUnknownEditorError",
            "editor": req.editor,
            "message": format!("Unknown editor: {}", req.editor),
        }));
    };
    let mut cmd = std::process::Command::new(cmd_name);
    if req.editor == "file-manager" {
        if cfg!(windows) && req.reveal == Some(true) {
            cmd.arg(format!("/select,{target}"));
        } else if cfg!(target_os = "macos") && req.reveal == Some(true) {
            cmd.args(["-R", target]);
        } else {
            let dir_target = Path::new(target)
                .parent()
                .filter(|_| req.reveal == Some(true) && !Path::new(target).is_dir())
                .unwrap_or_else(|| Path::new(target));
            cmd.arg(dir_target);
        }
    } else {
        if req.editor == "cursor" {
            cmd.arg("--classic");
        } else if req.editor == "kiro" {
            cmd.arg("ide");
        }
        cmd.arg(target);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match cmd.spawn() {
        Ok(_child) => RpcExit::Success { value: Value::Null },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => rpc_fail_obj(json!({
            "_tag": "ExternalLauncherCommandNotFoundError",
            "editor": req.editor,
            "command": cmd_name,
            "message": format!("Editor command not found: {cmd_name}"),
        })),
        Err(err) => rpc_fail_obj(json!({
            "_tag": "ExternalLauncherEditorSpawnError",
            "editor": req.editor,
            "command": cmd_name,
            "args": [req.cwd],
            "target": req.cwd,
            "message": err.to_string(),
        })),
    }
}

fn map_editor_command(editor: &str) -> Option<&'static str> {
    match editor {
        "cursor" => Some("cursor"),
        "vscode" => Some("code"),
        "vscode-insiders" => Some("code-insiders"),
        "vscodium" => Some("codium"),
        "zed" => Some("zed"),
        "antigravity" => Some("antigravity-ide"),
        "kiro" => Some("kiro"),
        "trae" => Some("trae"),
        "idea" => Some("idea"),
        "webstorm" => Some("webstorm"),
        "goland" => Some("goland"),
        "pycharm" => Some("pycharm"),
        "rustrover" => Some("rustrover"),
        "file-manager" if cfg!(windows) => Some("explorer.exe"),
        "file-manager" if cfg!(target_os = "macos") => Some("open"),
        "file-manager" => Some("xdg-open"),
        _ => None,
    }
}

fn resolve_browse_path(partial: &str, cwd: Option<&str>) -> PathBuf {
    if partial.is_empty() || partial == "~" {
        return home_dir_path();
    }
    let expanded = expand_tilde(partial);
    if expanded.is_absolute() {
        expanded
    } else if let Some(base) = cwd {
        Path::new(base).join(expanded)
    } else {
        home_dir_path().join(expanded)
    }
}

fn expand_tilde(raw: &str) -> PathBuf {
    if raw == "~" {
        return home_dir_path();
    }
    if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        return home_dir_path().join(rest);
    }
    PathBuf::from(raw)
}

fn home_dir_path() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}
