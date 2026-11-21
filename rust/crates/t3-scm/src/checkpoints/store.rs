//! `SQLite` persistence wrapper over `t3-persistence`'s `checkpoint_diff_blobs` table.

use rusqlite::{Connection, OptionalExtension, params};
use t3_contracts::ids::ThreadId;

use crate::checkpoints::{CheckpointError, checkpoint_ref_for_turn};

/// Stored unified diff payload between two turn counts on a single thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointDiffBlob {
    /// Owning thread identifier.
    pub thread_id: ThreadId,
    /// Starting turn count (inclusive baseline).
    pub from_turn_count: u32,
    /// Ending turn count for this diff snapshot.
    pub to_turn_count: u32,
    /// Serialized unified diff text.
    pub diff: String,
    /// `ISO-8601` creation timestamp stored in `checkpoint_diff_blobs.created_at`.
    pub created_at: String,
}

/// Metadata summary row returned by [`CheckpointStore::list_thread_checkpoints`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadCheckpointSummary {
    /// Starting turn count of the stored blob.
    pub from_turn_count: u32,
    /// Target turn count represented by this checkpoint entry.
    pub to_turn_count: u32,
    /// UTF-8 byte length of the stored diff text.
    pub diff_bytes: usize,
    /// Stored creation timestamp.
    pub created_at: String,
    /// Canonical hidden git checkpoint ref for `to_turn_count`, if `thread_id` is ref-safe.
    pub checkpoint_ref: Option<String>,
}

/// Thin `SQLite` store for persisting and pruning `checkpoint_diff_blobs`.
///
/// ```
/// use t3_contracts::ids::ThreadId;
/// use t3_persistence::sqlite::SqliteStore;
/// use t3_scm::checkpoints::store::{CheckpointDiffBlob, CheckpointStore};
///
/// let db = SqliteStore::open_in_memory()?;
/// let store = CheckpointStore::new(db.connection());
/// let thread_id = ThreadId::new("thread-1")?;
/// store.save_diff_blob(&CheckpointDiffBlob {
///     thread_id: thread_id.clone(),
///     from_turn_count: 0,
///     to_turn_count: 1,
///     diff: "--- a/f.txt\n+++ b/f.txt\n".into(),
///     created_at: "2026-11-20T00:00:00Z".into(),
/// })?;
/// assert!(store.get_diff_blob(&thread_id, 0, 1)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy)]
pub struct CheckpointStore<'conn> {
    conn: &'conn Connection,
}

impl<'conn> CheckpointStore<'conn> {
    /// Creates a [`CheckpointStore`] borrowing an initialized `SQLite` connection.
    #[must_use]
    pub const fn new(conn: &'conn Connection) -> Self {
        Self { conn }
    }

    /// Inserts or replaces a diff blob in `checkpoint_diff_blobs`.
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError::InvalidInput`] if `from_turn_count > to_turn_count`, or
    /// [`CheckpointError::Sqlite`] if the insert fails.
    pub fn save_diff_blob(&self, blob: &CheckpointDiffBlob) -> Result<(), CheckpointError> {
        if blob.from_turn_count > blob.to_turn_count {
            return Err(CheckpointError::InvalidInput(format!(
                "from_turn_count ({}) exceeds to_turn_count ({})",
                blob.from_turn_count, blob.to_turn_count
            )));
        }
        self.conn.execute(
            "INSERT INTO checkpoint_diff_blobs (thread_id, from_turn_count, to_turn_count, diff, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (thread_id, from_turn_count, to_turn_count)
             DO UPDATE SET diff = excluded.diff, created_at = excluded.created_at",
            params![
                blob.thread_id.as_str(),
                i64::from(blob.from_turn_count),
                i64::from(blob.to_turn_count),
                &blob.diff,
                &blob.created_at,
            ],
        )?;
        Ok(())
    }

    /// Fetches a persisted diff blob for `(thread_id, from_turn_count, to_turn_count)`.
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError::Sqlite`] on query failure or [`CheckpointError::InvalidInput`]
    /// if stored turn counts are negative.
    pub fn get_diff_blob(
        &self,
        thread_id: &ThreadId,
        from_turn_count: u32,
        to_turn_count: u32,
    ) -> Result<Option<CheckpointDiffBlob>, CheckpointError> {
        let row = self
            .conn
            .query_row(
                "SELECT from_turn_count, to_turn_count, diff, created_at
                 FROM checkpoint_diff_blobs
                 WHERE thread_id = ?1 AND from_turn_count = ?2 AND to_turn_count = ?3",
                params![
                    thread_id.as_str(),
                    i64::from(from_turn_count),
                    i64::from(to_turn_count)
                ],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;

        let Some((from_i64, to_i64, diff, created_at)) = row else {
            return Ok(None);
        };
        let from_turn = u32_from_sqlite(from_i64, "from_turn_count")?;
        let to_turn = u32_from_sqlite(to_i64, "to_turn_count")?;
        Ok(Some(CheckpointDiffBlob {
            thread_id: thread_id.clone(),
            from_turn_count: from_turn,
            to_turn_count: to_turn,
            diff,
            created_at,
        }))
    }

    /// Lists checkpoint summaries for `thread_id` ordered by `(to_turn_count, from_turn_count)`.
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError::Sqlite`] or [`CheckpointError::InvalidInput`] on invalid rows.
    pub fn list_thread_checkpoints(
        &self,
        thread_id: &ThreadId,
    ) -> Result<Vec<ThreadCheckpointSummary>, CheckpointError> {
        let mut stmt = self.conn.prepare(
            "SELECT from_turn_count, to_turn_count, LENGTH(CAST(diff AS BLOB)), created_at
             FROM checkpoint_diff_blobs
             WHERE thread_id = ?1
             ORDER BY to_turn_count ASC, from_turn_count ASC",
        )?;

        let mut rows = stmt.query(params![thread_id.as_str()])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let from_turn = u32_from_sqlite(row.get::<_, i64>(0)?, "from_turn_count")?;
            let to_turn = u32_from_sqlite(row.get::<_, i64>(1)?, "to_turn_count")?;
            let bytes_i64 = row.get::<_, i64>(2)?;
            let diff_bytes = usize::try_from(bytes_i64).map_err(|_| {
                CheckpointError::InvalidInput(format!("negative diff byte size: {bytes_i64}"))
            })?;
            let created_at = row.get::<_, String>(3)?;
            let checkpoint_ref = checkpoint_ref_for_turn(thread_id, to_turn)
                .ok()
                .map(|r| r.as_str().to_owned());
            out.push(ThreadCheckpointSummary {
                from_turn_count: from_turn,
                to_turn_count: to_turn,
                diff_bytes,
                created_at,
                checkpoint_ref,
            });
        }
        Ok(out)
    }

    /// Deletes all diff blobs on `thread_id` whose `to_turn_count` exceeds `turn_count`.
    ///
    /// Called when rolling back a thread so invalidated future diffs are pruned.
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError::Sqlite`] if deletion fails.
    pub fn delete_blobs_after_turn(
        &self,
        thread_id: &ThreadId,
        turn_count: u32,
    ) -> Result<usize, CheckpointError> {
        let deleted = self.conn.execute(
            "DELETE FROM checkpoint_diff_blobs
             WHERE thread_id = ?1 AND to_turn_count > ?2",
            params![thread_id.as_str(), i64::from(turn_count)],
        )?;
        Ok(deleted)
    }
}

fn u32_from_sqlite(value: i64, column: &str) -> Result<u32, CheckpointError> {
    u32::try_from(value)
        .map_err(|_| CheckpointError::InvalidInput(format!("invalid {column}: {value}")))
}
