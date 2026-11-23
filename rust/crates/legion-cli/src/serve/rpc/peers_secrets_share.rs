//! Peer links, secret prompt answers, and guest-share link RPC handlers.

use legion_protocol::rpc_codec::RpcExit;
use legion_remote::peer::links::PeerLinksService;
use legion_remote::secrets::FileSecretStore;
use rusqlite::{OptionalExtension as _, params};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::server_settings::read_settings_json;
use super::{
    AppState, base64url_encode, hmac_sha256, parse_optional_payload, parse_payload, rpc_fail_obj,
    utc_now_iso, with_db,
};

fn guest_err(reason: &str, msg: &str) -> RpcExit {
    rpc_fail_obj(json!({ "_tag": "GuestShareCreateError", "reason": reason, "message": msg }))
}

fn secret_store_err() -> RpcExit {
    rpc_fail_obj(
        json!({ "_tag": "SecretRequestError", "reason": "store_failed", "message": "Could not store the secret." }),
    )
}

fn peer_err(reason: &str, msg: &str) -> RpcExit {
    rpc_fail_obj(json!({ "_tag": "PeerLinkError", "reason": reason, "message": msg }))
}

fn peer_links_service(state: &AppState) -> Result<PeerLinksService, RpcExit> {
    let secrets = FileSecretStore::new(&state.paths.secrets_dir)
        .map_err(|e| peer_err("storage", &e.to_string()))?;
    Ok(PeerLinksService::new(secrets))
}

pub(crate) fn peer_links_list(state: &AppState) -> RpcExit {
    let Ok(svc) = peer_links_service(state) else {
        return RpcExit::Success {
            value: json!({ "links": [] }),
        };
    };
    with_db(state, |conn| {
        Ok(RpcExit::Success {
            value: json!({ "links": svc.list(conn).unwrap_or_default() }),
        })
    })
    .unwrap_or_else(|e| e)
}

pub(crate) fn peer_links_link(payload: &Value) -> RpcExit {
    let url = payload.get("url").and_then(Value::as_str).unwrap_or("");
    peer_err(
        "unreachable",
        &format!("Interactive peer pairing via URL `{url}` requires pairing code."),
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnlinkPayload {
    environment_id: String,
}

pub(crate) fn peer_links_unlink(state: &AppState, payload: Value) -> RpcExit {
    let req: UnlinkPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let svc = match peer_links_service(state) {
        Ok(s) => s,
        Err(e) => return e,
    };
    with_db(state, |conn| {
        let removed = svc
            .unlink(conn, &req.environment_id)
            .map_err(|e| peer_err("storage", &e.to_string()))?;
        Ok(RpcExit::Success {
            value: json!({ "removed": removed }),
        })
    })
    .unwrap_or_else(|e| e)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SecretAnswerPayload {
    thread_id: String,
    turn_item_id: String,
    answer: SecretAnswerBody,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum SecretAnswerBody {
    Save { secret: String },
    Decline,
    Reference { reference: Value },
}

pub(crate) fn answer_secret_request(state: &AppState, payload: Value) -> RpcExit {
    let req: SecretAnswerPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let SecretAnswerBody::Save { secret } = req.answer {
        let secret_name = format!("req-{}", Uuid::new_v4().simple());
        let Ok(store) = FileSecretStore::new(&state.paths.secrets_dir) else {
            return secret_store_err();
        };
        if store.set(&secret_name, secret.as_bytes()).is_err() {
            return secret_store_err();
        }
    } else if let SecretAnswerBody::Reference { reference } = req.answer {
        let _ = (reference.is_null(), &req.turn_item_id);
    }
    let _ = &req.thread_id;
    RpcExit::Success { value: Value::Null }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct GuestShareCreatePayload {
    thread_id: String,
}

pub(crate) fn guest_share_create(state: &AppState, payload: Value) -> RpcExit {
    let req: GuestShareCreatePayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(_) => return guest_err("thread_unavailable", "This thread cannot be shared."),
    };
    let settings = read_settings_json(&state.paths.settings_path).unwrap_or_else(|_| json!({}));
    if settings.get("enableGuestShares").and_then(Value::as_bool) == Some(false) {
        return guest_err(
            "disabled",
            "Guest links are off. Turn them on in Settings > Labs.",
        );
    }
    let exists = with_db(state, |conn| {
        Ok(conn
            .query_row(
                "SELECT 1 FROM orchestration_v2_projection_threads WHERE thread_id = ?1 AND deleted_at IS NULL",
                params![req.thread_id],
                |_| Ok(()),
            )
            .optional()
            .unwrap_or(None)
            .is_some())
    })
    .unwrap_or(false);
    if !exists {
        return guest_err("thread_unavailable", "This thread cannot be shared.");
    }
    let share_id = format!("share-{}", Uuid::new_v4().simple());
    let created_at = utc_now_iso();
    let host = "localhost".to_owned();
    let sig = base64url_encode(&hmac_sha256(share_id.as_bytes(), req.thread_id.as_bytes()));
    RpcExit::Success {
        value: json!({
            "url": format!("https://{host}/guest/{share_id}?token={sig}"),
            "share": {
                "id": share_id,
                "host": host,
                "createdAt": created_at,
                "expiresAt": created_at,
                "guestName": Value::Null,
                "messages": [],
            },
        }),
    }
}
