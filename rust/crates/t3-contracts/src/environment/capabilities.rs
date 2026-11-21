//! Server capability flags and installation descriptors (`environment.ts:61-215`).

use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};

use super::{
    deserialize_non_empty_raw, deserialize_trimmed_non_empty, forward_compatible_optional,
};

/// Strategy used by a server to apply self-updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServerSelfUpdateCapability {
    /// Managed by the installed background boot service.
    BootService,
    /// Respawns the server binary in place.
    Respawn,
    /// Managed by the surrounding desktop shell.
    DesktopManaged,
}

/// Description of how a manual server installation was invoked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ServerInstallation {
    /// Ephemeral `npx` execution.
    Npx,
    /// Ephemeral `pnpm dlx` execution.
    PnpmDlx,
    /// Ephemeral `bunx` execution.
    Bunx,
    /// Global npm install with an explicit prefix.
    NpmGlobal {
        /// Trimmed non-empty npm global prefix path.
        #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
        prefix: String,
    },
    /// Launcher script or symlink managed by `t3 update`.
    #[serde(rename_all = "camelCase")]
    T3Launcher {
        /// Exact untrimmed non-empty launcher path.
        #[serde(deserialize_with = "deserialize_non_empty_raw")]
        launcher: String,
        /// Exact untrimmed non-empty installation base directory.
        #[serde(deserialize_with = "deserialize_non_empty_raw")]
        base_dir: String,
    },
}

/// File attachment size capability advertised by the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileAttachmentsCapability {
    /// Maximum upload size in bytes (`>= 1`).
    pub max_upload_bytes: NonZeroU64,
}

