//! Cross-language optional-null parity tests for `WorkflowBoardProposeAmendmentInput` nested M2
//! models, `WorkflowBoardApprovalReceipt`, `WorkflowCardDetail`, and M1/M2 RPC inputs.

use legion_contracts::workflow_board::{
    WorkflowBoardApprovalReceipt, WorkflowBoardApproveSpecInput, WorkflowBoardCreateCardInput,
    WorkflowBoardGetArtifactInput, WorkflowBoardGetCardInput, WorkflowBoardProposeAmendmentInput,
    WorkflowBoardUpdateSpecInput, WorkflowCardDetail,
};
use serde_json::{Value, json};

use super::workflow_board_fixtures::sample_detail;

#[test]
fn nested_amendment_models_reject_explicit_null_on_non_nullable_optional_fields() {
    for (field, valid_val) in [
        ("id", json!("t-alias")),
        ("dependsOn", json!([])),
        ("blockedBy", json!([])),
        ("blocks", json!([])),
        ("filesForbidden", json!([])),
        ("expectedArtifacts", json!([])),
        ("verificationCommands", json!([])),
        ("status", json!("todo")),
    ] {
        let mut task = json!({
            "taskId": "TSK-1",
            "title": "Task",
            "filesAllowed": ["src/a.rs"],
            "testingMethod": "test-first",
        });
        assert!(task.get(field).is_none());
        task[field] = valid_val;
        assert!(
            serde_json::from_value::<WorkflowBoardProposeAmendmentInput>(json!({
                "cardId": "card-1", "expectedRevision": 1, "reason": "Amend", "tasks": [task],
            }))
            .is_ok(),
            "tasks[0].{field} valid value must succeed"
        );
        let mut bad_task = json!({
            "taskId": "TSK-1",
            "title": "Task",
            "filesAllowed": ["src/a.rs"],
            "testingMethod": "test-first",
        });
        bad_task[field] = Value::Null;
        assert!(
            serde_json::from_value::<WorkflowBoardProposeAmendmentInput>(json!({
                "cardId": "card-1", "expectedRevision": 1, "reason": "Amend", "tasks": [bad_task],
            }))
            .is_err(),
            "tasks[0].{field}: null must fail"
        );
    }

    assert!(
        serde_json::from_value::<WorkflowBoardProposeAmendmentInput>(json!({
            "cardId": "card-1",
            "expectedRevision": 1,
            "reason": "Amend",
            "plannedChecks": [{ "id": "chk-1", "criterionId": null, "argv": ["cargo", "test"] }],
        }))
        .is_err()
    );

    for (method, field) in [
        ("automated", "checkIds"),
        ("automated", "command"),
        ("manual", "procedure"),
        ("external", "externalRef"),
        ("external", "externalSource"),
    ] {
        let mut ac = json!({
            "criterionId": "AC-01",
            "method": method,
            "checkIds": ["chk-1"],
            "procedure": "Manual step",
            "externalRef": "ext-1",
            "externalSource": "ci",
        });
        ac[field] = Value::Null;
        assert!(
            serde_json::from_value::<WorkflowBoardProposeAmendmentInput>(json!({
                "cardId": "card-1", "expectedRevision": 1, "reason": "Amend", "acMappings": [ac],
            }))
            .is_err(),
            "acMappings[0].{field}: null must fail"
        );
    }
}

#[test]
fn m1_m2_rpc_and_detail_structs_reject_explicit_null_while_preserving_nullable_fields() {
    assert!(
        serde_json::from_value::<WorkflowBoardGetCardInput>(json!({
            "cardId": "card-1", "projectId": null
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardGetArtifactInput>(json!({
            "cardId": "card-1", "key": null, "kind": "spec"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardGetArtifactInput>(json!({
            "cardId": "card-1", "key": "spec.md", "kind": null
        }))
        .is_err()
    );

    for field in [
        "brief",
        "briefPath",
        "inputPaths",
        "intakeGoal",
        "wireframesEnabled",
        "sourceType",
        "assistanceMode",
        "workspaceStrategy",
        "modelSelection",
        "runtimeMode",
        "interactionMode",
    ] {
        let mut payload = json!({ "projectId": "proj-1", "title": "Card" });
        payload[field] = Value::Null;
        assert!(
            serde_json::from_value::<WorkflowBoardCreateCardInput>(payload).is_err(),
            "createCard.{field}: null must fail"
        );
    }

    for field in [
        "title",
        "brief",
        "intakeGoal",
        "inputPaths",
        "acceptanceCriteria",
        "constraints",
        "sourceRefs",
    ] {
        let mut payload = json!({ "cardId": "card-1", "expectedRevision": 1 });
        payload[field] = Value::Null;
        assert!(
            serde_json::from_value::<WorkflowBoardUpdateSpecInput>(payload).is_err(),
            "updateSpec.{field}: null must fail"
        );
    }

    assert!(
        serde_json::from_value::<WorkflowBoardApproveSpecInput>(json!({
            "cardId": "card-1", "expectedRevision": 1, "expectedDigest": null
        }))
        .is_err()
    );

    let mut detail_json = serde_json::to_value(sample_detail()).expect("detail");
    let mut receipt = detail_json["specApproval"].clone();
    receipt["acceptanceCriteriaIds"] = Value::Null;
    assert!(serde_json::from_value::<WorkflowBoardApprovalReceipt>(receipt).is_err());

    for field in ["maxChallengeConcerns", "alternativesRevisionCount"] {
        let mut bad = detail_json.clone();
        bad[field] = Value::Null;
        assert!(
            serde_json::from_value::<WorkflowCardDetail>(bad).is_err(),
            "detail.{field}: null must fail"
        );
    }

    // Nullable Schema.optional(Schema.NullOr(...)) fields remain nullable
    detail_json["wireframesIndex"] = Value::Null;
    detail_json["strategy"] = Value::Null;
    detail_json["strategyMetadata"] = Value::Null;
    detail_json["granularity"] = Value::Null;
    assert!(serde_json::from_value::<WorkflowCardDetail>(detail_json).is_ok());
}
