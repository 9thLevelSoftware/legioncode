//! Task-contract replacement preserving runtime state (`completed`, `skipped`, timestamps, attempts)
//! for existing tasks unless explicitly overridden.

use std::collections::HashMap;

use rusqlite::{Connection, params};

use super::WorkflowBoardRepoError;
use super::invalidation::{
    TaskDigestFields, compute_aggregate_tasks_digest, compute_task_contract_digest,
};
use super::types_m2::TaskContractInput;

struct ExistingTaskRuntime {
    status: String,
    active_stage_run_id: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    attempt_count: i64,
    last_error: Option<String>,
    last_diff_digest: Option<String>,
    blocker_reason: Option<String>,
    created_at: String,
}

fn load_existing_task_runtimes(
    conn: &Connection,
    card_id: &str,
) -> Result<HashMap<String, ExistingTaskRuntime>, WorkflowBoardRepoError> {
    let mut stmt = conn
        .prepare(
            "SELECT task_id, status, active_stage_run_id, started_at, completed_at,
                    attempt_count, last_error, last_diff_digest, blocker_reason, created_at
             FROM workflow_board_tasks
             WHERE card_id = ?1",
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "load_prior_task_runtimes",
            source,
        })?;
    let rows = stmt
        .query_map(params![card_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                ExistingTaskRuntime {
                    status: r.get(1)?,
                    active_stage_run_id: r.get(2)?,
                    started_at: r.get(3)?,
                    completed_at: r.get(4)?,
                    attempt_count: r.get(5)?,
                    last_error: r.get(6)?,
                    last_diff_digest: r.get(7)?,
                    blocker_reason: r.get(8)?,
                    created_at: r.get(9)?,
                },
            ))
        })
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "query_prior_task_runtimes",
            source,
        })?;
    let mut map = HashMap::new();
    for row in rows {
        let (tid, rt) = row.map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "collect_prior_task_runtimes",
            source,
        })?;
        map.insert(tid, rt);
    }
    Ok(map)
}

pub(crate) fn replace_tasks(
    conn: &Connection,
    card_id: &str,
    tasks: &[TaskContractInput<'_>],
    now: &str,
) -> Result<String, WorkflowBoardRepoError> {
    let prior = load_existing_task_runtimes(conn, card_id)?;
    conn.execute(
        "DELETE FROM workflow_board_tasks WHERE card_id = ?1",
        params![card_id],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "delete_prior_tasks",
        source,
    })?;

    let mut contract_digests = Vec::with_capacity(tasks.len());
    for (idx, task) in tasks.iter().enumerate() {
        let digest = insert_one_task_contract(
            conn,
            card_id,
            idx,
            task,
            prior.get(task.task_id.trim()),
            now,
        )?;
        contract_digests.push(digest);
    }
    Ok(compute_aggregate_tasks_digest(
        contract_digests.iter().map(String::as_str),
    ))
}

struct SerializedTaskFields {
    blocked_by_json: String,
    blocks_json: String,
    allowed_json: String,
    forbidden_json: String,
    artifacts_json: String,
    cmds_json: String,
    contract_digest: String,
}

fn serialize_task_contract_fields(
    t: &TaskContractInput<'_>,
) -> Result<SerializedTaskFields, WorkflowBoardRepoError> {
    let tid = t.task_id.trim();
    let title = t.title.trim();
    let testing_method = t.testing_method.trim();
    let blocked_by_json =
        serde_json::to_string(t.depends_on).map_err(|source| WorkflowBoardRepoError::Json {
            operation: "serialize_task_depends_on",
            source,
        })?;
    let blocks_json =
        serde_json::to_string(t.blocks).map_err(|source| WorkflowBoardRepoError::Json {
            operation: "serialize_task_blocks",
            source,
        })?;
    let allowed_json =
        serde_json::to_string(t.files_allowed).map_err(|source| WorkflowBoardRepoError::Json {
            operation: "serialize_task_files_allowed",
            source,
        })?;
    let forbidden_json = serde_json::to_string(t.files_forbidden).map_err(|source| {
        WorkflowBoardRepoError::Json {
            operation: "serialize_task_files_forbidden",
            source,
        }
    })?;
    let artifacts_json = serde_json::to_string(t.expected_artifacts).map_err(|source| {
        WorkflowBoardRepoError::Json {
            operation: "serialize_task_artifacts",
            source,
        }
    })?;
    let cmds_json = serde_json::to_string(t.verification_commands).map_err(|source| {
        WorkflowBoardRepoError::Json {
            operation: "serialize_task_commands",
            source,
        }
    })?;
    let contract_digest = compute_task_contract_digest(&TaskDigestFields {
        task_id: tid,
        title,
        blocked_by_json: &blocked_by_json,
        blocks_json: &blocks_json,
        files_allowed_json: &allowed_json,
        files_forbidden_json: &forbidden_json,
        expected_artifacts_json: &artifacts_json,
        verification_commands_json: &cmds_json,
        testing_method,
    });
    Ok(SerializedTaskFields {
        blocked_by_json,
        blocks_json,
        allowed_json,
        forbidden_json,
        artifacts_json,
        cmds_json,
        contract_digest,
    })
}

