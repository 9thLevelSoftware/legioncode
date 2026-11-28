//! M3 4-way permission parity, `WsMethod` variants, and human-only caller guard tests.

use legion_contracts::permissions::{
    EnvironmentScope, RpcCallerKind, WsMethod, caller_kind_from_subject, required_scopes_for_rpc,
    requires_human_client_caller, server_required_scope_for_ws_method,
    server_required_scopes_for_rpc,
};

#[test]
fn m3_rpcs_have_3way_rust_scope_parity_and_human_only_guards() {
    let cases = [
        (
            "workflowBoard.getCardDiff",
            WsMethod::WorkflowBoardGetCardDiff,
            EnvironmentScope::OrchestrationRead,
            false,
        ),
        (
            "workflowBoard.startStage",
            WsMethod::WorkflowBoardStartStage,
            EnvironmentScope::OrchestrationOperate,
            false,
        ),
        (
            "workflowBoard.pauseCard",
            WsMethod::WorkflowBoardPauseCard,
            EnvironmentScope::OrchestrationOperate,
            false,
        ),
        (
            "workflowBoard.resumeCard",
            WsMethod::WorkflowBoardResumeCard,
            EnvironmentScope::OrchestrationOperate,
            false,
        ),
        (
            "workflowBoard.retryStep",
            WsMethod::WorkflowBoardRetryStep,
            EnvironmentScope::OrchestrationOperate,
            false,
        ),
        (
            "workflowBoard.proposeAmendment",
            WsMethod::WorkflowBoardProposeAmendment,
            EnvironmentScope::OrchestrationOperate,
            false,
        ),
        (
            "workflowBoard.resolveFinding",
            WsMethod::WorkflowBoardResolveFinding,
            EnvironmentScope::OrchestrationOperate,
            true,
        ),
        (
            "workflowBoard.recordManualAcceptance",
            WsMethod::WorkflowBoardRecordManualAcceptance,
            EnvironmentScope::OrchestrationOperate,
            true,
        ),
        (
            "workflowBoard.approveDelivery",
            WsMethod::WorkflowBoardApproveDelivery,
            EnvironmentScope::OrchestrationOperate,
            true,
        ),
    ];

    for (wire, variant, expected_scope, human_only) in cases {
        let parsed = WsMethod::parse(wire).expect("parse M3 RPC method");
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

    for human_method in [
        "workflowBoard.approveSpec",
        "workflowBoard.approvePlan",
        "workflowBoard.returnForRevision",
        "workflowBoard.ship",
    ] {
        let parsed = WsMethod::parse(human_method).expect("parse human method");
        assert!(requires_human_client_caller(&parsed));
    }
}

#[test]
fn caller_kind_classifies_agent_and_client_subjects_strictly() {
    for agent_subj in [
        "thread:thr-1",
        "agent",
        "agent:sub-1",
        "mcp-client",
        "mcp-client:run-1",
        "mcp-oauth",
        "mcp-oauth:sess-1",
        "mcp-approval:apr-1",
        "peer-link",
        "peer-link:full-access",
    ] {
        assert_eq!(
            caller_kind_from_subject(agent_subj),
            RpcCallerKind::AgentToken,
            "expected AgentToken for {agent_subj}"
        );
    }

    for client_subj in [
        "local",
        "owner",
        "one-time-token",
        "cli-issued-session",
        "cloud-connect",
        "administrative-bootstrap",
        "reusable-dev-token-child",
        "paired-admin",
    ] {
        assert_eq!(
            caller_kind_from_subject(client_subj),
            RpcCallerKind::Client,
            "expected Client for {client_subj}"
        );
    }
}
