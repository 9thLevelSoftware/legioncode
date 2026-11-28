//! Policy acknowledgement merge, Case A/Case B evaluation, and explicit policy ack persistence.

use std::collections::HashSet;

use rusqlite::{Connection, params};
use serde_json::{Value, json};

use super::cards::load_card_row;
use super::cli_import_helpers::select_cli_import_by_card;
use super::invalidation::{RevisionWrite, insert_revision_ledger};
use super::types_m4::{ApplyCliImportInput, CliImportRow};
use super::{WorkflowBoardRepoError, WriteTx, require_non_empty};

pub(super) struct MergedPolicyPayload {
    pub unsupported_policies_json: String,
    pub acknowledged_policies_json: String,
    pub policies_acknowledged_at: Option<String>,
    pub import_provenance_json: String,
    pub all_blocking_acked: bool,
}

fn parse_key_set(raw_json: &str) -> HashSet<String> {
    serde_json::from_str::<Vec<String>>(raw_json)
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

fn collect_policy_keys(policies: &[Value]) -> HashSet<String> {
    policies
        .iter()
        .filter_map(|p| p.get("key").and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn apply_acked_keys_to_policies(policies: &mut [Value], acked: &HashSet<String>) -> (usize, usize) {
    let mut blocking_total = 0_usize;
    let mut blocking_acked = 0_usize;
    for p in policies {
        let key = p
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned();
        let blocks = p
            .get("blocksExecution")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let is_acked = acked.contains(&key);
        if let Some(obj) = p.as_object_mut() {
            obj.insert(
                "status".to_owned(),
                Value::String(
                    if is_acked {
                        "acknowledged"
                    } else {
                        "unsupported"
                    }
                    .to_owned(),
                ),
            );
        }
        if blocks {
            blocking_total += 1;
            if is_acked {
                blocking_acked += 1;
            }
        }
    }
    (blocking_total, blocking_acked)
}

fn update_provenance_policies(
    base_prov_json: &str,
    policies: Vec<Value>,
    acked_vec: &[String],
    synced_at: Option<&str>,
) -> String {
    let mut prov: Value = serde_json::from_str(base_prov_json).unwrap_or_else(|_| json!({}));
    if let Some(obj) = prov.as_object_mut() {
        obj.insert("unsupportedPolicies".to_owned(), Value::Array(policies));
        obj.insert(
            "acknowledgedPolicies".to_owned(),
            Value::Array(acked_vec.iter().cloned().map(Value::String).collect()),
        );
        if let Some(ts) = synced_at {
            obj.insert("lastSyncedAt".to_owned(), Value::String(ts.to_owned()));
        }
    }
    serde_json::to_string(&prov).unwrap_or_else(|_| base_prov_json.to_owned())
}

fn policy_key(p: &Value) -> Option<&str> {
    p.get("key").and_then(Value::as_str).map(str::trim)
}

fn can_carry_forward_ack(key: &str, old_policies: &[Value], new_policies: &[Value]) -> bool {
    let old_matching: Vec<&Value> = old_policies
        .iter()
        .filter(|p| policy_key(p) == Some(key))
        .collect();
    let new_matching: Vec<&Value> = new_policies
        .iter()
        .filter(|p| policy_key(p) == Some(key))
        .collect();
    if old_matching.is_empty() || old_matching.len() != new_matching.len() {
        return false;
    }
    new_matching.iter().all(|new_p| {
        let new_src = new_p.get("sourcePath").and_then(Value::as_str);
        let old_p = if let Some(src) = new_src {
            old_matching
                .iter()
                .copied()
                .find(|op| op.get("sourcePath").and_then(Value::as_str) == Some(src))
                .or_else(|| (old_matching.len() == 1).then_some(old_matching[0]))
        } else {
            (old_matching.len() == 1).then_some(old_matching[0])
        };
        let Some(old_p) = old_p else {
            return false;
        };
        let old_blocks = old_p
            .get("blocksExecution")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let new_blocks = new_p
            .get("blocksExecution")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !old_blocks && new_blocks {
            return false;
        }
        let old_val = old_p.get("configuredValue").filter(|v| !v.is_null());
        let new_val = new_p.get("configuredValue").filter(|v| !v.is_null());
        old_val == new_val
    })
}

/// Merges existing policy acknowledgements with incoming Case B re-import policies.
/// Clears `policies_acknowledged_at` whenever 0 blocking policies remain acknowledged or
/// any newly discovered or escalated blocking policy is unacknowledged.
pub(super) fn merge_case_b_policies(
    existing: &CliImportRow,
    input: &ApplyCliImportInput<'_>,
    now: &str,
) -> MergedPolicyPayload {
    let old_policies: Vec<Value> =
        serde_json::from_str(&existing.unsupported_policies_json).unwrap_or_default();
    let mut new_policies: Vec<Value> =
        serde_json::from_str(input.unsupported_policies_json).unwrap_or_default();
    let present_keys = collect_policy_keys(&new_policies);
    let existing_acked: HashSet<String> = parse_key_set(&existing.acknowledged_policies_json)
        .into_iter()
        .filter(|k| {
            present_keys.contains(k) && can_carry_forward_ack(k, &old_policies, &new_policies)
        })
        .collect();
    let req_acked: HashSet<String> = parse_key_set(input.acknowledged_policies_json)
        .into_iter()
        .filter(|k| present_keys.contains(k))
        .collect();
    let mut merged_acked = existing_acked.clone();
    merged_acked.extend(req_acked);

    let (blocking_total, blocking_acked) =
        apply_acked_keys_to_policies(&mut new_policies, &merged_acked);
    let all_blocking_acked = blocking_total > 0 && blocking_acked == blocking_total;
    let policies_acknowledged_at = if blocking_acked == 0 || !all_blocking_acked {
        None
    } else if merged_acked == existing_acked {
        existing
            .policies_acknowledged_at
            .clone()
            .or_else(|| input.policies_acknowledged_at.map(str::to_owned))
            .or_else(|| Some(now.to_owned()))
    } else {
        input
            .policies_acknowledged_at
            .map(str::to_owned)
            .or_else(|| Some(now.to_owned()))
    };

    let mut acked_vec: Vec<String> = merged_acked.into_iter().collect();
    acked_vec.sort();
    let unsupported_policies_json =
        serde_json::to_string(&new_policies).unwrap_or_else(|_| "[]".to_owned());
    let acknowledged_policies_json =
        serde_json::to_string(&acked_vec).unwrap_or_else(|_| "[]".to_owned());
    let import_provenance_json =
        update_provenance_policies(input.import_provenance_json, new_policies, &acked_vec, None);

    MergedPolicyPayload {
        unsupported_policies_json,
        acknowledged_policies_json,
        policies_acknowledged_at,
        import_provenance_json,
        all_blocking_acked,
    }
}

/// Evaluates whether an unchanged-digest Case A re-import supplies new policy acknowledgements.
pub(super) fn compute_case_a_policy_update(
    existing: &CliImportRow,
    input: &ApplyCliImportInput<'_>,
    now: &str,
) -> Option<MergedPolicyPayload> {
    let mut policies: Vec<Value> =
        serde_json::from_str(&existing.unsupported_policies_json).unwrap_or_default();
    let present_keys = collect_policy_keys(&policies);
    let existing_acked = parse_key_set(&existing.acknowledged_policies_json);
    let req_acked: HashSet<String> = parse_key_set(input.acknowledged_policies_json)
        .into_iter()
        .filter(|k| present_keys.contains(k))
        .collect();
    let mut merged_acked: HashSet<String> = existing_acked
        .iter()
        .filter(|k| present_keys.contains(*k))
        .cloned()
        .collect();
    merged_acked.extend(req_acked);

    if merged_acked == existing_acked {
        return None;
    }

    let (blocking_total, blocking_acked) =
        apply_acked_keys_to_policies(&mut policies, &merged_acked);
    let all_blocking_acked = blocking_total > 0 && blocking_acked == blocking_total;
    let policies_acknowledged_at = if blocking_acked == 0 || !all_blocking_acked {
        None
    } else {
        existing
            .policies_acknowledged_at
            .clone()
            .or_else(|| input.policies_acknowledged_at.map(str::to_owned))
            .or_else(|| Some(now.to_owned()))
    };

    let mut acked_vec: Vec<String> = merged_acked.into_iter().collect();
    acked_vec.sort();
    let unsupported_policies_json =
        serde_json::to_string(&policies).unwrap_or_else(|_| "[]".to_owned());
    let acknowledged_policies_json =
        serde_json::to_string(&acked_vec).unwrap_or_else(|_| "[]".to_owned());
    let import_provenance_json = update_provenance_policies(
        &existing.import_provenance_json,
        policies,
        &acked_vec,
        Some(now),
    );

    Some(MergedPolicyPayload {
        unsupported_policies_json,
        acknowledged_policies_json,
        policies_acknowledged_at,
        import_provenance_json,
        all_blocking_acked,
    })
}

/// Persists explicit acknowledgement of unsupported adopted `.legion-cli` policies on `card_id`.
pub fn acknowledge_card_policies(
    conn: &Connection,
    card_id: &str,
    expected_revision: i64,
    revision_id: &str,
    acknowledged_keys: Option<&[String]>,
    now_iso: &str,
) -> Result<(CliImportRow, i64), WorkflowBoardRepoError> {
    let card_id = require_non_empty(card_id, "card_id")?;
    let revision_id = require_non_empty(revision_id, "revision_id")?;
    let now = require_non_empty(now_iso, "now_iso")?;

    let tx = WriteTx::begin(conn)?;
    let card = load_card_row(conn, card_id)?;
    if card.revision != expected_revision {
        return Err(WorkflowBoardRepoError::RevisionConflict {
            card_id: card_id.to_owned(),
            expected_revision,
            actual_revision: card.revision,
        });
    }
    let Some(row) = select_cli_import_by_card(conn, card_id)? else {
        return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
            "Card `{card_id}` has no CLI import metadata"
        )));
    };

    let (policies_json, acked_json, prov_json, has_blocking_acked) =
        compute_acknowledged_policy_payloads(&row, acknowledged_keys, now)?;
    let next_rev = card.revision + 1;
    let acked_at = has_blocking_acked.then_some(now);

    conn.execute(
        "UPDATE workflow_board_cli_imports
         SET unsupported_policies_json = ?2,
             acknowledged_policies_json = ?3,
             policies_acknowledged_at = ?4,
             import_provenance_json = ?5,
             last_synced_at = ?6
         WHERE card_id = ?1",
        params![card_id, policies_json, acked_json, acked_at, prov_json, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "acknowledge_cli_import_policies",
        source,
    })?;
    conn.execute(
        "UPDATE workflow_board_cards
         SET revision = ?3,
             import_provenance_json = ?4,
             updated_at = ?5
         WHERE card_id = ?1 AND revision = ?2",
        params![card_id, expected_revision, next_rev, prov_json, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "bump_card_revision_on_policy_ack",
        source,
    })?;
    insert_revision_ledger(
        conn,
        &RevisionWrite {
            revision_id,
            card_id,
            revision: next_rev,
            stage: &card.stage,
            kind: "cli_policies_acknowledged",
            summary: "Acknowledged unsupported adopted CLI policies",
            spec_digest: card.spec_digest.as_deref(),
            plan_digest: card.plan_digest.as_deref(),
            payload_json: &acked_json,
            now,
        },
    )?;
    let updated = select_cli_import_by_card(conn, card_id)?.ok_or_else(|| {
        WorkflowBoardRepoError::CardNotFound {
            card_id: card_id.to_owned(),
        }
    })?;
    tx.commit()?;
    Ok((updated, next_rev))
}

