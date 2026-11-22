//! OTLP probe, signal, client activity, and npm release check handlers for `server.*` RPCs.

use legion_protocol::rpc_codec::RpcExit;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{parse_optional_payload, parse_payload, rpc_fail, server_telemetry, utc_now_iso};

pub(super) async fn dispatch(tag: &str, payload: Value) -> RpcExit {
    match tag {
        "server.checkOtlpEndpoint" => check_otlp_endpoint(payload).await,
        "server.signalProcess" => signal_process(payload),
        "server.reportClientActivity" => report_client_activity(payload),
        "server.getNpmRelease" => get_npm_release(payload).await,
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckOtlpPayload {
    endpoint: String,
}

async fn check_otlp_endpoint(payload: Value) -> RpcExit {
    let req: CheckOtlpPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let trimmed = req.endpoint.trim();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return RpcExit::Success {
            value: json!({
                "ok": false,
                "status": "invalid",
                "message": "OTLP endpoint must use http:// or https://",
            }),
        };
    }
    let Ok(http) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    else {
        return RpcExit::Success {
            value: json!({ "ok": false, "message": "HTTP client build failed." }),
        };
    };
    match http.post(trimmed).json(&json!({})).send().await {
        Ok(resp) => RpcExit::Success {
            value: json!({
                "ok": resp.status().is_success(),
                "statusCode": resp.status().as_u16(),
            }),
        },
        Err(err) => RpcExit::Success {
            value: json!({ "ok": false, "message": err.to_string() }),
        },
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignalProcessPayload {
    pid: u32,
    signal: String,
}

fn signal_process(payload: Value) -> RpcExit {
    let req: SignalProcessPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let msg = if req.pid == std::process::id() {
        "Refusing to signal the LegionCode server process."
    } else {
        "Process is not an active managed child process."
    };
    RpcExit::Success {
        value: json!({
            "pid": req.pid,
            "signal": req.signal,
            "signaled": false,
            "message": msg,
        }),
    }
}

fn report_client_activity(payload: Value) -> RpcExit {
    let _: Value = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let now = utc_now_iso();
    RpcExit::Success {
        value: json!({
            "hostPower": server_telemetry::empty_telemetry_snapshot(&now)["power"],
            "leases": [],
        }),
    }
}

async fn get_npm_release(payload: Value) -> RpcExit {
    let _: Value = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    else {
        return RpcExit::Success { value: Value::Null };
    };
    let Ok(resp) = client
        .get("https://registry.npmjs.org/legioncode/latest")
        .send()
        .await
    else {
        return RpcExit::Success { value: Value::Null };
    };
    if !resp.status().is_success() {
        return RpcExit::Success { value: Value::Null };
    }
    let Ok(pkg) = resp.json::<Value>().await else {
        return RpcExit::Success { value: Value::Null };
    };
    let Some(version) = pkg.get("version").and_then(Value::as_str) else {
        return RpcExit::Success { value: Value::Null };
    };
    RpcExit::Success {
        value: json!({
            "version": version,
            "publishedAt": pkg.get("time").cloned().unwrap_or(Value::Null),
        }),
    }
}
