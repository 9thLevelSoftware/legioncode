#![forbid(unsafe_code)]
//! E2E-M2 Rust integration test entry for Features F1-F12 (Board Navigation, Spec & Plan).

#[expect(
    dead_code,
    reason = "shared E2E harness includes M3/M4 fixtures unused in E2E-M2 planning tests"
)]
#[path = "workflow_board_e2e/mod.rs"]
mod workflow_board_e2e;

#[path = "workflow_board_e2e_planning/mod.rs"]
mod planning;

use workflow_board_e2e::TestResult;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uncond_r01_unsupported_rpc_preserves_ws_and_workflows_subscribe() -> TestResult<()> {
    planning::board_nav_tests::uncond_r01_unsupported_rpc_preserves_ws_and_workflows_subscribe()
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uncond_r02_orchestration_read_token_rejected_on_workflow_board_create_card()
-> TestResult<()> {
    planning::board_nav_tests::uncond_r02_orchestration_read_token_rejected_on_workflow_board_create_card()
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uncond_r03_scoped_read_snapshot_vs_dispatch_command_guard() -> TestResult<()> {
    planning::board_nav_tests::uncond_r03_scoped_read_snapshot_vs_dispatch_command_guard().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f1_t2_05_board_overview_survives_restart() -> TestResult<()> {
    planning::board_nav_tests::f1_t2_05_board_overview_survives_restart().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f2_t1_05_card_retains_identity_and_history_across_attempts() -> TestResult<()> {
    planning::board_nav_tests::f2_t1_05_card_retains_identity_and_history_across_attempts().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f2_t2_05_large_artifact_chunked_over_frame_parts() -> TestResult<()> {
    planning::board_nav_tests::f2_t2_05_large_artifact_chunked_over_frame_parts().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f3_t1_05_revision_return_history_survives_restart() -> TestResult<()> {
    planning::board_nav_tests::f3_t1_05_revision_return_history_survives_restart().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f4_t1_05_deliverables_written_without_autocommit() -> TestResult<()> {
    planning::spec_tests::f4_t1_05_deliverables_written_without_autocommit().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f4_t2_02_brief_injection_and_shell_treated_as_inert() -> TestResult<()> {
    planning::spec_tests::f4_t2_02_brief_injection_and_shell_treated_as_inert().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f4_t2_03_input_traversal_and_credential_files_rejected() -> TestResult<()> {
    planning::spec_tests::f4_t2_03_input_traversal_and_credential_files_rejected().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f5_t1_05_paused_exploration_survives_restart() -> TestResult<()> {
    planning::spec_tests::f5_t1_05_paused_exploration_survives_restart().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f6_t1_05_wireframes_saved_uncommitted_and_digest_bound() -> TestResult<()> {
    planning::spec_tests::f6_t1_05_wireframes_saved_uncommitted_and_digest_bound().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f6_t2_04_disabling_wireframes_nullifies_index_cleanly() -> TestResult<()> {
    planning::spec_tests::f6_t2_04_disabling_wireframes_nullifies_index_cleanly().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f7_t2_04_external_spec_edit_invalidates_challenge_binding() -> TestResult<()> {
    planning::plan_and_journal_tests::f7_t2_04_external_spec_edit_invalidates_challenge_binding()
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f8_t1_04_spec_approval_and_files_survive_restart() -> TestResult<()> {
    planning::plan_and_journal_tests::f8_t1_04_spec_approval_and_files_survive_restart().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f8_t1_05_write_journal_and_disk_sha256_consistency() -> TestResult<()> {
    planning::plan_and_journal_tests::f8_t1_05_write_journal_and_disk_sha256_consistency().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f8_t2_04_external_disk_edit_invalidates_spec_and_plan_approvals() -> TestResult<()> {
    planning::plan_and_journal_tests::f8_t2_04_external_disk_edit_invalidates_spec_and_plan_approvals()
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f9_t1_04_six_lenses_and_rationales_survive_restart() -> TestResult<()> {
    planning::plan_and_journal_tests::f9_t1_04_six_lenses_and_rationales_survive_restart().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f10_t1_04_comparison_and_selection_survive_restart() -> TestResult<()> {
    planning::plan_and_journal_tests::f10_t1_04_comparison_and_selection_survive_restart().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f11_t1_04_task_contracts_written_to_disk_uncommitted() -> TestResult<()> {
    planning::plan_and_journal_tests::f11_t1_04_task_contracts_written_to_disk_uncommitted().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f12_t1_04_plan_approval_and_deliverables_survive_restart() -> TestResult<()> {
    planning::plan_and_journal_tests::f12_t1_04_plan_approval_and_deliverables_survive_restart()
        .await
}
