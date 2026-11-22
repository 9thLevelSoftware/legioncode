//! VCS status/branch and resource telemetry streaming RPCs.

use std::process::Stdio;

use legion_orchestrator::time::now_iso_utc;
use legion_process_ffi::resource_monitor::{
    MonitorEventV3, MonitorRequestV3, RESOURCE_MONITOR_PROTOCOL_VERSION, ResourceMonitorEngine,
};
use legion_protocol::rpc_codec::RpcExit;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;

use super::stream_ctx::StreamContext;
use crate::serve::ws_mcp::rpc_fail;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VcsCwdInput {
    cwd: String,
}

/// Streams `subscribeVcsStatus` initial snapshot and keeps subscription open.
pub(super) async fn stream_vcs_status(ctx: &StreamContext, payload: Value) -> RpcExit {
    let req: VcsCwdInput = match serde_json::from_value(payload) {
        Ok(parsed) => parsed,
        Err(err) => return rpc_fail("GitManagerError", &err.to_string()),
    };
    let (is_repo, ref_name, files) = inspect_git_worktree(&req.cwd).await;
    let event = json!({
        "_tag": "snapshot",
        "local": {
            "isRepo": is_repo,
            "hasPrimaryRemote": false,
            "isDefaultRef": false,
            "refName": ref_name,
            "hasWorkingTreeChanges": !files.is_empty(),
            "workingTree": {
                "files": files,
                "insertions": 0,
                "deletions": 0,
            },
        },
        "remote": null,
    });
    if ctx.send_chunk(event).await {
        std::future::pending::<()>().await;
    }
    RpcExit::Success { value: Value::Null }
}

/// Streams `subscribeVcsBranch` initial branch state and keeps subscription open.
pub(super) async fn stream_vcs_branch(ctx: &StreamContext, payload: Value) -> RpcExit {
    let req: VcsCwdInput = match serde_json::from_value(payload) {
        Ok(parsed) => parsed,
        Err(err) => return rpc_fail("GitManagerError", &err.to_string()),
    };
    let (is_repo, ref_name, _) = inspect_git_worktree(&req.cwd).await;
    let val = json!({
        "refName": ref_name,
        "isRepo": is_repo,
    });
    if ctx.send_chunk(val).await {
        std::future::pending::<()>().await;
    }
    RpcExit::Success { value: Value::Null }
}

async fn inspect_git_worktree(cwd: &str) -> (bool, Option<String>, Vec<Value>) {
    let out = Command::new("git")
        .args(["-C", cwd, "status", "--porcelain=v1", "--branch"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    let Ok(output) = out else {
        return (false, None, Vec::new());
    };
    if !output.status.success() {
        return (false, None, Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut branch: Option<String> = None;
    let mut files = Vec::new();
    for (idx, line) in stdout.lines().enumerate() {
        if idx == 0
            && let Some(rest) = line.strip_prefix("## ")
        {
            let head = rest.split("...").next().unwrap_or(rest).trim();
            if !head.is_empty() && !head.starts_with("HEAD (") && !head.starts_with("No commits") {
                branch = Some(head.to_owned());
            }
            continue;
        }
        if line.len() > 3 {
            let path = line[3..].trim();
            if !path.is_empty() {
                files.push(json!({
                    "path": path,
                    "insertions": 0,
                    "deletions": 0,
                }));
            }
        }
    }
    (true, branch, files)
}

/// Streams `subscribeResourceTelemetry` snapshot using `ResourceMonitorEngine::os_default()`.
pub(super) async fn stream_resource_telemetry(ctx: &StreamContext) -> RpcExit {
    let now = now_iso_utc();
    let mut engine = ResourceMonitorEngine::os_default();
    let _ = engine.handle_request(MonitorRequestV3::Configure {
        version: RESOURCE_MONITOR_PROTOCOL_VERSION,
        root_pid: std::process::id(),
        sample_interval_ms: 2_000,
        external_processes: Vec::new(),
    });
    let events = engine.handle_request(MonitorRequestV3::SampleNow {
        version: RESOURCE_MONITOR_PROTOCOL_VERSION,
        request_id: "ws-stream-init".to_owned(),
    });

    let (count, cpu, rss) = match events.into_iter().next() {
        Some(MonitorEventV3::Snapshot(sample)) => {
            let total_rss: u64 = sample.processes.iter().map(|p| p.resident_bytes).sum();
            let total_cpu: f64 = sample.processes.iter().map(|p| p.cpu_percent).sum();
            (sample.processes.len(), total_cpu, total_rss)
        }
        _ => (0, 0.0, 0),
    };

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
    let backend_agg = json!({
        "processCount": count,
        "currentCpuPercent": cpu,
        "cpuTimeMs": 0,
        "currentRssBytes": rss,
        "peakRssBytes": rss,
        "ioReadBytes": 0,
        "ioWriteBytes": 0,
        "ioReadBytesPerSecond": 0.0,
        "ioWriteBytesPerSecond": 0.0,
        "processStarts": 0,
        "processExits": 0,
    });

    let snap = json!({
        "readAt": now,
        "sampleIntervalMs": 2_000,
        "processes": [],
        "groups": {
            "backend": backend_agg,
            "electron": empty_agg,
            "monitor": empty_agg,
            "allLegion": backend_agg,
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
        "attribution": {
            "readAt": now,
            "entries": [],
        },
        "health": {
            "native": {
                "status": "healthy",
                "lastSampleAt": { "_tag": "Some", "value": now },
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
            "scannedProcessCount": count,
            "retainedProcessCount": count,
            "inaccessibleProcessCount": 0,
        },
    });
    if ctx.send_chunk(snap).await {
        std::future::pending::<()>().await;
    }
    RpcExit::Success { value: Value::Null }
}
