//! Integration tests for `t3_persistence::project_store`.

use std::error::Error;

use rusqlite::{Connection, params};
use t3_persistence::project_store::{
    CreateProjectInput, ProjectStoreError, add_or_unarchive_project, archive_project,
    create_project, find_project_by_id_or_workspace, list_non_deleted_projects, list_projects,
    rename_project, soft_delete_project, unarchive_project,
};
use t3_persistence::sqlite::SqliteStore;

#[test]
fn read_helpers_handle_missing_files_and_bootstrap_minimal_ddl() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("statev2.sqlite");

    assert_eq!(list_projects(&db_path)?, Vec::new());
    assert_eq!(find_project_by_id_or_workspace(&db_path, "missing")?, None);
    assert!(!db_path.exists());

    let created = create_project(&db_path, &CreateProjectInput::new("/work/alpha-repo"))?;
    assert_eq!(created.title, "alpha-repo");
    assert!(!created.has_custom_title);
    assert_eq!(created.scripts_json, "[]");
    assert!(created.is_active());

    let listed = list_projects(&db_path)?;
    assert_eq!(listed, vec![created.clone()]);
    assert_eq!(
        find_project_by_id_or_workspace(&db_path, &created.id)?,
        Some(created.clone()),
    );
    assert_eq!(
        find_project_by_id_or_workspace(&db_path, "/work/alpha-repo")?,
        Some(created),
    );
    Ok(())
}

#[test]
fn create_rejects_invalid_inputs_and_workspace_or_id_conflicts() -> Result<(), Box<dyn Error>> {
    let store = SqliteStore::open_in_memory()?;

    let empty_root = create_project(&store, &CreateProjectInput::new("   "));
    assert!(matches!(
        empty_root,
        Err(ProjectStoreError::InvalidWorkspaceRoot)
    ));

    let empty_title = create_project(
        &store,
        &CreateProjectInput::new("/work/app").with_title("   "),
    );
    assert!(matches!(empty_title, Err(ProjectStoreError::InvalidTitle)));

    let bad_scripts = create_project(
        &store,
        &CreateProjectInput::new("/work/app").with_scripts_json("{}"),
    );
    assert!(matches!(
        bad_scripts,
        Err(ProjectStoreError::InvalidScriptsJson)
    ));

    let first = store.create_project(
        &CreateProjectInput::new("/work/app")
            .with_project_id("proj-1")
            .with_title("  App One  "),
    )?;
    assert_eq!(first.id, "proj-1");
    assert_eq!(first.title, "App One");

    let dup_id =
        store.create_project(&CreateProjectInput::new("/work/other").with_project_id("proj-1"));
    assert!(matches!(
        dup_id,
        Err(ProjectStoreError::ProjectConflict { project_id }) if project_id == "proj-1"
    ));

    let dup_ws = store.create_project(&CreateProjectInput::new("/work/app"));
    assert!(matches!(
        dup_ws,
        Err(ProjectStoreError::WorkspaceConflict {
            conflicting_project_id,
            ..
        }) if conflicting_project_id == "proj-1"
    ));
    Ok(())
}

#[test]
fn add_or_unarchive_restores_archived_projects_with_optional_title_override()
-> Result<(), Box<dyn Error>> {
    let store = SqliteStore::open_in_memory()?;
    let created = store.create_project(
        &CreateProjectInput::new("/work/beta")
            .with_project_id("proj-beta")
            .with_title("Beta")
            .with_now_iso("2026-01-01T00:00:00.000Z"),
    )?;

    let archived = archive_project(&store, "/work/beta")?;
    assert!(archived.is_archived());
    assert!(create_project(&store, &CreateProjectInput::new("/work/beta")).is_err());

    let restored_keep = store.add_or_unarchive_project(&CreateProjectInput::new("/work/beta"))?;
    assert!(!restored_keep.created);
    assert!(restored_keep.unarchived);
    assert_eq!(restored_keep.project.id, created.id);
    assert_eq!(restored_keep.project.title, "Beta");
    assert!(restored_keep.project.is_active());

    archive_project(&store, &created.id)?;
    let restored_renamed = add_or_unarchive_project(
        &store,
        &CreateProjectInput::new("/work/beta").with_title("  Back again  "),
    )?;
    assert!(restored_renamed.unarchived);
    assert_eq!(restored_renamed.project.title, "Back again");
    assert!(restored_renamed.project.has_custom_title);

    let conflict = add_or_unarchive_project(&store, &CreateProjectInput::new("/work/beta"));
    assert!(matches!(
        conflict,
        Err(ProjectStoreError::WorkspaceConflict { .. })
    ));

    let unarchived_direct = unarchive_project(&store, &created.id, Some("Direct"))?;
    assert_eq!(unarchived_direct.title, "Direct");
    assert!(unarchived_direct.is_active());
    Ok(())
}

