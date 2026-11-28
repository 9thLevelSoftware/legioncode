//! Multi-repo workspace folder RPC handlers (`workspace.{listFolders,scanFolders,taskFolder,addTaskFolderRepository}`).

use legion_contracts::workspace_folders::{
    WorkspaceAddTaskFolderRepositoryInput, WorkspaceListFoldersInput, WorkspaceScanFoldersInput,
    WorkspaceTaskFolderInput,
};
use legion_protocol::rpc_codec::RpcExit;
use legion_server_runtime::{
    ServerRuntimeError, add_task_folder_repository_service, get_workspace_task_folder_service,
    list_workspace_folders_service, scan_workspace_folders_service,
};
use serde_json::{Value, json};

use super::{AppState, parse_payload, rpc_fail, rpc_fail_obj};

pub(crate) fn dispatch(state: &AppState, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "workspace.listFolders" => {
            let req: WorkspaceListFoldersInput = match parse_payload(payload) {
                Ok(p) => p,
                Err(e) => return e,
            };
            match list_workspace_folders_service(&state.runtime_http, &req) {
                Ok(res) => RpcExit::Success { value: json!(res) },
                Err(err) => map_workspace_err("WorkspaceFoldersError", "list", &err),
            }
        }
        "workspace.scanFolders" => {
            let req: WorkspaceScanFoldersInput = match parse_payload(payload) {
                Ok(p) => p,
                Err(e) => return e,
            };
            match scan_workspace_folders_service(&req) {
                Ok(res) => RpcExit::Success { value: json!(res) },
                Err(err) => map_workspace_err("WorkspaceFoldersError", "scan", &err),
            }
        }
        "workspace.taskFolder" => {
            let req: WorkspaceTaskFolderInput = match parse_payload(payload) {
                Ok(p) => p,
                Err(e) => return e,
            };
            match get_workspace_task_folder_service(&state.runtime_http, &req) {
                Ok(res) => RpcExit::Success { value: json!(res) },
                Err(err) => map_workspace_err("TaskFolderError", "read", &err),
            }
        }
        "workspace.addTaskFolderRepository" => {
            let req: WorkspaceAddTaskFolderRepositoryInput = match parse_payload(payload) {
                Ok(p) => p,
                Err(e) => return e,
            };
            match add_task_folder_repository_service(&state.runtime_http, &req) {
                Ok(res) => RpcExit::Success { value: json!(res) },
                Err(err) => map_workspace_err("TaskFolderError", "add-repository", &err),
            }
        }
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

fn map_workspace_err(tag: &str, operation: &str, err: &ServerRuntimeError) -> RpcExit {
    rpc_fail_obj(json!({
        "_tag": tag,
        "operation": operation,
        "detail": err.to_string(),
        "message": err.to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use legion_contracts::workspace_folders::{TaskFolder, TaskFolderRepository};
    use legion_scm::workspace_folders::TaskFolderCoordinator;
    use serde_json::json;

    use super::super::dispatch;
    use super::super::tests_core::{assert_err_tag, assert_ok, build_test_state};

    #[tokio::test]
    async fn workspace_folder_rpcs_return_direct_task_folder_and_exact_error_tags()
    -> Result<(), Box<dyn std::error::Error>> {
        let (tmp, state) = build_test_state();
        let list_err = assert_err_tag(
            dispatch(
                &state,
                "workspace.listFolders",
                json!({ "projectId": "missing" }),
            )
            .await,
            "WorkspaceFoldersError",
            "workspace.listFolders",
        );
        assert_eq!(list_err["operation"], "list");

        let scan_err = assert_err_tag(
            dispatch(
                &state,
                "workspace.scanFolders",
                json!({ "workspaceRoot": tmp.path().join("nope") }),
            )
            .await,
            "WorkspaceFoldersError",
            "workspace.scanFolders",
        );
        assert_eq!(scan_err["operation"], "scan");

        let add_err = assert_err_tag(
            dispatch(
                &state,
                "workspace.addTaskFolderRepository",
                json!({ "threadId": "missing-thr", "repositoryPath": tmp.path() }),
            )
            .await,
            "TaskFolderError",
            "workspace.addTaskFolderRepository",
        );
        assert_eq!(add_err["operation"], "add-repository");

        let anchor = tmp.path().join("ws");
        let task_root = tmp.path().join("tasks/t1");
        let wt_a = task_root.join("repo-a");
        let repo_a = anchor.join("repo-a");
        let repo_b = anchor.join("repo-b");
        std::fs::create_dir_all(&wt_a)?;
        std::fs::create_dir_all(&repo_b)?;
        let tf = seed_task_folder_thread(&state, &task_root, &anchor, &repo_a, &wt_a).await?;
        let added = assert_ok(
            dispatch(
                &state,
                "workspace.addTaskFolderRepository",
                json!({ "threadId": "t1", "repositoryPath": repo_b }),
            )
            .await,
            "workspace.addTaskFolderRepository",
        );
        assert!(added.get("taskFolder").is_none());
        assert_eq!(added["anchor"], tf.anchor);
        assert_eq!(added["repositories"].as_array().map(Vec::len), Some(2));
        assert_eq!(
            added["repositories"][1]["repositoryPath"],
            repo_b.to_string_lossy().as_ref()
        );
        Ok(())
    }

    async fn seed_task_folder_thread(
        state: &crate::serve::AppState,
        task_root: &std::path::Path,
        anchor: &std::path::Path,
        repo_a: &std::path::Path,
        wt_a: &std::path::Path,
    ) -> Result<TaskFolder, Box<dyn std::error::Error>> {
        use legion_contracts::ids::{CommandId, ProjectId, ProviderInstanceId, ThreadId};
        use legion_contracts::orchestration::{
            CreationSource, ModelSelection, OrchestrationActor, OrchestrationCommand,
            ProviderInteractionMode, ThreadCreateCommand,
        };
        use legion_contracts::policy::RuntimeMode;
        use legion_persistence::project_store::{
            CreateProjectInput, create_project, ensure_project_store_schema,
        };
        let tf = TaskFolder {
            root: task_root.to_string_lossy().into_owned(),
            project_id: ProjectId::new("p1")?,
            project_root: repo_a.to_string_lossy().into_owned(),
            anchor: anchor.to_string_lossy().into_owned(),
            project_path: wt_a.to_string_lossy().into_owned(),
            project_is_worktree: true,
            repositories: vec![TaskFolderRepository {
                repository_path: repo_a.to_string_lossy().into_owned(),
                worktree_path: wt_a.to_string_lossy().into_owned(),
                name: "repo-a".to_owned(),
            }],
        };
        {
            let conn = state.orchestrator.sink().db().lock().map_err(|_| "db")?;
            ensure_project_store_schema(&conn)?;
            let _ = create_project(
                &*conn,
                &CreateProjectInput::new(repo_a.to_string_lossy().as_ref())
                    .with_project_id("p1")
                    .with_title("Repo"),
            )?;
            TaskFolderCoordinator::record_claimed(&conn, &tf, None)?;
        }
        let cmd = OrchestrationCommand::ThreadCreate(ThreadCreateCommand {
            created_by: OrchestrationActor::User,
            creation_source: CreationSource::Web,
            command_id: CommandId::new("cmd-tf-1")?,
            thread_id: ThreadId::new("t1")?,
            project_id: ProjectId::new("p1")?,
            title: "T1".to_owned(),
            generate_title: None,
            model_selection: ModelSelection {
                instance_id: ProviderInstanceId::new("codex")?,
                model: "gpt-5".to_owned(),
                options: None,
            },
            runtime_mode: RuntimeMode::ApprovalRequired,
            interaction_mode: ProviderInteractionMode::Default,
            branch: None,
            worktree_path: Some(wt_a.to_string_lossy().into_owned()),
            link_origin: None,
            delegated_from: None,
            imported_native_thread: None,
            initial_message: None,
            title_renamed: None,
            workflow_run: None,
        });
        let _ = state.orchestrator.dispatch_now(&cmd).await?;
        Ok(tf)
    }
}
