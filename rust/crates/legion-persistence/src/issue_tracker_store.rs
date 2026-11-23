//! SQLite repository helpers for `issue_tracker_bindings` and `issue_tracker_accounts`.
//!
//! Mirrors `apps/server/src/persistence/IssueTrackerBindings.ts` (`#39179`, `#40587`).

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors returned by [`crate::issue_tracker_store`] operations.
#[derive(Debug, Error)]
pub enum IssueTrackerStoreError {
    /// Underlying SQLite failure.
    #[error("issue tracker store SQLite error during '{operation}': {source}")]
    Sqlite {
        /// Operation label.
        operation: &'static str,
        /// Underlying SQLite error.
        #[source]
        source: rusqlite::Error,
    },
    /// Failed to parse or serialize `additional_scopes` JSON.
    #[error("issue tracker store JSON error during '{operation}': {source}")]
    Json {
        /// Operation label.
        operation: &'static str,
        /// Underlying JSON error.
        #[source]
        source: serde_json::Error,
    },
}

fn sqlite_err(operation: &'static str, source: rusqlite::Error) -> IssueTrackerStoreError {
    IssueTrackerStoreError::Sqlite { operation, source }
}

fn json_err(operation: &'static str, source: serde_json::Error) -> IssueTrackerStoreError {
    IssueTrackerStoreError::Json { operation, source }
}

/// Persisted standalone issue tracker binding for one project (`issue_tracker_bindings`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueTrackerBindingEntry {
    /// Bound project identifier.
    pub project_id: String,
    /// Standalone tracker identifier (e.g. `"linear"`).
    pub tracker: String,
    /// Added account identifier when bound to a non-primary account (`None` for the primary account).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// Primary bound scope (e.g. Linear team key).
    pub scope: String,
    /// Additional bound scopes beyond `scope`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_scopes: Vec<String>,
}

/// Added standalone tracker account row (`issue_tracker_accounts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueTrackerAccountEntry {
    /// Standalone tracker identifier (e.g. `"linear"`).
    pub tracker: String,
    /// Added account identifier.
    pub account_id: String,
}

/// Ensures `issue_tracker_bindings` (with `additional_scopes` and `account_id`) and
/// `issue_tracker_accounts` exist on `conn`.
pub fn ensure_issue_tracker_schema(conn: &Connection) -> Result<(), IssueTrackerStoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS issue_tracker_bindings (
           project_id TEXT PRIMARY KEY,
           tracker TEXT NOT NULL,
           scope TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           additional_scopes TEXT NOT NULL DEFAULT '[]',
           account_id TEXT
         ) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS issue_tracker_accounts (
           tracker TEXT NOT NULL,
           account_id TEXT NOT NULL,
           created_at TEXT NOT NULL,
           PRIMARY KEY (tracker, account_id)
         ) WITHOUT ROWID;",
    )
    .map_err(|e| sqlite_err("ensure_issue_tracker_schema", e))
}

fn decode_binding_row(
    row: &rusqlite::Row<'_>,
) -> Result<IssueTrackerBindingEntry, IssueTrackerStoreError> {
    let project_id: String = row
        .get(0)
        .map_err(|e| sqlite_err("read_binding:project_id", e))?;
    let tracker: String = row
        .get(1)
        .map_err(|e| sqlite_err("read_binding:tracker", e))?;
    let account_id: Option<String> = row
        .get(2)
        .map_err(|e| sqlite_err("read_binding:account_id", e))?;
    let scope: String = row
        .get(3)
        .map_err(|e| sqlite_err("read_binding:scope", e))?;
    let raw_scopes: String = row
        .get(4)
        .map_err(|e| sqlite_err("read_binding:additional_scopes", e))?;
    let additional_scopes: Vec<String> =
        serde_json::from_str(&raw_scopes).map_err(|e| json_err("decode_additional_scopes", e))?;
    Ok(IssueTrackerBindingEntry {
        project_id,
        tracker,
        account_id,
        scope,
        additional_scopes,
    })
}

/// Lists bindings for all projects or only `project_id` (`#40587`).
///
/// Passing `Some(project_id)` filters in SQL (`WHERE project_id = ?1`) so corrupt rows on
/// unrelated projects do not break reading the selected project's binding.
pub fn list_issue_tracker_bindings(
    conn: &Connection,
    project_id: Option<&str>,
) -> Result<Vec<IssueTrackerBindingEntry>, IssueTrackerStoreError> {
    let mut out = Vec::new();
    if let Some(target_project) = project_id {
        let mut stmt = conn
            .prepare(
                "SELECT project_id, tracker, account_id, scope, additional_scopes
                 FROM issue_tracker_bindings
                 WHERE project_id = ?1",
            )
            .map_err(|e| sqlite_err("list_issue_tracker_bindings:prepare_one", e))?;
        let mut rows = stmt
            .query([target_project])
            .map_err(|e| sqlite_err("list_issue_tracker_bindings:query_one", e))?;
        while let Some(row) = rows
            .next()
            .map_err(|e| sqlite_err("list_issue_tracker_bindings:next_one", e))?
        {
            out.push(decode_binding_row(row)?);
        }
    } else {
        let mut stmt = conn
            .prepare(
                "SELECT project_id, tracker, account_id, scope, additional_scopes
                 FROM issue_tracker_bindings",
            )
            .map_err(|e| sqlite_err("list_issue_tracker_bindings:prepare_all", e))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| sqlite_err("list_issue_tracker_bindings:query_all", e))?;
        while let Some(row) = rows
            .next()
            .map_err(|e| sqlite_err("list_issue_tracker_bindings:next_all", e))?
        {
            out.push(decode_binding_row(row)?);
        }
    }
    Ok(out)
}

