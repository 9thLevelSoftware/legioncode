//! Scheduled task webhook management and MCP Apps RPC handlers.

use std::time::{Duration, SystemTime};

use legion_persistence::ownership::format_iso8601_utc;
use legion_protocol::rpc_codec::RpcExit;
use rusqlite::{Connection, OptionalExtension as _, params};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::tasks_sessions_mcp::{TaskIdPayload, ensure_tasks_schema, task_err};
use super::{AppState, mcp_apps, parse_optional_payload, parse_payload, utc_now_iso, with_db};

pub(crate) fn dispatch(state: &AppState, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "scheduledTasks.rotateWebhookToken" => rotate_webhook_token(state, payload),
        "scheduledTasks.listWebhookDeliveries" => list_webhook_deliveries(state, payload),
        "scheduledTasks.getWebhookDelivery" => get_webhook_delivery(state, payload),
        _ => mcp_apps::dispatch(state, tag, payload),
    }
}

fn rotate_webhook_token(state: &AppState, payload: Value) -> RpcExit {
    let req: TaskIdPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let now = utc_now_iso();
    let new_token = format!("wh_{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    with_db(state, |conn| {
        ensure_tasks_schema(conn)?;
        let changed = conn
            .execute(
                "UPDATE scheduled_tasks SET webhook_token = ?1, updated_at = ?2 WHERE task_id = ?3",
                params![new_token, now, req.task_id],
            )
            .map_err(|e| task_err(&e.to_string()))?;
        if changed == 0 {
            return Err(task_err("Scheduled task not found."));
        }
        let task = read_task_row(conn, &req.task_id)?
            .ok_or_else(|| task_err("Scheduled task not found."))?;
        Ok(RpcExit::Success {
            value: json!({ "task": task }),
        })
    })
    .unwrap_or_else(|e| e)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ListDeliveriesPayload {
    #[serde(alias = "id")]
    task_id: String,
    limit: Option<usize>,
}

fn list_webhook_deliveries(state: &AppState, payload: Value) -> RpcExit {
    let req: ListDeliveriesPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let limit = i64::try_from(req.limit.unwrap_or(50).clamp(1, 200)).unwrap_or(50);
    with_db(state, |conn| {
        ensure_tasks_schema(conn)?;
        let mut stmt = conn
            .prepare(
                "SELECT delivery_id, task_id, received_at, method, body_bytes, outcome, signature_verified, error
                 FROM scheduled_task_webhook_deliveries
                 WHERE task_id = ?1 ORDER BY received_at DESC LIMIT ?2",
            )
            .map_err(|e| task_err(&e.to_string()))?;
        let deliveries: Vec<Value> = stmt
            .query_map(params![req.task_id, limit], |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?,
                    "taskId": r.get::<_, String>(1)?,
                    "receivedAt": r.get::<_, String>(2)?,
                    "method": r.get::<_, String>(3)?,
                    "contentType": Value::Null,
                    "bodyBytes": r.get::<_, i64>(4).unwrap_or(0),
                    "outcome": r.get::<_, String>(5)?,
                    "signatureVerified": r.get::<_, i64>(6)? != 0,
                    "missingFields": [],
                    "error": r.get::<_, Option<String>>(7)?,
                }))
            })
            .and_then(Iterator::collect)
            .unwrap_or_default();
        Ok(RpcExit::Success {
            value: json!({ "deliveries": deliveries }),
        })
    })
    .unwrap_or_else(|e| e)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetDeliveryPayload {
    #[serde(alias = "id")]
    task_id: String,
    delivery_id: String,
}

fn get_webhook_delivery(state: &AppState, payload: Value) -> RpcExit {
    let req: GetDeliveryPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    with_db(state, |conn| {
        ensure_tasks_schema(conn)?;
        let row: Option<Value> = conn
            .query_row(
                "SELECT delivery_id, task_id, received_at, method, query, headers_json, body, body_bytes, body_truncated, outcome, signature_verified, rendered_prompt, error
                 FROM scheduled_task_webhook_deliveries
                 WHERE task_id = ?1 AND delivery_id = ?2",
                params![req.task_id, req.delivery_id],
                |r| {
                    let headers_str: String = r.get(5)?;
                    let headers: Value =
                        serde_json::from_str(&headers_str).unwrap_or_else(|_| json!({}));
                    Ok(json!({
                        "id": r.get::<_, String>(0)?,
                        "taskId": r.get::<_, String>(1)?,
                        "receivedAt": r.get::<_, String>(2)?,
                        "method": r.get::<_, String>(3)?,
                        "contentType": Value::Null,
                        "query": r.get::<_, String>(4)?,
                        "headers": headers,
                        "body": r.get::<_, String>(6)?,
                        "bodyBytes": r.get::<_, i64>(7)?,
                        "bodyTruncated": r.get::<_, i64>(8)? != 0,
                        "outcome": r.get::<_, String>(9)?,
                        "signatureVerified": r.get::<_, i64>(10)? != 0,
                        "missingFields": [],
                        "renderedPrompt": r.get::<_, Option<String>>(11)?,
                        "error": r.get::<_, Option<String>>(12)?,
                    }))
                },
            )
            .optional()
            .map_err(|e| task_err(&e.to_string()))?;
        let Some(delivery) = row else {
            return Err(task_err("Scheduled task webhook delivery not found."));
        };
        Ok(RpcExit::Success {
            value: json!({ "delivery": delivery }),
        })
    })
    .unwrap_or_else(|e| e)
}

