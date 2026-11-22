//! Operational `server.*` RPCs: storage cleanup, DB compaction, triage, source-control/secret checks.

use std::fs;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use legion_persistence::maintenance::inspect_database;
use legion_persistence::triage_session::{
    TriageEntry, WriteTriageSessionInput, describe_server_process, write_triage_session,
};
use legion_protocol::rpc_codec::RpcExit;
use legion_providers::drivers::BUILTIN_DRIVER_SPECS;
use legion_remote::secrets::{FileSecretStore, SecretRef};
use legion_server_runtime::storage_cleanup::{
    OsCleanupGitStatusRunner, evaluate_and_clean_worktrees,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;

use super::projects_fs::base_dir_from_state;
use super::{
    AppState, parse_optional_payload, rpc_fail_obj, server_telemetry, server_updates, utc_now_iso,
    with_db,
};

pub(crate) async fn dispatch(state: &Arc<AppState>, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "server.getStorageCleanupReport" => get_storage_cleanup_report(state, false),
        "server.runStorageCleanup" => get_storage_cleanup_report(state, true),
        "server.getDatabaseStorage" => get_database_storage(state),
        "server.reclaimDatabaseSpace" => reclaim_database_space(state),
        "server.discoverSourceControl" => discover_source_control(payload).await,
        "server.checkSecretSource" => check_secret_source(state, payload),
        "server.prepareTriage" => prepare_triage(state),
        "server.listTextGeneratorModels" => list_text_generator_models(),
        "server.checkOtlpEndpoint"
        | "server.signalProcess"
        | "server.reportClientActivity"
        | "server.getNpmRelease" => server_updates::dispatch(tag, payload).await,
        _ => server_telemetry::dispatch(state, tag, payload),
    }
}

fn get_storage_cleanup_report(state: &AppState, apply_changes: bool) -> RpcExit {
    let started_at = utc_now_iso();
    let mut runner = OsCleanupGitStatusRunner;
    let mut entries = Vec::new();
    let mut removed = 0_u64;
    let mut kept = 0_u64;
    let failed = 0_u64;
    let candidates = collect_worktree_candidates(&state.paths.worktrees_dir);
    if state.paths.worktrees_dir.is_dir() && !candidates.is_empty() {
        let dry_run = !apply_changes;
        let eval = evaluate_and_clean_worktrees(&candidates, &mut runner, dry_run);
        for item in eval {
            let outcome = if item.removed {
                removed += 1;
                "removed"
            } else {
                kept += 1;
                "kept"
            };
            entries.push(json!({
                "kind": "worktree",
                "outcome": outcome,
                "reason": item.reason,
                "path": item.path,
                "projectPath": Value::Null,
                "threadId": Value::Null,
                "threadTitle": Value::Null,
                "bytes": Value::Null,
                "files": Value::Null,
            }));
        }
    }
    RpcExit::Success {
        value: json!({
            "trigger": if apply_changes { "manual" } else { "automatic" },
            "startedAt": started_at,
            "finishedAt": utc_now_iso(),
            "entries": entries,
            "counts": { "removed": removed, "kept": kept, "failed": failed },
            "omittedCount": 0,
            "bytesFreed": 0,
        }),
    }
}

fn collect_worktree_candidates(
    _dir: &Path,
) -> Vec<legion_server_runtime::storage_cleanup::WorktreeCleanupCandidate> {
    Vec::new()
}

fn get_database_storage(state: &AppState) -> RpcExit {
    match inspect_database(&state.paths.db_path) {
        Ok(status) => RpcExit::Success {
            value: json!({
                "databaseBytes": status.database_bytes,
                "reclaimableBytes": status.reclaimable_bytes,
                "incrementalVacuumEnabled": status.incremental_vacuum_enabled,
                "compactCommand": "legioncode doctor --fix",
                "canReclaim": true,
            }),
        },
        Err(_) => RpcExit::Success { value: Value::Null },
    }
}

