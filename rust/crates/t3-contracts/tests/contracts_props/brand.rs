//! Tests for brand presets, workspace folders, run output, MCP sign-in, peer links, and Apple Speech contracts.

use std::collections::BTreeMap;

use serde_json::json;
use t3_contracts::brand::{
    ACTIVE_BRAND, BrandConfig, LEGION_CODE, T3_COMPAT, canonical_mcp_tool_suffix,
    desktop_pipe_prefix, env_var_candidates, read_env_from, tunnel_header_candidates,
};
use t3_contracts::ipc::{
    DesktopAppleSpeechErrorCode, DesktopAppleSpeechResult, DesktopAppleSpeechTranscribeInput,
};
use t3_contracts::mcp_sign_in::{
    McpSignInRequest, McpSignInStatus, OrchestrationMcpSignInNotice,
    ProviderMcpSignInCompleteInput, ProviderMcpSignInError, ProviderMcpSignInStartInput,
};
use t3_contracts::orchestration::{
    DomainEventPayload, MessageDispatchCommand, OrchestrationCommand, RunOutputPayload,
    SecretRequestField, SubmitRunOutputCommand, TurnItemKind, format_run_output_instructions,
    is_internal_command_type, validate_run_output_schema_size,
};
use t3_contracts::peer_links::{
    PeerLinkClientPairingRequest, PeerLinkClientPairingResult, PeerLinkError, PeerLinkErrorReason,
    PeerLinkFailureKind, PeerLinkLinkPairingInput, PeerLinkName, PeerLinkPairingLinkAccess,
    PeerLinkPairingUnsupportedReason, PeerLinkThreadReadError, PeerLinkThreadReadReason,
    PeerMcpClientAccess, peer_link_failure_kind,
};
use t3_contracts::permissions::{
    EnvironmentScope, WsMethod, required_scopes_for_rpc, server_required_scope_for_ws_method,
};
use t3_contracts::policy::{
    ManagedPolicySummary, ManagedPolicySummaryLimits, allows_preview_tunnel,
    describe_manageable_settings, list_manageable_settings,
};
use t3_contracts::secrets::SecretRequestFailureReason;
use t3_contracts::workspace_folders::{
    TASK_FOLDER_DEFAULT_REPOSITORIES_MAX, TaskFolder, TaskFolderRepository, WorkspaceFolder,
    WorkspaceFolderState, WorkspaceScanFoldersResult, WorkspaceScannedFolder,
    WorkspaceScannedFolderSource,
};

