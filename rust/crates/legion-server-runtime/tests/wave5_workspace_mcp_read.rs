//! Wave 5 workspace scan/task folders (`#35925`), MCP OAuth sign-in (`#39921`), and
//! `legion_project_read` saved Settings actions (`#39676`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use legion_contracts::ids::{ProjectId, ProviderThreadId, ThreadId};
use legion_contracts::workspace_folders::{ProjectTrackedFolder, TaskFolder, TaskFolderRepository};
use legion_persistence::project_store::{
    CreateProjectInput, create_project, ensure_project_store_schema,
};
use legion_persistence::sqlite::SqliteStore;
use legion_scm::workspace_folders::TaskFolderCoordinator;
use legion_server_runtime::{
    McpOAuthFlowConfig, ProjectScriptAction, ProjectSettingsOverride, RuntimeHttpState,
    runtime_full_router, utc_now_iso,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn spawn_state_server(
    state: Arc<RuntimeHttpState>,
) -> Result<(String, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
    let router = runtime_full_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok((format!("http://{addr}"), handle))
}

fn init_multi_repo_workspace(ws: &Path) -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
    let repo_a = ws.join("repo-a");
    let repo_b = ws.join("repo-b");
    std::fs::create_dir_all(&repo_a)?;
    std::fs::create_dir_all(&repo_b)?;
    for r in [&repo_a, &repo_b] {
        let ok = Command::new("git")
            .arg("init")
            .arg(r)
            .output()?
            .status
            .success();
        assert!(ok);
    }
    Ok((repo_a, repo_b))
}

fn seed_multi_repo_project_and_task_folder(
    state: &Arc<RuntimeHttpState>,
    db: &Mutex<rusqlite::Connection>,
    ws: &Path,
    repo_a: &Path,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let conn = db.lock().map_err(|_| "db")?;
    ensure_project_store_schema(&conn)?;
    let created = create_project(
        &*conn,
        &CreateProjectInput::new(ws.to_string_lossy().as_ref())
            .with_project_id("proj-multi")
            .with_scripts_json(
                r#"[{"id":"stale","name":"Stale","command":"true","icon":"play","runOnWorktreeCreate":false}]"#,
            ),
    )?;
    let plan = TaskFolderCoordinator::plan(
        ProjectId::new(&created.id)?,
        ws,
        &[ProjectTrackedFolder {
            path: repo_a.to_string_lossy().into_owned(),
            name: "repo-a".to_owned(),
            discovered: None,
        }],
        None,
    )?
    .ok_or("expected task folder plan")?;
    let wt_root = ws.join("wt-task");
    let wt_repo_a = wt_root.join("repo-a");
    std::fs::create_dir_all(&wt_repo_a)?;
    let wt_repo_a_str = wt_repo_a.to_string_lossy().into_owned();
    let tf = TaskFolder {
        root: wt_root.to_string_lossy().into_owned(),
        project_id: plan.project_id,
        project_root: plan.project_root.to_string_lossy().into_owned(),
        anchor: plan.anchor.to_string_lossy().into_owned(),
        project_path: wt_repo_a_str.clone(),
        project_is_worktree: false,
        repositories: vec![TaskFolderRepository {
            repository_path: repo_a.to_string_lossy().into_owned(),
            worktree_path: wt_repo_a_str,
            name: "repo-a".to_owned(),
        }],
    };
    TaskFolderCoordinator::record_claimed(&conn, &tf, Some(&utc_now_iso()))?;
    conn.execute(
        "INSERT INTO orchestration_v2_projection_threads (thread_id, project_id, title, default_provider, runtime_mode, interaction_mode, payload_json, created_at, updated_at)
         VALUES (?1, ?2, 'Multi', 'codex', 'full-access', 'default', ?3, '2026-11-20T00:00:00Z', '2026-11-20T00:00:00Z')",
        rusqlite::params![
            "thr-multi",
            &created.id,
            serde_json::to_string(&json!({ "worktreePath": tf.repositories[0].worktree_path }))?
        ],
    )?;
    let proj_id = created.id;
    drop(conn);

    state.set_project_settings_override(
        &proj_id,
        ProjectSettingsOverride {
            default_model_selection: Some(
                json!({ "providerInstanceId": "codex", "model": "gpt-5.4" }),
            ),
            default_project_scripts: Some(vec![ProjectScriptAction {
                id: "test".to_owned(),
                name: "Test".to_owned(),
                command: "pnpm test".to_owned(),
                icon: "test".to_owned(),
                run_on_worktree_create: false,
                keybinding: None,
            }]),
            tracked_folders: None,
        },
    );
    state.mcp_sign_in.register_flow(
        &ThreadId::new("thr-multi")?,
        &ProviderThreadId::new("pt-1")?,
        "github-mcp",
        McpOAuthFlowConfig {
            redirect_uri: "http://127.0.0.1:43123/callback".to_owned(),
            authorization_url: Some("https://auth.example/authorize?state=s1".to_owned()),
            expected_state: "s1".to_owned(),
            expected_client_id: None,
        },
    )?;

    Ok((proj_id, tf.repositories[0].worktree_path.clone()))
}

