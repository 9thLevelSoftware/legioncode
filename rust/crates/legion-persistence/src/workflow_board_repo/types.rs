//! Persisted row and input/outcome DTOs for [`crate::workflow_board_repo`].

/// Full row representation from `workflow_board_cards`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardRow {
    pub card_id: String,
    pub project_id: String,
    pub title: String,
    pub stage: String,
    pub status: String,
    pub revision: i64,
    pub brief: String,
    pub intake_goal: Option<String>,
    pub brief_path: Option<String>,
    pub brief_digest: Option<String>,
    pub input_paths_json: String,
    pub source_type: String,
    pub assistance_mode: String,
    pub workspace_strategy_json: String,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub model_selection_json: Option<String>,
    pub runtime_mode: Option<String>,
    pub interaction_mode: Option<String>,
    pub spec_digest: Option<String>,
    pub wireframes_digest: Option<String>,
    pub design_digest: Option<String>,
    pub plan_digest: Option<String>,
    pub tasks_digest: Option<String>,
    pub current_task_id: Option<String>,
    pub blocker_reason: Option<String>,
    pub next_action: Option<String>,
    pub spec_json: String,
    pub plan_json: Option<String>,
    pub import_provenance_json: Option<String>,
    pub metadata_json: String,
    pub product_snapshot_digest: Option<String>,
    pub paused_from_status: Option<String>,
    pub pause_reason: Option<String>,
    pub paused_at: Option<String>,
    pub external_edit_state: Option<String>,
    pub external_edit_detail_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub archived_at: Option<String>,
}

/// Compact summary row used for project board subscriptions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardSummary {
    pub card_id: String,
    pub project_id: String,
    pub title: String,
    pub stage: String,
    pub status: String,
    pub revision: i64,
    pub current_task_id: Option<String>,
    pub blocker_reason: Option<String>,
    pub next_action: Option<String>,
    pub spec_approved_revision: Option<i64>,
    pub spec_approval_status: Option<String>,
    pub plan_approved_revision: Option<i64>,
    pub plan_approval_status: Option<String>,
    pub active_thread_id: Option<String>,
    pub incomplete_journal_count: i64,
    pub external_edit_state: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Project board snapshot aggregate returned by [`super::get_board_snapshot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardSnapshot {
    pub project_id: String,
    pub revision: i64,
    pub cards: Vec<CardSummary>,
    pub generated_at: String,
}

/// Row representation from `workflow_board_revisions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionRow {
    pub id: String,
    pub card_id: String,
    pub revision: i64,
    pub stage: String,
    pub kind: String,
    pub actor_kind: String,
    pub summary: String,
    pub spec_digest: Option<String>,
    pub plan_digest: Option<String>,
    pub payload_json: String,
    pub created_at: String,
}

/// Row representation from `workflow_board_approvals`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRow {
    pub id: String,
    pub card_id: String,
    pub gate: String,
    pub status: String,
    pub revision: i64,
    pub artifact_digest: String,
    pub spec_digest: Option<String>,
    pub plan_digest: Option<String>,
    pub tasks_digest: Option<String>,
    pub wireframes_digest: Option<String>,
    pub product_snapshot_digest: Option<String>,
    pub approved_by: String,
    pub approved_at: String,
    pub invalidated_at: Option<String>,
    pub invalidation_reason: Option<String>,
    pub metadata_json: String,
}

/// Row representation from `workflow_board_stage_runs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageRunRow {
    pub id: String,
    pub card_id: String,
    pub stage: String,
    pub task_id: Option<String>,
    pub attempt: i64,
    pub stable_launch_key: String,
    pub status: String,
    pub card_revision: i64,
    pub provider_instance_id: String,
    pub model: String,
    pub model_options_json: Option<String>,
    pub runtime_mode: String,
    pub interaction_mode: String,
    pub workspace_strategy_json: String,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub thread_id: Option<String>,
    pub workflow_run_id: Option<String>,
    pub brief_digest: Option<String>,
    pub result_json: Option<String>,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub ended_at: Option<String>,
}

/// Row representation from `workflow_board_write_journal`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteJournalRow {
    pub id: String,
    pub card_id: String,
    pub target_path: String,
    pub staging_path: Option<String>,
    pub operation: String,
    pub status: String,
    pub expected_digest: Option<String>,
    pub previous_digest: Option<String>,
    pub revision: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