#[test]
fn brand_configs_cover_t3_compat_and_legion_code() {
    assert_eq!(ACTIVE_BRAND, T3_COMPAT);
    assert_ne!(T3_COMPAT.product_name, LEGION_CODE.product_name);
    assert_ne!(T3_COMPAT.url_scheme, LEGION_CODE.url_scheme);

    let check_brand = |b: &BrandConfig| {
        assert_ne!(b.product_name, "");
        assert_ne!(b.org_domain, "");
        assert_ne!(b.cli_bin_name, "");
        assert_ne!(b.desktop_bin_name, "");
        assert!(b.home_dir_name.starts_with('.'));
        assert!(b.fallback_home_dir_name.starts_with('.'));
        assert!(b.systemd_unit_name.ends_with(".service"));
        assert_ne!(b.launchd_label, "");
        assert_ne!(b.windows_task_name, "");
        assert!(b.windows_policy_key.contains(r"SOFTWARE\Policies\"));
        assert_ne!(b.macos_policy_domain, "");
    };
    check_brand(&T3_COMPAT);
    check_brand(&LEGION_CODE);

    assert_eq!(env_var_candidates("HOME"), vec!["T3CODE_HOME", "T3_HOME"]);
    assert_eq!(
        LEGION_CODE.env_var_candidates("HOME"),
        vec!["LEGION_HOME", "T3CODE_HOME"]
    );

    let env = BTreeMap::from([
        ("T3_HOME".to_owned(), "/legacy".to_owned()),
        ("LEGION_HOME".to_owned(), "/legion".to_owned()),
        ("T3CODE_PORT".to_owned(), "  ".to_owned()),
        ("T3_PORT".to_owned(), "4100".to_owned()),
    ]);
    assert_eq!(
        read_env_from("HOME", |k| env.get(k).cloned()),
        Some("/legacy".to_owned())
    );
    assert_eq!(
        LEGION_CODE.read_env_from("HOME", |k| env.get(k).cloned()),
        Some("/legion".to_owned())
    );
    assert_eq!(
        read_env_from("PORT", |k| env.get(k).cloned()),
        Some("4100".to_owned())
    );

    assert_eq!(
        canonical_mcp_tool_suffix("mcp__t3_code__t3_submit_output"),
        "submit_output"
    );
    assert_eq!(
        canonical_mcp_tool_suffix("mcp__legion_code__legion_delegate_task"),
        "delegate_task"
    );
    assert_eq!(canonical_mcp_tool_suffix("t3_ask_user"), "ask_user");
    assert_eq!(canonical_mcp_tool_suffix("legion_ask_user"), "ask_user");
    assert_eq!(
        LEGION_CODE.canonical_mcp_tool_suffix("mcp__t3_code__task_status"),
        "task_status"
    );

    assert_eq!(
        tunnel_header_candidates("mode-limit"),
        vec!["x-t3-mode-limit", "t3-mode-limit"]
    );
    assert_eq!(
        LEGION_CODE.tunnel_header_candidates("mode-limit"),
        vec!["x-legion-mode-limit", "x-t3-mode-limit"]
    );

    assert_eq!(desktop_pipe_prefix(), r"\\.\pipe\t3code-app-");
    assert_eq!(
        LEGION_CODE.desktop_pipe_prefix(),
        r"\\.\pipe\legioncode-app-"
    );
}

#[test]
fn preview_tunnel_policy_and_manageable_settings_match_ts() {
    let denied = ManagedPolicySummary {
        organization: "Acme".to_owned(),
        support_url: None,
        locked_settings: vec![],
        limits: Some(ManagedPolicySummaryLimits {
            preview_tunnel: Some(false),
            ..Default::default()
        }),
    };
    let allowed = ManagedPolicySummary {
        organization: "Acme".to_owned(),
        support_url: None,
        locked_settings: vec![],
        limits: Some(ManagedPolicySummaryLimits {
            preview_tunnel: Some(true),
            ..Default::default()
        }),
    };
    assert!(allows_preview_tunnel(true, None));
    assert!(!allows_preview_tunnel(false, None));
    assert!(!allows_preview_tunnel(true, Some(&denied)));
    assert!(allows_preview_tunnel(true, Some(&allowed)));

    let settings = list_manageable_settings();
    assert!(settings.windows(2).all(|w| w[0].0 < w[1].0));
    assert!(
        settings
            .iter()
            .any(|(p, v)| *p == "enablePreviewTunnel" && *v == Some(json!(true)))
    );
    assert!(
        settings
            .iter()
            .any(|(p, v)| *p == "defaultThreadEnvMode" && v.is_none())
    );
    let formatted = describe_manageable_settings();
    assert!(formatted.contains("enablePreviewTunnel = true"));
    assert!(formatted.contains("defaultThreadEnvMode = (no default)"));
}

#[test]
fn workspace_folders_run_output_secret_fields_and_wave1_contracts_round_trip() {
    assert_eq!(TASK_FOLDER_DEFAULT_REPOSITORIES_MAX, 8);
    let tf = TaskFolder {
        root: "/wt/task-1".to_owned(),
        project_id: "proj-1".parse().expect("project id"),
        project_root: "/repo".to_owned(),
        anchor: "/repo".to_owned(),
        project_path: "/wt/task-1/repo".to_owned(),
        project_is_worktree: false,
        repositories: vec![TaskFolderRepository {
            repository_path: "/repo/api".to_owned(),
            worktree_path: "/wt/task-1/repo/api".to_owned(),
            name: "api".to_owned(),
        }],
    };
    let tf_json = serde_json::to_value(&tf).expect("serialize task folder");
    assert_eq!(
        serde_json::from_value::<TaskFolder>(tf_json).expect("roundtrip"),
        tf
    );

    let wf = WorkspaceFolder {
        name: "api".to_owned(),
        path: "/repo/api".to_owned(),
        state: WorkspaceFolderState::Git,
        discovered: Some(true),
    };
    assert_eq!(
        serde_json::to_value(&wf).expect("wf json")["state"],
        json!("git")
    );
    let scan = WorkspaceScanFoldersResult {
        workspace_root: "/repo".to_owned(),
        is_git_repository: false,
        code_workspace_file: None,
        folders: vec![WorkspaceScannedFolder {
            path: "/repo/api".to_owned(),
            name: "api".to_owned(),
            is_git_repository: true,
            source: WorkspaceScannedFolderSource::CodeWorkspace,
        }],
    };
    assert_eq!(
        serde_json::to_value(&scan).expect("scan")["folders"][0]["source"],
        json!("code-workspace")
    );
}

#[test]
fn run_output_secret_fields_and_apple_speech_round_trip() {
    assert!(is_internal_command_type("run.output.submit"));
    let submit = SubmitRunOutputCommand {
        command_id: "cmd-1".parse().expect("cmd"),
        thread_id: "th-1".parse().expect("thread"),
        run_id: "run-1".parse().expect("run"),
        value: json!({"ok": true}),
    };
    assert_eq!(submit.value, json!({"ok": true}));

    let mut schema = serde_json::Map::new();
    schema.insert("type".to_owned(), json!("object"));
    assert!(validate_run_output_schema_size(&schema).is_ok());
    assert!(format_run_output_instructions(&schema).contains("t3_submit_output"));

    let evt_json =
        json!({"runId":"run-1","output":{"value":{"a":1},"submittedAt":"2026-01-01T00:00:00Z"}});
    let payload: RunOutputPayload =
        serde_json::from_value(evt_json["output"].clone()).expect("out");
    assert_eq!(payload.value, json!({"a": 1}));
    let domain_raw = json!({
        "id": "evt-1", "threadId": "th-1", "occurredAt": "2026-01-01T00:00:00Z",
        "type": "run.output-submitted", "payload": evt_json
    });
    let evt: t3_contracts::orchestration::DomainEvent =
        serde_json::from_value(domain_raw).expect("domain event");
    assert!(matches!(
        evt.payload,
        DomainEventPayload::RunOutputSubmitted(_)
    ));

    let field: SecretRequestField =
        serde_json::from_value(json!({"name":"email","label":"Email","secret":false}))
            .expect("field");
    assert!(!field.is_secret());
    assert!(field.revealable);

    let item: TurnItemKind = serde_json::from_value(json!({
        "type":"secret_request","label":"Creds","reason":"Need login","secretStatus":"pending",
        "fields":[{"name":"user","label":"User","secret":false}]
    }))
    .expect("secret item");
    assert!(matches!(item, TurnItemKind::SecretRequest { .. }));
    assert_eq!(
        serde_json::to_value(SecretRequestFailureReason::FieldsMismatch).expect("reason"),
        json!("fields_mismatch")
    );

    let notice: TurnItemKind = serde_json::from_value(json!({
        "type":"system_notice","message":"Auth needed","mcpSignIn":{"server":"linear"}
    }))
    .expect("notice");
    assert!(matches!(
        notice,
        TurnItemKind::SystemNotice {
            mcp_sign_in: Some(OrchestrationMcpSignInNotice { .. }),
            ..
        }
    ));

    let speech_ok = DesktopAppleSpeechResult::ok("en-US");
    let speech_err = DesktopAppleSpeechResult::err(DesktopAppleSpeechErrorCode::UnsupportedLocale);
    assert_eq!(
        serde_json::from_value::<DesktopAppleSpeechResult>(
            serde_json::to_value(&speech_ok).expect("ok")
        )
        .expect("rt"),
        speech_ok
    );
    assert_eq!(
        serde_json::to_value(&speech_err).expect("err"),
        json!({"ok": false, "code": "unsupported-locale"})
    );
    let speech_in = DesktopAppleSpeechTranscribeInput {
        locale: "en-US".to_owned(),
        pcm: vec![1, 2, 3, 4],
    };
    assert_eq!(speech_in.pcm.len(), 4);
}

#[test]
fn peer_link_mcp_sign_in_and_rpc_scopes_round_trip() {
    let pl_err = PeerLinkError {
        reason: PeerLinkErrorReason::UnknownLink,
        message: "wrong".to_owned(),
        answered_as: Some("env-other".to_owned()),
    };
    assert_eq!(
        peer_link_failure_kind(&pl_err),
        PeerLinkFailureKind::WrongEnvironment
    );
    let _ = (
        PeerLinkName {
            environment_id: "env-1".parse().expect("env"),
            label: "Box".to_owned(),
        },
        PeerLinkLinkPairingInput {
            environment_id: "env-1".parse().expect("env"),
            access: PeerMcpClientAccess::Auto,
        },
        PeerLinkClientPairingRequest {
            scopes: vec![],
            client_label: "Desk".to_owned(),
            link: Some(PeerLinkPairingLinkAccess {
                access: PeerMcpClientAccess::ApprovalRequired,
            }),
        },
        PeerLinkClientPairingResult::Unsupported {
            environment_id: "env-1".parse().expect("env"),
            label: "Box".to_owned(),
            urls: vec![],
            reason: PeerLinkPairingUnsupportedReason::Version,
        },
        PeerLinkThreadReadError {
            reason: PeerLinkThreadReadReason::Incompatible,
            label: None,
            message: "old".to_owned(),
        },
    );

    let _mcp_req = McpSignInRequest::Start(ProviderMcpSignInStartInput {
        thread_id: "th-1".parse().expect("th"),
        provider_thread_id: "pt-1".parse().expect("pt"),
        server: "github".to_owned(),
    });
    let _mcp_done = McpSignInRequest::Complete(ProviderMcpSignInCompleteInput {
        thread_id: "th-1".parse().expect("th"),
        provider_thread_id: "pt-1".parse().expect("pt"),
        server: "github".to_owned(),
        callback_url: "http://localhost:9999/cb?code=1&state=s".to_owned(),
    });
    let _mcp_status = McpSignInStatus::SignedIn {
        server: "github".to_owned(),
        signed_in_at: "2026-01-01T00:00:00Z".to_owned(),
    };
    let _mcp_err = ProviderMcpSignInError {
        server: "github".to_owned(),
        detail: "stopped".to_owned(),
    };

    assert_eq!(
        required_scopes_for_rpc(&WsMethod::WorkspaceAddTaskFolderRepository, None).expect("scope"),
        vec![EnvironmentScope::SourceControlWrite]
    );
    assert_eq!(
        required_scopes_for_rpc(&WsMethod::PeerLinksPairLink, None).expect("scope"),
        vec![EnvironmentScope::AccessWrite]
    );
    assert_eq!(
        required_scopes_for_rpc(&WsMethod::ProviderMcpSignInStart, None).expect("scope"),
        vec![EnvironmentScope::ProvidersManage]
    );
    assert_eq!(
        server_required_scope_for_ws_method(&WsMethod::WorkspaceScanFolders),
        Some(EnvironmentScope::FilesystemRead)
    );
    assert_eq!(
        server_required_scope_for_ws_method(&WsMethod::PeerLinksSubscribeThread),
        Some(EnvironmentScope::OrchestrationRead)
    );
    let tunnel_patch = json!({"patch": {"enablePreviewTunnel": false}});
    assert_eq!(
        required_scopes_for_rpc(&WsMethod::ServerUpdateSettings, Some(&tunnel_patch)).expect("tun"),
        vec![EnvironmentScope::AccessWrite]
    );
    let _unused: (Option<MessageDispatchCommand>, Option<OrchestrationCommand>) = (None, None);
}
