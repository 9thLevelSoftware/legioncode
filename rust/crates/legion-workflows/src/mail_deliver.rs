//! Buffered workflow mail (`workflow_mail`), 7-day signal dedup (`workflow_signals`), and delivery.

use std::collections::HashSet;

use rusqlite::{OptionalExtension, params};
use serde_json::{Map, Value};

use crate::schema::validate_json_schema_value;
use crate::store::{WorkflowStore, storage_err};
use crate::time::{format_millis_iso, now_epoch_ms, now_iso_utc};
use crate::types::{SignalAddress, SignalDelivery, WorkflowError};
use crate::waits_mail::{DEFAULT_SIGNAL_TTL_MS, WaitRow};

/// Buffered signal retrieved by [`WorkflowStore::mail_next`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferedMail {
    /// Deduplication signal ID.
    pub signal_id: String,
    /// Serialized signal value JSON.
    pub value_json: String,
    /// Target signal address matched.
    pub address: SignalAddress,
}

/// Result of attempting to deliver a buffered mail item to an open wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailDeliverOutcome {
    /// Mail delivered and wait claimed atomically.
    Delivered,
    /// Signal already consumed or spent by another wait.
    Gone,
    /// Target wait was already claimed by another signal.
    Claimed,
}

/// Encodes a [`SignalAddress`] into a tuple JSON string so names/keys never collide.
pub fn encode_mail_address(address: &SignalAddress) -> String {
    match address {
        SignalAddress::Run { run_id, wait } => serde_json::json!(["run", run_id, wait]).to_string(),
        SignalAddress::Channel { workflow, channel } => {
            serde_json::json!(["key", workflow, channel]).to_string()
        }
    }
}

impl WorkflowStore {
    /// Buffers a signal in `workflow_mail` and extends any matching dedup receipt TTL.
    pub fn mail_put(
        &self,
        signal_id: &str,
        address: &SignalAddress,
        value_json: &str,
        expires_at: &str,
    ) -> Result<(), WorkflowError> {
        let encoded_addr = encode_mail_address(address);
        let mut guard = self.lock_conn()?;
        let tx = guard.transaction().map_err(storage_err)?;
        tx.execute(
            "INSERT INTO workflow_mail (signal_id, address, value_json, expires_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (signal_id) DO NOTHING",
            params![signal_id, encoded_addr, value_json, expires_at],
        )
        .map_err(storage_err)?;
        tx.execute(
            "UPDATE workflow_signals SET kept_until = ?1
             WHERE signal_id = ?2 AND kept_until < ?1",
            params![expires_at, signal_id],
        )
        .map_err(storage_err)?;
        tx.commit().map_err(storage_err)?;
        Ok(())
    }

