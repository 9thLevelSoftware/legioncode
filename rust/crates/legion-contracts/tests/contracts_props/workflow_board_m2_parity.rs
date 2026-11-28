//! Regression tests for M2.1 TS <-> Rust contract validation parity.

use legion_contracts::workflow_board::{
    WorkflowBoardCreateCardInput, WorkflowBoardTaskContract, WorkflowBoardUpdateSpecInput,
};
use serde_json::json;

#[test]
fn create_card_brief_path_rejects_credentials_traversal_and_null() {
    for bad in [
        ".env",
        ".env.local",
        "config/.env.production",
        "certs/server.pem",
        "keys/id_rsa",
        "keys/id_ed25519",
        ".ssh/config",
        ".npmrc",
        ".netrc",
        "tls/site.key",
        "tls/ca.crt",
        "../../etc/passwd",
        "/etc/passwd",
        ".git/config",
        ".GIT/HEAD",
        "sub/.git./foo",
        "GIT~1/config",
        "",
        "   ",
    ] {
        assert!(
            serde_json::from_value::<WorkflowBoardCreateCardInput>(json!({
                "projectId": "proj-1",
                "title": "Brief path guard",
                "briefPath": bad,
            }))
            .is_err(),
            "createCard.briefPath must reject {bad:?}"
        );
    }

    let valid: WorkflowBoardCreateCardInput = serde_json::from_value(json!({
        "projectId": "proj-1",
        "title": "Valid brief path",
        "briefPath": "  docs/brief.md  ",
    }))
    .expect("valid briefPath");
    assert_eq!(valid.brief_path.as_deref(), Some("docs/brief.md"));

    assert!(
        serde_json::from_value::<WorkflowBoardCreateCardInput>(json!({
            "projectId": "proj-1",
            "title": "Null brief path",
            "briefPath": null,
        }))
        .is_err(),
        "createCard.briefPath must reject explicit null"
    );

    let omitted_brief: WorkflowBoardCreateCardInput = serde_json::from_value(json!({
        "projectId": "proj-1",
        "title": "Omitted brief path",
    }))
    .expect("omitted briefPath");
    assert_eq!(omitted_brief.brief_path, None);
}

#[test]
fn standalone_task_contract_rejects_self_references() {
    for edge_field in ["dependsOn", "blockedBy", "blocks"] {
        for self_ref in ["T1", " T1 "] {
            assert!(
                serde_json::from_value::<WorkflowBoardTaskContract>(json!({
                    "taskId": "T1",
                    "title": "Self-reference check",
                    edge_field: [self_ref],
                    "filesAllowed": ["src/a.rs"],
                    "testingMethod": "test-first",
                }))
                .is_err(),
                "standalone WorkflowBoardTaskContract.{edge_field} must reject {self_ref:?}"
            );
        }
    }

    let valid: WorkflowBoardTaskContract = serde_json::from_value(json!({
        "taskId": "T1",
        "title": "Valid standalone task",
        "dependsOn": ["T0"],
        "blockedBy": ["T0"],
        "blocks": ["T2"],
        "filesAllowed": ["src/a.rs"],
        "testingMethod": "test-first",
    }))
    .expect("valid standalone task contract");
    assert_eq!(valid.task_id, "T1");
}

#[test]
fn wireframes_index_and_task_files_reject_git_metadata_and_accept_trimmed_keys() {
    for git_path in [
        ".git/index.html",
        ".GIT/HEAD",
        "sub/.git./index.html",
        "GIT~1/config",
        "  .git/hooks  ",
    ] {
        assert!(
            serde_json::from_value::<WorkflowBoardCreateCardInput>(json!({
                "projectId": "proj-1",
                "title": "Reject git wireframe",
                "wireframesIndex": git_path,
            }))
            .is_err(),
            "createCard.wireframesIndex must reject {git_path:?}"
        );
        assert!(
            serde_json::from_value::<WorkflowBoardUpdateSpecInput>(json!({
                "cardId": "card-1",
                "expectedRevision": 1,
                "wireframesIndex": git_path,
            }))
            .is_err(),
            "updateSpec.wireframesIndex must reject {git_path:?}"
        );
        assert!(
            serde_json::from_value::<WorkflowBoardTaskContract>(json!({
                "taskId": "T1",
                "title": "Reject git filesAllowed",
                "filesAllowed": [git_path],
                "testingMethod": "test-first",
            }))
            .is_err(),
            "task.filesAllowed must reject {git_path:?}"
        );
        assert!(
            serde_json::from_value::<WorkflowBoardTaskContract>(json!({
                "taskId": "T1",
                "title": "Reject git filesForbidden",
                "filesAllowed": ["src/a.rs"],
                "filesForbidden": [git_path],
                "testingMethod": "test-first",
            }))
            .is_err(),
            "task.filesForbidden must reject {git_path:?}"
        );
    }

    let trimmed_spec: WorkflowBoardUpdateSpecInput = serde_json::from_value(json!({
        "cardId": "card-1",
        "expectedRevision": 1,
        "wireframesIndex": "  wireframes/index.html  ",
    }))
    .expect("trimmed wireframesIndex");
    assert_eq!(
        trimmed_spec.wireframes_index,
        legion_contracts::orchestration::PatchValue::Set("wireframes/index.html".to_owned())
    );
}
