//! M4 4-way permission parity, `WsMethod` variants, and human-only caller guard tests.

use legion_contracts::permissions::{
    EnvironmentScope, WsMethod, required_scopes_for_rpc, requires_human_client_caller,
    server_required_scope_for_ws_method, server_required_scopes_for_rpc,
};

#[test]
fn m4_cli_import_rpcs_have_rust_scope_parity_and_human_only_guard() {
    let cases = [
        (
            "workflowBoard.previewCliImport",
            WsMethod::WorkflowBoardPreviewCliImport,
            EnvironmentScope::OrchestrationOperate,
            false,
        ),
        (
            "workflowBoard.applyCliImport",
            WsMethod::WorkflowBoardApplyCliImport,
            EnvironmentScope::OrchestrationOperate,
            true,
        ),
    ];

    for (wire, variant, expected_scope, human_only) in cases {
        let parsed = WsMethod::parse(wire).expect("parse M4 RPC method");
        assert_eq!(parsed, variant);
        assert_eq!(parsed.as_str(), wire);
        assert_eq!(
            required_scopes_for_rpc(&parsed, None).expect("client scope"),
            vec![expected_scope]
        );
        assert_eq!(
            server_required_scopes_for_rpc(&parsed, None).expect("server scope"),
            vec![expected_scope]
        );
        assert_eq!(
            server_required_scope_for_ws_method(&parsed),
            Some(expected_scope)
        );
        assert_eq!(requires_human_client_caller(&parsed), human_only);
    }
}