#[test]
fn rename_checks_unchanged_title_and_tracks_custom_title() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("statev2.sqlite");

    let created = create_project(
        &db_path,
        &CreateProjectInput::new("/work/gamma")
            .with_title("Gamma")
            .with_now_iso("2026-02-02T10:00:00.000Z"),
    )?;

    let unchanged = rename_project(&db_path, "/work/gamma", "  Gamma  ")?;
    assert!(!unchanged.changed);
    assert_eq!(unchanged.project.updated_at, "2026-02-02T10:00:00.000Z");
    assert!(!unchanged.project.has_custom_title);

    let renamed = rename_project(&db_path, &created.id, "  Gamma Pro  ")?;
    assert!(renamed.changed);
    assert_eq!(renamed.project.title, "Gamma Pro");
    assert!(renamed.project.has_custom_title);
    assert_ne!(renamed.project.updated_at, "2026-02-02T10:00:00.000Z");

    let missing = rename_project(&db_path, "nonexistent", "Other");
    assert!(matches!(missing, Err(ProjectStoreError::NotFound { .. })));
    Ok(())
}

#[test]
fn soft_delete_enforces_non_deleted_thread_guard_and_force_cascades() -> Result<(), Box<dyn Error>>
{
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let target = create_project(
        conn,
        &CreateProjectInput::new("/work/delta").with_project_id("p-delta"),
    )?;
    let archived_proj = create_project(
        conn,
        &CreateProjectInput::new("/work/epsilon").with_project_id("p-eps"),
    )?;
    let archived_proj = archive_project(conn, &archived_proj.id)?;

    conn.execute(
        "INSERT INTO orchestration_v2_projection_threads (
           thread_id, project_id, title, default_provider, runtime_mode, interaction_mode,
           created_at, updated_at, archived_at, deleted_at, payload_json
         ) VALUES
           ('th-v2-archived', 'p-delta', 'A', 'codex', 'full-access', 'default', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z', NULL, '{}'),
           ('th-v2-deleted', 'p-delta', 'D', 'codex', 'full-access', 'default', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', NULL, '2026-01-03T00:00:00Z', '{}'),
           ('th-v2-other', 'p-eps', 'O', 'codex', 'full-access', 'default', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', NULL, NULL, '{}')",
        [],
    )?;
    conn.execute(
        "INSERT INTO projection_threads (
           thread_id, project_id, title, created_at, updated_at, deleted_at
         ) VALUES
           ('th-v1-active', 'p-delta', 'Legacy', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', NULL)",
        [],
    )?;

    let blocked = soft_delete_project(conn, &target.id, false);
    assert!(matches!(
        blocked,
        Err(ProjectStoreError::ProjectNotEmpty {
            active_thread_count: 2,
            ..
        })
    ));

    let deleted = soft_delete_project(conn, "/work/delta", true)?;
    assert!(deleted.project.is_deleted());
    assert_eq!(deleted.deleted_thread_count, 2);

    let v2_left: i64 = conn.query_row(
        "SELECT COUNT(*) FROM orchestration_v2_projection_threads WHERE project_id = 'p-delta' AND deleted_at IS NULL",
        [],
        |r| r.get(0),
    )?;
    let v1_left: i64 = conn.query_row(
        "SELECT COUNT(*) FROM projection_threads WHERE project_id = 'p-delta' AND deleted_at IS NULL",
        [],
        |r| r.get(0),
    )?;
    let other_left: i64 = conn.query_row(
        "SELECT COUNT(*) FROM orchestration_v2_projection_threads WHERE project_id = 'p-eps' AND deleted_at IS NULL",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(v2_left, 0);
    assert_eq!(v1_left, 0);
    assert_eq!(other_left, 1);

    assert_eq!(find_project_by_id_or_workspace(conn, &target.id)?, None);
    assert!(matches!(
        rename_project(conn, &target.id, "AfterDelete"),
        Err(ProjectStoreError::NotFound { .. })
    ));

    let all = list_projects(conn)?;
    assert_eq!(all.len(), 2);
    assert!(all.iter().any(|p| p.id == "p-delta" && p.is_deleted()));
    assert!(all.iter().any(|p| p.id == "p-eps" && p.is_archived()));
    assert_eq!(list_non_deleted_projects(conn)?, vec![archived_proj]);

    let recreated = create_project(
        conn,
        &CreateProjectInput::new("/work/delta").with_project_id("p-delta-2"),
    )?;
    assert!(recreated.is_active());
    Ok(())
}

#[test]
fn upgrades_partial_legacy_projection_projects_table_and_reads_partial_ro()
-> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("legacy.sqlite");
    let conn = Connection::open(&db_path)?;
    conn.execute_batch(
        "CREATE TABLE projection_projects (
           project_id TEXT PRIMARY KEY,
           title TEXT NOT NULL,
           workspace_root TEXT NOT NULL,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL
         );",
    )?;
    conn.execute(
        "INSERT INTO projection_projects (project_id, title, workspace_root, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)",
        params!["old-1", "Legacy", "/work/legacy", "2026-01-01T00:00:00Z"],
    )?;
    conn.close().map_err(|(_, err)| err)?;

    let before = list_projects(&db_path)?;
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].scripts_json, "[]");
    assert!(before[0].is_active());

    let renamed = rename_project(&db_path, "old-1", "Migrated")?;
    assert!(renamed.changed);
    assert!(renamed.project.has_custom_title);
    Ok(())
}
