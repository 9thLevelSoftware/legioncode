//! Tests covering Wave 1 `legion-persistence` migrations, task folders, auth revocation,
//! Claude history filtering/truncation, transcript cleanup errors, favicon fallback, and branding paths.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use legion_contracts::brand::LEGION_CODE;
use legion_contracts::workspace_folders::{
    ProjectTrackedFolder, TASK_FOLDER_DEFAULT_REPOSITORIES_MAX, TaskFolder, TaskFolderRepository,
};
use legion_persistence::auth_mutations::{
    IssueSessionInput, issue_scoped_session_at, revoke_other_sessions_at,
};
use legion_persistence::claude_history::{
    CLAUDE_IMPORT_TITLE_MAX_UTF16, ClaudeHistoryOptions, derive_imported_title,
    get_session_messages,
};
use legion_persistence::maintenance::{MaintenanceError, cleanup_imported_transcript};
use legion_persistence::pairing_tokens::{IssuePairingTokenInput, issue_pairing_token_with_conn};
use legion_persistence::paths::{ServerDerivedPaths, resolve_base_dir_for_brand};
use legion_persistence::project_store::{
    CreateProjectInput, create_project, read_project_tracked_folders, resolve_project_favicon,
    set_project_tracked_folders,
};
use legion_persistence::sqlite::{BUILD_MIGRATION_HEAD, SqliteStore, read_recorded_migrations};
use legion_persistence::task_folder_store::{
    TaskFolderStoreError, add_repository, get_by_worktree_or_child_path, list_task_folders,
    remove_by_root, upsert_task_folder,
};
use rusqlite::Connection;
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn migrations_v105_apply_idempotently_and_guard_existing_columns() -> TestResult {
    assert_eq!(BUILD_MIGRATION_HEAD, 105);
    let temp = tempfile::tempdir()?;
    let db_path = temp.path().join("preexisting.sqlite");

    // Pre-create `projection_projects` with `tracked_folders_json` already present.
    let pre = Connection::open(&db_path)?;
    pre.execute_batch(
        "CREATE TABLE projection_projects (
           project_id TEXT PRIMARY KEY,
           title TEXT NOT NULL,
           workspace_root TEXT NOT NULL,
           scripts_json TEXT NOT NULL,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           deleted_at TEXT,
           tracked_folders_json TEXT NOT NULL DEFAULT '[]'
         );",
    )?;
    pre.close().map_err(|(_, e)| e)?;

    let store = SqliteStore::open(&db_path)?;
    assert_eq!(store.recorded_migration_head()?, Some(105));

    // Verify migrations 103..=105 recorded cleanly and reopening stays idempotent.
    store.close()?;
    let reopened = SqliteStore::open(&db_path)?;
    let rconn = reopened.connection();
    let scopes_col: i64 = rconn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('issue_tracker_bindings') WHERE name = 'additional_scopes'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(scopes_col, 1);

    let idx_count: i64 = rconn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_task_folders_real_root'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(idx_count, 1);
    reopened.close()?;

    let recorded = read_recorded_migrations(&db_path).expect("migrations");
    assert!(
        recorded
            .iter()
            .any(|m| m.migration_id == 103 && m.name == "IssueTrackerAdditionalScopes")
    );
    assert!(
        recorded
            .iter()
            .any(|m| m.migration_id == 104 && m.name == "ProjectionProjectsTrackedFolders")
    );
    assert!(
        recorded
            .iter()
            .any(|m| m.migration_id == 105 && m.name == "TaskFolders")
    );
    Ok(())
}