    /// Returns the oldest buffered signal matching any address in `addresses` not in `passed`.
    pub fn mail_next(
        &self,
        addresses: &[SignalAddress],
        passed: &HashSet<String>,
    ) -> Result<Option<BufferedMail>, WorkflowError> {
        let guard = self.lock_conn()?;
        let mut stmt = guard
            .prepare(
                "SELECT signal_id, value_json FROM workflow_mail
                 WHERE address = ?1 ORDER BY rowid",
            )
            .map_err(storage_err)?;
        for address in addresses {
            let encoded = encode_mail_address(address);
            let rows = stmt
                .query_map(params![encoded], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(storage_err)?;
            for row in rows {
                let (signal_id, value_json) = row.map_err(storage_err)?;
                if !passed.contains(&signal_id) {
                    return Ok(Some(BufferedMail {
                        signal_id,
                        value_json,
                        address: address.clone(),
                    }));
                }
            }
        }
        Ok(None)
    }

    /// Deletes a buffered signal from `workflow_mail`.
    pub fn mail_delete(&self, signal_id: &str) -> Result<(), WorkflowError> {
        let guard = self.lock_conn()?;
        guard
            .execute(
                "DELETE FROM workflow_mail WHERE signal_id = ?1",
                params![signal_id],
            )
            .map_err(storage_err)?;
        Ok(())
    }

    /// Deletes expired `workflow_mail` and `workflow_signals` rows as of `now_iso`.
    pub fn mail_expire(&self, now_iso: &str) -> Result<(), WorkflowError> {
        let mut guard = self.lock_conn()?;
        let tx = guard.transaction().map_err(storage_err)?;
        tx.execute(
            "DELETE FROM workflow_mail WHERE expires_at <= ?1",
            params![now_iso],
        )
        .map_err(storage_err)?;
        tx.execute(
            "DELETE FROM workflow_signals WHERE kept_until <= ?1",
            params![now_iso],
        )
        .map_err(storage_err)?;
        tx.commit().map_err(storage_err)?;
        Ok(())
    }

    /// Returns `true` if `signal_id` already woke a wait within its retention window.
    pub fn signal_seen(&self, signal_id: &str) -> Result<bool, WorkflowError> {
        let guard = self.lock_conn()?;
        let found: Option<i64> = guard
            .query_row(
                "SELECT 1 FROM workflow_signals WHERE signal_id = ?1",
                params![signal_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_err)?;
        Ok(found.is_some())
    }

    /// Atomically delivers a buffered mail item to `(run_id, name)` if not already seen/claimed.
    pub fn mail_deliver(
        &self,
        signal_id: &str,
        run_id: &str,
        name: &str,
        woke: &Value,
    ) -> Result<MailDeliverOutcome, WorkflowError> {
        let at = now_iso_utc();
        let woke_json = serde_json::to_string(woke).map_err(storage_err)?;
        let keep_default = format_millis_iso(now_epoch_ms().saturating_add(DEFAULT_SIGNAL_TTL_MS));
        let outcome = {
            let mut guard = self.lock_conn()?;
            let tx = guard.transaction().map_err(storage_err)?;
            let seen: Option<i64> = tx
                .query_row(
                    "SELECT 1 FROM workflow_signals WHERE signal_id = ?1",
                    params![signal_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage_err)?;
            if seen.is_some() {
                tx.execute(
                    "DELETE FROM workflow_mail WHERE signal_id = ?1",
                    params![signal_id],
                )
                .map_err(storage_err)?;
                tx.commit().map_err(storage_err)?;
                return Ok(MailDeliverOutcome::Gone);
            }
            let claimed: Option<String> = tx
                .query_row(
                    "UPDATE workflow_waits SET signalled_at = ?1, woke_json = ?2
                     WHERE run_id = ?3 AND name = ?4 AND signalled_at IS NULL
                     RETURNING name",
                    params![at, woke_json, run_id, name],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage_err)?;
            if claimed.is_none() {
                return Ok(MailDeliverOutcome::Claimed);
            }
            let expires_at: Option<String> = tx
                .query_row(
                    "DELETE FROM workflow_mail WHERE signal_id = ?1 RETURNING expires_at",
                    params![signal_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage_err)?;
            let Some(mail_expires) = expires_at else {
                return Ok(MailDeliverOutcome::Gone);
            };
            let kept_until = if mail_expires > keep_default {
                mail_expires
            } else {
                keep_default
            };
            let receipt: Option<String> = tx
                .query_row(
                    "INSERT INTO workflow_signals (signal_id, kept_until)
                     VALUES (?1, ?2)
                     ON CONFLICT (signal_id) DO NOTHING
                     RETURNING signal_id",
                    params![signal_id, kept_until],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage_err)?;
            if receipt.is_none() {
                return Ok(MailDeliverOutcome::Gone);
            }
            tx.commit().map_err(storage_err)?;
            MailDeliverOutcome::Delivered
        };
        if outcome == MailDeliverOutcome::Delivered {
            self.notify_change(run_id);
            self.emit_question_resolved(run_id, name, woke, &at);
        }
        Ok(outcome)
    }

    /// Delivers a signal to `address`, validating schema and buffering into `workflow_mail` if needed.
    pub fn signal_or_buffer(
        &self,
        address: &SignalAddress,
        value: &Value,
        signal_id: &str,
        ttl_ms: Option<i64>,
    ) -> Result<(SignalDelivery, Option<WaitRow>), WorkflowError> {
        if self.signal_seen(signal_id)? {
            return Ok((SignalDelivery::Delivered, None));
        }
        let value_json = serde_json::to_string(value).map_err(storage_err)?;
        let woke = serde_json::json!({ "expired": false, "value": value });
        while let Some(wait) = self.wait_at(address)? {
            let schema_map = parse_schema_map(&wait.schema_json);
            if let Err(err) = validate_json_schema_value(&schema_map, value, true) {
                return Err(WorkflowError::Input {
                    problem: format!("The value does not fit what {} waits for. {err}", wait.name),
                });
            }
            if self.wait_signalled(&wait.run_id, &wait.name, &woke, Some(signal_id))? {
                return Ok((SignalDelivery::Delivered, Some(wait)));
            }
            if self.signal_seen(signal_id)? {
                return Ok((SignalDelivery::Delivered, None));
            }
        }
        let ttl = ttl_ms.unwrap_or(DEFAULT_SIGNAL_TTL_MS).max(0);
        let expires_at = format_millis_iso(now_epoch_ms().saturating_add(ttl));
        if self.signal_seen(signal_id)? {
            return Ok((SignalDelivery::Delivered, None));
        }
        self.mail_put(signal_id, address, &value_json, &expires_at)?;
        while let Some(late) = self.wait_at(address)? {
            let schema_map = parse_schema_map(&late.schema_json);
            if validate_json_schema_value(&schema_map, value, true).is_err() {
                return Ok((SignalDelivery::Buffered, None));
            }
            match self.mail_deliver(signal_id, &late.run_id, &late.name, &woke)? {
                MailDeliverOutcome::Delivered => {
                    return Ok((SignalDelivery::Delivered, Some(late)));
                }
                MailDeliverOutcome::Gone => return Ok((SignalDelivery::Delivered, None)),
                MailDeliverOutcome::Claimed => {}
            }
        }
        Ok((SignalDelivery::Buffered, None))
    }

    /// Broadcasts `value` to all currently open waits on `channel` (never buffered).
    pub fn signal_all_open(
        &self,
        channel: &str,
        workflow: Option<&str>,
        value: &Value,
    ) -> Result<Vec<WaitRow>, WorkflowError> {
        let woke = serde_json::json!({ "expired": false, "value": value });
        let mut woken = Vec::new();
        for wait in self.waits_on(channel, workflow)? {
            let schema_map = parse_schema_map(&wait.schema_json);
            if validate_json_schema_value(&schema_map, value, true).is_err() {
                continue;
            }
            if self.wait_signalled(&wait.run_id, &wait.name, &woke, None)? {
                woken.push(wait);
            }
        }
        Ok(woken)
    }
}

fn parse_schema_map(raw: &str) -> Map<String, Value> {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}