pub(crate) fn validate_schedule(schedule: &Value) -> Result<(), String> {
    let kind = schedule
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "Schedule missing `type`".to_owned())?;
    match kind {
        "interval" => {
            let valid_ms = schedule
                .get("everyMs")
                .and_then(Value::as_u64)
                .is_some_and(|v| v > 0)
                || schedule
                    .get("everyMinutes")
                    .or_else(|| schedule.get("intervalMinutes"))
                    .and_then(Value::as_u64)
                    .is_some_and(|v| v > 0);
            if !valid_ms {
                return Err(
                    "Interval schedule requires positive `everyMs` or `everyMinutes`.".to_owned(),
                );
            }
            Ok(())
        }
        "fixed_time" => {
            let time = schedule
                .get("timeOfDay")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !is_valid_hh_mm(time) {
                return Err("Fixed-time schedule requires `timeOfDay` in HH:MM format.".to_owned());
            }
            Ok(())
        }
        "once" => {
            let run_at = schedule
                .get("runAt")
                .and_then(Value::as_str)
                .map_or("", str::trim);
            if run_at.is_empty() {
                return Err("Once schedule requires `runAt` ISO datetime.".to_owned());
            }
            Ok(())
        }
        "webhook" => Ok(()),
        "cron" => {
            let expr = schedule
                .get("expression")
                .and_then(Value::as_str)
                .unwrap_or("");
            if expr.split_whitespace().count() != 5 {
                return Err("Cron schedule requires a 5-field expression.".to_owned());
            }
            Ok(())
        }
        other => Err(format!("Unsupported schedule type `{other}`")),
    }
}

fn is_valid_hh_mm(raw: &str) -> bool {
    let Some((h_str, m_str)) = raw.split_once(':') else {
        return false;
    };
    let Ok(h) = h_str.parse::<u8>() else {
        return false;
    };
    let Ok(m) = m_str.parse::<u8>() else {
        return false;
    };
    m_str.len() == 2 && h < 24 && m < 60
}

pub(crate) fn compute_next_run_at(schedule: &Value) -> Option<String> {
    let kind = schedule.get("type").and_then(Value::as_str)?;
    match kind {
        "interval" => {
            let ms = schedule
                .get("everyMs")
                .and_then(Value::as_u64)
                .or_else(|| {
                    schedule
                        .get("everyMinutes")
                        .or_else(|| schedule.get("intervalMinutes"))
                        .and_then(Value::as_u64)
                        .map(|mins| mins.saturating_mul(60_000))
                })?;
            Some(format_iso8601_utc(
                SystemTime::now() + Duration::from_millis(ms),
            ))
        }
        "fixed_time" | "cron" => Some(format_iso8601_utc(
            SystemTime::now() + Duration::from_secs(60),
        )),
        "once" => schedule
            .get("runAt")
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => None,
    }
}

pub(crate) fn read_task_row(conn: &Connection, task_id: &str) -> Result<Option<Value>, RpcExit> {
    conn.query_row(
        "SELECT task_id, title, prompt, enabled, schedule_json, project_id, thread_id,
                workspace_strategy_json, model_selection_json, runtime_mode, interaction_mode,
                created_by, creation_source, created_at, updated_at, next_run_at, last_run_at,
                last_run_status, last_run_error, run_count, webhook_token, thread_mode
         FROM scheduled_tasks WHERE task_id = ?1",
        params![task_id],
        |r| {
            let sched_raw: String = r.get(4)?;
            let ws_raw: String = r.get(7)?;
            let ms_raw: String = r.get(8)?;
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "title": r.get::<_, String>(1)?,
                "prompt": r.get::<_, String>(2)?,
                "enabled": r.get::<_, i64>(3)? != 0,
                "schedule": serde_json::from_str::<Value>(&sched_raw).unwrap_or_else(|_| json!({})),
                "projectId": r.get::<_, String>(5)?,
                "threadId": r.get::<_, Option<String>>(6)?,
                "threadMode": r.get::<_, String>(21)?,
                "workspaceStrategy": serde_json::from_str::<Value>(&ws_raw).unwrap_or_else(|_| json!({ "type": "root", "branch": null })),
                "modelSelection": serde_json::from_str::<Value>(&ms_raw).unwrap_or_else(|_| json!({})),
                "runtimeMode": r.get::<_, String>(9)?,
                "interactionMode": r.get::<_, String>(10)?,
                "createdBy": r.get::<_, String>(11)?,
                "creationSource": r.get::<_, String>(12)?,
                "createdAt": r.get::<_, String>(13)?,
                "updatedAt": r.get::<_, String>(14)?,
                "nextRunAt": r.get::<_, Option<String>>(15)?,
                "lastRunAt": r.get::<_, Option<String>>(16)?,
                "lastRunStatus": r.get::<_, String>(17)?,
                "lastRunError": r.get::<_, Option<String>>(18)?,
                "runCount": r.get::<_, i64>(19)?,
            }))
        },
    )
    .optional()
    .map_err(|e| task_err(&e.to_string()))
}
