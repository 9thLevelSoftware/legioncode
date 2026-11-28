//! M4 `.legion-cli` import contract serialization, nullability, and validator parity tests.

use legion_contracts::workflow_board::{
    WorkflowBoardApplyCliImportInput, WorkflowBoardApplyCliImportResult,
    WorkflowBoardCliImportProvenance, WorkflowBoardPreviewCliImportInput,
    WorkflowBoardPreviewCliImportResult, WorkflowBoardShipBlockerKind,
    WorkflowBoardStartStageInput, WorkflowCardDetail,
};
use serde_json::{Value, json};

use super::workflow_board_fixtures::sample_detail;

fn sample_provenance_json() -> Value {
    json!({
        "sourceSystem": "legion-cli",
        "sourceCliDir": ".legion-cli",
        "sourceSpecId": "spec-t3-code",
        "owner": "legioncode",
        "importedAt": "2026-11-26T00:00:00Z",
        "lastSyncedAt": "2026-11-26T00:00:00Z",
        "sourceDigest": "sha256:cli-1",
        "decisionIds": ["D-001"],
        "assumptionIds": ["ASM-0001"],
        "acceptanceCriterionIds": ["AC-P0-01"],
        "historicalReceipts": [{
            "id": "hist-apr-spec-card-1",
            "kind": "spec_approval",
            "status": "historical",
            "sourcePath": ".legion-cli/workflow/spec-approval.yaml",
            "recordedAt": "2026-10-22T01:00:17.476Z",
            "recordedBy": "user",
            "summary": "Historical spec approval",
            "requiresNativeRevalidation": true
        }],
        "unsupportedPolicies": [{
            "key": "sandbox.requireHardened",
            "category": "sandbox",
            "summary": "Hardened sandbox policy requires acknowledgement",
            "configuredValue": "true",
            "status": "unsupported",
            "blocksExecution": true,
            "sourcePath": ".legion-cli/config.yaml"
        }],
        "acknowledgedPolicies": [],
        "missingLogNotes": [{
            "taskId": "TSK-0002",
            "checkId": "chk-tsk-0002",
            "relativePath": ".legion-cli/cache/runs/execute-mviwmzlq-24861406/verify-1.log",
            "command": "node --test packages/provider-mimo/qualification/cli.test.mjs",
            "note": "Missing gitignored cache log"
        }],
        "preservedArtifactPaths": [".legion-cli/specs/spec-t3-code/SPEC.md"],
        "assurancePolicySummary": null
    })
}

#[test]
fn m4_cli_import_contracts_roundtrip_and_reject_invalid_paths_and_nulls() {
    assert!(
        serde_json::from_value::<WorkflowBoardShipBlockerKind>(json!("policy_unacknowledged"))
            .is_ok()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardCliImportProvenance>(sample_provenance_json())
            .is_ok()
    );

    assert!(
        serde_json::from_value::<WorkflowBoardPreviewCliImportInput>(json!({
            "projectId": "proj-1",
            "cliDir": ".legion-cli"
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardPreviewCliImportInput>(json!({
            "projectId": "proj-1",
            "cliDir": "../.legion-cli"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardPreviewCliImportInput>(json!({
            "projectId": "proj-1",
            "cliDir": null
        }))
        .is_err()
    );

    assert!(
        serde_json::from_value::<WorkflowBoardApplyCliImportInput>(json!({
            "projectId": "proj-1",
            "acknowledgeUnsupportedPolicies": true,
            "acknowledgedPolicies": ["sandbox.requireHardened"]
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardApplyCliImportInput>(json!({
            "projectId": "proj-1",
            "acknowledgedPolicies": null
        }))
        .is_err()
    );

    assert!(
        serde_json::from_value::<WorkflowBoardStartStageInput>(json!({
            "cardId": "cli-spec-t3-code",
            "expectedRevision": 1,
            "stage": "execute",
            "acknowledgeUnsupportedPolicies": true,
            "acknowledgedPolicies": ["sandbox.requireHardened", "qa.passScore"]
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardStartStageInput>(json!({
            "cardId": "cli-spec-t3-code",
            "expectedRevision": 1,
            "stage": "execute",
            "acknowledgeUnsupportedPolicies": null
        }))
        .is_err()
    );
}

#[test]
fn m4_cli_import_card_detail_and_results_roundtrip() {
    let mut detail = sample_detail();
    detail.cli_import =
        Some(serde_json::from_value(sample_provenance_json()).expect("valid provenance"));
    let detail_json = serde_json::to_value(&detail).expect("serialize detail");
    assert!(detail_json.get("cliImport").is_some());
    assert!(serde_json::from_value::<WorkflowCardDetail>(detail_json.clone()).is_ok());

    let preview_ok = serde_json::from_value::<WorkflowBoardPreviewCliImportResult>(json!({
        "projectId": "proj-1",
        "sourcePath": ".legion-cli",
        "specId": "spec-t3-code",
        "title": "T3 Code",
        "stage": "execute",
        "status": "blocked",
        "existingCardId": null,
        "alreadyImported": false,
        "activeLocks": [],
        "staleLocks": [],
        "canApply": true,
        "refusalReason": null,
        "tasks": [],
        "decisionIds": ["D-001"],
        "assumptionIds": ["ASM-0001"],
        "acceptanceCriterionIds": ["AC-P0-01"],
        "historicalReceipts": [],
        "unsupportedPolicies": [],
        "missingLogNotes": [],
        "warnings": []
    }));
    assert!(preview_ok.is_ok());

    let apply_ok = serde_json::from_value::<WorkflowBoardApplyCliImportResult>(json!({
        "cardId": "cli-spec-t3-code",
        "projectId": "proj-1",
        "revision": 1,
        "created": true,
        "sourceSpecId": "spec-t3-code",
        "tasks": [],
        "historicalReceipts": [],
        "unsupportedPolicies": [],
        "missingLogNotes": [],
        "card": detail_json
    }));
    assert!(apply_ok.is_ok());
}
