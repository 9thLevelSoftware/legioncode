//! Digest computation helpers for task contracts, aggregate tasks, and SHA-256 prefixes.

use std::fmt::Write as _;

use legion_contracts::orchestration::sha256_digest;
use serde_json::json;

pub(crate) struct TaskDigestFields<'a> {
    pub task_id: &'a str,
    pub title: &'a str,
    pub blocked_by_json: &'a str,
    pub blocks_json: &'a str,
    pub files_allowed_json: &'a str,
    pub files_forbidden_json: &'a str,
    pub expected_artifacts_json: &'a str,
    pub verification_commands_json: &'a str,
    pub testing_method: &'a str,
}

pub(crate) fn compute_task_contract_digest(fields: &TaskDigestFields<'_>) -> String {
    let payload = json!({
        "taskId": fields.task_id,
        "title": fields.title,
        "dependsOn": fields.blocked_by_json,
        "blocks": fields.blocks_json,
        "filesAllowed": fields.files_allowed_json,
        "filesForbidden": fields.files_forbidden_json,
        "expectedArtifacts": fields.expected_artifacts_json,
        "verificationCommands": fields.verification_commands_json,
        "testingMethod": fields.testing_method,
    });
    compute_prefixed_sha256(payload.to_string().as_bytes())
}

pub(crate) fn compute_aggregate_tasks_digest<'a>(
    contract_digests: impl IntoIterator<Item = &'a str>,
) -> String {
    let mut buf = String::new();
    for digest in contract_digests {
        if !buf.is_empty() {
            buf.push('\n');
        }
        buf.push_str(digest);
    }
    compute_prefixed_sha256(buf.as_bytes())
}

pub(crate) fn compute_prefixed_sha256(bytes: &[u8]) -> String {
    let digest = sha256_digest(bytes);
    let mut out = String::with_capacity("sha256:".len() + digest.len() * 2);
    out.push_str("sha256:");
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
