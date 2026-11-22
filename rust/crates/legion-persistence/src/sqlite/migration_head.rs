//! Read-only inspection of `effect_sql_migrations` and effective build migration head.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use super::BUILD_MIGRATION_HEAD;

/// A recorded row from `effect_sql_migrations`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedMigration {
    /// Monotonic numeric migration identifier.
    pub migration_id: i64,
    /// Human-readable migration name.
    pub name: String,
}

/// Reads all recorded migrations from `effect_sql_migrations` using a fast read-only connection.
///
/// Returns `Some(vec![])` if `database_path` does not exist or has no `effect_sql_migrations` table,
/// and `None` if an existing file cannot be opened or queried. Matches `recordedMigrations` in
/// `apps/server/src/persistence/databaseMigrationHead.ts:11-40`.
pub fn read_recorded_migrations(database_path: impl AsRef<Path>) -> Option<Vec<RecordedMigration>> {
    let database_path = database_path.as_ref();
    let Ok(conn) = Connection::open_with_flags(
        database_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return (!database_path.exists()).then(Vec::new);
    };

    let has_table: Option<i64> = match conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'effect_sql_migrations'",
        [],
        |row| row.get(0),
    ) {
        Ok(found) => Some(found),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(_) => return None,
    };

    if has_table.is_none() {
        return Some(Vec::new());
    }

    let mut stmt = conn
        .prepare("SELECT migration_id, name FROM effect_sql_migrations ORDER BY migration_id")
        .ok()?;
    let rows = stmt
        .query_map([], |row| {
            Ok(RecordedMigration {
                migration_id: row.get(0)?,
                name: row.get(1)?,
            })
        })
        .ok()?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.ok()?);
    }
    Some(out)
}

/// Returns the effective migration head for `database_path`: `max(BUILD_MIGRATION_HEAD, recorded...)`,
/// or `None` when an existing database cannot be read.
///
/// Matches `databaseMigrationHead` in `apps/server/src/persistence/databaseMigrationHead.ts:44-49`.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use legion_persistence::sqlite::{BUILD_MIGRATION_HEAD, SqliteStore, read_migration_head};
///
/// let dir = tempfile::tempdir()?;
/// let db_path = dir.path().join("statev2.sqlite");
/// assert_eq!(read_migration_head(&db_path), Some(BUILD_MIGRATION_HEAD));
///
/// let store = SqliteStore::open(&db_path)?;
/// store.connection().execute(
///     "INSERT INTO effect_sql_migrations (migration_id, name) VALUES (?1, ?2)",
///     rusqlite::params![150, "FutureMigration"],
/// )?;
/// drop(store);
///
/// assert_eq!(read_migration_head(&db_path), Some(150));
/// # Ok(())
/// # }
/// ```
pub fn read_migration_head(database_path: impl AsRef<Path>) -> Option<i64> {
    let recorded = read_recorded_migrations(database_path)?;
    Some(
        recorded
            .into_iter()
            .fold(BUILD_MIGRATION_HEAD, |acc, item| acc.max(item.migration_id)),
    )
}
