use proptest::prelude::*;
use serde_json::{Map, json};
use t3_contracts::permissions::{
    EmptyWsMethodError, EnvironmentScope, GuardEvalError, SessionGrantInput, WsMethod,
    contains_secret_reference, grants_all_scopes, required_scopes_for_project_mutation,
    required_scopes_for_rpc, required_scopes_for_rpc_json,
    required_scopes_for_server_settings_patch, session_grants_scope,
};

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
        json!({ "provider": "keychain", "reference": "t3/token" }),
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
