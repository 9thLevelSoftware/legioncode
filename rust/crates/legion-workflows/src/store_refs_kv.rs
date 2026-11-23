//! Reference, trigger, hook, key-value store, and count queries on [`WorkflowStore`].

use rusqlite::{OptionalExtension, params};
use serde_json::Value;

use crate::store::{WorkflowStore, storage_err, truncate_chars};
use crate::store_queries::{ExistingRefRow, apply_reference_upsert};
use crate::time::now_iso_utc;
use crate::types::{
    StoredEntry, WorkflowCounts, WorkflowError, WorkflowReferenceInput, WorkflowReferenceOrigin,
    WorkflowRunReference,
};

const REFERENCE_TITLE_KEPT: usize = 300;

impl WorkflowStore {
    /// Records or updates an entity touched by `run_id` (`workflow_run_references`).
    pub fn reference_noted(
        &self,
        run_id: &str,
        step: Option<&str>,
        reference: &WorkflowReferenceInput,
    ) -> Result<(), WorkflowError> {
        let at = now_iso_utc();
        let address_json = serde_json::to_string(&reference.address).map_err(storage_err)?;
        let title = truncate_chars(&reference.title, REFERENCE_TITLE_KEPT);
        let changed = {
            let mut guard = self.lock_conn()?;
            let tx = guard.transaction().map_err(storage_err)?;
            let existing: Option<ExistingRefRow> = tx
                .query_row(
                    "SELECT address_json, title, url, state, steps_json
                     FROM workflow_run_references
                     WHERE run_id = ?1 AND kind = ?2 AND key = ?3",
                    params![run_id, reference.kind, reference.key],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .optional()
                .map_err(storage_err)?;
            let did_change = apply_reference_upsert(
                &tx,
                run_id,
                step,
                reference,
                &address_json,
                &title,
                &at,
                existing,
            )?;
            tx.commit().map_err(storage_err)?;
            did_change
        };
        if changed {
            self.notify_change(run_id);
        }
        Ok(())
    }

    /// Reads all references noted for `run_id` in insertion order.
    pub fn references(&self, run_id: &str) -> Result<Vec<WorkflowRunReference>, WorkflowError> {
        let guard = self.lock_conn()?;
        let mut stmt = guard
            .prepare(
                "SELECT kind, key, address_json, title, url, state, steps_json, origin
                 FROM workflow_run_references WHERE run_id = ?1 ORDER BY rowid",
            )
            .map_err(storage_err)?;
        let rows = stmt
            .query_map(params![run_id], |row| {
                let addr_raw: String = row.get(2)?;
                let steps_raw: String = row.get(6)?;
                let origin_raw: String = row.get(7)?;
                Ok(WorkflowRunReference {
                    kind: row.get(0)?,
                    key: row.get(1)?,
                    address: serde_json::from_str(&addr_raw).unwrap_or(Value::Null),
                    title: row.get(3)?,
                    url: row.get(4)?,
                    state: row.get(5)?,
                    steps: serde_json::from_str(&steps_raw).unwrap_or_default(),
                    origin: WorkflowReferenceOrigin::parse(&origin_raw)
                        .unwrap_or(WorkflowReferenceOrigin::Step),
                })
            })
            .map_err(storage_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(storage_err)?);
        }
        Ok(out)
    }

