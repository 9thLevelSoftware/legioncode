//! Safe Rust bridges for `BUG-26` HTTP header-only host swapping and per-pane theme plans.

use t3_surfaces::mobile_tunnel::{
    HttpHeaderHostSwap, HttpProxyPhase, TerminalThemeFileStore, render_ghostty_theme_config,
    sanitize_pane_id,
};

use crate::core_tunnel_types::{
    HostSwapChunksRequest, HostSwapChunksResponse, ThemePlanAction, ThemePlanOperation,
    ThemePlanRequest, ThemePlanResponse, ThemePlanStepOutput,
};
use crate::core_types::MobileFfiError;

pub(crate) fn host_swap_chunks_json(input_json: &str) -> Result<String, MobileFfiError> {
    let req: HostSwapChunksRequest = serde_json::from_str(input_json)
        .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))?;

    if req.from_host.trim().is_empty() || req.to_host.trim().is_empty() {
        return Err(MobileFfiError::InvalidInput(
            "fromHost and toHost must be non-empty".to_owned(),
        ));
    }

    let mut swap = HttpHeaderHostSwap::new(req.from_host, req.to_host);
    let mut output_chunks = Vec::with_capacity(req.chunks.len() + 1);
    let mut output_bytes = Vec::new();
    let mut transitioned_at_chunk = None;

    for (idx, chunk) in req.chunks.iter().enumerate() {
        let emitted = swap.push(chunk.as_bytes());
        if swap.phase() == HttpProxyPhase::BodyPassthrough && transitioned_at_chunk.is_none() {
            transitioned_at_chunk = Some(idx);
        }
        output_bytes.extend_from_slice(&emitted);
        output_chunks.push(String::from_utf8_lossy(&emitted).into_owned());
    }

    if req.finish {
        let flushed = swap.finish();
        if !flushed.is_empty() {
            output_bytes.extend_from_slice(&flushed);
            output_chunks.push(String::from_utf8_lossy(&flushed).into_owned());
        }
    }

    let response = HostSwapChunksResponse {
        phase: swap.phase(),
        transitioned_at_chunk,
        output_chunks,
        output: String::from_utf8_lossy(&output_bytes).into_owned(),
        output_bytes,
    };

    serde_json::to_string(&response).map_err(|err| MobileFfiError::InvalidJson(err.to_string()))
}

pub(crate) fn theme_plan_json(input_json: &str) -> Result<String, MobileFfiError> {
    let req: ThemePlanRequest = serde_json::from_str(input_json)
        .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))?;

    if req.pane_id.is_none() && req.operations.is_empty() {
        return Err(MobileFfiError::InvalidInput(
            "paneId or operations must be provided".to_owned(),
        ));
    }

    let mut store = TerminalThemeFileStore::new();
    let mut pane_order = Vec::new();
    for seed in req.prior_entries {
        if seed.action != ThemePlanAction::Upsert {
            return Err(MobileFfiError::InvalidInput(
                "priorEntries must use upsert action".to_owned(),
            ));
        }
        let payload = render_operation_contents(&seed)?;
        if store.get(&seed.pane_id).is_none() {
            pane_order.push(seed.pane_id.clone());
        }
        let _ = store.upsert(&seed.pane_id, payload);
    }

    let mut steps = req.operations;
    if let Some(pane_id) = req.pane_id {
        steps.push(ThemePlanOperation {
            action: ThemePlanAction::Upsert,
            pane_id,
            theme_config: req.theme_config,
            font_size: req.font_size,
            appearance: req.appearance,
            contents: req.contents,
        });
    }

    let mut results = Vec::with_capacity(steps.len());
    for op in steps {
        let sanitized = sanitize_pane_id(&op.pane_id);
        match op.action {
            ThemePlanAction::Upsert => {
                let payload = render_operation_contents(&op)?;
                if store.get(&op.pane_id).is_none() {
                    pane_order.push(op.pane_id.clone());
                }
                let plan = store.upsert(&op.pane_id, payload);
                results.push(ThemePlanStepOutput {
                    action: ThemePlanAction::Upsert,
                    pane_id: op.pane_id,
                    sanitized_pane_id: sanitized,
                    filename: Some(plan.filename),
                    contents: Some(plan.contents),
                    changed: Some(plan.changed),
                    obsolete_filename: plan.obsolete_filename,
                    removed: false,
                });
            }
            ThemePlanAction::Remove => {
                let removed = store.remove(&op.pane_id);
                results.push(ThemePlanStepOutput {
                    action: ThemePlanAction::Remove,
                    pane_id: op.pane_id,
                    sanitized_pane_id: sanitized,
                    filename: removed.as_ref().map(|entry| entry.filename.clone()),
                    contents: None,
                    changed: None,
                    obsolete_filename: None,
                    removed: removed.is_some(),
                });
            }
        }
    }

    let last = results.last();
    let entries = pane_order
        .into_iter()
        .filter_map(|pane_id| store.get(&pane_id).cloned())
        .collect();

    let response = ThemePlanResponse {
        pane_count: store.pane_count(),
        sanitized_pane_id: last.map(|step| step.sanitized_pane_id.clone()),
        filename: last.and_then(|step| step.filename.clone()),
        contents: last.and_then(|step| step.contents.clone()),
        changed: last.and_then(|step| step.changed),
        obsolete_filename: last.and_then(|step| step.obsolete_filename.clone()),
        results,
        entries,
    };

    serde_json::to_string(&response).map_err(|err| MobileFfiError::InvalidJson(err.to_string()))
}

fn render_operation_contents(op: &ThemePlanOperation) -> Result<String, MobileFfiError> {
    if let Some(raw_contents) = op.contents.as_ref() {
        return Ok(raw_contents.clone());
    }
    let font_size = op.font_size.unwrap_or(14.0);
    if !font_size.is_finite() || font_size <= 0.0 {
        return Err(MobileFfiError::InvalidInput(
            "fontSize must be finite and positive".to_owned(),
        ));
    }
    let theme_config = op.theme_config.as_deref().unwrap_or("");
    Ok(render_ghostty_theme_config(
        theme_config,
        font_size,
        op.appearance.as_deref(),
    ))
}