/// Capability flags advertised by an execution environment (82 fields on wire).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentCapabilities {
    /// Whether the server reports canonical git repository identity.
    #[serde(default)]
    pub repository_identity: bool,
    /// Supports connection probing RPCs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_probe: Option<bool>,
    /// `DPoP` sessions can renew access tokens at `/oauth/token`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token_renewal: Option<bool>,
    /// Supports HTTP attachment uploads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_uploads: Option<bool>,
    /// Supports attachments on user-input question responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_attachments: Option<bool>,
    /// Advertised file attachment upload limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_attachments: Option<FileAttachmentsCapability>,
    /// Supports pull request operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_requests: Option<bool>,
    /// Supports pull request CI check summaries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request_checks: Option<bool>,
    /// Supports issue tracker integrations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issues: Option<bool>,
    /// Supports hidden side-chat forks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side_chats: Option<bool>,
    /// Supports live VCS branch status streams.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vcs_branch_status: Option<bool>,
    /// Supports inline message context metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_message_context: Option<bool>,
    /// Requires worktree bootstrap status handling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_worktree_bootstrap: Option<bool>,
    /// Supports explicit thread settlement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_settlement: Option<bool>,
    /// Supports automatic thread settlement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_auto_settlement: Option<bool>,
    /// Supports storage cleanup reporting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_cleanup: Option<bool>,
    /// Supports running storage cleanup on demand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_cleanup_run: Option<bool>,
    /// Supports database storage inspection and compaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_storage: Option<bool>,
    /// Supports OTLP endpoint connectivity checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub otlp_endpoint_check: Option<bool>,
    /// Serves `server.checkSecretSource`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_source_check: Option<bool>,
    /// Supports per-project worktree cleanup configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_worktree_cleanup: Option<bool>,
    /// Supports settled worktree cleanup retention rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_worktree_cleanup: Option<bool>,
    /// Supports configurable worktrees directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktrees_directory: Option<bool>,
    /// Honors `fastWorktreeCheckout` (macOS APFS clonefile).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_worktree_checkout: Option<bool>,
    /// Supports auto-settlement scope options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_auto_settlement_scope: Option<bool>,
    /// Supports automatic continuation after turn restarts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_restart_continuation: Option<bool>,
    /// Supports per-project settings overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_settings_overrides: Option<bool>,
    /// Supports per-project provider instance overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_provider_instance_overrides: Option<bool>,
    /// Supports thread snooze and unsnooze.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_snooze: Option<bool>,
    /// Serves `orchestration.forceStopThread`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_force_stop: Option<bool>,
    /// Advertises server environment themes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_themes: Option<bool>,
    /// Supports usage limit source tracking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_limit_sources: Option<bool>,
    /// Honors `workspaceSnapshotsOnDemand`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_workspace_snapshots_on_demand: Option<bool>,
    /// Honors `modelCatalogOnly` during provider refresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_catalog_refresh: Option<bool>,
    /// Supports usage price overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_price_overrides: Option<bool>,
    /// Supports usage model aliases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_model_aliases: Option<bool>,
    /// Supports pinning threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_pinning: Option<bool>,
    /// Supports reordering pinned threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_pin_reorder: Option<bool>,
    /// Supports reordering active threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_active_reorder: Option<bool>,
    /// Supports per-thread auto-settle opt-out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_auto_settle_opt_out: Option<bool>,
    /// Supports creating new projects directly from a thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_project_from_scratch: Option<bool>,
    /// Supports regenerating thread titles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_title_regeneration: Option<bool>,
    /// Supports tracking last-visited timestamps per thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_visited_tracking: Option<bool>,
    /// Supports linking pull requests to threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_pull_request_linking: Option<bool>,
    /// Resolves command context server-side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_resolved_command_context: Option<bool>,
    /// Supports `thread.stop`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_stop: Option<bool>,
    /// Supports stopping a discovered local server (absent on Windows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_server_stop: Option<bool>,
    /// Supports multi-PR `pullRequests[]` links on threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_pull_requests: Option<bool>,
    /// Supports automated PR watch monitoring on threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_pull_request_watch: Option<bool>,
    /// Supports PR stack actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request_stack_actions: Option<bool>,
    /// Server self-update mechanism, if supported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_self_update: Option<ServerSelfUpdateCapability>,
    /// Answers `server.getNpmRelease`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npm_release_lookup: Option<bool>,
    /// Manual installation descriptor; unrecognized variants decode as `None`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "forward_compatible_optional"
    )]
    pub server_installation: Option<ServerInstallation>,
    /// Streams progress during server self-updates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_self_update_progress: Option<bool>,
    /// Supports staged boot-service updates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_update_staging: Option<bool>,
    /// Supports continuing threads after server updates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_update_thread_continuation: Option<bool>,
    /// Dynamic opt-in flag for live agent activity publishing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_activity_publishing: Option<bool>,
    /// Supports tracking remote repository clones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_clone_tracking: Option<bool>,
    /// Persists `pinned` and `archived` flags on projects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_organization: Option<bool>,
    /// Serves `projects.listIgnoredFiles`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignored_file_listing: Option<bool>,
    /// `projects.listIgnoredFiles` honors `query`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignored_file_search: Option<bool>,
    /// Supports custom environment icons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_icon: Option<bool>,
    /// Desktop shell can apply updates via control fd.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop_app_update: Option<bool>,
    /// Desktop shell can switch release channels on update requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop_app_channel_switch: Option<bool>,
    /// Supports live command output streaming.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_output_streaming: Option<bool>,
    /// Hosts server-rendered headless Chromium preview tabs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_browser: Option<bool>,
    /// Supports linking to peer environments (`PeerLink`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_links: Option<bool>,
    /// `/mcp` honors `T3-Mode-Limit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_mode_limit_header: Option<bool>,
    /// Isolates linked work within its `linkOrigin`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_fence: Option<bool>,
    /// Mints approval codes (`AUTO-...`) for `/mcp` sign-ins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_approval_codes: Option<bool>,
    /// Supports peer-link pairing codes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_link_pairing_codes: Option<bool>,
    /// Supports hosted MCP approval through T3 Connect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_hosted_approval: Option<bool>,
    /// Serves `server.prepareTriage`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage: Option<bool>,
    /// Accepts `{ type: "once" }` scheduled tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub one_time_schedules: Option<bool>,
    /// Honors `freshContext` on dispatched messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fresh_context: Option<bool>,
    /// Saves and executes scheduled task `threadMode`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_task_thread_modes: Option<bool>,
    /// Supports local 16 kHz mono PCM voice transcription.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_transcription: Option<bool>,
    /// Supports cloud-backed voice transcription.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_voice_transcription: Option<bool>,
    /// Supports `getThreadFamily` and `omitIdleSubagents`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_thread_families: Option<bool>,
    /// Supports `getOlderSettledThreads` and `omitOlderSettled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_older_settled: Option<bool>,
    /// Publishes pull request refreshes when linked threads settle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_thread_refreshes: Option<bool>,
    /// Answers `issues.linkedThreads`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_linked_threads: Option<bool>,
}