fn compute_acknowledged_policy_payloads(
    row: &CliImportRow,
    acknowledged_keys: Option<&[String]>,
    now: &str,
) -> Result<(String, String, String, bool), WorkflowBoardRepoError> {
    let mut policies: Vec<Value> =
        serde_json::from_str(&row.unsupported_policies_json).unwrap_or_default();
    let mut acked = parse_key_set(&row.acknowledged_policies_json);
    match acknowledged_keys {
        Some(keys) => {
            for k in keys {
                let t = k.trim();
                if !t.is_empty() {
                    acked.insert(t.to_owned());
                }
            }
        }
        None => {
            for p in &policies {
                if let Some(k) = p.get("key").and_then(Value::as_str) {
                    acked.insert(k.to_owned());
                }
            }
        }
    }

    let mut still_blocking = Vec::new();
    let mut blocking_acked = 0_usize;
    for p in &mut policies {
        let key = p
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let blocks = p
            .get("blocksExecution")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if acked.contains(&key) {
            if let Some(obj) = p.as_object_mut() {
                obj.insert(
                    "status".to_owned(),
                    Value::String("acknowledged".to_owned()),
                );
            }
            if blocks {
                blocking_acked += 1;
            }
        } else if blocks {
            still_blocking.push(key);
        }
    }
    if !still_blocking.is_empty() {
        return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
            "Unsupported adopted CLI policies still require acknowledgement: {}",
            still_blocking.join(", ")
        )));
    }

    let mut acked_vec: Vec<String> = acked.into_iter().collect();
    acked_vec.sort();
    let policies_json = serde_json::to_string(&policies).unwrap_or_else(|_| "[]".to_owned());
    let acked_json = serde_json::to_string(&acked_vec).unwrap_or_else(|_| "[]".to_owned());
    let prov_json =
        update_provenance_policies(&row.import_provenance_json, policies, &acked_vec, Some(now));
    Ok((policies_json, acked_json, prov_json, blocking_acked > 0))
}
