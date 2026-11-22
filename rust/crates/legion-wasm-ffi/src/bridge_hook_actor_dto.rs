//! Request and response DTOs for [`crate::bridge_hook_actor::HookInboxDurableObjectBridge`].

use std::collections::BTreeMap;

use legion_edge::cf_bindings::HookInboxDurableActor;
use legion_edge::relay_hooks::{DeliveryBatch, DeliveryOutcome, DeliveryRecordOutcome, HeldHook};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct HoldHookRequest {
    #[serde(alias = "delivery_id", alias = "deliveryId")]
    pub id: Option<String>,
    #[serde(alias = "hook_key", alias = "hookId", alias = "hook_id")]
    pub hook_key: Option<String>,
    #[serde(alias = "raw_hook_id")]
    pub raw_hook_id: Option<String>,
    #[serde(alias = "raw_token")]
    pub raw_token: Option<String>,
    #[serde(alias = "targetUrl", alias = "target_url", alias = "base_url")]
    pub base_url: Option<String>,
    #[serde(alias = "receivedAtMs", alias = "received_at_ms", alias = "now_ms")]
    pub now_ms: Option<u64>,
    pub method: Option<String>,
    pub query: Option<String>,
    pub headers: BTreeMap<String, String>,
    #[serde(alias = "payload")]
    pub body: Option<serde_json::Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct RecordOutcomeRequest {
    #[serde(alias = "snapshot_generation")]
    pub snapshot_generation: u64,
    #[serde(alias = "hook_id", alias = "deliveryId", alias = "delivery_id")]
    pub hook_id: String,
    pub status: Option<u16>,
    pub outcome: Option<DeliveryOutcome>,
    #[serde(alias = "now_ms")]
    pub now_ms: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ActorFetchRequest {
    pub method: Option<String>,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    #[serde(alias = "payload")]
    pub body: Option<serde_json::Value>,
    #[serde(alias = "now_ms")]
    pub now_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActorSummaryOut {
    pub endpoint_key: String,
    pub target_generation: u64,
    pub base_url: Option<String>,
    pub failures: u32,
    pub pending_count: usize,
    pub pending_bytes: usize,
    pub alarm_at_ms: Option<u64>,
}

impl From<&HookInboxDurableActor> for ActorSummaryOut {
    fn from(actor: &HookInboxDurableActor) -> Self {
        let target = actor.state().target();
        Self {
            endpoint_key: actor.endpoint_key.clone(),
            target_generation: actor.target_generation(),
            base_url: target.map(|t| t.base_url.clone()),
            failures: target.map_or(0, |t| t.failures),
            pending_count: actor.pending_count(),
            pending_bytes: actor.state().pending_bytes(),
            alarm_at_ms: actor.alarm_at_ms(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HeldHookOut {
    pub id: String,
    pub hook_key: String,
    pub received_at_ms: u64,
    pub method: String,
    pub raw_hook_id: String,
    pub raw_token: String,
    pub query: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
    pub body_text: String,
}

impl From<&HeldHook> for HeldHookOut {
    fn from(hook: &HeldHook) -> Self {
        let headers: BTreeMap<String, String> = hook
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let body_text = String::from_utf8_lossy(&hook.body).into_owned();
        Self {
            id: hook.id.clone(),
            hook_key: hook.hook_key.clone(),
            received_at_ms: hook.received_at_ms,
            method: hook.method.clone(),
            raw_hook_id: hook.raw_hook_id.clone(),
            raw_token: hook.raw_token.clone(),
            query: hook.query.clone(),
            headers,
            body: hook.body.clone(),
            body_text,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AlarmBatchOut {
    pub empty: bool,
    pub target_generation: Option<u64>,
    pub base_url: Option<String>,
    pub hooks: Vec<HeldHookOut>,
    pub alarm_at_ms: Option<u64>,
    pub pending_count: usize,
}

impl AlarmBatchOut {
    pub fn from_option(batch: Option<DeliveryBatch>, actor: &HookInboxDurableActor) -> Self {
        match batch {
            Some(batch) => Self {
                empty: false,
                target_generation: Some(batch.target_generation),
                base_url: Some(batch.base_url),
                hooks: batch.hooks.iter().map(HeldHookOut::from).collect(),
                alarm_at_ms: actor.alarm_at_ms(),
                pending_count: actor.pending_count(),
            },
            None => Self {
                empty: true,
                target_generation: None,
                base_url: None,
                hooks: Vec::new(),
                alarm_at_ms: actor.alarm_at_ms(),
                pending_count: actor.pending_count(),
            },
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordOutcomeOut {
    pub outcome_kind: &'static str,
    pub ignored_stale: bool,
    pub remaining_pending: Option<bool>,
    pub failures: u32,
    pub next_retry_at_ms: Option<u64>,
    pub snapshot_generation: Option<u64>,
    pub current_generation: u64,
    pub alarm_at_ms: Option<u64>,
    pub pending_count: usize,
}

impl RecordOutcomeOut {
    pub fn from_result(result: DeliveryRecordOutcome, actor: &HookInboxDurableActor) -> Self {
        let current_failures = actor.state().target().map_or(0, |t| t.failures);
        let current_generation = actor.target_generation();
        let alarm_at_ms = actor.alarm_at_ms();
        let pending_count = actor.pending_count();
        match result {
            DeliveryRecordOutcome::Delivered { remaining_pending } => Self {
                outcome_kind: "delivered",
                ignored_stale: false,
                remaining_pending: Some(remaining_pending),
                failures: current_failures,
                next_retry_at_ms: None,
                snapshot_generation: None,
                current_generation,
                alarm_at_ms,
                pending_count,
            },
            DeliveryRecordOutcome::Busy => Self {
                outcome_kind: "busy",
                ignored_stale: false,
                remaining_pending: Some(pending_count > 0),
                failures: current_failures,
                next_retry_at_ms: None,
                snapshot_generation: None,
                current_generation,
                alarm_at_ms,
                pending_count,
            },
            DeliveryRecordOutcome::Unreachable {
                failures,
                next_retry_at_ms,
            } => Self {
                outcome_kind: "unreachable",
                ignored_stale: false,
                remaining_pending: Some(pending_count > 0),
                failures,
                next_retry_at_ms: Some(next_retry_at_ms),
                snapshot_generation: None,
                current_generation,
                alarm_at_ms,
                pending_count,
            },
            DeliveryRecordOutcome::IgnoredStale {
                snapshot_generation,
                current_generation: cur_gen,
                next_attempt_at_ms,
            } => Self {
                outcome_kind: "ignored_stale",
                ignored_stale: true,
                remaining_pending: None,
                failures: current_failures,
                next_retry_at_ms: next_attempt_at_ms,
                snapshot_generation: Some(snapshot_generation),
                current_generation: cur_gen,
                alarm_at_ms,
                pending_count,
            },
            DeliveryRecordOutcome::IgnoredCleared => Self {
                outcome_kind: "ignored_cleared",
                ignored_stale: false,
                remaining_pending: None,
                failures: current_failures,
                next_retry_at_ms: None,
                snapshot_generation: None,
                current_generation,
                alarm_at_ms,
                pending_count,
            },
        }
    }
}
