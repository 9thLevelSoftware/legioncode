//! Test harness helpers + core server/settings/projects/fs/assets parity tests for `serve::rpc::dispatch`.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use legion_contracts::ids::EnvironmentId;
use legion_orchestrator::Orchestrator;
use legion_persistence::paths::{ServerDerivedPaths, ensure_server_directories};
use legion_persistence::sqlite::SqliteStore;
use legion_protocol::rpc_codec::{OutputWindowRegistry, RpcCauseReason, RpcExit};
use legion_server_runtime::{RuntimeHttpState, TerminalRuntime};
use serde_json::{Value, json};
use tempfile::TempDir;

use super::super::AppState;
use super::super::config::build_descriptor;
use super::dispatch;

pub(super) fn build_test_state() -> (TempDir, Arc<AppState>) {
    let temp = TempDir::new().expect("tempdir");
    let paths = ServerDerivedPaths::derive(temp.path(), false, true);
    ensure_server_directories(&paths).expect("ensure dirs");
    let store = SqliteStore::open(&paths.db_path).expect("sqlite store");
    let orchestrator = Arc::new(Orchestrator::from_sqlite_store(store));
    let terminal_runtime = Arc::new(TerminalRuntime::default());
    let runtime_http = Arc::new(
        RuntimeHttpState::new(Arc::clone(orchestrator.sink().db()), paths.logs_dir.clone())
            .with_orchestrator(Arc::clone(&orchestrator))
            .with_terminal_runtime(Arc::clone(&terminal_runtime)),
    );
    let env_id = EnvironmentId::new("env-rpc-test").expect("env id");
    let descriptor = build_descriptor(env_id, temp.path(), Some(105));
    let state = Arc::new(AppState {
        descriptor,
        orchestrator,
        runtime_http,
        terminal_runtime,
        workflow_store: None,
        paths,
        next_client_id: AtomicU64::new(1),
        live_clients: Mutex::new(HashSet::new()),
        output_windows: Mutex::new(OutputWindowRegistry::new()),
    });
    (temp, state)
}

pub(super) fn assert_ok(exit: RpcExit, label: &str) -> Value {
    match exit {
        RpcExit::Success { value } => value,
        RpcExit::Failure { cause } => panic!("{label} failed unexpectedly: {cause:?}"),
    }
}

pub(super) fn assert_err_tag(exit: RpcExit, expected_tag: &str, label: &str) -> Value {
    match exit {
        RpcExit::Failure { cause } => {
            let Some(RpcCauseReason::Fail { error }) = cause.into_iter().next() else {
                panic!("{label}: expected Fail reason");
            };
            assert_eq!(
                error.get("_tag").and_then(Value::as_str),
                Some(expected_tag),
                "{label} error tag mismatch: {error}"
            );
            error
        }
        other @ RpcExit::Success { .. } => {
            panic!("{label}: expected failure {expected_tag}, got {other:?}")
        }
    }
}

