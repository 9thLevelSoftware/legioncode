//! Tests for multi-repo workspace folder scanning (`Child` + `.code-workspace`, `legioncode.json`,
//! `.gitmodules`) and SQLite task-folder coordinator (`60863edc41` `#35925`).

use std::fs;
use std::path::PathBuf;

use legion_contracts::ids::ProjectId;
use legion_contracts::workspace_folders::{
    PROJECT_TRACKED_FOLDERS_MAX, ProjectTrackedFolder, TASK_FOLDER_DEFAULT_REPOSITORIES_MAX,
    TaskFolder, TaskFolderOperation, TaskFolderRepository, WorkspaceScanFoldersInput,
    WorkspaceScannedFolderSource,
};
use legion_persistence::sqlite::SqliteStore;
use legion_scm::workspace_folders::{
    TaskFolderCoordinator, build_mirrored_worktree_path, common_ancestor, discover_project_folders,
    layout_anchor, merge_project_folders, parse_gitmodules_paths, scan_workspace_folders,
    strip_lenient_jsonc,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn workspace_scanner_handles_children_code_workspace_gitmodules_and_legioncode_json() -> TestResult
{
    let dir = tempfile::tempdir()?;
    let root = dir.path();

    // Direct child repo + nested repo under packages/ + submodule + ignored repo + plain worktree file.
    fs::create_dir_all(root.join("api").join(".git"))?;
    fs::create_dir_all(root.join("packages").join("web").join(".git"))?;
    fs::create_dir_all(root.join("vendor-sub").join(".git"))?;
    fs::create_dir_all(root.join("ignored-repo").join(".git"))?;
    fs::create_dir_all(root.join("linked-wt"))?;
    fs::write(
        root.join("linked-wt").join(".git"),
        "gitdir: ../.git/worktrees/wt\n",
    )?;
    fs::create_dir_all(root.join("docs"))?;

    fs::write(
        root.join(".gitmodules"),
        "[submodule \"vendor\"]\n\tpath = \"vendor-sub\"\n",
    )?;
    assert_eq!(
        parse_gitmodules_paths("[sub]\n  path = \"foo\\bar/\"\n"),
        vec!["foo/bar"]
    );

    // Lenient .code-workspace with comments and trailing commas adding `docs` (plain folder).
    let raw_ws = r#"
    {
      // workspace folders
      "folders": [
        { "path": "./docs", "name": "Docs Folder" },
      ],
    }
    "#;
    assert!(strip_lenient_jsonc(raw_ws).contains("\"Docs Folder\""));
    fs::write(root.join("mono.code-workspace"), raw_ws)?;
    fs::write(
        root.join("legioncode.json"),
        r#"{ "ignoreRepositories": ["ignored-repo"] }"#,
    )?;

    let scan = scan_workspace_folders(
        &WorkspaceScanFoldersInput {
            workspace_root: root.to_string_lossy().into_owned(),
        },
        None,
    )?;
    assert!(scan.code_workspace_file.is_some());
    assert!(scan.folders.iter().any(|f| f.name == "Docs Folder"
        && !f.is_git_repository
        && f.source == WorkspaceScannedFolderSource::CodeWorkspace));
    assert!(scan.folders.iter().any(|f| f.name == "api"
        && f.is_git_repository
        && f.source == WorkspaceScannedFolderSource::Child));
    assert!(
        scan.folders
            .iter()
            .any(|f| f.name == "web" && f.is_git_repository)
    );
    assert!(!scan.folders.iter().any(|f| f.name == "vendor-sub"));
    assert!(!scan.folders.iter().any(|f| f.name == "linked-wt"));

    // discover_project_folders keeps git repos inside root and excludes legioncode.json ignoreRepositories & plain docs.
    let discovered = discover_project_folders(root);
    let names: Vec<&str> = discovered.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["api", "web"]);

    // merge_project_folders caps cleanly at PROJECT_TRACKED_FOLDERS_MAX favoring manual additions.
    let many: Vec<ProjectTrackedFolder> = (0..PROJECT_TRACKED_FOLDERS_MAX + 5)
        .map(|i| ProjectTrackedFolder {
            path: format!("/repos/d-{i}"),
            name: format!("d-{i}"),
            discovered: Some(true),
        })
        .collect();
    let manual = vec![ProjectTrackedFolder {
        path: "/external/m1".to_owned(),
        name: "m1".to_owned(),
        discovered: None,
    }];
    let merged = merge_project_folders(&many, &manual);
    assert_eq!(merged.len(), PROJECT_TRACKED_FOLDERS_MAX);
    assert_eq!(merged.last().map(|f| f.name.as_str()), Some("m1"));
    Ok(())
}