#[test]
fn task_folder_store_upserts_looks_up_ancestors_caps_repos_and_removes() -> TestResult {
    let temp = tempfile::tempdir()?;
    let db_path = temp.path().join("statev2.sqlite");
    let store = SqliteStore::open(&db_path)?;
    let conn = store.connection();

    let outer_root = temp.path().join("wt-outer");
    let inner_root = outer_root.join("nested");
    let child_file = inner_root.join("sub").join("file.rs");
    fs::create_dir_all(child_file.parent().expect("parent"))?;

    let outer = TaskFolder {
        root: outer_root.to_string_lossy().into_owned(),
        project_id: "proj-1".parse()?,
        project_root: temp.path().to_string_lossy().into_owned(),
        anchor: outer_root.to_string_lossy().into_owned(),
        project_path: outer_root.to_string_lossy().into_owned(),
        project_is_worktree: true,
        repositories: Vec::new(),
    };
    let inner = TaskFolder {
        root: inner_root.to_string_lossy().into_owned(),
        ..outer.clone()
    };

    upsert_task_folder(conn, &outer, Some("2026-11-21T00:00:00Z"))?;
    upsert_task_folder(conn, &inner, Some("2026-11-21T00:00:01Z"))?;
    assert_eq!(list_task_folders(conn)?.len(), 2);

    // Deepest matching ancestor wins for nested child paths.
    let found = get_by_worktree_or_child_path(conn, &child_file)?.expect("task folder");
    assert_eq!(found.root, inner.root);

    // Adding repositories enforces max cap and handles duplicate idempotently.
    let mut updated = found;
    for i in 0..TASK_FOLDER_DEFAULT_REPOSITORIES_MAX {
        updated = add_repository(
            conn,
            &inner.root,
            TaskFolderRepository {
                repository_path: format!("/repos/r{i}"),
                worktree_path: format!("/wt/r{i}"),
                name: format!("r{i}"),
            },
            None,
        )?;
    }
    assert_eq!(
        updated.repositories.len(),
        TASK_FOLDER_DEFAULT_REPOSITORIES_MAX
    );
    let cap_err = add_repository(
        conn,
        &inner.root,
        TaskFolderRepository {
            repository_path: "/repos/overflow".to_owned(),
            worktree_path: "/wt/overflow".to_owned(),
            name: "overflow".to_owned(),
        },
        None,
    )
    .expect_err("cap exceeded");
    assert!(matches!(
        cap_err,
        TaskFolderStoreError::RepositoryLimitExceeded { .. }
    ));

    assert!(remove_by_root(conn, &inner.root)?);
    assert!(!remove_by_root(conn, &inner.root)?);
    let fallback_found = get_by_worktree_or_child_path(conn, &child_file)?.expect("outer matches");
    assert_eq!(fallback_found.root, outer.root);
    Ok(())
}

#[test]
fn revoke_other_sessions_also_revokes_unspent_unbound_and_other_pairing_links() -> TestResult {
    let temp = tempfile::tempdir()?;
    let db_path = temp.path().join("statev2.sqlite");
    let store = SqliteStore::open(&db_path)?;
    let conn = store.connection();

    let now_ms = 1_700_000_000_000;
    let keep = issue_scoped_session_at(
        conn,
        IssueSessionInput::default().with_label("Keep"),
        now_ms,
    )?;
    let other = issue_scoped_session_at(
        conn,
        IssueSessionInput::default().with_label("Other"),
        now_ms,
    )?;

    let unbound_link =
        issue_pairing_token_with_conn(conn, &IssuePairingTokenInput::default(), now_ms)?;
    conn.execute(
        "INSERT INTO auth_connect_devices (proof_key_thumbprint, client_json, approved_at)
         VALUES ('thumb-other', '{\"label\":\"Phone\"}', '2026-11-21T00:00:00Z')",
        [],
    )?;

    let outcome = revoke_other_sessions_at(conn, &keep.session_id, now_ms + 1_000)?;
    assert_eq!(outcome.revoked_sessions, 1);
    assert_eq!(outcome.revoked_pairing_links, 1);
    assert_eq!(outcome.removed_connect_devices, 1);

    let keep_revoked: Option<String> = conn.query_row(
        "SELECT revoked_at FROM auth_sessions WHERE session_id = ?1",
        [&keep.session_id],
        |r| r.get(0),
    )?;
    assert_eq!(keep_revoked, None);

    let other_revoked: Option<String> = conn.query_row(
        "SELECT revoked_at FROM auth_sessions WHERE session_id = ?1",
        [&other.session_id],
        |r| r.get(0),
    )?;
    assert!(other_revoked.is_some());

    let link_revoked: Option<String> = conn.query_row(
        "SELECT revoked_at FROM auth_pairing_links WHERE id = ?1",
        [&unbound_link.id],
        |r| r.get(0),
    )?;
    assert!(link_revoked.is_some());
    Ok(())
}

