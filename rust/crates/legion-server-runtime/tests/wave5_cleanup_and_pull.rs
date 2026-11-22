//! Wave 5 startup auto-pull fetch await (`#39915`) and storage cleanup dangling gitlinks &
//! fork-merged PR cleanup tests (`#40119`, `8e2949e0ef`, `1de7eaecec`).

use std::path::{Path, PathBuf};
use std::process::Command;

use legion_server_runtime::{
    AutoPullOutcome, AutoPullProjectSpec, CleanupGitStatusRunner, ForkPullRequestRef,
    GitStatusFetchOptions, StatusFetchCoordinator, WorktreeCleanupCandidate, auto_pull_projects,
    contains_non_dangling_nested_git_repo, evaluate_and_clean_worktrees,
    find_merged_pr_in_head_repository, is_dangling_gitfile,
};
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn run_git_ok(dir: Option<&Path>, args: &[&str]) -> TestResult {
    let mut cmd = Command::new("git");
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    assert!(cmd.args(args).output()?.status.success());
    Ok(())
}

fn setup_cloned_upstream_fixture(dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let remote = dir.join("remote.git");
    let seed = dir.join("seed");
    let work = dir.join("work");
    let remote_s = remote.to_string_lossy().into_owned();
    let seed_s = seed.to_string_lossy().into_owned();
    let work_s = work.to_string_lossy().into_owned();

    run_git_ok(None, &["init", "--bare", &remote_s])?;
    run_git_ok(
        None,
        &["-c", "core.autocrlf=false", "clone", &remote_s, &seed_s],
    )?;
    run_git_cfg(&seed)?;
    std::fs::write(seed.join("README.md"), "v1\n")?;
    run_git_ok(Some(&seed), &["add", "."])?;
    run_git_ok(Some(&seed), &["commit", "-m", "v1"])?;
    run_git_ok(Some(&seed), &["push"])?;
    run_git_ok(
        None,
        &["-c", "core.autocrlf=false", "clone", &remote_s, &work_s],
    )?;
    run_git_cfg(&work)?;
    std::fs::write(seed.join("README.md"), "v2\n")?;
    run_git_ok(Some(&seed), &["add", "."])?;
    run_git_ok(Some(&seed), &["commit", "-m", "v2"])?;
    run_git_ok(Some(&seed), &["push"])?;
    Ok(work)
}

#[tokio::test]
async fn startup_auto_pull_awaits_slow_fetch_before_reading_status() -> TestResult {
    let dir = TempDir::new()?;
    let work = setup_cloned_upstream_fixture(dir.path())?;

    let coord = StatusFetchCoordinator::new();
    let completed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let completed_clone = std::sync::Arc::clone(&completed);
    let _fetch_job = coord
        .start_fetch(&work, "origin", async move {
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            completed_clone.store(true, std::sync::atomic::Ordering::Release);
        })
        .await;

    let short_waited = coord
        .wait_for_pending_fetch(
            &work,
            "origin",
            GitStatusFetchOptions {
                await_status_fetch: false,
            },
            std::time::Duration::from_millis(5),
        )
        .await;
    assert!(!short_waited);

    let outcomes = auto_pull_projects(
        &[AutoPullProjectSpec {
            project_id: "p-behind".to_owned(),
            workspace_root: work.clone(),
            auto_pull: true,
        }],
        true,
        &coord,
    )
    .await;
    assert!(completed.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(outcomes[0].1, AutoPullOutcome::FastForwarded);
    assert_eq!(std::fs::read_to_string(work.join("README.md"))?, "v2\n");
    Ok(())
}

struct CountingStatusRunner {
    calls: usize,
}

impl CleanupGitStatusRunner for CountingStatusRunner {
    fn has_local_changes(&mut self, _path: &Path) -> Result<bool, String> {
        self.calls += 1;
        Ok(false)
    }
}

#[test]
fn storage_cleanup_removes_dangling_gitlinks_checks_fork_prs_and_skips_kept_status() -> TestResult {
    let dir = TempDir::new()?;
    let wt_dangling = dir.path().join("wt-dangling");
    std::fs::create_dir_all(wt_dangling.join("dist/suite"))?;
    std::fs::write(
        wt_dangling.join("dist/suite/.git"),
        "gitdir: ../.git/modules/x\n",
    )?;
    assert!(is_dangling_gitfile(&wt_dangling.join("dist/suite"), ".git"));
    assert!(!contains_non_dangling_nested_git_repo(&wt_dangling));

    let wt_trailing_space = dir.path().join("wt-space");
    std::fs::create_dir_all(wt_trailing_space.join("dist/suite/git "))?;
    std::fs::write(wt_trailing_space.join("dist/suite/.git"), "gitdir: git \n")?;
    assert!(!is_dangling_gitfile(
        &wt_trailing_space.join("dist/suite"),
        ".git"
    ));
    assert!(contains_non_dangling_nested_git_repo(&wt_trailing_space));

    let unc_dir = dir.path().join("wt-unc");
    std::fs::create_dir_all(&unc_dir)?;
    std::fs::write(
        unc_dir.join(".git"),
        "gitdir: \\\\fileserver\\share\\repo\n",
    )?;
    assert!(!is_dangling_gitfile(&unc_dir, ".git"));

    let wt_kept = dir.path().join("wt-kept-active");
    std::fs::create_dir_all(&wt_kept)?;

    let fork_candidate = WorktreeCleanupCandidate {
        path: wt_dangling.clone(),
        in_use_by_thread: false,
        branch: Some("feature/fork-merged".to_owned()),
        merged_upstream: false,
        head_repository_name_with_owner: Some("contributor/codething-mvp".to_owned()),
        fork_pull_requests: vec![ForkPullRequestRef {
            number: 12,
            state: "merged".to_owned(),
            repository_name_with_owner: "contributor/codething-mvp".to_owned(),
            head_branch: "feature/fork-merged".to_owned(),
            updated_at: "2026-11-20T00:00:00Z".to_owned(),
        }],
        worktree_on_merge: true,
    };
    assert!(find_merged_pr_in_head_repository(&fork_candidate, true).is_some());

    let mut runner = CountingStatusRunner { calls: 0 };
    let outcomes = evaluate_and_clean_worktrees(
        &[
            WorktreeCleanupCandidate {
                path: wt_kept,
                in_use_by_thread: true,
                branch: Some("feat/active".to_owned()),
                merged_upstream: false,
                head_repository_name_with_owner: None,
                fork_pull_requests: Vec::new(),
                worktree_on_merge: true,
            },
            WorktreeCleanupCandidate {
                path: wt_trailing_space,
                in_use_by_thread: false,
                branch: Some("feat/nested".to_owned()),
                merged_upstream: true,
                head_repository_name_with_owner: None,
                fork_pull_requests: Vec::new(),
                worktree_on_merge: true,
            },
            fork_candidate,
        ],
        &mut runner,
        false,
    );
    assert_eq!(runner.calls, 1);
    assert!(!outcomes[0].removed);
    assert!(!outcomes[1].removed);
    assert_eq!(outcomes[1].reason, "Contains a nested Git repository");
    assert!(outcomes[2].removed);
    assert!(!wt_dangling.exists());
    Ok(())
}

fn run_git_cfg(dir: &Path) -> TestResult {
    for (k, v) in [
        ("user.email", "t@example.com"),
        ("user.name", "Test"),
        ("core.autocrlf", "false"),
    ] {
        let ok = Command::new("git")
            .current_dir(dir)
            .args(["config", k, v])
            .output()?
            .status
            .success();
        assert!(ok);
    }
    Ok(())
}