/// Reads the single binding row for `project_id` (`#40587`) without scanning or decoding other rows.
pub fn get_issue_tracker_binding(
    conn: &Connection,
    project_id: &str,
) -> Result<Option<IssueTrackerBindingEntry>, IssueTrackerStoreError> {
    Ok(list_issue_tracker_bindings(conn, Some(project_id))?
        .into_iter()
        .next())
}

/// Inserts, updates, or clears (`binding = None`) the tracker binding for `project_id`.
pub fn set_issue_tracker_binding(
    conn: &Connection,
    project_id: &str,
    binding: Option<&IssueTrackerBindingEntry>,
    updated_at: &str,
) -> Result<(), IssueTrackerStoreError> {
    let Some(binding) = binding else {
        conn.execute(
            "DELETE FROM issue_tracker_bindings WHERE project_id = ?1",
            [project_id],
        )
        .map_err(|e| sqlite_err("delete_issue_tracker_binding", e))?;
        return Ok(());
    };
    let additional_scopes = serde_json::to_string(&binding.additional_scopes)
        .map_err(|e| json_err("encode_additional_scopes", e))?;
    conn.execute(
        "INSERT INTO issue_tracker_bindings (
           project_id, tracker, account_id, scope, additional_scopes, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (project_id) DO UPDATE SET
           tracker = excluded.tracker,
           account_id = excluded.account_id,
           scope = excluded.scope,
           additional_scopes = excluded.additional_scopes,
           updated_at = excluded.updated_at",
        params![
            project_id,
            binding.tracker,
            binding.account_id,
            binding.scope,
            additional_scopes,
            updated_at,
        ],
    )
    .map_err(|e| sqlite_err("set_issue_tracker_binding", e))?;
    Ok(())
}

/// Lists added issue tracker accounts oldest first (`ORDER BY created_at, account_id`).
pub fn list_issue_tracker_accounts(
    conn: &Connection,
) -> Result<Vec<IssueTrackerAccountEntry>, IssueTrackerStoreError> {
    let mut stmt = conn
        .prepare(
            "SELECT tracker, account_id
             FROM issue_tracker_accounts
             ORDER BY created_at, account_id",
        )
        .map_err(|e| sqlite_err("list_issue_tracker_accounts:prepare", e))?;
    let mut rows = stmt
        .query([])
        .map_err(|e| sqlite_err("list_issue_tracker_accounts:query", e))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|e| sqlite_err("list_issue_tracker_accounts:next", e))?
    {
        out.push(IssueTrackerAccountEntry {
            tracker: row
                .get(0)
                .map_err(|e| sqlite_err("read_account:tracker", e))?,
            account_id: row
                .get(1)
                .map_err(|e| sqlite_err("read_account:account_id", e))?,
        });
    }
    Ok(out)
}

/// Adds an extra issue tracker account row (`issue_tracker_accounts`).
pub fn add_issue_tracker_account(
    conn: &Connection,
    tracker: &str,
    account_id: &str,
    created_at: &str,
) -> Result<(), IssueTrackerStoreError> {
    conn.execute(
        "INSERT INTO issue_tracker_accounts (tracker, account_id, created_at)
         VALUES (?1, ?2, ?3)",
        params![tracker, account_id, created_at],
    )
    .map_err(|e| sqlite_err("add_issue_tracker_account", e))?;
    Ok(())
}

/// Unbinds every project pointing at `(tracker, account_id)` and deletes the account row if
/// `account_id` is `Some`. When `account_id` is `None`, only bindings with `account_id IS NULL`
/// are removed (`IssueTrackerBindings.ts`).
pub fn remove_issue_tracker_account(
    conn: &Connection,
    tracker: &str,
    account_id: Option<&str>,
) -> Result<(), IssueTrackerStoreError> {
    let savepoint = !conn.is_autocommit();
    let begin_sql = if savepoint {
        "SAVEPOINT legion_issue_tracker_remove;"
    } else {
        "BEGIN IMMEDIATE;"
    };
    conn.execute_batch(begin_sql)
        .map_err(|e| sqlite_err("remove_issue_tracker_account:begin", e))?;

    let result: Result<(), IssueTrackerStoreError> = (|| {
        match account_id {
            None => {
                conn.execute(
                    "DELETE FROM issue_tracker_bindings WHERE tracker = ?1 AND account_id IS NULL",
                    [tracker],
                )
                .map_err(|e| sqlite_err("remove_issue_tracker_account:bindings_null", e))?;
            }
            Some(id) => {
                conn.execute(
                    "DELETE FROM issue_tracker_bindings WHERE tracker = ?1 AND account_id = ?2",
                    params![tracker, id],
                )
                .map_err(|e| sqlite_err("remove_issue_tracker_account:bindings", e))?;
                conn.execute(
                    "DELETE FROM issue_tracker_accounts WHERE tracker = ?1 AND account_id = ?2",
                    params![tracker, id],
                )
                .map_err(|e| sqlite_err("remove_issue_tracker_account:account", e))?;
            }
        }
        Ok(())
    })();

    if result.is_err() {
        let rollback_sql = if savepoint {
            "ROLLBACK TO SAVEPOINT legion_issue_tracker_remove; RELEASE SAVEPOINT legion_issue_tracker_remove;"
        } else {
            "ROLLBACK;"
        };
        let _ = conn.execute_batch(rollback_sql);
        return result;
    }

    let commit_sql = if savepoint {
        "RELEASE SAVEPOINT legion_issue_tracker_remove;"
    } else {
        "COMMIT;"
    };
    conn.execute_batch(commit_sql)
        .map_err(|e| sqlite_err("remove_issue_tracker_account:commit", e))?;
    Ok(())
}