fn reclaim_database_space(state: &AppState) -> RpcExit {
    match with_db(state, |conn| {
        conn.execute_batch("PRAGMA incremental_vacuum; PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|err| {
                rpc_fail_obj(json!({
                    "_tag": "DatabaseReclaimError",
                    "reason": err.to_string(),
                    "message": err.to_string(),
                }))
            })
    }) {
        Ok(()) => RpcExit::Success { value: Value::Null },
        Err(e) => e,
    }
}

async fn discover_source_control(_payload: Value) -> RpcExit {
    let gh_ok = Command::new("gh")
        .args(["auth", "status"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success());
    let git_ok = Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success());
    RpcExit::Success {
        value: json!({
            "versionControlSystems": [{
                "kind": "git",
                "implemented": true,
                "label": "Git",
                "status": if git_ok { "available" } else { "missing" },
                "version": { "_tag": "None" },
                "installHint": "Install Git from https://git-scm.com/downloads",
                "detail": { "_tag": "None" },
            }],
            "sourceControlProviders": [{
                "kind": "github",
                "label": "GitHub",
                "status": if gh_ok { "available" } else { "missing" },
                "version": { "_tag": "None" },
                "installHint": "Install GitHub CLI (gh) and run `gh auth login`",
                "detail": { "_tag": "None" },
                "auth": {
                    "status": if gh_ok { "authenticated" } else { "unauthenticated" },
                    "account": { "_tag": "None" },
                    "host": { "_tag": "Some", "value": "github.com" },
                    "detail": { "_tag": "None" },
                },
            }],
        }),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct CheckSecretSourcePayload {
    source: Option<Value>,
    reference: Option<Value>,
    value: Option<String>,
}

fn check_secret_source(state: &AppState, payload: Value) -> RpcExit {
    let req: CheckSecretSourcePayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let candidate = req
        .source
        .or(req.reference)
        .unwrap_or_else(|| json!({ "value": req.value.unwrap_or_default() }));
    if let Ok(parsed) =
        serde_json::from_value::<SecretRef>(candidate.get("value").cloned().unwrap_or(candidate))
    {
        if parsed.vault().is_empty() {
            return RpcExit::Success {
                value: json!({ "_tag": "Failed", "detail": "Secret vault identifier is empty." }),
            };
        }
        return RpcExit::Success {
            value: json!({ "_tag": "Resolved" }),
        };
    }
    let _ = fs::create_dir_all(&state.paths.secrets_dir);
    match FileSecretStore::new(&state.paths.secrets_dir) {
        Ok(_) => RpcExit::Success {
            value: json!({ "_tag": "Resolved" }),
        },
        Err(err) => RpcExit::Success {
            value: json!({ "_tag": "Failed", "detail": err.to_string() }),
        },
    }
}

fn prepare_triage(state: &AppState) -> RpcExit {
    let base_dir = base_dir_from_state(state);
    let server = describe_server_process(&state.paths.server_runtime_state_path)
        .unwrap_or_else(|e| format!("unknown ({e})"));
    match write_triage_session(&WriteTriageSessionInput {
        base_dir,
        paths: state.paths.clone(),
        entry: TriageEntry::Cli {
            cli_version: env!("CARGO_PKG_VERSION").to_owned(),
            launched_as: "legioncode triage".to_owned(),
        },
        server,
        source_clone: None,
        now_ms: None,
        os_label: None,
        runtime_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
    }) {
        Ok(session) => RpcExit::Success {
            value: json!({
                "prompt": format!("Review triage context in `{}`.", session.prompt_file_path.display()),
            }),
        },
        Err(err) => rpc_fail_obj(json!({
            "_tag": "ServerPrepareTriageError",
            "cause": err.to_string(),
        })),
    }
}

fn list_text_generator_models() -> RpcExit {
    let models: Vec<Value> = BUILTIN_DRIVER_SPECS
        .iter()
        .filter(|spec| spec.supports_text_generation)
        .map(|spec| {
            json!({
                "id": spec.slug,
                "name": spec.display_name,
            })
        })
        .collect();
    RpcExit::Success {
        value: json!({ "models": models }),
    }
}