/// Full hydrated card aggregate returned by [`super::create_card`], [`super::get_card_detail`],
/// [`super::update_spec`], and [`super::approve_spec`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardDetail {
    pub card: CardRow,
    pub revisions: Vec<RevisionRow>,
    pub approvals: Vec<ApprovalRow>,
    pub stage_runs: Vec<StageRunRow>,
    pub incomplete_journal_count: i64,
}

/// Input parameters for [`super::create_card`].
#[derive(Debug, Clone)]
pub struct CreateCardInput<'a> {
    pub card_id: &'a str,
    pub revision_id: &'a str,
    pub project_id: &'a str,
    pub title: &'a str,
    pub brief: &'a str,
    pub intake_goal: Option<&'a str>,
    pub brief_path: Option<&'a str>,
    pub brief_digest: Option<&'a str>,
    pub input_paths_json: Option<&'a str>,
    pub source_type: &'a str,
    pub assistance_mode: &'a str,
    pub workspace_strategy_json: &'a str,
    pub worktree_path: Option<&'a str>,
    pub branch: Option<&'a str>,
    pub model_selection_json: Option<&'a str>,
    pub runtime_mode: Option<&'a str>,
    pub interaction_mode: Option<&'a str>,
    pub spec_json: &'a str,
    pub spec_digest: Option<&'a str>,
    pub wireframes_digest: Option<&'a str>,
}

/// Input parameters for [`super::update_spec`].
#[derive(Debug, Clone)]
pub struct UpdateSpecInput<'a> {
    pub card_id: &'a str,
    pub revision_id: &'a str,
    pub expected_revision: i64,
    pub title: Option<&'a str>,
    pub brief: Option<&'a str>,
    pub intake_goal: Option<&'a str>,
    pub input_paths_json: Option<&'a str>,
    pub spec_json: Option<&'a str>,
    pub spec_digest: Option<&'a str>,
    pub wireframes_digest: Option<&'a str>,
    pub clear_wireframes_digest: bool,
}

/// Input parameters for [`super::approve_spec`].
#[derive(Debug, Clone)]
pub struct ApproveSpecInput<'a> {
    pub card_id: &'a str,
    pub revision_id: &'a str,
    pub approval_id: &'a str,
    pub expected_revision: i64,
    pub expected_spec_digest: Option<&'a str>,
    pub wireframes_digest: Option<&'a str>,
    pub approved_by: &'a str,
}

/// Input parameters for [`super::upsert_or_get_stage_run_by_launch_key`].
#[derive(Debug, Clone)]
pub struct UpsertStageRunInput<'a> {
    pub id: &'a str,
    pub card_id: &'a str,
    pub stage: &'a str,
    pub task_id: Option<&'a str>,
    pub attempt: i64,
    pub stable_launch_key: &'a str,
    pub card_revision: i64,
    pub provider_instance_id: &'a str,
    pub model: &'a str,
    pub model_options_json: Option<&'a str>,
    pub runtime_mode: &'a str,
    pub interaction_mode: &'a str,
    pub workspace_strategy_json: &'a str,
    pub worktree_path: Option<&'a str>,
    pub branch: Option<&'a str>,
    pub brief_digest: Option<&'a str>,
}

/// Outcome of [`super::upsert_or_get_stage_run_by_launch_key`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpsertStageRunOutcome {
    /// Newly inserted stage run row.
    Created(StageRunRow),
    /// Existing stage run row matched by `stable_launch_key`.
    Existing(StageRunRow),
}

/// Input parameters for [`super::link_stage_run_thread`].
#[derive(Debug, Clone)]
pub struct LinkStageRunThreadInput<'a> {
    pub id: &'a str,
    pub status: &'a str,
    pub thread_id: Option<&'a str>,
    pub workflow_run_id: Option<&'a str>,
    pub worktree_path: Option<&'a str>,
    pub branch: Option<&'a str>,
    pub last_error: Option<&'a str>,
    pub ended_at: Option<&'a str>,
}

/// Input parameters for [`super::stage_write`].
#[derive(Debug, Clone)]
pub struct StageWriteInput<'a> {
    pub id: &'a str,
    pub card_id: &'a str,
    pub target_path: &'a str,
    pub staging_path: Option<&'a str>,
    pub operation: &'a str,
    pub expected_digest: Option<&'a str>,
    pub previous_digest: Option<&'a str>,
    pub revision: Option<i64>,
}
