//! Effect descriptors, preloaded Cloudflare binding inputs, and dispatch outcome envelopes
//! for [`crate::cf_worker::CloudflareWorkerRouter::dispatch_with_bindings`].

use std::collections::BTreeMap;

use legion_edge::cf_bindings::{
    D1BindingPlan, HookInboxDurableActor, HyperdriveRelayPlan, R2BindingOp,
};
use legion_edge::task_cache::SavingsTotals;
use serde::{Deserialize, Serialize};

use crate::cf_worker::WorkerHttpResponse;

/// Declarative side effect emitted by [`crate::cf_worker::CloudflareWorkerRouter`] for execution
/// by a native host runtime or JS Cloudflare Worker entrypoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EdgeHostEffect {
    /// Parameterized Cloudflare D1 query targeting `"INDEX"` or `"SAVINGS"`.
    D1Query {
        /// Parameterized SQL statement and positional bind arguments.
        plan: D1BindingPlan,
    },
    /// Typed Cloudflare R2 bucket operation targeting `"ARTIFACTS"` or `"CLIENTS"`.
    R2Op {
        /// Typed R2 storage command.
        op: R2BindingOp,
    },
    /// Parameterized Postgres query over Cloudflare Hyperdrive for `infra/relay`.
    HyperdriveQuery {
        /// Parameterized PostgreSQL statement and bind arguments.
        plan: HyperdriveRelayPlan,
    },
    /// Cloudflare Rate Limiting binding evaluation.
    RateLimitCheck {
        /// Binding identifier (`"READ_LIMITER"`, `"STORE_LIMITER"`, `"HOOK_RATE_LIMIT"`).
        binding: String,
        /// Per-operation rate limit key.
        key: String,
        /// Allowed operations per window.
        limit: u32,
        /// Window duration in seconds.
        period_seconds: u32,
    },
    /// Durable Object stub invocation against `HookInboxObject`.
    DurableObjectCall {
        /// Durable Object namespace identifier.
        namespace: String,
        /// Durable Object instance name (`endpoint_key`).
        object_id: String,
        /// Method invoked (`"hold"`, `"wake"`, `"deliver-batch"`, `"outcome"`).
        method: String,
        /// Serialized actor snapshot including `alarm_at_ms` and `BUG-29` `target_generation`.
        actor: Option<HookInboxDurableActor>,
    },
    /// Structured telemetry/metric emission.
    EmitMetric {
        /// Metric or span event name.
        name: String,
        /// Dimensional string attributes.
        tags: BTreeMap<String, String>,
        /// Numeric observation value.
        value: u64,
    },
}

impl EdgeHostEffect {
    /// Wraps a [`D1BindingPlan`] as an [`EdgeHostEffect::D1Query`].
    pub const fn d1_query(plan: D1BindingPlan) -> Self {
        Self::D1Query { plan }
    }

    /// Wraps an [`R2BindingOp`] as an [`EdgeHostEffect::R2Op`].
    pub const fn r2_op(op: R2BindingOp) -> Self {
        Self::R2Op { op }
    }

    /// Wraps a [`HyperdriveRelayPlan`] as an [`EdgeHostEffect::HyperdriveQuery`].
    pub const fn hyperdrive_query(plan: HyperdriveRelayPlan) -> Self {
        Self::HyperdriveQuery { plan }
    }

    /// Constructs an [`EdgeHostEffect::RateLimitCheck`].
    pub fn rate_limit_check(
        binding: impl Into<String>,
        key: impl Into<String>,
        limit: u32,
        period_seconds: u32,
    ) -> Self {
        Self::RateLimitCheck {
            binding: binding.into(),
            key: key.into(),
            limit,
            period_seconds,
        }
    }

    /// Constructs an [`EdgeHostEffect::DurableObjectCall`].
    pub fn durable_object_call(
        namespace: impl Into<String>,
        object_id: impl Into<String>,
        method: impl Into<String>,
        actor: Option<HookInboxDurableActor>,
    ) -> Self {
        Self::DurableObjectCall {
            namespace: namespace.into(),
            object_id: object_id.into(),
            method: method.into(),
            actor,
        }
    }

    /// Constructs an [`EdgeHostEffect::EmitMetric`].
    pub fn emit_metric(
        name: impl Into<String>,
        tags: BTreeMap<String, String>,
        value: u64,
    ) -> Self {
        Self::EmitMetric {
            name: name.into(),
            tags,
            value,
        }
    }
}

/// Preloaded D1 `INDEX` row returned before cache metadata resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreloadedCacheHit {
    /// Stored artifact size in bytes.
    pub bytes: u64,
    /// `true` when matched via secondary fallback association.
    pub fallback: bool,
}

/// Optional host-supplied binding read results injected into
/// [`crate::cf_worker::CloudflareWorkerRouter::dispatch_with_bindings`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BindingReadResults {
    /// Preloaded rate limiter decision (`Some(false)` rejects the request with `429 rate_limit`).
    pub rate_limit_allowed: Option<bool>,
    /// Preloaded Hyperdrive result from `relay_upstream_api_pauses` (`BUG-32`).
    /// `Ok(Some(ms))` / `Ok(None)` advance `checked_at_ms`, while `Err(_)` fails open without
    /// advancing `checked_at_ms`.
    pub hyperdrive_paused_until_ms: Option<Result<Option<u64>, String>>,
    /// Preloaded D1 `INDEX` lookup outcome for `POST /projects/legioncode/fetch`.
    pub d1_index_hit: Option<Result<Option<PreloadedCacheHit>, String>>,
    /// Preloaded `SAVINGS` lifetime totals snapshot.
    pub d1_savings_totals: Option<SavingsTotals>,
    /// Preloaded R2 object payload bytes for native-client downloads.
    pub r2_object_bytes: Option<Vec<u8>>,
    /// Preloaded Hyperdrive check indicating whether the target environment link is active.
    pub relay_environment_link_active: Option<bool>,
    /// Optional serialized Durable Object actor snapshot to hydrate an uninitialized endpoint.
    pub hook_actor: Option<HookInboxDurableActor>,
}

/// Unified HTTP response and planned Cloudflare binding side effects returned by
/// [`crate::cf_worker::CloudflareWorkerRouter::dispatch_with_bindings`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerDispatchOutcome {
    /// HTTP response returned to the caller.
    pub response: WorkerHttpResponse,
    /// Ordered Cloudflare host effects planned during dispatch.
    pub effects: Vec<EdgeHostEffect>,
}

pub(crate) fn metric_tags<const N: usize>(pairs: [(&str, &str); N]) -> BTreeMap<String, String> {
    pairs
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

pub(crate) fn check_rate_limit(
    effects: &mut Vec<EdgeHostEffect>,
    binding: &str,
    key: &str,
    limit: u32,
    period_seconds: u32,
    preloaded: Option<&BindingReadResults>,
) -> bool {
    effects.push(EdgeHostEffect::rate_limit_check(
        binding,
        key,
        limit,
        period_seconds,
    ));
    !matches!(preloaded.and_then(|p| p.rate_limit_allowed), Some(false))
}
