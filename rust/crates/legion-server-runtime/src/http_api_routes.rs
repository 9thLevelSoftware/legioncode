//! HTTP route table for `runtime_api_router` (`legion-server-runtime`).

use std::sync::Arc;

use axum::Router;
use axum::routing::{any, delete, get, post};

use crate::http_api::RuntimeHttpState;
use crate::http_auth::handle_pair;
use crate::http_auth_admin::{
    handle_create_pairing, handle_issue_session, handle_list_connect_devices, handle_list_pairing,
    handle_list_sessions, handle_revoke_pairing, handle_revoke_session,
};
use crate::http_env::assets::{
    handle_api_not_found, handle_signed_asset, handle_signed_attachment_upload,
};
use crate::http_env::auth::{
    handle_get_session, handle_post_browser_session, handle_ws_ticket, handle_ws_ticket_for,
};
use crate::http_env::auth_manage::{
    handle_post_approve_connect_device, handle_post_pairing_token, handle_post_revoke_client,
    handle_post_revoke_connect_device, handle_post_revoke_other_clients,
    handle_post_revoke_pairing_link,
};
use crate::http_env::connect::{
    handle_connect_link_proof, handle_connect_link_state, handle_connect_live_activity_preference,
    handle_connect_preferences, handle_connect_reachability, handle_connect_relay_config,
    handle_connect_unlink,
};
use crate::http_env::connect_relay::{
    handle_legion_connect_health, handle_legion_connect_mint_credential,
};
use crate::http_env::cors::browser_api_cors_middleware;
use crate::http_env::mcp_oauth::{
    handle_mcp_approval, handle_mcp_authorization_server, handle_mcp_authorize,
    handle_mcp_protected_resource, handle_mcp_register,
};
use crate::http_env::mcp_oauth_tokens::{handle_mcp_decision, handle_mcp_token};
use crate::http_env::oauth_token::handle_oauth_token_exchange;
use crate::http_env::orchestration::{
    handle_shell_snapshot, handle_thread_bounded, handle_thread_detail, handle_thread_history,
    handle_thread_transcript,
};
use crate::http_env::pull_requests::handle_pull_request_diff;
use crate::http_env::voice::{
    handle_voice_cloud_keys, handle_voice_cloud_openrouter_models, handle_voice_cloud_transcribe,
    handle_voice_post_process, handle_voice_set_cloud_key, handle_voice_status,
    handle_voice_transcribe,
};
use crate::http_env::voice_models::{
    handle_voice_acceleration, handle_voice_models_cancel, handle_voice_models_download,
    handle_voice_models_link, handle_voice_models_list, handle_voice_models_prepare,
    handle_voice_models_remove, handle_voice_models_select, handle_voice_unload_timeout,
};
use crate::http_env::webhooks::handle_webhook_delivery;
use crate::http_integrations::{
    handle_link_peer, handle_list_peers, handle_preview_action, handle_provider_probe,
    handle_unlink_peer,
};
use crate::http_integrations_scm::{
    handle_get_scm_diff, handle_get_scm_pr, handle_post_scm_diff, handle_post_scm_pr,
};
use crate::http_mcp_signin::{
    handle_mcp_sign_in_complete, handle_mcp_sign_in_notice, handle_mcp_sign_in_start,
};
use crate::http_peer_pairing::{handle_issue_client_pairing, handle_pair_link_through_peer};
use crate::http_peer_relay::{
    handle_link_thread_history, handle_link_thread_item, handle_link_thread_stream,
    handle_list_peer_threads, handle_peer_thread_stream,
};
use crate::http_preview_tunnel::handle_preview_tunnel;
use crate::http_projects::{handle_get_projects, handle_mutate_project};
use crate::http_projects_read::{handle_read_project_by_id, handle_read_project_query};
use crate::http_traces::{handle_otlp_traces, redacted_access_log_middleware};
use crate::http_workspace_folders::{
    handle_add_task_folder_repository, handle_get_task_folder_get, handle_get_task_folder_post,
    handle_list_workspace_folders_get, handle_list_workspace_folders_post,
    handle_scan_workspace_folders,
};
use crate::static_assets::handle_workspace_asset;

