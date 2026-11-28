//! Contract tests for `workflow_board`, `projectWorkflowBoard` capability, and WS permission parity.

use legion_contracts::environment::{
    ExecutionEnvironmentDescriptor, PlatformArch, PlatformInfo, PlatformOs,
};
use legion_contracts::ids::{
    EnvironmentId, ProjectId, WorkflowBoardApprovalId, WorkflowBoardStageRunId, WorkflowCardId,
};
use legion_contracts::orchestration::PatchValue;
use legion_contracts::permissions::{
    EnvironmentScope, WsMethod, required_scopes_for_rpc, requires_human_client_caller,
    server_required_scopes_for_rpc,
};
use legion_contracts::workflow_board::{
    ProjectWorkflowBoardSnapshot, WorkflowBoardApproveSpecInput, WorkflowBoardArtifact,
    WorkflowBoardArtifactKind, WorkflowBoardCreateCardInput, WorkflowBoardGetArtifactInput,
    WorkflowBoardRequestError, WorkflowBoardRequestErrorCode, WorkflowBoardStartStageInput,
    WorkflowBoardStartStageResult, WorkflowBoardUpdateSpecInput, WorkflowBoardUpdateSpecResult,
    WorkflowCardDetail, is_valid_artifact_key,
};
use proptest::prelude::*;
use serde_json::{Value, json};

use super::workflow_board_fixtures::{sample_detail, sample_summary};

#[test]
fn capability_enabled_by_default_and_decodes_booleans() {
    let env_id = EnvironmentId::new("env-1").expect("valid env id");
    let platform = PlatformInfo::new(PlatformOs::Linux, PlatformArch::X64);
    let desc = ExecutionEnvironmentDescriptor::builder(env_id, "Local", platform, "0.1.0").build();

    assert_eq!(desc.capabilities.project_workflow_board, Some(true));
    let wire = serde_json::to_value(&desc).expect("serialize descriptor");
    assert_eq!(
        wire.get("capabilities")
            .and_then(|caps| caps.get("projectWorkflowBoard")),
        Some(&json!(true)),
        "projectWorkflowBoard must be advertised as true once live RPC handlers exist"
    );

    let mut with_true = wire.clone();
    with_true["capabilities"]["projectWorkflowBoard"] = json!(true);
    let decoded_true: ExecutionEnvironmentDescriptor =
        serde_json::from_value(with_true).expect("decode true capability");
    assert_eq!(decoded_true.capabilities.project_workflow_board, Some(true));

    let mut with_false = wire;
    with_false["capabilities"]["projectWorkflowBoard"] = json!(false);
    let decoded_false: ExecutionEnvironmentDescriptor =
        serde_json::from_value(with_false).expect("decode false capability");
    assert_eq!(
        decoded_false.capabilities.project_workflow_board,
        Some(false)
    );
}

#[test]
fn snapshot_and_artifact_wire_parity_round_trip() {
    let summary = sample_summary();
    let snapshot = ProjectWorkflowBoardSnapshot {
        project_id: ProjectId::new("proj-1").expect("project id"),
        revision: 3,
        cards: vec![summary],
        generated_at: "2026-11-24T01:00:00Z".to_owned(),
    };
    let snapshot_json = serde_json::to_value(&snapshot).expect("serialize snapshot");
    assert_eq!(snapshot_json["cards"][0]["cardId"], "card-1");
    assert_eq!(snapshot_json["cards"][0]["stage"], "plan");
    assert!(snapshot_json["cards"][0].get("spec").is_none());

    let artifact = WorkflowBoardArtifact {
        card_id: WorkflowCardId::new("card-1").expect("card id"),
        key: "spec.md".to_owned(),
        kind: WorkflowBoardArtifactKind::Spec,
        relative_path: ".legion/workflows/card-1/spec.md".to_owned(),
        digest: "sha256:deadbeef".to_owned(),
        content: "# Spec\n".to_owned(),
        revision: 2,
        updated_at: "2026-11-24T00:30:00Z".to_owned(),
    };
    let artifact_json = serde_json::to_value(&artifact).expect("serialize artifact");
    let decoded_artifact: WorkflowBoardArtifact =
        serde_json::from_value(artifact_json).expect("deserialize artifact");
    assert_eq!(decoded_artifact, artifact);
}

