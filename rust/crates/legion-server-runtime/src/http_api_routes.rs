//! HTTP route table for `runtime_api_router` (`legion-server-runtime`).

use std::sync::Arc;

use axum::Router;
use axum::routing::{any, delete, get, post};

use crate::http_api::RuntimeHttpState;
use crate::http_auth::{handle_oauth_token, handle_pair};
use crate::http_auth_admin::{
    handle_approve_connect_device, handle_create_pairing, handle_issue_session,
    handle_list_connect_devices, handle_list_pairing, handle_list_sessions,
    handle_revoke_connect_device, handle_revoke_pairing, handle_revoke_session,
};
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
    Router::new()
        .route("/api/auth/pair", post(handle_pair))
        .route("/oauth/token", post(handle_oauth_token))
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
            post(handle_approve_connect_device),
        )
        .route(
            "/api/auth/connect-devices/revoke",
            post(handle_revoke_connect_device),
        )
        .route("/api/projects", get(handle_get_projects))
        .route("/api/projects/read", get(handle_read_project_query))
        .route("/api/projects/{id}", get(handle_read_project_by_id))
        .route("/api/projects/mutate", post(handle_mutate_project))
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
        .route("/api/observability/v1/traces", post(handle_otlp_traces))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            redacted_access_log_middleware,
        ))
        .with_state(state)
}
