//! Wave H WS RPC handlers (`terminal.snapshot`, `resources.sample`,
//! `surfaces.decorateDiff`, `surfaces.mobileChrome`) wired into `legioncode serve /ws`.

use legion_client_core::diff_markdown::Utf16Span;
use legion_process_ffi::resource_monitor::{
    MonitorEventV3, MonitorRequestV3, RESOURCE_MONITOR_PROTOCOL_VERSION, ResourceMonitorEngine,
};
use legion_protocol::rpc_codec::RpcExit;
use legion_surfaces::mobile_diff::{
    AttributedDiffRun, SyntaxTokenInput, build_syntax_runs_from_tokens, decorate_diff_runs,
    decorate_diff_runs_with_background,
};
use legion_surfaces::mobile_i18n::{
    AgentActivityInput, AgentLiveActivityPresenter, MobileNavigationPresenter,
};
use legion_terminal_vt::palette::parse_color;
use legion_terminal_vt::{TerminalGrid, encode_lcvt, sanitize_bracketed_paste};
use serde::Deserialize;
use serde_json::{Value, json};

use super::ws_mcp::rpc_fail;

/// Dispatches Wave H WS RPC methods (`terminal.snapshot`, `resources.sample`,
/// `surfaces.decorateDiff`, `surfaces.mobileChrome`).
pub(crate) fn dispatch(tag: &str, payload: Value) -> RpcExit {
    match tag {
        "terminal.snapshot" => handle_terminal_snapshot(payload),
        "resources.sample" => handle_resources_sample(payload),
        "surfaces.decorateDiff" => handle_surfaces_decorate_diff(payload),
        "surfaces.mobileChrome" => handle_surfaces_mobile_chrome(payload),
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PaletteOverrideInput {
    index: u8,
    color: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct TerminalSnapshotPayload {
    cols: Option<u16>,
    rows: Option<u16>,
    feed: Option<String>,
    feed_bytes: Option<Vec<u8>>,
    paste_input: Option<String>,
    palette_overrides: Option<Vec<PaletteOverrideInput>>,
}

pub(crate) fn handle_terminal_snapshot(payload: Value) -> RpcExit {
    let req: TerminalSnapshotPayload = match parse_optional_payload(payload) {
        Ok(parsed) => parsed,
        Err(exit) => return exit,
    };

    let cols = req.cols.unwrap_or(80);
    let rows = req.rows.unwrap_or(24);
    let mut grid = match TerminalGrid::try_new(cols, rows) {
        Ok(g) => g,
        Err(err) => return rpc_fail("TerminalVtError", &err.to_string()),
    };

    if let Some(overrides) = req.palette_overrides {
        let mut pairs = Vec::with_capacity(overrides.len());
        for entry in overrides {
            match parse_color(&entry.color) {
                Ok(rgba) => pairs.push((entry.index, rgba)),
                Err(err) => return rpc_fail("TerminalVtColorError", &err.to_string()),
            }
        }
        grid.palette_mut().apply_theme_overrides(&pairs);
    }

    if let Some(bytes) = req.feed_bytes.as_deref() {
        grid.feed(bytes);
    }
    if let Some(text) = req.feed.as_deref() {
        grid.feed_str(text);
    }

    let snapshot = grid.snapshot();
    let lcvt_bytes = encode_lcvt(&snapshot);
    let sanitized_paste = req.paste_input.as_deref().map(sanitize_bracketed_paste);

    RpcExit::Success {
        value: json!({
            "cols": snapshot.cols,
            "rows": snapshot.rows,
            "cursorX": snapshot.cursor_x,
            "cursorY": snapshot.cursor_y,
            "cursorVisible": snapshot.cursor_visible,
            "alternateScreen": snapshot.alternate_screen,
            "title": grid.title(),
            "snapshot": snapshot,
            "lcvtBytes": lcvt_bytes,
            "lcvtBase64": encode_standard_base64(&lcvt_bytes),
            "sanitizedPaste": sanitized_paste,
        }),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ResourceSamplePayload {
    root_pid: Option<u32>,
    sample_interval_ms: Option<u64>,
    request_id: Option<String>,
    include_process_table: Option<bool>,
}

fn handle_resources_sample(payload: Value) -> RpcExit {
    let req: ResourceSamplePayload = match parse_optional_payload(payload) {
        Ok(parsed) => parsed,
        Err(exit) => return exit,
    };

    let mut engine = ResourceMonitorEngine::os_default();
    let root_pid = req.root_pid.unwrap_or_else(std::process::id);
    let _ = engine.handle_request(MonitorRequestV3::Configure {
        version: RESOURCE_MONITOR_PROTOCOL_VERSION,
        root_pid,
        sample_interval_ms: req.sample_interval_ms.unwrap_or(0),
        external_processes: Vec::new(),
    });

    let request_id = req.request_id.unwrap_or_else(|| "rpc-sample".to_owned());
    let sample_events = engine.handle_request(MonitorRequestV3::SampleNow {
        version: RESOURCE_MONITOR_PROTOCOL_VERSION,
        request_id: request_id.clone(),
    });
    let sample = match sample_events.into_iter().next() {
        Some(MonitorEventV3::Snapshot(sample)) => sample,
        Some(MonitorEventV3::Error(err)) => {
            return rpc_fail(
                "ResourceMonitorError",
                &format!("{}: {}", err.code, err.message),
            );
        }
        other => {
            return rpc_fail(
                "ResourceMonitorError",
                &format!("Unexpected resource monitor response: {other:?}"),
            );
        }
    };

    let process_table = if req.include_process_table.unwrap_or(false) {
        let pt_events = engine.handle_request(MonitorRequestV3::ProcessTable {
            version: RESOURCE_MONITOR_PROTOCOL_VERSION,
            request_id,
        });
        match pt_events.into_iter().next() {
            Some(MonitorEventV3::ProcessTable(pt)) => Some(pt.processes),
            _ => None,
        }
    } else {
        None
    };

    RpcExit::Success {
        value: json!({
            "protocolVersion": RESOURCE_MONITOR_PROTOCOL_VERSION,
            "sample": sample,
            "processTable": process_table,
        }),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct DecorateDiffPayload {
    tokens: Option<Vec<SyntaxTokenInput>>,
    runs: Option<Vec<AttributedDiffRun>>,
    #[serde(alias = "highlights")]
    word_highlights: Vec<Utf16Span>,
    highlight_background: Option<String>,
}

fn handle_surfaces_decorate_diff(payload: Value) -> RpcExit {
    let req: DecorateDiffPayload = match parse_optional_payload(payload) {
        Ok(parsed) => parsed,
        Err(exit) => return exit,
    };

    let base_runs = if let Some(runs) = req.runs {
        runs
    } else if let Some(tokens) = req.tokens {
        build_syntax_runs_from_tokens(&tokens)
    } else {
        return rpc_fail(
            "InvalidDecorateDiffPayload",
            "Pass `runs` or `tokens` to `surfaces.decorateDiff`.",
        );
    };

    let decorated = match req.highlight_background.as_deref() {
        Some(bg) => decorate_diff_runs_with_background(&base_runs, &req.word_highlights, bg),
        None => decorate_diff_runs(&base_runs, &req.word_highlights),
    };

    RpcExit::Success {
        value: json!({ "runs": decorated }),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct MobileChromePayload {
    language: Option<String>,
    system_locales: Vec<String>,
    activity: Option<AgentActivityInput>,
}

fn handle_surfaces_mobile_chrome(payload: Value) -> RpcExit {
    let req: MobileChromePayload = match parse_optional_payload(payload) {
        Ok(parsed) => parsed,
        Err(exit) => return exit,
    };

    let language = req.language.as_deref().unwrap_or("system");
    let sys_refs: Vec<&str> = req.system_locales.iter().map(String::as_str).collect();
    let nav = MobileNavigationPresenter::from_settings(language, &sys_refs);
    let live_activity = req.activity.as_ref().map(|activity| {
        let presenter = AgentLiveActivityPresenter::from_settings(language, &sys_refs);
        presenter.present(activity)
    });

    RpcExit::Success {
        value: json!({
            "locale": nav.locale(),
            "formattingLocale": nav.formatting_locale(),
            "navigation": nav.strings(),
            "liveActivity": live_activity,
        }),
    }
}

fn parse_optional_payload<T: Default + serde::de::DeserializeOwned>(
    payload: Value,
) -> Result<T, RpcExit> {
    if payload.is_null() {
        Ok(T::default())
    } else {
        serde_json::from_value(payload).map_err(|err| rpc_fail("ParseError", &err.to_string()))
    }
}

pub(crate) fn encode_standard_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = usize::from(chunk[0]);
        let b1 = usize::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = usize::from(chunk.get(2).copied().unwrap_or(0));
        let combined = (b0 << 16) | (b1 << 8) | b2;
        out.push(char::from(TABLE[(combined >> 18) & 0x3f]));
        out.push(char::from(TABLE[(combined >> 12) & 0x3f]));
        out.push(if chunk.len() > 1 {
            char::from(TABLE[(combined >> 6) & 0x3f])
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            char::from(TABLE[combined & 0x3f])
        } else {
            '='
        });
    }
    out
}