pub(crate) fn build_runtime_api_router(state: Arc<RuntimeHttpState>) -> Router {
    let router = mount_auth_and_mcp_routes(Router::new());
    let router = mount_orchestration_project_scm_routes(router);
    let router = mount_voice_and_connect_routes(router);
    let router = mount_peer_and_asset_routes(router);
    router
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            redacted_access_log_middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            browser_api_cors_middleware,
        ))
        .with_state(state)
}

fn mount_auth_and_mcp_routes(
    router: Router<Arc<RuntimeHttpState>>,
) -> Router<Arc<RuntimeHttpState>> {
    router
        .route("/api/auth/pair", post(handle_pair))
        .route("/oauth/token", post(handle_oauth_token_exchange))
        .route("/api/auth/session", get(handle_get_session))
        .route(
            "/api/auth/browser-session",
            post(handle_post_browser_session),
        )
        .route("/api/auth/websocket-ticket", post(handle_ws_ticket))
        .route(
            "/api/auth/websocket-ticket/{use}",
            post(handle_ws_ticket_for),
        )
        .route("/api/auth/pairing-token", post(handle_post_pairing_token))
        .route(
            "/api/auth/pairing-links/revoke",
            post(handle_post_revoke_pairing_link),
        )
        .route("/api/auth/clients/revoke", post(handle_post_revoke_client))
        .route(
            "/api/auth/clients/revoke-others",
            post(handle_post_revoke_other_clients),
        )
        .route(
            "/api/auth/pairing",
            get(handle_list_pairing).post(handle_create_pairing),
        )
        .route("/api/auth/pairing/{id}", delete(handle_revoke_pairing))
        .route(
            "/api/auth/sessions",
            get(handle_list_sessions).post(handle_issue_session),
        )
        .route("/api/auth/sessions/{id}", delete(handle_revoke_session))
        .route(
            "/api/auth/connect-devices",
            get(handle_list_connect_devices),
        )
        .route(
            "/api/auth/connect-devices/approve",
            post(handle_post_approve_connect_device),
        )
        .route(
            "/api/auth/connect-devices/revoke",
            post(handle_post_revoke_connect_device),
        )
        .route(
            "/.well-known/oauth-protected-resource",
            get(handle_mcp_protected_resource),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(handle_mcp_protected_resource),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(handle_mcp_authorization_server),
        )
        .route("/oauth/mcp/register", post(handle_mcp_register))
        .route("/oauth/mcp/authorize", get(handle_mcp_authorize))
        .route("/oauth/mcp/approval", post(handle_mcp_approval))
        .route("/oauth/mcp/decision", post(handle_mcp_decision))
        .route("/oauth/mcp/token", post(handle_mcp_token))
}

fn mount_orchestration_project_scm_routes(
    router: Router<Arc<RuntimeHttpState>>,
) -> Router<Arc<RuntimeHttpState>> {
    router
        .route("/api/orchestration/shell", get(handle_shell_snapshot))
        .route(
            "/api/orchestration/threads/{threadId}",
            get(handle_thread_detail),
        )
        .route(
            "/api/orchestration/threads/{threadId}/transcript",
            get(handle_thread_transcript),
        )
        .route(
            "/api/orchestration/threads/{threadId}/bounded",
            get(handle_thread_bounded),
        )
        .route(
            "/api/orchestration/threads/{threadId}/history",
            get(handle_thread_history),
        )
        .route("/api/projects", get(handle_get_projects))
        .route("/api/projects/read", get(handle_read_project_query))
        .route("/api/projects/{id}", get(handle_read_project_by_id))
        .route("/api/projects/mutate", post(handle_mutate_project))
        .route("/api/pull-requests/diff", post(handle_pull_request_diff))
        .route(
            "/api/workspace/folders",
            get(handle_list_workspace_folders_get).post(handle_list_workspace_folders_post),
        )
        .route(
            "/api/workspace/folders/scan",
            post(handle_scan_workspace_folders),
        )
        .route(
            "/api/workspace/task-folders",
            get(handle_get_task_folder_get).post(handle_get_task_folder_post),
        )
        .route(
            "/api/workspace/task-folders/repositories",
            post(handle_add_task_folder_repository),
        )
        .route(
            "/api/scm/pr",
            get(handle_get_scm_pr).post(handle_post_scm_pr),
        )
        .route(
            "/api/scm/diff",
            get(handle_get_scm_diff).post(handle_post_scm_diff),
        )
}

