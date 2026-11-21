//! Stage 8 Wave T1 end-to-end live integration tests for `t3-server-runtime`.

mod stage8_runtime;

#[tokio::test]
async fn non_acp_codex_and_claude_subprocesses_stream_steer_and_approve()
-> stage8_runtime::TestResult {
    stage8_runtime::providers::run_subprocess_tests().await
}

#[tokio::test]
async fn non_acp_http_sse_opencode_and_grok_turns_and_title_gen() -> stage8_runtime::TestResult {
    stage8_runtime::providers::run_http_sse_and_title_tests().await
}

#[tokio::test]
async fn http_auth_admin_scm_peers_and_provider_probe_control_plane() -> stage8_runtime::TestResult
{
    stage8_runtime::control_plane::run_control_plane_tests().await
}

#[tokio::test]
async fn preview_browser_cdp_websocket_actions_and_launch_diagnostics() -> stage8_runtime::TestResult
{
    stage8_runtime::preview_cdp::run_preview_cdp_tests().await
}
