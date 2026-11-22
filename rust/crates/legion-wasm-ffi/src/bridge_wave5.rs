//! Pure Safe Rust host-testable Wave 5 bridges exported through `legion-wasm-ffi`.

use std::collections::BTreeSet;

use legion_client_core::linked_threads::{
    LINK_BATCH_CONCURRENCY, LinkBatchQueue, LinkListState, LinkableMachine, LinkedThreadTarget,
    link_batch_candidates, resolve_linked_thread_open,
};
use legion_contracts::brand::ACTIVE_BRAND;
use legion_contracts::ids::EnvironmentId;
use legion_contracts::peer_links::PeerLinkName;
use legion_contracts::policy::{ManagedPolicySummary, allows_preview_tunnel};
use legion_shared::text_norm::file_display_name;
use legion_terminal_vt::sanitize_terminal_history;
use legion_web_ui::hdl_highlight::{resolve_hdl_language, tokenize_hdl};
use serde::{Deserialize, Serialize};

/// Normalizes macOS NFD filenames and extracts clean NFC display names (`legion_shared::text_norm`).
#[must_use]
pub fn normalize_nfd_filename(raw: &str) -> String {
    file_display_name(raw)
}

/// Strips standalone `OSC 4` palette queries and dynamic color queries from terminal history
/// (`legion_terminal_vt::history_sanitizer`).
#[must_use]
pub fn strip_osc4_queries(raw: &str) -> String {
    sanitize_terminal_history(raw)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AllowsPreviewTunnelRequest {
    enable_preview_tunnel: bool,
    #[serde(default, alias = "policySummary")]
    managed_policy: Option<ManagedPolicySummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AllowsPreviewTunnelResponse {
    allowed: bool,
}

/// Evaluates whether loopback preview tunnels are permitted by settings and managed policy
/// (`legion_contracts::policy::allows_preview_tunnel`).
pub fn allows_preview_tunnel_json(input_json: &str) -> Result<String, String> {
    let req: AllowsPreviewTunnelRequest =
        serde_json::from_str(input_json).map_err(|err| err.to_string())?;
    let allowed = allows_preview_tunnel(req.enable_preview_tunnel, req.managed_policy.as_ref());
    serde_json::to_string(&AllowsPreviewTunnelResponse { allowed }).map_err(|err| err.to_string())
}

/// Serializes the compile-time [`ACTIVE_BRAND`] configuration as `JSON`.
pub fn active_brand_json() -> Result<String, String> {
    serde_json::to_string(&ACTIVE_BRAND).map_err(|err| err.to_string())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveLinkedThreadRequest {
    target: LinkedThreadTarget,
    #[serde(default)]
    own_connected: Option<bool>,
    #[serde(default)]
    linked: Option<bool>,
    #[serde(default)]
    peer_links: Vec<PeerLinkName>,
}

/// Resolves whether a linked thread opens directly or via a peer relay stream.
pub fn resolve_linked_thread_open_json(input_json: &str) -> Result<String, String> {
    let req: ResolveLinkedThreadRequest =
        serde_json::from_str(input_json).map_err(|err| err.to_string())?;
    let is_linked = req.linked.unwrap_or_else(|| {
        req.peer_links.iter().any(|p| {
            p.environment_id == req.target.environment_id
                || p.environment_id == req.target.via_environment_id
        })
    });
    let strategy = resolve_linked_thread_open(
        &req.target.environment_id,
        &req.target.via_environment_id,
        req.own_connected,
        is_linked,
    );
    serde_json::to_string(&strategy).map_err(|err| err.to_string())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinkBatchPlanRequest {
    #[serde(default)]
    machines: Vec<LinkableMachine>,
    #[serde(default)]
    writable_environment_ids: Vec<String>,
    #[serde(default = "default_loaded_state")]
    list_state: LinkListState,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkBatchPlanCandidate {
    environment_id: String,
    blocked: Option<legion_client_core::linked_threads::LinkBlockReason>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkBatchPlanResponse {
    concurrency: usize,
    started_ids: Vec<String>,
    candidates: Vec<LinkBatchPlanCandidate>,
}

const fn default_loaded_state() -> LinkListState {
    LinkListState::Loaded
}

/// Plans bounded-concurrency batch P2P linking (`LINK_BATCH_CONCURRENCY = 2`).
pub fn link_batch_plan_json(input_json: &str) -> Result<String, String> {
    let req: LinkBatchPlanRequest =
        serde_json::from_str(input_json).map_err(|err| err.to_string())?;
    let writable: BTreeSet<EnvironmentId> = req
        .writable_environment_ids
        .iter()
        .filter_map(|id| EnvironmentId::new(id).ok())
        .collect();
    let cands = link_batch_candidates(&req.machines, &writable, req.list_state);
    let mut queue = LinkBatchQueue::from_candidates(&cands);
    let started_ids: Vec<String> = queue
        .take_next_batch()
        .into_iter()
        .map(|id| id.as_str().to_owned())
        .collect();
    let candidates = cands
        .into_iter()
        .map(|cand| LinkBatchPlanCandidate {
            environment_id: cand.machine.environment_id.as_str().to_owned(),
            blocked: cand.blocked,
        })
        .collect();
    serde_json::to_string(&LinkBatchPlanResponse {
        concurrency: LINK_BATCH_CONCURRENCY,
        started_ids,
        candidates,
    })
    .map_err(|err| err.to_string())
}

/// Tokenizes Verilog, `SystemVerilog`, or VHDL snippets (`legion_web_ui::hdl_highlight`).
pub fn highlight_hdl_json(lang_or_path: &str, source: &str) -> Result<String, String> {
    let Some(lang) = resolve_hdl_language(lang_or_path) else {
        return serde_json::to_string(&serde_json::json!({ "language": null, "tokens": [] }))
            .map_err(|err| err.to_string());
    };
    let tokens = tokenize_hdl(lang, source);
    serde_json::to_string(&serde_json::json!({ "language": lang, "tokens": tokens }))
        .map_err(|err| err.to_string())
}