fn mount_voice_and_connect_routes(
    router: Router<Arc<RuntimeHttpState>>,
) -> Router<Arc<RuntimeHttpState>> {
    router
        .route("/api/voice/cloud/keys", get(handle_voice_cloud_keys))
        .route(
            "/api/voice/cloud/{provider}/key",
            post(handle_voice_set_cloud_key),
        )
        .route(
            "/api/voice/cloud/openrouter/models",
            get(handle_voice_cloud_openrouter_models),
        )
        .route(
            "/api/voice/cloud/{provider}/transcribe",
            post(handle_voice_cloud_transcribe),
        )
        .route("/api/voice/status", get(handle_voice_status))
        .route(
            "/api/voice/models/prepare",
            post(handle_voice_models_prepare),
        )
        .route("/api/voice/models", get(handle_voice_models_list))
        .route(
            "/api/voice/models/download",
            post(handle_voice_models_download),
        )
        .route("/api/voice/models/select", post(handle_voice_models_select))
        .route("/api/voice/models/cancel", post(handle_voice_models_cancel))
        .route("/api/voice/models/remove", post(handle_voice_models_remove))
        .route("/api/voice/models/link", post(handle_voice_models_link))
        .route("/api/voice/acceleration", post(handle_voice_acceleration))
        .route(
            "/api/voice/model-unload-timeout",
            post(handle_voice_unload_timeout),
        )
        .route("/api/voice/transcribe", post(handle_voice_transcribe))
        .route("/api/voice/post-process", post(handle_voice_post_process))
        .route("/api/connect/link-proof", post(handle_connect_link_proof))
        .route(
            "/api/connect/relay-config",
            post(handle_connect_relay_config),
        )
        .route("/api/connect/link-state", get(handle_connect_link_state))
        .route(
            "/api/connect/reachability",
            post(handle_connect_reachability),
        )
        .route("/api/connect/unlink", post(handle_connect_unlink))
        .route("/api/connect/preferences", post(handle_connect_preferences))
        .route(
            "/api/connect/live-activity-preference",
            post(handle_connect_live_activity_preference),
        )
        .route(
            "/api/legion-connect/health",
            post(handle_legion_connect_health),
        )
        .route(
            "/api/legion-connect/mint-credential",
            post(handle_legion_connect_mint_credential),
        )
}

fn mount_peer_and_asset_routes(
    router: Router<Arc<RuntimeHttpState>>,
) -> Router<Arc<RuntimeHttpState>> {
    router
        .route("/api/peers", get(handle_list_peers).post(handle_link_peer))
        .route("/api/peers/{id}", delete(handle_unlink_peer))
        .route("/api/peers/pair-link", post(handle_pair_link_through_peer))
        .route("/api/peer/threads", get(handle_list_peer_threads))
        .route(
            "/api/peer/threads/{id}/stream",
            get(handle_peer_thread_stream),
        )
        .route("/api/links/thread/stream", get(handle_link_thread_stream))
        .route("/api/links/thread/history", get(handle_link_thread_history))
        .route("/api/links/thread/item", get(handle_link_thread_item))
        .route(
            "/api/links/client-pairing",
            post(handle_issue_client_pairing),
        )
        .route("/api/mcp/sign-in/start", post(handle_mcp_sign_in_start))
        .route(
            "/api/mcp/sign-in/complete",
            post(handle_mcp_sign_in_complete),
        )
        .route("/api/mcp/sign-in/notice", post(handle_mcp_sign_in_notice))
        .route("/api/providers/probe", post(handle_provider_probe))
        .route("/api/preview/action", post(handle_preview_action))
        .route("/api/preview/tunnel", any(handle_preview_tunnel))
        .route("/api/assets/workspace", get(handle_workspace_asset))
        .route("/api/assets/{*tail}", any(handle_signed_asset))
        .route(
            "/api/attachments/{*token}",
            post(handle_signed_attachment_upload),
        )
        .route("/api/hooks/{hookId}/{token}", any(handle_webhook_delivery))
        .route("/api/observability/v1/traces", post(handle_otlp_traces))
        .route("/api", any(handle_api_not_found))
        .route("/api/{*rest}", any(handle_api_not_found))
        .route("/oauth", any(handle_api_not_found))
        .route("/oauth/{*rest}", any(handle_api_not_found))
}