pub(super) fn init_git_repo(dir: &Path) {
    let run = |args: &[&str]| {
        let status = StdCommand::new("git")
            .current_dir(dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(args)
            .status()
            .expect("git exec");
        assert!(status.success(), "git {args:?} failed");
    };
    run(&["init"]);
    run(&["config", "user.email", "test@legion.local"]);
    run(&["config", "user.name", "Legion Test"]);
    fs::write(dir.join("README.md"), "# Legion Test\nhello world\n").expect("write readme");
    run(&["add", "."]);
    run(&["commit", "-m", "initial commit"]);
}

#[tokio::test]
async fn server_probe_config_settings_and_keybindings_persist() {
    let (_tmp, state) = build_test_state();

    let probe = assert_ok(
        dispatch(&state, "server.probe", json!({})).await,
        "server.probe",
    );
    assert!(probe["serverTime"].as_str().is_some_and(|s| !s.is_empty()));

    let cfg = assert_ok(
        dispatch(&state, "server.getConfig", json!({})).await,
        "server.getConfig",
    );
    assert_eq!(cfg["shellThreadFamilies"], true);
    assert_eq!(cfg["shellOlderSettled"], true);
    assert_eq!(cfg["threadSnapshotPagination"], true);
    assert!(
        cfg["providers"]
            .as_array()
            .expect("providers array")
            .iter()
            .any(|p| p["driver"] == "devReplay" && p["instanceId"] == "devReplay")
    );

    let updated = assert_ok(
        dispatch(
            &state,
            "server.updateSettings",
            json!({ "responseStreamingMode": "token" }),
        )
        .await,
        "server.updateSettings",
    );
    assert_eq!(updated["responseStreamingMode"], "token");

    let reloaded = assert_ok(
        dispatch(&state, "server.getSettings", json!({})).await,
        "server.getSettings",
    );
    assert_eq!(reloaded["responseStreamingMode"], "token");

    let kb = assert_ok(
        dispatch(
            &state,
            "server.upsertKeybinding",
            json!({ "key": "mod+k", "command": "commandPalette.toggle" }),
        )
        .await,
        "server.upsertKeybinding",
    );
    assert_eq!(kb["keybindings"].as_array().map(Vec::len), Some(1));

    let after_rm = assert_ok(
        dispatch(
            &state,
            "server.removeKeybinding",
            json!({ "key": "mod+k", "command": "commandPalette.toggle" }),
        )
        .await,
        "server.removeKeybinding",
    );
    assert_eq!(after_rm["keybindings"], json!([]));
}

#[tokio::test]
async fn projects_fs_read_write_and_search_work() {
    let (tmp, state) = build_test_state();
    let repo_dir = tmp.path().join("workspace");
    fs::create_dir_all(&repo_dir).expect("mkdir");
    init_git_repo(&repo_dir);
    fs::write(repo_dir.join(".gitignore"), "ignored.log\n").expect("gitignore");
    fs::write(repo_dir.join("ignored.log"), "secret ignored log\n").expect("ignored");
    let repo_str = repo_dir.to_string_lossy().into_owned();

    let created = assert_ok(
        dispatch(
            &state,
            "projects.mutate",
            json!({ "type": "create", "workspaceRoot": repo_str, "title": "Alpha" }),
        )
        .await,
        "projects.mutate",
    );
    assert!(created["id"].is_string());

    let entries = assert_ok(
        dispatch(&state, "projects.listEntries", json!({ "cwd": repo_str })).await,
        "projects.listEntries",
    );
    assert!(
        entries["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .any(|e| e["path"] == "README.md")
    );
    assert!(
        !entries["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .any(|e| e["path"] == "ignored.log")
    );

    let ignored = assert_ok(
        dispatch(
            &state,
            "projects.listIgnoredFiles",
            json!({ "cwd": repo_str, "query": "ignored" }),
        )
        .await,
        "projects.listIgnoredFiles",
    );
    assert!(
        ignored["files"]
            .as_array()
            .expect("ignored files")
            .iter()
            .any(|f| f["path"] == "ignored.log")
    );

    let _ = assert_ok(
        dispatch(
            &state,
            "projects.writeFile",
            json!({ "cwd": repo_str, "relativePath": "src/lib.rs", "contents": "pub fn greet() {}\n" }),
        )
        .await,
        "projects.writeFile",
    );
    let read = assert_ok(
        dispatch(
            &state,
            "projects.readFile",
            json!({ "cwd": repo_str, "relativePath": "src/lib.rs" }),
        )
        .await,
        "projects.readFile",
    );
    assert_eq!(read["contents"], "pub fn greet() {}\n");

    let _ = assert_err_tag(
        dispatch(
            &state,
            "projects.readFile",
            json!({ "cwd": repo_str, "relativePath": ".git/config" }),
        )
        .await,
        "ProjectReadFileError",
        "escape .git",
    );

    let search = assert_ok(
        dispatch(
            &state,
            "projects.searchContents",
            json!({ "cwd": repo_str, "query": "greet", "wholeWord": true }),
        )
        .await,
        "projects.searchContents",
    );
    assert_eq!(search["matches"].as_array().expect("matches").len(), 1);
}