    /// Reads the next due time for `trigger_id`.
    pub fn trigger_next(&self, trigger_id: &str) -> Result<Option<String>, WorkflowError> {
        let guard = self.lock_conn()?;
        guard
            .query_row(
                "SELECT next_at FROM workflow_triggers WHERE trigger_id = ?1",
                params![trigger_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_err)
    }

    /// Upserts `next_at` for `trigger_id`.
    pub fn trigger_set(&self, trigger_id: &str, next_at: &str) -> Result<(), WorkflowError> {
        let guard = self.lock_conn()?;
        guard
            .execute(
                "INSERT INTO workflow_triggers (trigger_id, next_at) VALUES (?1, ?2)
                 ON CONFLICT (trigger_id) DO UPDATE SET next_at = excluded.next_at",
                params![trigger_id, next_at],
            )
            .map_err(storage_err)?;
        Ok(())
    }

    /// Prunes stored trigger cursors not in `kept_ids` so removed triggers start fresh if re-added.
    pub fn triggers_keep(&self, kept_ids: &[String]) -> Result<(), WorkflowError> {
        let mut guard = self.lock_conn()?;
        let tx = guard.transaction().map_err(storage_err)?;
        let existing: Vec<String> = {
            let mut stmt = tx
                .prepare("SELECT trigger_id FROM workflow_triggers")
                .map_err(storage_err)?;
            let rows = stmt.query_map([], |row| row.get(0)).map_err(storage_err)?;
            let mut items = Vec::new();
            for row in rows {
                items.push(row.map_err(storage_err)?);
            }
            items
        };
        for id in existing {
            if !kept_ids.iter().any(|k| k == &id) {
                tx.execute(
                    "DELETE FROM workflow_triggers WHERE trigger_id = ?1",
                    params![id],
                )
                .map_err(storage_err)?;
            }
        }
        tx.commit().map_err(storage_err)?;
        Ok(())
    }

    /// Returns the persisted secret token for `hook_id`, inserting `fresh` if not yet stored.
    pub fn hook_token(&self, hook_id: &str, fresh: &str) -> Result<String, WorkflowError> {
        let guard = self.lock_conn()?;
        guard
            .execute(
                "INSERT INTO workflow_hooks (hook_id, token) VALUES (?1, ?2)
                 ON CONFLICT (hook_id) DO NOTHING",
                params![hook_id, fresh],
            )
            .map_err(storage_err)?;
        let token = guard
            .query_row(
                "SELECT token FROM workflow_hooks WHERE hook_id = ?1",
                params![hook_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_err)?;
        Ok(token.unwrap_or_else(|| fresh.to_owned()))
    }

    /// Reads one entry from `workflow_store`.
    pub fn kv_get(&self, key: &str) -> Result<Option<Value>, WorkflowError> {
        let guard = self.lock_conn()?;
        let raw: Option<String> = guard
            .query_row(
                "SELECT value_json FROM workflow_store WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_err)?;
        raw.map(|text| serde_json::from_str(&text).map_err(storage_err))
            .transpose()
    }

    /// Sets `key` in `workflow_store`.
    pub fn kv_set(&self, key: &str, value: &Value) -> Result<(), WorkflowError> {
        let at = now_iso_utc();
        let json_text = serde_json::to_string(value).map_err(storage_err)?;
        let guard = self.lock_conn()?;
        guard
            .execute(
                "INSERT INTO workflow_store (key, value_json, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT (key) DO UPDATE
                 SET value_json = excluded.value_json, updated_at = excluded.updated_at",
                params![key, json_text, at],
            )
            .map_err(storage_err)?;
        Ok(())
    }

    /// Deletes `key` from `workflow_store`.
    pub fn kv_delete(&self, key: &str) -> Result<(), WorkflowError> {
        let guard = self.lock_conn()?;
        guard
            .execute("DELETE FROM workflow_store WHERE key = ?1", params![key])
            .map_err(storage_err)?;
        Ok(())
    }

    /// Lists entries in `workflow_store` whose keys begin with `prefix` in lexicographic order.
    pub fn kv_list(&self, prefix: &str) -> Result<Vec<StoredEntry>, WorkflowError> {
        let upper = format!("{prefix}\u{10FFFF}");
        let guard = self.lock_conn()?;
        let mut stmt = guard
            .prepare(
                "SELECT key, value_json FROM workflow_store
                 WHERE key >= ?1 AND key < ?2 ORDER BY key",
            )
            .map_err(storage_err)?;
        let rows = stmt
            .query_map(params![prefix, upper], |row| {
                let key: String = row.get(0)?;
                let value_raw: String = row.get(1)?;
                Ok((key, value_raw))
            })
            .map_err(storage_err)?;
        let mut out = Vec::new();
        for row in rows {
            let (key, raw) = row.map_err(storage_err)?;
            let value = serde_json::from_str(&raw).map_err(storage_err)?;
            out.push(StoredEntry { key, value });
        }
        Ok(out)
    }

    /// Atomically updates `key` in `workflow_store` inside a SQLite transaction.
    pub fn kv_update(
        &self,
        key: &str,
        change: impl FnOnce(Option<Value>) -> Result<Option<Value>, WorkflowError>,
    ) -> Result<Option<Value>, WorkflowError> {
        let at = now_iso_utc();
        let mut guard = self.lock_conn()?;
        let tx = guard.transaction().map_err(storage_err)?;
        let current_raw: Option<String> = tx
            .query_row(
                "SELECT value_json FROM workflow_store WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_err)?;
        let current = current_raw
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(storage_err)?;
        let next = change(current)?;
        match &next {
            None => {
                tx.execute("DELETE FROM workflow_store WHERE key = ?1", params![key])
                    .map_err(storage_err)?;
            }
            Some(val) => {
                let json_text = serde_json::to_string(val).map_err(storage_err)?;
                tx.execute(
                    "INSERT INTO workflow_store (key, value_json, updated_at)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT (key) DO UPDATE
                     SET value_json = excluded.value_json, updated_at = excluded.updated_at",
                    params![key, json_text, at],
                )
                .map_err(storage_err)?;
            }
        }
        tx.commit().map_err(storage_err)?;
        Ok(next)
    }

    /// Computes live [`WorkflowCounts`] badge numbers.
    pub fn counts(&self) -> Result<WorkflowCounts, WorkflowError> {
        let guard = self.lock_conn()?;
        guard
            .query_row(
                "SELECT
                   (SELECT COUNT(*) FROM workflow_waits
                     WHERE request_json IS NOT NULL AND signalled_at IS NULL) AS questions,
                   (SELECT COUNT(*) FROM workflow_runs WHERE status = 'paused') AS paused,
                   (SELECT COUNT(*) FROM workflow_runs WHERE ended_at IS NULL) AS active,
                   (SELECT COUNT(*) FROM workflow_runs) AS total",
                [],
                |row| {
                    let questions: i64 = row.get(0)?;
                    let paused: i64 = row.get(1)?;
                    let active: i64 = row.get(2)?;
                    let total: i64 = row.get(3)?;
                    Ok(WorkflowCounts {
                        needs_you: questions + paused,
                        active,
                        total,
                    })
                },
            )
            .map_err(storage_err)
    }
}
