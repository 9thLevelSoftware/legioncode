use proptest::prelude::*;
use t3_contracts::environment::{
    CLIENT_CAPABILITIES_QUERY_PARAM, ExecutionEnvironmentDescriptor, MachineKind,
    ORCHESTRATION_PROTOCOL_HEADER, ORCHESTRATION_PROTOCOL_QUERY_PARAM,
    ORCHESTRATION_PROTOCOL_VERSION, ORCHESTRATION_PROTOCOL_VERSION_TEXT,
    PROVIDER_SEND_TURN_MAX_FILE_BYTES, PlatformArch, PlatformInfo, PlatformOs,
    SETTINGS_SECRET_REFERENCES_CAPABILITY, ServerInstallation, ServerSelfUpdateCapability,
};
use t3_contracts::ids::{
    CheckpointRef, CommandId, EnvironmentId, EventId, IdError, IssueTrackerKind, MessageId, NodeId,
    ProjectId, ProviderDriverKind, ProviderInstanceId, RunAttemptId, RunId,
    SourceControlProviderKind, ThreadId, TurnItemId,
};

proptest! {
    #[test]
    fn non_empty_trimmed_ids_round_trip(raw in ".*") {
        let trimmed = raw.trim();
        let env_res = EnvironmentId::new(&raw);
        let proj_res = ProjectId::new(&raw);
        let thread_res = ThreadId::new(&raw);
        let run_res = RunId::new(&raw);
        let cmd_res = CommandId::new(&raw);
        let msg_res = MessageId::new(&raw);
        let inst_res = ProviderInstanceId::new(&raw);
        let item_res = TurnItemId::new(&raw);

        if trimmed.is_empty() {
            prop_assert!(matches!(env_res, Err(IdError::Empty { .. })), "env empty");
            prop_assert!(matches!(proj_res, Err(IdError::Empty { .. })), "proj empty");
            prop_assert!(matches!(thread_res, Err(IdError::Empty { .. })), "thread empty");
            prop_assert!(matches!(run_res, Err(IdError::Empty { .. })), "run empty");
            prop_assert!(matches!(cmd_res, Err(IdError::Empty { .. })), "cmd empty");
            prop_assert!(matches!(msg_res, Err(IdError::Empty { .. })), "msg empty");
            prop_assert!(matches!(inst_res, Err(IdError::Empty { .. })), "inst empty");
            prop_assert!(matches!(item_res, Err(IdError::Empty { .. })), "item empty");
        } else {
            let env = env_res.expect("trimmed non-empty succeeds");
            prop_assert_eq!(env.as_str(), trimmed);
            let json = serde_json::to_string(&env).expect("serialize id");
            let decoded: EnvironmentId = serde_json::from_str(&json).expect("deserialize id");
            prop_assert_eq!(decoded, env);
            prop_assert_eq!(proj_res.as_ref().map(ProjectId::as_str), Ok(trimmed));
            prop_assert_eq!(thread_res.as_ref().map(ThreadId::as_str), Ok(trimmed));
            prop_assert_eq!(run_res.as_ref().map(RunId::as_str), Ok(trimmed));
            prop_assert_eq!(cmd_res.as_ref().map(CommandId::as_str), Ok(trimmed));
            prop_assert_eq!(msg_res.as_ref().map(MessageId::as_str), Ok(trimmed));
            prop_assert_eq!(item_res.as_ref().map(TurnItemId::as_str), Ok(trimmed));
        }
    }

    #[test]
    fn provider_instance_id_enforces_ascii_slug_format(slug in "[a-zA-Z][a-zA-Z0-9_-]{0,31}") {
        let padded = format!("  {slug}  ");
        let parsed = ProviderInstanceId::new(padded).expect("valid slug");
        prop_assert_eq!(parsed.as_str(), slug);
    }
}