#[test]
fn card_detail_wire_parity_round_trip() {
    let detail = sample_detail();

    let detail_json = serde_json::to_value(&detail).expect("serialize detail");
    let round_tripped: WorkflowCardDetail =
        serde_json::from_value(detail_json.clone()).expect("deserialize detail");
    assert_eq!(round_tripped, detail);
    assert_eq!(detail_json["cardId"], "card-1");
    assert_eq!(detail_json["spec"]["wireframesIndex"], Value::Null);
    assert_eq!(detail_json["wireframesIndex"], Value::Null);
}

#[test]
fn rpc_validators_reject_invalid_artifact_keys_and_revisions() {
    assert!(
        serde_json::from_value::<WorkflowBoardGetArtifactInput>(json!({ "cardId": "card-1" }))
            .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardGetArtifactInput>(
            json!({ "cardId": "card-1", "kind": "spec" })
        )
        .is_ok()
    );
    for bad_key in [
        "",
        "   ",
        "../../.env",
        "wireframes/../../secret.html",
        "/etc/passwd",
        "wireframes\\index.html",
        "C:spec.md",
        "spec\0.md",
        ".",
        "..",
        "wireframes//index.html",
    ] {
        assert!(!is_valid_artifact_key(bad_key));
        assert!(
            serde_json::from_value::<WorkflowBoardGetArtifactInput>(
                json!({ "cardId": "card-1", "key": bad_key })
            )
            .is_err()
        );
    }
    for (count, ok) in [(200, true), (256, true), (257, false)] {
        let key = "🚀".repeat(count);
        assert_eq!(is_valid_artifact_key(&key), ok);
        let parsed = serde_json::from_value::<WorkflowBoardGetArtifactInput>(
            json!({ "cardId": "card-1", "key": key }),
        );
        assert_eq!(parsed.is_ok(), ok);
    }
    for bad_rev in [json!(0), json!(-1), json!(1.5)] {
        assert!(
            serde_json::from_value::<WorkflowBoardUpdateSpecInput>(
                json!({ "cardId": "card-1", "expectedRevision": bad_rev })
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<WorkflowBoardApproveSpecInput>(
                json!({ "cardId": "card-1", "expectedRevision": bad_rev })
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<WorkflowBoardStartStageInput>(
                json!({ "cardId": "card-1", "expectedRevision": bad_rev, "stage": "plan" })
            )
            .is_err()
        );
    }

    let omitted: WorkflowBoardUpdateSpecInput =
        serde_json::from_value(json!({ "cardId": "card-1", "expectedRevision": 1 }))
            .expect("omitted");
    assert_eq!(omitted.wireframes_index, PatchValue::Absent);
}

#[test]
fn patch_value_and_tagged_error_round_trip() {
    let cleared: WorkflowBoardUpdateSpecInput = serde_json::from_value(json!({
        "cardId": "card-1",
        "expectedRevision": 1,
        "wireframesIndex": null,
    }))
    .expect("null");
    assert_eq!(cleared.wireframes_index, PatchValue::Null);

    let set_wf: WorkflowBoardUpdateSpecInput = serde_json::from_value(json!({
        "cardId": "card-1",
        "expectedRevision": 1,
        "wireframesIndex": "  wireframes/index.html  ",
    }))
    .expect("set");
    assert_eq!(
        set_wf.wireframes_index,
        PatchValue::Set("wireframes/index.html".to_owned())
    );

    assert!(
        serde_json::from_value::<WorkflowBoardUpdateSpecInput>(json!({
            "cardId": "card-1",
            "expectedRevision": 1,
            "wireframesIndex": "   ",
        }))
        .is_err()
    );

    let err = WorkflowBoardRequestError::new(
        WorkflowBoardRequestErrorCode::RevisionConflict,
        "Expected revision 1, current revision is 2",
    )
    .with_current_revision(2);
    let err_json = serde_json::to_value(&err).expect("serialize error");
    assert_eq!(
        err_json,
        json!({
            "_tag": "WorkflowBoardRequestError",
            "code": "revision_conflict",
            "message": "Expected revision 1, current revision is 2",
            "currentRevision": 2,
        })
    );
    let decoded_err: WorkflowBoardRequestError =
        serde_json::from_value(err_json).expect("deserialize error");
    assert_eq!(decoded_err, err);

    let created: WorkflowBoardCreateCardInput = serde_json::from_value(json!({
        "projectId": "proj-1",
        "title": "  Trimmed Title  ",
    }))
    .expect("createCard");
    assert_eq!(created.title, "Trimmed Title");

    let start_res_json = json!({
        "cardId": "card-1",
        "revision": 3,
        "stage": "plan",
        "stageRunId": "sr-1",
        "runId": "run-1",
        "threadId": "thr-1",
        "resumed": false,
    });
    let start_res: WorkflowBoardStartStageResult =
        serde_json::from_value(start_res_json.clone()).expect("startStageResult");
    assert_eq!(
        serde_json::to_value(&start_res).expect("serialize startStageResult"),
        start_res_json
    );

    let card_json = serde_json::to_value(sample_detail()).expect("card json");
    let valid_null: WorkflowBoardUpdateSpecResult = serde_json::from_value(json!({
        "cardId": "card-1", "revision": 2, "specDigest": null, "specApprovalStatus": null, "card": card_json,
    }))
    .expect("null specDigest");
    assert_eq!(valid_null.spec_digest, None);
    let valid_update_res: WorkflowBoardUpdateSpecResult = serde_json::from_value(json!({
        "cardId": "card-1", "revision": 2, "specDigest": "  sha256:abc  ", "specApprovalStatus": null, "card": card_json,
    }))
    .expect("valid updateSpecResult");
    assert_eq!(valid_update_res.spec_digest.as_deref(), Some("sha256:abc"));
    for blank_digest in ["", "   "] {
        assert!(
            serde_json::from_value::<WorkflowBoardUpdateSpecResult>(json!({
                "cardId": "card-1", "revision": 2, "specDigest": blank_digest, "specApprovalStatus": null, "card": card_json,
            }))
            .is_err()
        );
    }
}

#[test]
fn permissions_and_human_caller_parity_for_m1_workflow_board_methods() {
    let cases = [
        (
            "workflowBoard.subscribe",
            WsMethod::WorkflowBoardSubscribe,
            EnvironmentScope::OrchestrationRead,
            false,
        ),
        (
            "workflowBoard.getCard",
            WsMethod::WorkflowBoardGetCard,
            EnvironmentScope::OrchestrationRead,
            false,
        ),
        (
            "workflowBoard.getArtifact",
            WsMethod::WorkflowBoardGetArtifact,
            EnvironmentScope::OrchestrationRead,
            false,
        ),
        (
            "workflowBoard.createCard",
            WsMethod::WorkflowBoardCreateCard,
            EnvironmentScope::OrchestrationOperate,
            false,
        ),
        (
            "workflowBoard.updateSpec",
            WsMethod::WorkflowBoardUpdateSpec,
            EnvironmentScope::OrchestrationOperate,
            false,
        ),
        (
            "workflowBoard.approveSpec",
            WsMethod::WorkflowBoardApproveSpec,
            EnvironmentScope::OrchestrationOperate,
            true,
        ),
        (
            "workflowBoard.startStage",
            WsMethod::WorkflowBoardStartStage,
            EnvironmentScope::OrchestrationOperate,
            false,
        ),
    ];
    for (wire, expected_variant, scope, human_only) in cases {
        let parsed = WsMethod::parse(wire).expect("parse method");
        assert_eq!(parsed, expected_variant);
        assert_eq!(
            required_scopes_for_rpc(&parsed, None).expect("client"),
            vec![scope]
        );
        assert_eq!(
            server_required_scopes_for_rpc(&parsed, None).expect("server"),
            vec![scope]
        );
        assert_eq!(requires_human_client_caller(&parsed), human_only);
    }
}

proptest! {
    #[test]
    fn workflow_card_and_stage_run_ids_trim_and_reject_blanks(ws in "[ \t\n]{1,6}") {
        prop_assert!(WorkflowCardId::new(&ws).is_err());
        prop_assert!(WorkflowBoardStageRunId::new(&ws).is_err());
        prop_assert!(WorkflowBoardApprovalId::new(&ws).is_err());
    }

    #[test]
    fn artifact_key_rejects_traversal_and_control_prefixes(seg in "[a-zA-Z0-9_-]{1,12}") {
        let valid = format!("{seg}.md");
        let trav = format!("../{seg}.md");
        let abs = format!("/{seg}.md");
        let backslash = format!("dir\\{seg}.md");
        let drive = format!("C:{seg}.md");
        prop_assert!(is_valid_artifact_key(&valid));
        prop_assert!(!is_valid_artifact_key(&trav));
        prop_assert!(!is_valid_artifact_key(&abs));
        prop_assert!(!is_valid_artifact_key(&backslash));
        prop_assert!(!is_valid_artifact_key(&drive));
    }
}