struct ResolvedTaskRuntime<'a> {
    status: &'a str,
    active_stage_run_id: Option<&'a str>,
    started_at: Option<&'a str>,
    completed_at: Option<&'a str>,
    attempt_count: i64,
    last_error: Option<&'a str>,
    last_diff_digest: Option<&'a str>,
    blocker_reason: Option<&'a str>,
    created_at: &'a str,
}

fn resolve_task_runtime<'a>(
    tid: &str,
    requested_status: Option<&'a str>,
    prev: Option<&'a ExistingTaskRuntime>,
    now: &'a str,
) -> Result<ResolvedTaskRuntime<'a>, WorkflowBoardRepoError> {
    if let Some(s) = requested_status
        && !matches!(
            s,
            "pending" | "todo" | "in_progress" | "blocked" | "completed" | "failed" | "skipped"
        )
    {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "unsupported task status `{s}` on `{tid}`"
        )));
    }
    Ok(match (requested_status, prev) {
        (None, Some(p)) => ResolvedTaskRuntime {
            status: p.status.as_str(),
            active_stage_run_id: p.active_stage_run_id.as_deref(),
            started_at: p.started_at.as_deref(),
            completed_at: p.completed_at.as_deref(),
            attempt_count: p.attempt_count,
            last_error: p.last_error.as_deref(),
            last_diff_digest: p.last_diff_digest.as_deref(),
            blocker_reason: p.blocker_reason.as_deref(),
            created_at: p.created_at.as_str(),
        },
        (Some(s), Some(p)) if s == p.status => ResolvedTaskRuntime {
            status: s,
            active_stage_run_id: p.active_stage_run_id.as_deref(),
            started_at: p.started_at.as_deref(),
            completed_at: p.completed_at.as_deref(),
            attempt_count: p.attempt_count,
            last_error: p.last_error.as_deref(),
            last_diff_digest: p.last_diff_digest.as_deref(),
            blocker_reason: p.blocker_reason.as_deref(),
            created_at: p.created_at.as_str(),
        },
        (Some("todo"), Some(p)) => ResolvedTaskRuntime {
            status: "todo",
            active_stage_run_id: None,
            started_at: None,
            completed_at: None,
            attempt_count: p.attempt_count,
            last_error: None,
            last_diff_digest: None,
            blocker_reason: None,
            created_at: p.created_at.as_str(),
        },
        (Some(s), Some(p)) => ResolvedTaskRuntime {
            status: s,
            active_stage_run_id: None,
            started_at: p.started_at.as_deref(),
            completed_at: p.completed_at.as_deref(),
            attempt_count: p.attempt_count,
            last_error: None,
            last_diff_digest: None,
            blocker_reason: None,
            created_at: p.created_at.as_str(),
        },
        (Some(s), None) => ResolvedTaskRuntime {
            status: s,
            active_stage_run_id: None,
            started_at: None,
            completed_at: None,
            attempt_count: 0,
            last_error: None,
            last_diff_digest: None,
            blocker_reason: None,
            created_at: now,
        },
        (None, None) => ResolvedTaskRuntime {
            status: "todo",
            active_stage_run_id: None,
            started_at: None,
            completed_at: None,
            attempt_count: 0,
            last_error: None,
            last_diff_digest: None,
            blocker_reason: None,
            created_at: now,
        },
    })
}

fn insert_one_task_contract(
    conn: &Connection,
    card_id: &str,
    idx: usize,
    t: &TaskContractInput<'_>,
    prev: Option<&ExistingTaskRuntime>,
    now: &str,
) -> Result<String, WorkflowBoardRepoError> {
    let tid = t.task_id.trim();
    let title = t.title.trim();
    let testing_method = t.testing_method.trim();
    let ser = serialize_task_contract_fields(t)?;
    let ordinal = i64::try_from(idx).unwrap_or(i64::MAX);
    let requested_status = t.status.map(str::trim).filter(|s| !s.is_empty());
    let rt = resolve_task_runtime(tid, requested_status, prev, now)?;
    let testing_boundary = t
        .testing_limitations
        .map(str::trim)
        .filter(|s| !s.is_empty());
    conn.execute(
        "INSERT INTO workflow_board_tasks (
           card_id, task_id, ordinal, title, status, blocked_by_json, blocks_json,
           files_allowed_json, files_forbidden_json, expected_artifacts_json,
           verification_commands_json, testing_method, testing_boundary,
           contract_digest, active_stage_run_id, started_at, completed_at,
           attempt_count, last_error, last_diff_digest, blocker_reason,
           created_at, updated_at
         ) VALUES (
           ?1, ?2, ?3, ?4, ?5, ?6, ?7,
           ?8, ?9, ?10,
           ?11, ?12, ?13,
           ?14, ?15, ?16, ?17,
           ?18, ?19, ?20, ?21,
           ?22, ?23
         )",
        params![
            card_id,
            tid,
            ordinal,
            title,
            rt.status,
            ser.blocked_by_json,
            ser.blocks_json,
            ser.allowed_json,
            ser.forbidden_json,
            ser.artifacts_json,
            ser.cmds_json,
            testing_method,
            testing_boundary,
            ser.contract_digest,
            rt.active_stage_run_id,
            rt.started_at,
            rt.completed_at,
            rt.attempt_count,
            rt.last_error,
            rt.last_diff_digest,
            rt.blocker_reason,
            rt.created_at,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "insert_task_contract",
        source,
    })?;
    Ok(ser.contract_digest)
}
