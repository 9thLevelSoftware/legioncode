//! Submodules for E2E-M3 (Features F13–F21: Execute & Ship), split to stay <= 400 LOC/file.

mod support;
mod support_board;
mod support_board_ship;
mod support_m3_mocks;

mod f13_worktree_launch;
mod f14_task_execution;
mod f15_pause_resume_amend;
mod f16_planned_checks;
mod f17_independent_review;
mod f18_ship_gate;
mod f19_git_delivery;
mod f20_sqlite_write_journal;
mod f21_stage_jobs;