#[test]
fn claude_history_skips_synthetic_error_only_sessions_and_truncates_titles_safely() -> TestResult {
    let dir = tempfile::tempdir()?;
    let synth_id = "synth-only";
    let synth_raw = [
        json!({"type": "user", "uuid": "u1", "parentUuid": null, "sessionId": synth_id, "message": {"role": "user", "content": "hello"}}),
        json!({"type": "assistant", "uuid": "a1", "parentUuid": "u1", "sessionId": synth_id, "message": {"role": "assistant", "model": "<synthetic>", "content": [{"type": "text", "text": "Invalid API key"}]}}),
    ]
    .iter()
    .map(|v| serde_json::to_string(v).expect("json"))
    .collect::<Vec<_>>()
    .join("\n");
    fs::write(dir.path().join(format!("{synth_id}.jsonl")), synth_raw)?;

    let msgs = get_session_messages(
        synth_id,
        &ClaudeHistoryOptions {
            dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        },
    )?;
    assert_eq!(msgs.len(), 0);

    let mixed_id = "mixed-real";
    let mixed_raw = [
        json!({"type": "user", "uuid": "u1", "parentUuid": null, "sessionId": mixed_id, "message": {"role": "user", "content": "hi"}}),
        json!({"type": "assistant", "uuid": "a1", "parentUuid": "u1", "sessionId": mixed_id, "message": {"role": "assistant", "model": "<synthetic>", "content": [{"type": "text", "text": "retry"}]}}),
        json!({"type": "assistant", "uuid": "a2", "parentUuid": "a1", "sessionId": mixed_id, "message": {"role": "assistant", "model": "claude-sonnet-4-6", "content": [{"type": "text", "text": "ok"}]}}),
    ]
    .iter()
    .map(|v| serde_json::to_string(v).expect("json"))
    .collect::<Vec<_>>()
    .join("\n");
    fs::write(dir.path().join(format!("{mixed_id}.jsonl")), mixed_raw)?;
    assert_eq!(
        get_session_messages(
            mixed_id,
            &ClaudeHistoryOptions {
                dir: Some(dir.path().to_path_buf()),
                ..Default::default()
            }
        )?
        .len(),
        3
    );

    let title_seed = format!(
        "{}\u{1F680} next line",
        "a".repeat(CLAUDE_IMPORT_TITLE_MAX_UTF16 - 1)
    );
    assert_eq!(
        derive_imported_title(&title_seed),
        Some("a".repeat(CLAUDE_IMPORT_TITLE_MAX_UTF16 - 1))
    );
    Ok(())
}

#[test]
fn cleanup_transcript_favicons_and_branded_paths_work_across_edge_cases() -> TestResult {
    let temp = tempfile::tempdir()?;
    let db_path = temp.path().join("statev2.sqlite");
    let store = SqliteStore::open(&db_path)?;

    // Passing a directory as transcript_path fails on remove_file with specific OS reason.
    let dir_target = temp.path().join("not-a-file");
    fs::create_dir_all(&dir_target)?;
    let err = cleanup_imported_transcript(store.connection(), "t-1", &dir_target)
        .expect_err("directory cannot be unlinked as file");
    assert!(matches!(err, MaintenanceError::TranscriptCleanup { .. }));
    assert!(err.to_string().contains("filesystem remove failed:"));

    // Flutter web/favicon.png fallback + well-known priority + tracked folders persistence.
    let flutter_root = temp.path().join("flutter_app");
    fs::create_dir_all(flutter_root.join("web"))?;
    let web_icon = flutter_root.join("web").join("favicon.png");
    fs::write(&web_icon, b"png")?;
    assert_eq!(
        resolve_project_favicon(&flutter_root),
        Some(fs::canonicalize(&web_icon)?)
    );

    let proj = create_project(
        &db_path,
        &CreateProjectInput::new(flutter_root.to_string_lossy().into_owned()),
    )?;
    let saved = set_project_tracked_folders(
        &db_path,
        &proj.id,
        &[ProjectTrackedFolder {
            path: "packages/app".to_owned(),
            name: "App".to_owned(),
            discovered: None,
        }],
    )?;
    assert_eq!(read_project_tracked_folders(&db_path, &proj.id)?, saved);

    // Branded base-dir resolution routes env vars & dotdirs via BrandConfig.
    let mut legion_env = HashMap::new();
    legion_env.insert("LEGIONCODE_HOME".to_owned(), "/env/legioncode".to_owned());
    let (resolved, explicit) =
        resolve_base_dir_for_brand(&LEGION_CODE, None, Some(temp.path()), |k| {
            legion_env.get(k).cloned()
        });
    assert!(explicit);
    assert_eq!(resolved, PathBuf::from("/env/legioncode"));

    let (default_dir, is_exp) =
        resolve_base_dir_for_brand(&LEGION_CODE, None, Some(temp.path()), |_| None);
    assert!(!is_exp);
    assert_eq!(default_dir, temp.path().join(".legioncode"));
    let derived =
        ServerDerivedPaths::resolve_for_brand(&LEGION_CODE, None, Some(temp.path()), false, |_| {
            None
        });
    assert_eq!(
        derived.state_dir,
        temp.path().join(".legioncode").join("userdata")
    );
    Ok(())
}
