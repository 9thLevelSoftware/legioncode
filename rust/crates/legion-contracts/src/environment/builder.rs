//! Builder matching `apps/server/src/environment/ServerEnvironment.ts:211-305`.

use std::num::NonZeroU64;

use crate::ids::EnvironmentId;

use super::{
    EnvironmentCapabilities, ExecutionEnvironmentDescriptor, FileAttachmentsCapability,
    ORCHESTRATION_PROTOCOL_VERSION, PROVIDER_SEND_TURN_MAX_FILE_BYTES, PlatformInfo, PlatformOs,
    ServerInstallation, ServerSelfUpdateCapability,
};

const MAX_FILE_BYTES: NonZeroU64 = match NonZeroU64::new(PROVIDER_SEND_TURN_MAX_FILE_BYTES) {
    Some(value) => value,
    None => NonZeroU64::MIN,
};

/// Builder constructing an [`ExecutionEnvironmentDescriptor`] with full server v2 capabilities.
#[derive(Debug, Clone)]
pub struct ExecutionEnvironmentBuilder {
    environment_id: EnvironmentId,
    label: String,
    platform: PlatformInfo,
    server_version: String,
    database_migration: Option<u32>,
    voice_transcription: bool,
    server_self_update: Option<ServerSelfUpdateCapability>,
    server_installation: Option<ServerInstallation>,
    desktop_app_update: bool,
    agent_activity_publishing: Option<bool>,
}

impl ExecutionEnvironmentBuilder {
    /// Creates a new builder with default protocol v2 server capabilities.
    pub fn new(
        environment_id: EnvironmentId,
        label: impl Into<String>,
        platform: PlatformInfo,
        server_version: impl Into<String>,
    ) -> Self {
        Self {
            environment_id,
            label: label.into().trim().to_owned(),
            platform,
            server_version: server_version.into().trim().to_owned(),
            database_migration: None,
            voice_transcription: false,
            server_self_update: None,
            server_installation: None,
            desktop_app_update: false,
            agent_activity_publishing: None,
        }
    }

    /// Sets the highest applied SQLite migration number (`databaseMigration`).
    #[must_use]
    pub fn database_migration(mut self, migration: u32) -> Self {
        self.database_migration = Some(migration);
        self
    }

    /// Sets whether native host voice transcription is supported.
    #[must_use]
    pub fn voice_transcription(mut self, supported: bool) -> Self {
        self.voice_transcription = supported;
        self
    }

    /// Sets the server self-update capability (clearing manual installation when set).
    #[must_use]
    pub fn server_self_update(mut self, capability: ServerSelfUpdateCapability) -> Self {
        self.server_self_update = Some(capability);
        self
    }

    /// Sets the detected manual server installation descriptor (used when `server_self_update` is absent).
    #[must_use]
    pub fn server_installation(mut self, installation: ServerInstallation) -> Self {
        self.server_installation = Some(installation);
        self
    }

    /// Sets whether desktop-managed app updates are active (`ServerEnvironment.ts:205-208`).
    #[must_use]
    pub fn desktop_app_update(mut self, enabled: bool) -> Self {
        self.desktop_app_update = enabled;
        self
    }

    /// Sets the runtime `agentActivityPublishing` capability flag (`ServerEnvironment.ts:310-315`).
    #[must_use]
    pub fn agent_activity_publishing(mut self, active: bool) -> Self {
        self.agent_activity_publishing = Some(active);
        self
    }

    /// Builds the [`ExecutionEnvironmentDescriptor`] matching `ServerEnvironment.ts:211-305`.
    pub fn build(self) -> ExecutionEnvironmentDescriptor {
        let is_darwin = self.platform.os == PlatformOs::Darwin;
        let is_windows = self.platform.os == PlatformOs::Windows;
        let desktop_update = self.server_self_update
            == Some(ServerSelfUpdateCapability::DesktopManaged)
            && self.desktop_app_update;
        let boot_service = self.server_self_update == Some(ServerSelfUpdateCapability::BootService);
        let installation = if self.server_self_update.is_none() {
            self.server_installation
        } else {
            None
        };

        let mut capabilities = base_server_capabilities();
        capabilities.fast_worktree_checkout = is_darwin.then_some(true);
        capabilities.local_server_stop = (!is_windows).then_some(true);
        capabilities.server_self_update = self.server_self_update;
        capabilities.server_installation = installation;
        capabilities.server_self_update_progress = (boot_service || desktop_update).then_some(true);
        capabilities.server_update_staging = boot_service.then_some(true);
        capabilities.agent_activity_publishing = self.agent_activity_publishing;
        capabilities.desktop_app_update = desktop_update.then_some(true);
        capabilities.desktop_app_channel_switch = desktop_update.then_some(true);
        capabilities.voice_transcription = Some(self.voice_transcription);

        ExecutionEnvironmentDescriptor {
            environment_id: self.environment_id,
            label: self.label,
            platform: self.platform,
            server_version: self.server_version,
            orchestration_protocol_version: Some(ORCHESTRATION_PROTOCOL_VERSION),
            database_migration: self.database_migration,
            capabilities,
        }
    }
}