#[test]
fn task_folder_coordinator_plans_and_persists_with_sqlite_store() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().join("workspace");
    fs::create_dir_all(&root)?;
    for idx in 0..10 {
        fs::create_dir_all(root.join(format!("repo-{idx:02}")).join(".git"))?;
    }

    assert_eq!(
        common_ancestor(&[PathBuf::from(r"C:\work\a"), PathBuf::from(r"D:\other\b")]),
        None
    );
    let anchor_res = layout_anchor(
        &PathBuf::from(r"C:\work\project"),
        &[
            PathBuf::from(r"C:\shared\repo-a"),
            PathBuf::from(r"D:\else\repo-b"),
        ],
    );
    assert_eq!(anchor_res.anchor, PathBuf::from(r"C:\"));
    assert_eq!(
        anchor_res.unreachable,
        vec![PathBuf::from(r"D:\else\repo-b")]
    );

    let pid = ProjectId::new("proj-multi")?;
    let plan = TaskFolderCoordinator::plan(pid.clone(), &root, &[], None)?
        .ok_or("expected multi-repo TaskFolderPlan")?;
    assert_eq!(
        plan.repositories.len(),
        TASK_FOLDER_DEFAULT_REPOSITORIES_MAX
    );
    assert_eq!(plan.tracked_repositories.len(), 10);

    let err = TaskFolderCoordinator::plan(pid.clone(), &root, &[], Some(&["/not-tracked"]))
        .expect_err("untracked repo must fail planning");
    assert_eq!(err.operation, TaskFolderOperation::Plan);

    let tf_root = dir.path().join("worktrees").join("task-1");
    let mirrored = build_mirrored_worktree_path(&tf_root, &plan.anchor, &root.join("repo-00"));
    assert_eq!(mirrored, tf_root.join("repo-00"));

    let db = SqliteStore::open_in_memory()?;
    let tf = TaskFolder {
        root: tf_root.to_string_lossy().into_owned(),
        project_id: pid,
        project_root: root.to_string_lossy().into_owned(),
        anchor: plan.anchor.to_string_lossy().into_owned(),
        project_path: tf_root.join("workspace").to_string_lossy().into_owned(),
        project_is_worktree: false,
        repositories: vec![TaskFolderRepository {
            repository_path: root.join("repo-00").to_string_lossy().into_owned(),
            worktree_path: mirrored.to_string_lossy().into_owned(),
            name: "repo-00".to_owned(),
        }],
    };
    TaskFolderCoordinator::record_claimed(db.connection(), &tf, Some("2026-11-21T00:00:00Z"))?;
    let found = TaskFolderCoordinator::find(db.connection(), &mirrored)?;
    assert_eq!(
        found.as_ref().map(|t| t.root.as_str()),
        Some(tf.root.as_str())
    );

    let updated = TaskFolderCoordinator::add_repository(
        db.connection(),
        &tf.root,
        TaskFolderRepository {
            repository_path: root.join("repo-01").to_string_lossy().into_owned(),
            worktree_path: tf_root.join("repo-01").to_string_lossy().into_owned(),
            name: "repo-01".to_owned(),
        },
        None,
    )?;
    assert_eq!(updated.repositories.len(), 2);
    assert_eq!(TaskFolderCoordinator::list(db.connection())?.len(), 1);
    assert!(TaskFolderCoordinator::forget(db.connection(), &tf.root)?);
    Ok(())
}
