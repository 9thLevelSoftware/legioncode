//! Canonical built-in `LegionCode` / `T3` MCP tool allowlist used to safely
//! pre-approve server-qualified Antigravity MCP calls (`#39822`).

use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

/// Dual server names accepted for built-in MCP tool pre-approval (`#39822`).
pub const LEGION_MCP_SERVER_NAMES: &[&str] = &["legion-code", "t3-code"];

const KNOWN_MCP_TOOL_SUFFIXES: &[&str] = &[
    "link_pull_request",
    "link_pull_requests",
    "unlink_pull_request",
    "unlink_pull_requests",
    "list_thread_pull_requests",
    "watch_pull_request",
    "unwatch_pull_request",
    "write_pull_request_guides",
    "read_pull_request_guide",
    "list_issues",
    "read_issue",
    "create_issue",
    "comment_on_issue",
    "set_issue_state",
    "link_issue",
    "unlink_issue",
    "list_thread_issues",
    "pull_request_read",
    "pull_request_list",
    "pull_request_create",
    "pull_request_update",
    "pull_request_respond",
    "pull_request_review",
    "ci_logs",
    "ci_rerun",
    "pull_request_merge",
    "orchestrator_capabilities",
    "delegate_task",
    "task_status",
    "task_cancel",
    "schedule_task",
    "list_scheduled_tasks",
    "update_scheduled_task",
    "delete_scheduled_task",
    "run_scheduled_task_now",
    "request_secret",
    "create_threads",
    "preview_status",
    "preview_open",
    "preview_navigate",
    "preview_dialog",
    "preview_snapshot",
    "preview_act",
    "preview_click",
    "preview_press",
    "preview_type",
    "preview_hover",
    "preview_select",
    "preview_drag",
    "preview_upload",
    "preview_scroll",
    "preview_resize",
    "preview_evaluate",
    "preview_wait_for",
    "preview_set_appearance",
    "preview_recording_start",
    "preview_recording_stop",
    "device_list",
    "device_open",
    "device_screenshot",
    "device_close",
    "html_preview",
    "html_render",
    "send_to_user",
];

const BRANDED_MCP_TOOL_STEMS: &[&str] = &[
    "feedback",
    "thread_start",
    "thread_list",
    "thread_read",
    "thread_send",
    "thread_wait",
    "workflow_stop",
    "background_wait",
    "thread_interrupt",
    "worktree_handoff",
    "worktree_status",
    "queue",
    "pending_request",
    "workflow",
    "submit_output",
    "queue_list",
    "queue_read",
    "queue_edit",
    "queue_cancel",
    "queue_reorder",
    "queue_promote_to_steer",
    "pending_request_list",
    "pending_request_read",
    "pending_request_respond",
    "thread_configuration",
    "thread_configure",
    "thread_transfer",
    "thread_fork",
    "thread_merge_back",
    "thread_search",
    "thread_transfers",
    "thread_organize",
    "thread_update",
    "worktree_list",
    "preview_list",
    "preview_close",
    "environment_read",
    "environment_links",
    "environment_link",
    "environment_unlink",
    "environment_preferences_update",
    "theme_read",
    "theme_set",
    "thread_launch",
    "project_list",
    "project_read",
    "project_create",
    "project_update",
    "project_delete",
    "project_clone",
    "attachment_prepare_upload",
    "attachment_discard",
    "thread_send_attachments",
];

fn antigravity_mcp_title_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"^Run mcp_(?:legion-code|t3-code)_([A-Za-z0-9_]+)\??$");
        // INFALLIBLE: constant Antigravity MCP title regex (`#39822`).
        compiled.expect("antigravity_mcp_title_regex must compile")
    })
}

/// Returns `true` if `tool_name` is a known built-in `LegionCode` (`legion_*`) or
/// upstream `T3` (`t3_*`) MCP tool identifier (`#39822`).
#[must_use]
pub fn is_known_legion_mcp_tool(tool_name: &str) -> bool {
    if KNOWN_MCP_TOOL_SUFFIXES.contains(&tool_name) {
        return true;
    }
    tool_name
        .strip_prefix("legion_")
        .or_else(|| tool_name.strip_prefix("t3_"))
        .is_some_and(|stem| BRANDED_MCP_TOOL_STEMS.contains(&stem))
}

/// Returns `true` only when an Antigravity permission request clearly names the `legion-code`
/// or `t3-code` server and a built-in MCP tool (never ambiguous `"Running <tool>"` or shell commands) (`#39822`).
#[must_use]
pub fn is_antigravity_legion_mcp_request(title: Option<&str>, raw_input: Option<&Value>) -> bool {
    if let Some(obj) = raw_input.and_then(Value::as_object) {
        if let Some(server) = obj.get("ServerName") {
            let valid_server = server
                .as_str()
                .is_some_and(|s| LEGION_MCP_SERVER_NAMES.contains(&s));
            let valid_tool = obj
                .get("ToolName")
                .and_then(Value::as_str)
                .is_some_and(is_known_legion_mcp_tool);
            return valid_server && valid_tool;
        }
        if obj.get("CommandLine").and_then(Value::as_str).is_some() {
            return false;
        }
    }
    let Some(trimmed_title) = title.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    let Some(tool) = antigravity_mcp_title_regex()
        .captures(trimmed_title)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str())
    else {
        return false;
    };
    is_known_legion_mcp_tool(tool)
}