async fn verify_mcp_signin_roundtrip(client: &reqwest::Client, base_url: &str) -> TestResult {
    let start_mcp: Value = client
        .post(format!("{base_url}/api/mcp/sign-in/start"))
        .json(&json!({
            "threadId": "thr-multi",
            "providerThreadId": "pt-1",
            "server": "github-mcp"
        }))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(
        start_mcp["authorizationUrl"],
        "https://auth.example/authorize?state=s1"
    );

    let bad_cb = client
        .post(format!("{base_url}/api/mcp/sign-in/complete"))
        .json(&json!({
            "threadId": "thr-multi",
            "providerThreadId": "pt-1",
            "server": "github-mcp",
            "callbackUrl": "http://127.0.0.1:43123/callback?code=c1&state=wrong"
        }))
        .send()
        .await?;
    assert_eq!(bad_cb.status(), StatusCode::BAD_REQUEST);

    let ok_cb: Value = client
        .post(format!("{base_url}/api/mcp/sign-in/complete"))
        .json(&json!({
            "threadId": "thr-multi",
            "providerThreadId": "pt-1",
            "server": "github-mcp",
            "callbackUrl": "http://127.0.0.1:43123/callback?code=c1&state=s1"
        }))
        .send()
        .await?
        .json()
        .await?;
    assert!(ok_cb["signedInAt"].as_str().is_some());

    let done_start: Value = client
        .post(format!("{base_url}/api/mcp/sign-in/start"))
        .json(&json!({
            "threadId": "thr-multi",
            "providerThreadId": "pt-1",
            "server": "github-mcp"
        }))
        .send()
        .await?
        .json()
        .await?;
    assert!(done_start["authorizationUrl"].is_null());
    assert!(done_start["signedInAt"].is_string());
    Ok(())
}

#[tokio::test]
async fn workspace_folders_mcp_signin_and_project_read_settings_override() -> TestResult {
    let temp = TempDir::new()?;
    let ws = temp.path().join("multi-ws");
    let (repo_a, repo_b) = init_multi_repo_workspace(&ws)?;
    let logs = TempDir::new()?;
    let store = SqliteStore::open_in_memory()?;
    let db = Arc::new(Mutex::new(store.into_connection()));
    let state =
        Arc::new(RuntimeHttpState::new(Arc::clone(&db), logs.path()).with_require_auth(false));
    let (proj_id, wt_path) = seed_multi_repo_project_and_task_folder(&state, &db, &ws, &repo_a)?;

    let (base_url, _h) = spawn_state_server(state).await?;
    let client = reqwest::Client::new();

    let proj_read: Value = client
        .get(format!("{base_url}/api/projects/read?projectId={proj_id}"))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(proj_read["defaultModelSelection"]["model"], "gpt-5.4");
    assert_eq!(proj_read["scripts"][0]["id"], "test");

    let scan: Value = client
        .post(format!("{base_url}/api/workspace/folders/scan"))
        .json(&json!({ "workspaceRoot": ws.to_string_lossy() }))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(scan["folders"].as_array().map(Vec::len), Some(2));

    let list: Value = client
        .get(format!(
            "{base_url}/api/workspace/folders?projectId={proj_id}"
        ))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(list["folders"].as_array().map(Vec::len), Some(2));

    let tf_get: Value = client
        .get(format!(
            "{base_url}/api/workspace/task-folders?worktreePath={wt_path}"
        ))
        .send()
        .await?
        .json()
        .await?;
    assert!(tf_get["taskFolder"].is_object());

    let added: Value = client
        .post(format!(
            "{base_url}/api/workspace/task-folders/repositories"
        ))
        .json(&json!({
            "threadId": "thr-multi",
            "repositoryPath": repo_b.to_string_lossy(),
        }))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(added["repositories"].as_array().map(Vec::len), Some(2));

    verify_mcp_signin_roundtrip(&client, &base_url).await
}
