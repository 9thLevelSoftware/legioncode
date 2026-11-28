//! M2 Project Workflow Board contracts, validators, proptests, and permissions parity tests.

use legion_contracts::permissions::{
    EnvironmentScope, WsMethod, required_scopes_for_rpc, requires_human_client_caller,
    server_required_scopes_for_rpc,
};
use legion_contracts::workflow_board::{
    WorkflowBoardAcEvidenceMapping, WorkflowBoardApprovePlanInput, WorkflowBoardApproveSpecInput,
    WorkflowBoardChallengeStepInput, WorkflowBoardCreateCardInput, WorkflowBoardExploreStepInput,
    WorkflowBoardReturnForRevisionInput, WorkflowBoardStrategyMetadata, WorkflowBoardTaskContract,
    WorkflowBoardUpdatePlanInput, WorkflowBoardUpdateSpecResult, WorkflowCardDetail,
    is_credential_file_path, is_valid_argv_command,
};
use proptest::prelude::*;
use serde_json::{Value, json};

use super::workflow_board_fixtures::sample_detail;

#[test]
fn m2_create_card_and_approve_spec_guards() {
    let valid: WorkflowBoardCreateCardInput = serde_json::from_value(json!({
        "projectId": "proj-1",
        "title": "M2 Intake",
        "brief": "Real brief",
        "inputPaths": ["README.md", "docs/spec.md"],
        "intakeGoal": "change",
        "wireframesEnabled": false,
    }))
    .expect("valid createCard");
    assert_eq!(valid.input_paths.as_ref().map(Vec::len), Some(2));

    assert!(
        serde_json::from_value::<WorkflowBoardCreateCardInput>(json!({
            "projectId": "proj-1",
            "title": "Blank brief",
            "brief": "   ",
        }))
        .is_err()
    );
    for cred in [".env", ".env.local", "certs/server.pem", "../../etc/passwd"] {
        assert!(
            serde_json::from_value::<WorkflowBoardCreateCardInput>(json!({
                "projectId": "proj-1",
                "title": "Cred reject",
                "inputPaths": [cred],
            }))
            .is_err()
        );
    }

    assert!(
        serde_json::from_value::<WorkflowBoardApproveSpecInput>(json!({
            "cardId": "card-1",
            "expectedRevision": 2,
            "expectedDigest": "sha256:abc",
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardApproveSpecInput>(json!({
            "cardId": "card-1",
            "expectedRevision": 2,
            "expectedDigest": "  ",
        }))
        .is_err()
    );

    let spec_res: WorkflowBoardUpdateSpecResult = serde_json::from_value(json!({
        "cardId": "card-1",
        "revision": 2,
        "specDigest": "sha256:abc",
        "specApprovalStatus": "stale",
        "card": serde_json::to_value(sample_detail()).expect("detail"),
    }))
    .expect("updateSpecResult");
    let serialized = serde_json::to_value(&spec_res).expect("serialize updateSpecResult");
    assert_eq!(serialized["wireframesIndex"], Value::Null);

    for field in ["wireframesDigest", "selectedAlternativeId"] {
        for blank in ["", "   "] {
            let mut bad = serde_json::to_value(sample_detail()).expect("detail");
            bad[field] = json!(blank);
            assert!(serde_json::from_value::<WorkflowCardDetail>(bad).is_err());
        }
        let mut null_val = serde_json::to_value(sample_detail()).expect("detail");
        null_val[field] = Value::Null;
        assert!(serde_json::from_value::<WorkflowCardDetail>(null_val).is_ok());
    }
}

#[test]
fn m2_explore_and_challenge_step_validators() {
    assert!(
        serde_json::from_value::<WorkflowBoardExploreStepInput>(json!({
            "cardId": "card-1",
            "expectedRevision": 1,
            "answers": [
                { "decisionId": "D1", "action": "edit", "answer": "Keep SQLite" },
                { "decisionId": "D2", "action": "pause" },
                { "decisionId": "D3", "choice": "recommend", "note": "Accepted" }
            ]
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardExploreStepInput>(json!({
            "cardId": "card-1",
            "expectedRevision": 1,
            "answers": [
                { "decisionId": "D1", "action": "explain" },
                { "decisionId": "D2", "action": "explain" },
                { "decisionId": "D3", "action": "explain" },
                { "decisionId": "D4", "action": "explain" }
            ]
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardExploreStepInput>(json!({
            "cardId": "card-1",
            "expectedRevision": 1,
            "answers": [{ "decisionId": "D1", "action": "edit" }]
        }))
        .is_err()
    );

    assert!(
        serde_json::from_value::<WorkflowBoardChallengeStepInput>(json!({
            "cardId": "card-1",
            "expectedRevision": 2,
            "action": "synthesize",
            "approved": true,
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardChallengeStepInput>(json!({
            "cardId": "card-1",
            "expectedRevision": 2,
            "action": "manualFallback",
            "acknowledgementToken": "I acknowledge",
            "unresolvedConcernIds": [],
            "answers": ["Measurable", "Rollback", "Compatible"],
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardChallengeStepInput>(json!({
            "cardId": "card-1",
            "expectedRevision": 2,
            "action": "manualFallback",
            "acknowledgementToken": "i acknowledge",
            "unresolvedConcernIds": [],
            "answers": ["a", "b", "c"],
        }))
        .is_err()
    );
}

#[test]
fn m2_update_plan_approve_plan_and_return_for_revision_validators() {
    let valid = serde_json::from_value::<WorkflowBoardUpdatePlanInput>(json!({
        "cardId": "card-1",
        "expectedRevision": 3,
        "lenses": [
            { "lens": "ux", "applicable": false, "reason": "No UI" },
            { "lens": "functional", "applicable": true, "summary": "Core RPCs" }
        ],
        "alternatives": [
            { "id": "alt-a", "title": "A", "summary": "Normalized", "tradeoffs": "Migrations" },
            { "id": "alt-b", "title": "B", "summary": "JSON blob", "tradeoffs": "Coarse updates" }
        ],
        "selectedAlternativeId": "alt-a",
        "selectionRationale": "Normalized tables",
        "strategy": "risk-first",
        "strategyMetadata": { "uncertainty": "WAL lock", "probeTaskId": "TSK-0001" },
        "granularity": "balanced",
        "tasks": [
            {
                "taskId": "TSK-0001",
                "title": "Probe WAL",
                "dependsOn": [],
                "filesAllowed": ["src/a.rs"],
                "filesForbidden": ["src/b.rs"],
                "testingMethod": "test-first"
            },
            {
                "taskId": "TSK-0002",
                "title": "Implement",
                "dependsOn": ["TSK-0001"],
                "filesAllowed": ["src/b.rs"],
                "filesForbidden": ["src/a.rs"],
                "testingMethod": "regression-first"
            }
        ],
        "acMappings": [
            { "criterionId": "AC-01", "method": "automated", "command": ["cargo", "test"] },
            { "criterionId": "AC-02", "method": "manual", "procedure": "Inspect board badge" }
        ]
    }));
    assert!(valid.is_ok());

    // Shell wrapper rejected
    assert!(
        serde_json::from_value::<WorkflowBoardUpdatePlanInput>(json!({
            "cardId": "card-1",
            "expectedRevision": 3,
            "acMappings": [
                { "criterionId": "AC-01", "method": "automated", "command": ["sh", "-c", "a && b"] }
            ]
        }))
        .is_err()
    );
    // Cyclic DAG rejected (dependsOn and blocks)
    for edge_field in ["dependsOn", "blocks"] {
        assert!(
            serde_json::from_value::<WorkflowBoardUpdatePlanInput>(json!({
                "cardId": "card-1",
                "expectedRevision": 3,
                "tasks": [
                    { "taskId": "T1", "title": "A", edge_field: ["T2"], "filesAllowed": ["a.rs"], "testingMethod": "test-first" },
                    { "taskId": "T2", "title": "B", edge_field: ["T1"], "filesAllowed": ["b.rs"], "testingMethod": "test-first" }
                ]
            }))
            .is_err()
        );
    }

    assert!(
        serde_json::from_value::<WorkflowBoardApprovePlanInput>(json!({
            "cardId": "card-1",
            "expectedRevision": 4,
            "expectedSpecDigest": "sha256:spec",
            "expectedPlanDigest": "sha256:plan",
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardReturnForRevisionInput>(json!({
            "cardId": "card-1",
            "expectedRevision": 4,
            "targetStage": "spec",
            "reason": "Clarify AC-02",
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardReturnForRevisionInput>(json!({
            "cardId": "card-1",
            "expectedRevision": 4,
            "targetStage": "ship",
            "reason": "Invalid",
        }))
        .is_err()
    );
}

#[test]
fn m2_permissions_and_human_caller_guards_match_ts_and_server() {
    let check = |wire: &str, expected_variant: WsMethod, human_only: bool| {
        let parsed = WsMethod::parse(wire).expect("parse M2 method");
        assert_eq!(parsed, expected_variant);
        assert_eq!(
            server_required_scopes_for_rpc(&parsed, None).expect("server scope"),
            vec![EnvironmentScope::OrchestrationOperate]
        );
        if !matches!(parsed, WsMethod::Custom(_)) {
            assert_eq!(
                required_scopes_for_rpc(&parsed, None).expect("client scope"),
                vec![EnvironmentScope::OrchestrationOperate]
            );
        }
        assert_eq!(requires_human_client_caller(&parsed), human_only);
    };
    check(
        "workflowBoard.exploreStep",
        WsMethod::WorkflowBoardExploreStep,
        false,
    );
    check(
        "workflowBoard.challengeStep",
        WsMethod::WorkflowBoardChallengeStep,
        false,
    );
    check(
        "workflowBoard.updatePlan",
        WsMethod::WorkflowBoardUpdatePlan,
        false,
    );
    check(
        "workflowBoard.approvePlan",
        WsMethod::WorkflowBoardApprovePlan,
        true,
    );
    check(
        "workflowBoard.returnForRevision",
        WsMethod::WorkflowBoardReturnForRevision,
        true,
    );
    check(
        "workflowBoard.approveSpec",
        WsMethod::WorkflowBoardApproveSpec,
        true,
    );
    check(
        "workflowBoard.approveDelivery",
        WsMethod::WorkflowBoardApproveDelivery,
        true,
    );
}

proptest! {
    #[test]
    fn argv_validator_rejects_shell_wrappers_and_control_tokens(cmd in "[a-zA-Z0-9_./-]{1,12}") {
        prop_assert!(is_valid_argv_command(&[cmd.clone(), "--version".to_owned()]));
        prop_assert!(!is_valid_argv_command(&["sh".to_owned(), "-c".to_owned(), cmd.clone()]));
        prop_assert!(!is_valid_argv_command(&["bash".to_owned(), "-lc".to_owned(), cmd.clone()]));
        prop_assert!(!is_valid_argv_command(&["cmd.exe".to_owned(), "/c".to_owned(), cmd.clone()]));
        let chained = format!("{cmd} && whoami");
        let pem = format!("config/{cmd}.pem");
        prop_assert!(!is_valid_argv_command(&[chained]));
        prop_assert!(is_credential_file_path(".env"));
        prop_assert!(is_credential_file_path(&pem));
    }
}

#[test]
fn m2_rejects_env_split_string_and_blank_string_array_elements() {
    for bad_env in [
        vec!["env", "-S", "sh -c id"],
        vec!["/usr/bin/env", "-iS", "sh -c id"],
        vec!["env", "--split-string=sh -c id"],
        vec!["env", "--split-string", "sh -c id"],
        vec!["env", "-u", "PATH", "sh", "-c", "id"],
        vec!["env", "-C", "/tmp", "bash", "-lc", "id"],
        vec!["env", "--unset", "FOO", "pwsh", "-Command", "id"],
        vec!["env", "env", "sh", "-c", "id"],
        vec!["env", "-u"],
    ] {
        let owned: Vec<String> = bad_env.into_iter().map(str::to_owned).collect();
        assert!(!is_valid_argv_command(&owned));
    }
    for good_env in [
        vec!["env", "FOO=1", "cargo", "test"],
        vec!["env", "--", "cargo", "test"],
        vec!["/usr/bin/env", "-i", "PATH=/usr/bin", "vp", "test", "run"],
    ] {
        let owned: Vec<String> = good_env.into_iter().map(str::to_owned).collect();
        assert!(is_valid_argv_command(&owned));
    }

    for blank in ["", "   "] {
        for field in ["dependsOn", "blockedBy", "blocks"] {
            assert!(
                serde_json::from_value::<WorkflowBoardTaskContract>(json!({
                    "taskId": "TSK-1",
                    "title": "Task",
                    field: [blank],
                    "filesAllowed": ["src/a.rs"],
                    "testingMethod": "test-first",
                }))
                .is_err(),
                "task.{field} must reject {blank:?}"
            );
        }
        assert!(
            serde_json::from_value::<WorkflowBoardAcEvidenceMapping>(json!({
                "criterionId": "AC-01",
                "method": "automated",
                "checkIds": [blank],
            }))
            .is_err()
        );
        for field in [
            "dependentTaskIds",
            "compatibilityTaskIds",
            "transitionTaskIds",
            "retirementTaskIds",
        ] {
            assert!(
                serde_json::from_value::<WorkflowBoardStrategyMetadata>(json!({
                    field: [blank],
                }))
                .is_err(),
                "strategyMetadata.{field} must reject {blank:?}"
            );
        }
        for field in ["unresolvedConcernIds", "answers"] {
            assert!(
                serde_json::from_value::<WorkflowBoardChallengeStepInput>(json!({
                    "cardId": "card-1",
                    "expectedRevision": 2,
                    "action": "resolve",
                    field: [blank],
                }))
                .is_err()
            );
        }
    }
}