fn base_server_capabilities() -> EnvironmentCapabilities {
    EnvironmentCapabilities {
        repository_identity: true,
        connection_probe: Some(true),
        access_token_renewal: Some(true),
        attachment_uploads: Some(true),
        question_attachments: Some(true),
        file_attachments: Some(FileAttachmentsCapability {
            max_upload_bytes: MAX_FILE_BYTES,
        }),
        pull_requests: Some(true),
        pull_request_checks: Some(true),
        issues: Some(true),
        side_chats: Some(true),
        vcs_branch_status: Some(true),
        inline_message_context: Some(true),
        required_worktree_bootstrap: Some(true),
        thread_settlement: Some(true),
        thread_auto_settlement: Some(true),
        storage_cleanup: Some(true),
        storage_cleanup_run: Some(true),
        database_storage: Some(true),
        otlp_endpoint_check: Some(true),
        secret_source_check: Some(true),
        project_worktree_cleanup: Some(true),
        settled_worktree_cleanup: Some(true),
        worktrees_directory: Some(true),
        fast_worktree_checkout: None,
        thread_auto_settlement_scope: Some(true),
        thread_restart_continuation: Some(true),
        project_settings_overrides: Some(true),
        project_provider_instance_overrides: Some(true),
        thread_snooze: Some(true),
        thread_force_stop: Some(true),
        environment_themes: Some(true),
        usage_limit_sources: Some(true),
        provider_workspace_snapshots_on_demand: Some(true),
        model_catalog_refresh: Some(true),
        usage_price_overrides: Some(true),
        usage_model_aliases: Some(true),
        thread_pinning: Some(true),
        thread_pin_reorder: Some(true),
        thread_active_reorder: Some(true),
        thread_auto_settle_opt_out: Some(true),
        thread_project_from_scratch: Some(true),
        thread_title_regeneration: Some(true),
        thread_visited_tracking: Some(true),
        thread_pull_request_linking: Some(true),
        server_resolved_command_context: Some(true),
        thread_stop: Some(true),
        local_server_stop: None,
        thread_pull_requests: Some(true),
        thread_pull_request_watch: Some(true),
        pull_request_stack_actions: Some(true),
        server_self_update: None,
        npm_release_lookup: Some(true),
        server_installation: None,
        server_self_update_progress: None,
        server_update_staging: None,
        server_update_thread_continuation: None,
        agent_activity_publishing: None,
        project_clone_tracking: Some(true),
        project_organization: Some(true),
        ignored_file_listing: Some(true),
        ignored_file_search: Some(true),
        environment_icon: Some(true),
        desktop_app_update: None,
        desktop_app_channel_switch: None,
        command_output_streaming: Some(true),
        server_browser: Some(true),
        peer_links: Some(true),
        mcp_mode_limit_header: Some(true),
        link_fence: Some(true),
        mcp_approval_codes: Some(true),
        peer_link_pairing_codes: Some(true),
        mcp_hosted_approval: Some(true),
        triage: Some(true),
        one_time_schedules: Some(true),
        fresh_context: Some(true),
        scheduled_task_thread_modes: Some(true),
        voice_transcription: Some(false),
        cloud_voice_transcription: Some(true),
        shell_thread_families: Some(true),
        shell_older_settled: Some(true),
        linked_thread_refreshes: Some(true),
        issue_linked_threads: Some(true),
        preview_tunnel_setting: Some(true),
        multi_repo_projects: Some(true),
        linked_thread_read: Some(true),
        link_thread_stream: Some(true),
        link_client_pairing: Some(true),
        link_pairing_for_link: Some(true),
        transitive_peer_linking: Some(true),
        agent_feedback: Some(true),
        issue_tracker_accounts: Some(true),
    }
}
