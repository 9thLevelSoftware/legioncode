//! Integration & behavior tests for git ref/config caching, single-pass queries, fast diff planning,
//! Git `< 2.36` authorship compatibility, Git LFS clone progress, and verified branch renames.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::process::Command;

use t3_scm::checkpoints::diff_patch_fast::{
    CachedTrackedDiff, ReviewDiffChangeKind, ReviewDiffSourceKind, TrackedPreviewCache,
    TrackedPreviewEntry, TrackedPreviewKey, build_untracked_no_index_diff_args,
    is_valid_preview_merge_base, isolated_untracked_index_plan, reuse_preview_merge_base,
    should_skip_untracked_listing,
};
use t3_scm::checkpoints::diff_query::DiffQueryOptions;
use t3_scm::git_clone_progress::{
    GitCloneStage, build_turn_authorship_log_args, collect_multi_parent_shared_paths,
    git_lfs_download_progress_line, git_supports_remerge_diff, is_git_filter_progress_line,
    parse_git_clone_progress_line, parse_merge_parent_groups, scan_lfs_incomplete_bytes,
};
use t3_scm::git_poll_queries::{
    read_remote_poll_metadata, reload_branch_list_budgeted, resolve_branch_pr_git_context,
};
use t3_scm::git_ref_cache::{
    BRANCH_LIST_MAX_GIT_INVOCATIONS, BRANCH_PR_LOOKUP_MAX_GIT_INVOCATIONS, BaseBranchCache,
    BaseBranchCacheKey, EpochRefSnapshotCache, IdleLocalStatusTracker, PrLookupGitMemo,
    PrimaryRemoteExistsCache, compute_base_branch_inputs_stamp, read_ref_snapshot,
};
use t3_scm::live_git_cli::collect_safe_base_env;
use t3_scm::live_git_perf::{
    RemoteStatusQueryOptions, compute_live_git_diff_with_mode, has_selected_staged_changes,
    is_branch_renamed_config_update_error, rename_branch_verified,
    resolve_pull_upstream_from_config, should_probe_scratch_index, worktree_delete_refresh_plan,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn run_git(repo: &Path, args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let out = Command::new("git").args(args).current_dir(repo).output()?;
    if !out.status.success() {
        return Err(format!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(())
}

fn init_test_repo(dir: &Path) -> TestResult {
    run_git(dir, &["init", "-b", "main"])?;
    run_git(dir, &["config", "user.email", "t3@example.com"])?;
    run_git(dir, &["config", "user.name", "T3"])?;
    fs::write(dir.join("tracked.txt"), "hello\n")?;
    run_git(dir, &["add", "tracked.txt"])?;
    run_git(dir, &["commit", "-m", "initial"])?;
    Ok(())
}

#[tokio::test]
async fn ref_cache_idle_poll_skip_base_branch_stamp_and_budgets() -> TestResult {
    let tmp = tempfile::tempdir()?;
    init_test_repo(tmp.path())?;
    let common = tmp.path().join(".git");
    let snap1 = read_ref_snapshot(&common);
    assert!(snap1.is_some());

    let mut idle = IdleLocalStatusTracker::new();
    assert!(idle.should_refresh_local("repo", snap1.as_deref()));
    assert!(!idle.should_refresh_local("repo", snap1.as_deref()));

    // #370b1fde77: touching loose refs contents changes stamp, while unrelated metadata does not.
    let cfg = b"branch.main.remote=origin\0";
    let stamp_a = compute_base_branch_inputs_stamp(&common, cfg);
    let stamp_b = compute_base_branch_inputs_stamp(&common, cfg);
    assert_eq!(stamp_a, stamp_b);

    let mut base_cache = BaseBranchCache::new();
    let bkey = BaseBranchCacheKey {
        cwd: tmp.path().to_string_lossy().into_owned(),
        branch: "feature/x".to_owned(),
        allow_remote_of_current: true,
        inputs_stamp: stamp_a,
    };
    base_cache.insert(bkey.clone(), Some("main".to_owned()), 1_000);
    assert_eq!(base_cache.get(&bkey, 1_500), Some(Some("main")));
    assert_eq!(base_cache.get(&bkey, 302_000), None);

    // #c1464d6774: repo without remotes skips remote lookups; PrimaryRemoteExistsCache never caches false.
    let env = collect_safe_base_env();
    let no_rem = read_remote_poll_metadata(tmp.path(), &env).await?;
    assert!(!no_rem.has_primary_remote);
    let mut rem_cache = PrimaryRemoteExistsCache::new();
    rem_cache.record("c", false, 100);
    assert_eq!(rem_cache.get("c", 101), None);
    rem_cache.record("c", true, 100);
    assert_eq!(rem_cache.get("c", 101), Some(true));

    // Add remote + tracking ref; single poll invocation when remote ref exists (#2c01259fb4).
    run_git(
        tmp.path(),
        &["remote", "add", "origin", "https://github.com/o/r.git"],
    )?;
    run_git(
        tmp.path(),
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    )?;
    let with_rem = read_remote_poll_metadata(tmp.path(), &env).await?;
    assert!(with_rem.has_primary_remote);
    assert_eq!(with_rem.head_ref.as_deref(), Some("refs/heads/main"));
    assert_eq!(with_rem.git_invocations, 1);

    let mut snap_cache = EpochRefSnapshotCache::new();
    let reload1 = reload_branch_list_budgeted(tmp.path(), &mut snap_cache, &env).await?;
    assert!(reload1.git_invocations <= BRANCH_LIST_MAX_GIT_INVOCATIONS);
    let reload2 = reload_branch_list_budgeted(tmp.path(), &mut snap_cache, &env).await?;
    assert_eq!(reload2.epoch, 2);
    assert_eq!(snap_cache.len(), 1);

    let mut pr_memo = PrLookupGitMemo::default();
    let pr_ctx = resolve_branch_pr_git_context(tmp.path(), "main", &mut pr_memo, &env).await?;
    assert!(pr_ctx.git_invocations <= BRANCH_PR_LOOKUP_MAX_GIT_INVOCATIONS);
    assert_eq!(
        pr_ctx.remote_url.as_deref(),
        Some("https://github.com/o/r.git")
    );
    pr_memo.mark_provider_queried();
    assert!(!pr_memo.can_reuse());
    Ok(())
}

#[tokio::test]
async fn diff_fast_paths_single_pass_workflows_and_git_compat() -> TestResult {
    let tmp = tempfile::tempdir()?;
    init_test_repo(tmp.path())?;
    fs::write(tmp.path().join("tracked.txt"), "hello\nworld\n")?;
    fs::write(tmp.path().join("untracked.txt"), "new line\n")?;

    let env = collect_safe_base_env();
    let tracked_only = compute_live_git_diff_with_mode(
        tmp.path(),
        "HEAD",
        &DiffQueryOptions::default(),
        true,
        &env,
    )
    .await?;
    assert!(tracked_only.diff.contains("tracked.txt"));
    assert!(!tracked_only.diff.contains("untracked.txt"));

    let complete = compute_live_git_diff_with_mode(
        tmp.path(),
        "HEAD",
        &DiffQueryOptions::default(),
        false,
        &env,
    )
    .await?;
    assert!(complete.diff.contains("untracked.txt"));

    assert_eq!(
        build_untracked_no_index_diff_args("a.txt"),
        [
            "diff",
            "--no-index",
            "--no-ext-diff",
            "--no-color",
            "--unified=3",
            "--",
            "/dev/null",
            "a.txt"
        ]
    );
    assert!(isolated_untracked_index_plan(Some("a.txt"), 1).isolated_single_file);
    assert!(should_skip_untracked_listing(
        false,
        Some(ReviewDiffChangeKind::Change),
        true
    ));
    assert!(!should_skip_untracked_listing(
        false,
        Some(ReviewDiffChangeKind::New),
        true
    ));

    let mut preview_cache = TrackedPreviewCache::new();
    let pkey = TrackedPreviewKey {
        cwd: "/repo".to_owned(),
        base_ref: Some("main".to_owned()),
        source_kind: Some(ReviewDiffSourceKind::WorkingTree),
        ignore_whitespace: false,
        metadata_only: false,
    };
    preview_cache.insert(
        "tok-1".to_owned(),
        TrackedPreviewEntry {
            key: pkey.clone(),
            expires_at_ms: 5_000,
            base_ref: Some("main".to_owned()),
            merge_base: None,
            dirty: CachedTrackedDiff {
                stdout: "d".to_owned(),
                stdout_truncated: false,
                files_count: 1,
            },
            base: CachedTrackedDiff {
                stdout: "b".to_owned(),
                stdout_truncated: false,
                files_count: 1,
            },
        },
    );
    assert!(preview_cache.take_if_valid("tok-1", &pkey, 4_000).is_some());
    assert!(preview_cache.take_if_valid("tok-1", &pkey, 4_000).is_none());

    assert!(!is_valid_preview_merge_base("--upload-pack=evil"));
    let valid_oid = "a".repeat(40);
    assert!(is_valid_preview_merge_base(&valid_oid));
    let reused = reuse_preview_merge_base(
        Some(ReviewDiffSourceKind::BranchRange),
        Some(&valid_oid),
        false,
    );
    assert!(!reused.needs_current_branch);
    Ok(())
}

#[tokio::test]
async fn single_pass_workflows_remerge_compat_lfs_and_verified_rename() -> TestResult {
    let tmp = tempfile::tempdir()?;
    init_test_repo(tmp.path())?;
    let env = collect_safe_base_env();

    // Single-pass workflow + branch rename + <2.36 compat + LFS progress.
    assert!(should_probe_scratch_index(true, false));
    assert!(has_selected_staged_changes(
        b"a.txt\0b.txt\0",
        &HashSet::from(["b.txt".to_owned()])
    ));
    assert!(RemoteStatusQueryOptions::default().include_ahead_of_default);
    assert_eq!(
        resolve_pull_upstream_from_config(Some("origin"), Some("refs/heads/main")).as_deref(),
        Some("origin/main")
    );
    assert!(worktree_delete_refresh_plan().skip_duplicate_project_root_refresh);

    assert!(!git_supports_remerge_diff("git version 2.34.1"));
    assert!(git_supports_remerge_diff("git version 2.45.1.windows.1"));
    assert_eq!(
        build_turn_authorship_log_args(false, "a..b"),
        ["log", "a..b", "--format=", "--cc"]
    );
    assert_eq!(
        parse_merge_parent_groups("m1 p1 p2\n"),
        vec![vec!["p1".to_owned(), "p2".to_owned()]]
    );
    assert_eq!(
        collect_multi_parent_shared_paths(&[
            vec!["shared.rs".to_owned(), "left.rs".to_owned()],
            vec!["shared.rs".to_owned(), "right.rs".to_owned()],
        ]),
        vec!["shared.rs".to_owned()]
    );

    let lfs_line = "Filtering content:  50% (1/2), 1.50 MiB | 1.00 MiB/s";
    assert!(is_git_filter_progress_line(lfs_line));
    let parsed = parse_git_clone_progress_line(lfs_line).ok_or("missing parsed clone line")?;
    assert_eq!(parsed.stage, GitCloneStage::Checkout);
    assert_eq!(parsed.percent, Some(50));
    assert_eq!(
        git_lfs_download_progress_line(1_572_864, None)
            .detail
            .as_deref(),
        Some("Downloading Git LFS objects: 1.5 MiB in progress")
    );
    let incomplete_dir = tmp.path().join(".git").join("lfs").join("incomplete");
    fs::create_dir_all(&incomplete_dir)?;
    fs::write(incomplete_dir.join("part"), vec![0u8; 256])?;
    assert_eq!(scan_lfs_incomplete_bytes(tmp.path()).await, 256);

    assert!(is_branch_renamed_config_update_error(
        "fatal: branch is renamed, but update of config-file failed"
    ));
    rename_branch_verified(tmp.path(), "main", "trunk", &env).await?;
    Ok(())
}
