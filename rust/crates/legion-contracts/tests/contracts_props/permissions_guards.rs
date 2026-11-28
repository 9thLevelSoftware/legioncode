use legion_contracts::permissions::{
    EmptyWsMethodError, EnvironmentScope, GuardEvalError, RpcCallerKind, SessionGrantInput,
    WsMethod, auth_scope_required_response, caller_kind_from_subject, contains_secret_reference,
    grants_all_scopes, required_scopes_for_project_mutation, required_scopes_for_rpc,
    required_scopes_for_rpc_json, required_scopes_for_server_settings_patch,
    requires_human_client_caller, server_required_scopes_for_rpc, session_grants_scope,
};
use proptest::prelude::*;
use serde_json::{Map, json};

#[test]
fn ws_method_parses_known_custom_and_rejects_empty() {
    assert_eq!(
        WsMethod::parse("orchestration.dispatchCommand"),
        Ok(WsMethod::OrchestrationDispatchCommand)
    );
    assert_eq!(
        WsMethod::parse("server.updateSettings"),
        Ok(WsMethod::ServerUpdateSettings)
    );
    assert_eq!(
        WsMethod::parse("future.namespace.rpc"),
        Ok(WsMethod::Custom("future.namespace.rpc".to_owned()))
    );
    assert_eq!(WsMethod::parse("   \t"), Err(EmptyWsMethodError));

    let round_trip: WsMethod =
        serde_json::from_str(r#""git.runStackedAction""#).expect("deserialize method");
    assert_eq!(round_trip, WsMethod::GitRunStackedAction);
}

#[test]
fn static_and_dynamic_rpc_guards_cover_all_scope_paths() {
    assert_eq!(
        required_scopes_for_rpc(&WsMethod::ProjectsListEntries, None).expect("unguarded method"),
        Vec::<EnvironmentScope>::new()
    );
    assert_eq!(
        required_scopes_for_rpc(&WsMethod::ProjectsWriteFile, None).expect("filesystem write"),
        vec![EnvironmentScope::FilesystemWrite]
    );
    assert_eq!(
        required_scopes_for_rpc(&WsMethod::ServerReclaimDatabaseSpace, None)
            .expect("environment maintain"),
        vec![EnvironmentScope::EnvironmentMaintain]
    );
    assert_eq!(
        required_scopes_for_rpc(&WsMethod::GuestShareCreate, None).expect("access write"),
        vec![EnvironmentScope::AccessWrite]
    );

    let plain_patch = json!({ "patch": { "telemetry": { "enabled": false } } });
    assert_eq!(
        required_scopes_for_rpc(&WsMethod::ServerUpdateSettings, Some(&plain_patch))
            .expect("plain patch"),
        vec![EnvironmentScope::SettingsWrite]
    );

    let secret_patch = json!({
        "patch": {
            "theme": "dark",
            "customEnv": { "provider": "env", "reference": "OPENAI_API_KEY" }
        }
    });
    assert!(contains_secret_reference(&secret_patch));
    assert_eq!(
        required_scopes_for_rpc(&WsMethod::ServerUpdateSettings, Some(&secret_patch))
            .expect("secret patch"),
        vec![
            EnvironmentScope::SettingsWrite,
            EnvironmentScope::ProvidersManage,
        ]
    );

    assert_eq!(
        required_scopes_for_rpc(
            &WsMethod::GitPreparePullRequestThread,
            Some(&json!({
                "cwd": "/repo",
                "reference": "#42",
                "mode": "worktree",
                "threadId": "thread-1"
            })),
        )
        .expect("worktree pr"),
        vec![
            EnvironmentScope::SourceControlWrite,
            EnvironmentScope::OrchestrationOperate,
        ]
    );
    assert_eq!(
        required_scopes_for_rpc(
            &WsMethod::GitRunStackedAction,
            Some(&json!({
                "actionId": "act-1",
                "cwd": "/repo",
                "action": "commit_push_pr",
                "threadId": "thread-1"
            })),
        )
        .expect("commit_push_pr"),
        vec![
            EnvironmentScope::SourceControlWrite,
            EnvironmentScope::OrchestrationRead,
        ]
    );
}

#[test]
fn bug17_malformed_json_returns_err_and_grants_all_checks_every_scope() {
    assert!(matches!(
        required_scopes_for_rpc_json(&WsMethod::ServerUpdateSettings, Some("{not-json")),
        Err(GuardEvalError::MalformedJson { .. })
    ));
    assert!(matches!(
        required_scopes_for_rpc(&WsMethod::ServerUpdateSettings, Some(&json!([1, 2]))),
        Err(GuardEvalError::InvalidInput { .. })
    ));
    assert!(matches!(
        required_scopes_for_rpc(
            &WsMethod::GitRunStackedAction,
            Some(&json!({ "actionId": "a1", "cwd": "/repo", "action": "bogus" })),
        ),
        Err(GuardEvalError::InvalidInput { .. })
    ));

    let only_first = SessionGrantInput {
        authenticated: true,
        scopes: None,
        permissions: Some(vec![EnvironmentScope::SourceControlWrite]),
        server_update_scope: None,
    };
    let both = [
        EnvironmentScope::SourceControlWrite,
        EnvironmentScope::OrchestrationOperate,
    ];
    // BUG-17: missing the second required scope MUST fail.
    assert!(!grants_all_scopes(&only_first, &both));

    let legacy_operate = SessionGrantInput {
        authenticated: true,
        scopes: Some(vec![EnvironmentScope::OrchestrationOperate]),
        permissions: None,
        server_update_scope: None,
    };
    assert!(grants_all_scopes(
        &legacy_operate,
        &[
            EnvironmentScope::SettingsWrite,
            EnvironmentScope::SourceControlWrite,
        ]
    ));
    assert!(!session_grants_scope(
        &legacy_operate,
        EnvironmentScope::AccessWrite
    ));

    let mut patch_map = Map::new();
    patch_map.insert(
        "secretValue".to_owned(),
        json!({ "provider": "keychain", "reference": "legioncode/token" }),
    );
    assert_eq!(
        required_scopes_for_server_settings_patch(&patch_map).expect("patch scopes"),
        vec![
            EnvironmentScope::SettingsWrite,
            EnvironmentScope::ProvidersManage,
        ]
    );
    assert_eq!(
        required_scopes_for_project_mutation(
            "project.create",
            Some(&json!([{ "env": { "provider": "env", "reference": "TOKEN" } }]))
        )
        .expect("project create w/ secret"),
        vec![
            EnvironmentScope::OrchestrationOperate,
            EnvironmentScope::SettingsWrite,
            EnvironmentScope::ProvidersManage,
        ]
    );
}

proptest! {
    #[test]
    fn grants_all_scopes_requires_every_scope_and_malformed_json_never_panics(
        malformed in "\\{[^\\}]{0,32}",
        grant_first in any::<bool>(),
        grant_second in any::<bool>(),
    ) {
        let res = required_scopes_for_rpc_json(&WsMethod::ServerUpdateSettings, Some(&malformed));
        prop_assert!(res.is_err());

        let mut granted = Vec::new();
        if grant_first {
            granted.push(EnvironmentScope::SettingsWrite);
        }
        if grant_second {
            granted.push(EnvironmentScope::ProvidersManage);
        }
        let session = SessionGrantInput {
            authenticated: true,
            scopes: None,
            permissions: Some(granted),
            server_update_scope: None,
        };
        let required = [EnvironmentScope::SettingsWrite, EnvironmentScope::ProvidersManage];
        prop_assert_eq!(grants_all_scopes(&session, &required), grant_first && grant_second);
    }
}

#[test]
fn server_scope_rules_caller_kind_and_relay_configure_mapping_match_ts() {
    assert_eq!(
        server_required_scopes_for_rpc(&WsMethod::ProjectsListEntries, None).expect("fs read"),
        vec![EnvironmentScope::FilesystemRead]
    );
    assert_eq!(
        server_required_scopes_for_rpc(&WsMethod::ServerRetryResourceTelemetry, None)
            .expect("retry telemetry"),
        vec![
            EnvironmentScope::EnvironmentMaintain,
            EnvironmentScope::DiagnosticsRead,
        ]
    );
    assert_eq!(
        server_required_scopes_for_rpc(
            &WsMethod::Custom("orchestration.v2.launchThread".to_owned()),
            Some(&json!({})),
        )
        .expect("v2 launchThread"),
        vec![EnvironmentScope::OrchestrationOperate]
    );
    assert_eq!(
        server_required_scopes_for_rpc(
            &WsMethod::OrchestrationDispatchCommand,
            Some(&json!({
                "type": "message.dispatch",
                "text": "/logout",
                "attachments": []
            })),
        )
        .expect("logout dispatch"),
        vec![
            EnvironmentScope::OrchestrationOperate,
            EnvironmentScope::ProvidersManage,
        ]
    );
    assert_eq!(
        server_required_scopes_for_rpc(
            &WsMethod::AssetsCreateUrl,
            Some(&json!({ "resource": { "_tag": "workspace-file", "path": "a.txt" } })),
        )
        .expect("workspace file asset"),
        vec![EnvironmentScope::FilesystemRead]
    );

    let relay_err = auth_scope_required_response(EnvironmentScope::RelayConfigure);
    assert_eq!(relay_err["_tag"], "EnvironmentAuthorizationError");
    assert_eq!(relay_err["requiredScope"], "relay:write");
    assert_eq!(relay_err["requiredPermission"], "relay:configure");

    for client_subj in [
        "one-time-token",
        "owner",
        "cloud-connect",
        "cli-issued-session",
    ] {
        assert_eq!(caller_kind_from_subject(client_subj), RpcCallerKind::Client);
    }
    for agent_subj in [
        "thread:thr-1",
        "THREAD:thr-2",
        "mcp-client",
        "MCP-CLIENT:client-1",
        "mcp-oauth",
        "MCP-OAUTH:scope-1",
        "peer-link",
        "peer-link:full-access",
        "PEER-LINK:read-only",
    ] {
        assert_eq!(
            caller_kind_from_subject(agent_subj),
            RpcCallerKind::AgentToken
        );
    }

    assert!(requires_human_client_caller(&WsMethod::Custom(
        "workflowBoard.approveSpec".to_owned()
    )));
    assert!(requires_human_client_caller(&WsMethod::Custom(
        "workflowBoard.ship".to_owned()
    )));
    assert!(!requires_human_client_caller(&WsMethod::Custom(
        "workflowBoard.getCard".to_owned()
    )));

    // Server evaluation sets permissions: Some(...) so legacy parent grants NEVER expand.
    let server_operate_only = SessionGrantInput {
        authenticated: true,
        scopes: None,
        permissions: Some(vec![EnvironmentScope::OrchestrationOperate]),
        server_update_scope: Some("environment:maintain".to_owned()),
    };
    assert!(!session_grants_scope(
        &server_operate_only,
        EnvironmentScope::SettingsWrite
    ));
}

#[test]
fn server_scope_rules_close_dynamic_alias_and_project_command_bypasses() {
    for resource in [
        json!({ "kind": "workspace-file", "path": "a.txt" }),
        json!({ "_tag": null, "kind": "workspace-file", "path": "a.txt" }),
        json!({ "_tag": 123, "kind": "workspace-file", "path": "a.txt" }),
        json!({ "_tag": "native-app-icon", "app": { "_tag": null, "kind": "app-path" } }),
        json!({ "_tag": "native-app-icon", "app": { "_tag": 123, "kind": "app-path" } }),
    ] {
        assert_eq!(
            server_required_scopes_for_rpc(
                &WsMethod::AssetsCreateUrl,
                Some(&json!({ "resource": resource })),
            )
            .expect("workspace/app asset requires filesystem:read"),
            vec![EnvironmentScope::FilesystemRead]
        );
    }
    // Bypass 4A: projects.mutate with action alias or omitted type + scripts/SecretReference.
    assert_eq!(
        server_required_scopes_for_rpc(
            &WsMethod::ProjectsMutate,
            Some(&json!({
                "action": "project.create",
                "workspaceRoot": "/repo",
                "scripts": [{ "id": "s1", "name": "dev", "command": "echo hi" }]
            })),
        )
        .expect("projects.mutate action + scripts"),
        vec![
            EnvironmentScope::OrchestrationOperate,
            EnvironmentScope::SettingsWrite,
        ]
    );
    assert_eq!(
        server_required_scopes_for_rpc(
            &WsMethod::ProjectsMutate,
            Some(&json!({
                "workspaceRoot": "/repo",
                "scripts": [{ "id": "s1", "env": { "provider": "env", "reference": "SECRET" } }]
            })),
        )
        .expect("projects.mutate omitted type + secret"),
        vec![
            EnvironmentScope::OrchestrationOperate,
            EnvironmentScope::SettingsWrite,
            EnvironmentScope::ProvidersManage,
        ]
    );
    // Bypass 4B: orchestration.v2.dispatchCommand ProjectWireCommand + scripts/SecretReference.
    assert_eq!(
        server_required_scopes_for_rpc(
            &WsMethod::Custom("orchestration.v2.dispatchCommand".to_owned()),
            Some(&json!({
                "type": "project.create",
                "title": "Demo",
                "workspaceRoot": "/repo",
                "scripts": [{ "id": "s1", "env": { "provider": "env", "reference": "SECRET" } }]
            })),
        )
        .expect("v2 dispatchCommand project.create + secret"),
        vec![
            EnvironmentScope::OrchestrationOperate,
            EnvironmentScope::SettingsWrite,
            EnvironmentScope::ProvidersManage,
        ]
    );
    assert_eq!(
        server_required_scopes_for_rpc(
            &WsMethod::OrchestrationDispatchCommand,
            Some(&json!({
                "type": "project.update",
                "projectId": "proj-1",
                "scripts": [{ "id": "s1", "command": "echo hi" }]
            })),
        )
        .expect("dispatchCommand project.update + scripts"),
        vec![
            EnvironmentScope::OrchestrationOperate,
            EnvironmentScope::SettingsWrite,
        ]
    );
    // Bypass 4C: thread.turn.start nested message.text = "/logout".
    assert_eq!(
        server_required_scopes_for_rpc(
            &WsMethod::OrchestrationDispatchCommand,
            Some(&json!({
                "type": "thread.turn.start",
                "commandId": "cmd-1",
                "threadId": "thr-1",
                "message": { "messageId": "m1", "text": " /LOGOUT ", "attachments": [] }
            })),
        )
        .expect("turn.start nested /logout"),
        vec![
            EnvironmentScope::OrchestrationOperate,
            EnvironmentScope::ProvidersManage,
        ]
    );
}
