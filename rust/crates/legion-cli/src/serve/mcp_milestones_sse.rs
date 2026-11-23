//! SSE event stream formatting and auth session revocation watchdog for `/mcp` milestones.

use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures_util::Stream;
use legion_protocol::milestones::{McpEventStreamItem, MilestoneFollowStream};
use serde_json::{Value, json};

use super::AppState;

pub(super) fn build_sse_stream(
    state: Arc<AppState>,
    session_id: Option<String>,
    sub_id: Value,
    follow: MilestoneFollowStream,
    heartbeat_ms: u64,
) -> Pin<Box<dyn Stream<Item = Result<String, Infallible>> + Send>> {
    Box::pin(async_stream_sse(
        state,
        session_id,
        sub_id,
        follow,
        heartbeat_ms,
    ))
}

fn async_stream_sse(
    state: Arc<AppState>,
    session_id: Option<String>,
    sub_id: Value,
    mut moved: MilestoneFollowStream,
    heartbeat_ms: u64,
) -> impl Stream<Item = Result<String, Infallible>> + Send {
    let initial = json!({
        "jsonrpc": "2.0",
        "method": "notifications/events/active",
        "params": {
            "cursor": moved.cursor.clone(),
            "truncated": moved.truncated,
            "heartbeatMs": heartbeat_ms,
            "_meta": { "io.modelcontextprotocol/subscriptionId": sub_id.clone() },
        },
    });
    let mut last_cursor = moved.cursor.clone();
    let hb_dur = Duration::from_millis(heartbeat_ms);
    let session_poll_dur = Duration::from_millis(250);
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<String, Infallible>>(32);
    tokio::spawn(async move {
        if tx.send(Ok(format!("data: {initial}\n\n"))).await.is_err() {
            return;
        }
        let mut last_msg = tokio::time::Instant::now();
        loop {
            if let Some(ref sid) = session_id
                && is_session_revoked(&state, sid)
            {
                break;
            }
            let wait_for = hb_dur
                .saturating_sub(last_msg.elapsed())
                .min(session_poll_dur);
            match tokio::time::timeout(wait_for, moved.next()).await {
                Ok(Some(item)) => {
                    let converted = McpEventStreamItem::from_follow_item(item);
                    let frame = match converted {
                        McpEventStreamItem::Event {
                            cursor,
                            event_id,
                            timestamp,
                            data,
                        } => {
                            last_cursor.clone_from(&cursor);
                            let name = data.event_type();
                            json!({
                                "jsonrpc": "2.0",
                                "method": "notifications/events/event",
                                "params": {
                                    "cursor": cursor,
                                    "eventId": event_id,
                                    "timestamp": timestamp,
                                    "name": name,
                                    "data": data,
                                    "_meta": { "io.modelcontextprotocol/subscriptionId": sub_id },
                                },
                            })
                        }
                        McpEventStreamItem::Position { cursor } => {
                            last_cursor.clone_from(&cursor);
                            json!({
                                "jsonrpc": "2.0",
                                "method": "notifications/events/heartbeat",
                                "params": {
                                    "cursor": cursor,
                                    "_meta": { "io.modelcontextprotocol/subscriptionId": sub_id },
                                },
                            })
                        }
                        McpEventStreamItem::Gap { .. } => break,
                    };
                    last_msg = tokio::time::Instant::now();
                    if tx.send(Ok(format!("data: {frame}\n\n"))).await.is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => {
                    if last_msg.elapsed() >= hb_dur {
                        let hb = json!({
                            "jsonrpc": "2.0",
                            "method": "notifications/events/heartbeat",
                            "params": {
                                "cursor": last_cursor,
                                "_meta": { "io.modelcontextprotocol/subscriptionId": sub_id },
                            },
                        });
                        last_msg = tokio::time::Instant::now();
                        if tx.send(Ok(format!("data: {hb}\n\n"))).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    });
    tokio_stream_from_mpsc(rx)
}

fn tokio_stream_from_mpsc(
    mut rx: tokio::sync::mpsc::Receiver<Result<String, Infallible>>,
) -> impl Stream<Item = Result<String, Infallible>> + Send {
    futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx))
}

fn is_session_revoked(state: &AppState, session_id: &str) -> bool {
    let Ok(conn) = state.orchestrator.sink().db().lock() else {
        return false;
    };
    let revoked: Option<Option<String>> = conn
        .query_row(
            "SELECT revoked_at FROM auth_sessions WHERE session_id = ?1 LIMIT 1",
            [session_id],
            |r| r.get(0),
        )
        .ok();
    matches!(revoked, Some(Some(_)))
}
