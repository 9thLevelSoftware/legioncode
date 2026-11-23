//! Diagnostics, host resource telemetry, OTLP checks, client activity, and server update RPCs.

use legion_process_ffi::resource_monitor::{
    MonitorEventV3, MonitorRequestV3, RESOURCE_MONITOR_PROTOCOL_VERSION, ResourceMonitorEngine,
};
use legion_protocol::rpc_codec::RpcExit;
use legion_remote::trace::{DEFAULT_TRACE_MAX_FILES, summarize_trace_files};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{AppState, parse_optional_payload, rpc_fail, rpc_fail_obj, utc_now_iso};

pub(crate) fn dispatch(state: &AppState, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "server.getTraceDiagnostics" => get_trace_diagnostics(state, payload),
        "server.getProcessDiagnostics" => get_process_diagnostics(),
        "server.getHostResources" => get_host_resources(),
        "server.getProcessResourceHistory" => get_process_resource_history(payload),
        "server.getResourceTelemetryHistory" => get_resource_telemetry_history(payload),
        "server.retryResourceTelemetry" => retry_resource_telemetry(),
        "server.updateServer" | "server.commitDesktopUpdate" => rpc_fail_obj(json!({
            "_tag": "ServerSelfUpdateError",
            "reason": "Self-update is not supported on this standalone Rust build.",
        })),
        "server.updateServerWithProgress" => RpcExit::Success {
            value: json!({
                "type": "complete",
                "result": {
                    "targetVersion": env!("CARGO_PKG_VERSION"),
                    "method": "standalone",
                },
            }),
        },
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct TraceDiagPayload {
    max_files: Option<usize>,
}

fn get_trace_diagnostics(state: &AppState, payload: Value) -> RpcExit {
    let req: TraceDiagPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let trace_path = &state.paths.server_trace_path;
    let max_files = req.max_files.unwrap_or(DEFAULT_TRACE_MAX_FILES);
    let summary = summarize_trace_files(trace_path, max_files, None).ok();
    let count = summary
        .as_ref()
        .map_or(0, |s| s.span_count + s.skipped_line_count);
    let parse_err = summary.as_ref().map_or(0, |s| s.skipped_line_count);
    let path_str = trace_path.to_string_lossy().into_owned();
    RpcExit::Success {
        value: json!({
            "traceFilePath": path_str,
            "scannedFilePaths": [],
            "readAt": utc_now_iso(),
            "recordCount": count,
            "parseErrorCount": parse_err,
            "firstSpanAt": { "_tag": "None" },
            "lastSpanAt": { "_tag": "None" },
            "failureCount": 0,
            "interruptionCount": 0,
            "slowSpanThresholdMs": 1000,
            "slowSpanCount": 0,
            "logLevelCounts": {},
            "topSpansByCount": [],
            "slowestSpans": [],
            "commonFailures": [],
            "latestFailures": [],
            "latestWarningAndErrorLogs": [],
            "partialFailure": { "_tag": "None" },
            "error": { "_tag": "None" },
        }),
    }
}

fn sample_process_monitor() -> Value {
    let mut engine = ResourceMonitorEngine::os_default();
    let _ = engine.handle_request(MonitorRequestV3::Configure {
        version: RESOURCE_MONITOR_PROTOCOL_VERSION,
        root_pid: std::process::id(),
        sample_interval_ms: 0,
        external_processes: Vec::new(),
    });
    let sample_ev = engine
        .handle_request(MonitorRequestV3::SampleNow {
            version: RESOURCE_MONITOR_PROTOCOL_VERSION,
            request_id: "rpc-diag".to_owned(),
        })
        .into_iter()
        .next();
    match sample_ev {
        Some(MonitorEventV3::Snapshot(s)) => json!(s),
        _ => json!({}),
    }
}

fn get_process_diagnostics() -> RpcExit {
    let sample = sample_process_monitor();
    let rss = sample
        .get("residentBytes")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cpu = sample
        .get("cpuPercent")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    RpcExit::Success {
        value: json!({
            "serverPid": std::process::id(),
            "readAt": utc_now_iso(),
            "processCount": 1,
            "totalRssBytes": rss,
            "totalCpuPercent": cpu,
            "processes": [],
            "error": { "_tag": "None" },
        }),
    }
}

fn get_host_resources() -> RpcExit {
    let sampled_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let cpu_count = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    RpcExit::Success {
        value: json!({
            "sampledAt": u64::try_from(sampled_at).unwrap_or(u64::MAX),
            "cpuUtilization": Value::Null,
            "cpuCount": cpu_count,
            "availableMemoryBytes": 0,
            "totalMemoryBytes": 0,
        }),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ProcessHistoryPayload {
    window_ms: Option<u64>,
    bucket_ms: Option<u64>,
}

fn get_process_resource_history(payload: Value) -> RpcExit {
    let req: ProcessHistoryPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    RpcExit::Success {
        value: json!({
            "readAt": utc_now_iso(),
            "windowMs": req.window_ms.unwrap_or(300_000),
            "bucketMs": req.bucket_ms.unwrap_or(5_000),
            "sampleIntervalMs": 1000,
            "retainedSampleCount": 0,
            "totalCpuSecondsApprox": 0.0,
            "buckets": [],
            "topProcesses": [],
            "error": { "_tag": "None" },
        }),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct TelemetryHistoryPayload {
    window_ms: Option<u64>,
    bucket_ms: Option<u64>,
}

pub(crate) fn empty_telemetry_snapshot(now: &str) -> Value {
    let empty_agg = json!({
        "processCount": 0,
        "currentCpuPercent": 0.0,
        "cpuTimeMs": 0,
        "currentRssBytes": 0,
        "peakRssBytes": 0,
        "ioReadBytes": 0,
        "ioWriteBytes": 0,
        "ioReadBytesPerSecond": 0.0,
        "ioWriteBytesPerSecond": 0.0,
        "processStarts": 0,
        "processExits": 0,
    });
    json!({
        "readAt": now,
        "sampleIntervalMs": 1000,
        "processes": [],
        "groups": {
            "backend": empty_agg.clone(),
            "electron": empty_agg.clone(),
            "monitor": empty_agg.clone(),
            "allLegion": empty_agg,
        },
        "power": {
            "source": "unknown",
            "idle": "unknown",
            "idleSeconds": null,
            "locked": "unknown",
            "suspended": false,
            "onBattery": "unknown",
            "lowPowerMode": "unknown",
            "thermalState": "unknown",
            "stale": false,
            "updatedAt": now,
        },
        "speedLimitPercent": { "_tag": "None" },
        "attribution": { "readAt": now, "entries": [] },
        "health": {
            "native": {
                "status": "healthy",
                "lastSampleAt": { "_tag": "None" },
                "lastError": { "_tag": "None" },
            },
            "desktop": {
                "status": "unavailable",
                "lastSampleAt": { "_tag": "None" },
                "lastError": { "_tag": "None" },
            },
            "sidecarVersion": { "_tag": "None" },
            "sidecarPid": { "_tag": "None" },
            "restartCount": 0,
            "collectionDurationMicros": 0,
            "scannedProcessCount": 0,
            "retainedProcessCount": 0,
            "inaccessibleProcessCount": 0,
        },
    })
}

fn get_resource_telemetry_history(payload: Value) -> RpcExit {
    let req: TelemetryHistoryPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let now = utc_now_iso();
    RpcExit::Success {
        value: json!({
            "readAt": now,
            "windowMs": req.window_ms.unwrap_or(300_000),
            "bucketMs": req.bucket_ms.unwrap_or(5_000),
            "sampleIntervalMs": 1000,
            "retainedSampleCount": 0,
            "buckets": [],
            "topProcesses": [],
            "health": empty_telemetry_snapshot(&now)["health"],
        }),
    }
}

fn retry_resource_telemetry() -> RpcExit {
    let now = utc_now_iso();
    RpcExit::Success {
        value: json!({
            "accepted": true,
            "snapshot": empty_telemetry_snapshot(&now),
        }),
    }
}