#[test]
fn id_types_and_driver_kinds_validate_whitespace() {
    assert!(matches!(
        "   \t\n".parse::<EventId>(),
        Err(IdError::Empty { .. })
    ));
    assert!(serde_json::from_str::<NodeId>(r#""  ""#).is_err());
    assert!(matches!(
        ProviderInstanceId::new("1invalid"),
        Err(IdError::InvalidFormat { .. })
    ));
    assert!(matches!(
        ProviderInstanceId::new(format!("a{}", "b".repeat(64))),
        Err(IdError::TooLong { .. })
    ));
    let attempt = RunAttemptId::new(" attempt-1 ").expect("attempt id");
    assert_eq!(attempt.as_str(), "attempt-1");
    let cp_ref = CheckpointRef::new("refs/t3/cp-1").expect("checkpoint ref");
    assert_eq!(cp_ref.as_str(), "refs/t3/cp-1");

    let driver = ProviderDriverKind::new(" codex ").expect("driver");
    assert_eq!(driver.as_str(), "codex");
    let sc = SourceControlProviderKind::new("github").expect("sc provider");
    assert_eq!(sc.as_str(), "github");
    let tracker = IssueTrackerKind::new("linear").expect("tracker");
    assert_eq!(tracker.as_str(), "linear");
}

#[test]
fn environment_constants_match_ts_contracts() {
    assert_eq!(ORCHESTRATION_PROTOCOL_VERSION, 2);
    assert_eq!(ORCHESTRATION_PROTOCOL_VERSION_TEXT, "2");
    assert_eq!(ORCHESTRATION_PROTOCOL_QUERY_PARAM, "orchestrationProtocol");
    assert_eq!(ORCHESTRATION_PROTOCOL_HEADER, "x-t3-orchestration-protocol");
    assert_eq!(CLIENT_CAPABILITIES_QUERY_PARAM, "clientCapabilities");
    assert_eq!(
        SETTINGS_SECRET_REFERENCES_CAPABILITY,
        "settingsSecretReferences"
    );
    assert_eq!(PROVIDER_SEND_TURN_MAX_FILE_BYTES, 50 * 1024 * 1024);
}

#[test]
fn environment_builder_matches_server_environment_darwin_and_boot_service() {
    let env_id = EnvironmentId::new("env-mac").expect("env id");
    let platform = PlatformInfo::new(PlatformOs::Darwin, PlatformArch::Arm64)
        .with_machine(MachineKind::Desktop);
    let descriptor =
        ExecutionEnvironmentDescriptor::builder(env_id, " Mac Host ", platform, " 1.2.3 ")
            .database_migration(14)
            .voice_transcription(true)
            .agent_activity_publishing(true)
            .server_self_update(ServerSelfUpdateCapability::BootService)
            .server_installation(ServerInstallation::PnpmDlx)
            .build();

    assert_eq!(descriptor.label, "Mac Host");
    assert_eq!(descriptor.server_version, "1.2.3");
    assert_eq!(descriptor.orchestration_protocol_version, Some(2));
    assert_eq!(descriptor.database_migration, Some(14));
    assert!(descriptor.capabilities.repository_identity);
    assert_eq!(descriptor.capabilities.fast_worktree_checkout, Some(true));
    assert_eq!(descriptor.capabilities.local_server_stop, Some(true));
    assert_eq!(
        descriptor.capabilities.server_self_update,
        Some(ServerSelfUpdateCapability::BootService)
    );
    assert_eq!(descriptor.capabilities.server_installation, None);
    assert_eq!(
        descriptor.capabilities.server_self_update_progress,
        Some(true)
    );
    assert_eq!(descriptor.capabilities.server_update_staging, Some(true));
    assert_eq!(descriptor.capabilities.desktop_app_update, None);
    assert_eq!(descriptor.capabilities.voice_transcription, Some(true));
    assert_eq!(
        descriptor.capabilities.agent_activity_publishing,
        Some(true)
    );
    assert_eq!(
        descriptor
            .capabilities
            .file_attachments
            .as_ref()
            .map(|c| c.max_upload_bytes.get()),
        Some(PROVIDER_SEND_TURN_MAX_FILE_BYTES)
    );

    let json = serde_json::to_string(&descriptor).expect("serialize env");
    let parsed: ExecutionEnvironmentDescriptor =
        serde_json::from_str(&json).expect("deserialize env");
    assert_eq!(parsed, descriptor);
}

#[test]
fn environment_builder_windows_desktop_managed_vs_unmanaged_install() {
    let win = ExecutionEnvironmentDescriptor::builder(
        EnvironmentId::new("env-win").expect("id"),
        "Win Box",
        PlatformInfo::new(PlatformOs::Windows, PlatformArch::X64),
        "1.0.0",
    )
    .server_self_update(ServerSelfUpdateCapability::DesktopManaged)
    .desktop_app_update(true)
    .build();

    assert_eq!(win.capabilities.fast_worktree_checkout, None);
    assert_eq!(win.capabilities.local_server_stop, None);
    assert_eq!(win.capabilities.desktop_app_update, Some(true));
    assert_eq!(win.capabilities.desktop_app_channel_switch, Some(true));
    assert_eq!(win.capabilities.server_self_update_progress, Some(true));
    assert_eq!(win.capabilities.server_update_staging, None);

    let linux_npx = ExecutionEnvironmentDescriptor::builder(
        EnvironmentId::new("env-lin").expect("id"),
        "Linux",
        PlatformInfo::new(PlatformOs::Linux, PlatformArch::X64),
        "1.0.0",
    )
    .server_installation(ServerInstallation::Npx)
    .build();

    assert_eq!(linux_npx.capabilities.fast_worktree_checkout, None);
    assert_eq!(linux_npx.capabilities.local_server_stop, Some(true));
    assert_eq!(
        linux_npx.capabilities.server_installation,
        Some(ServerInstallation::Npx)
    );
    assert_eq!(linux_npx.capabilities.server_self_update_progress, None);
}
