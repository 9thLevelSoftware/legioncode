//! Wave 4 upstream alignment tests covering Git checkout guards, remote-prefixed publish
//! branch names, clone re-claims, branch race rollback protection, worktree stamps, and early ignore.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use legion_contracts::workspace_folders::{WorkspaceFolder, WorkspaceFolderState};
use legion_scm::git_ref_cache::{
    compute_worktree_branch_stamp, parse_worktree_branch_paths, read_ref_snapshot,
    read_ref_snapshot_with_worktrees,
};
use legion_scm::workspace_folders_scanner::{
    collect_checkpoint_repository_paths, discover_project_folders, is_whole_project_ignored,
    load_ignored_repository_paths,
};
use legion_scm::worktree_guards::{
    CREATE_WORKTREE_BRANCH_OPERATION, CheckoutThreadRef, ClonePhase, ProjectCloneTracker,
    PushDestinationCheck, TaskBranchCreationGuard, assert_checkout_idle, busy_threads_in_checkout,
    describe_busy_checkout, is_checkout_slot_preoccupied, parse_branch_with_remote_names,
    resolve_publish_branch_name, should_skip_nested_repository, should_skip_push_as_up_to_date,
};

#[test]
fn shared_checkout_guard_remote_publish_clone_claim_and_race_rollback_work() {
    let shared_projects: HashSet<String> = ["proj-1"].into_iter().map(str::to_owned).collect();
    let threads = vec![
        CheckoutThreadRef {
            id: "t1".into(),
            title: "Build server".into(),
            project_id: "proj-1".into(),
            worktree_path: None,
            is_busy: true,
        },
        CheckoutThreadRef {
            id: "t2".into(),
            title: "Separate wt".into(),
            project_id: "proj-1".into(),
            worktree_path: Some("/repo-wt-other".into()),
            is_busy: true,
        },
        CheckoutThreadRef {
            id: "t3".into(),
            title: "Idle archived".into(),
            project_id: "proj-1".into(),
            worktree_path: None,
            is_busy: false,
        },
    ];
    let busy = busy_threads_in_checkout(&["/repo"], &shared_projects, &threads);
    assert_eq!(busy.len(), 1);
    assert_eq!(busy[0].id, "t1");
    let refusal = assert_checkout_idle(&["/repo"], &shared_projects, &threads)
        .expect_err("shared checkout busy");
    assert_eq!(
        refusal,
        "'Build server' is still working in this checkout. Wait for it to finish, or use a worktree for a separate branch."
    );
    assert_eq!(
        describe_busy_checkout(&["A", "B", "C", "D"]),
        "'A', 'B' and 2 more are still working in this checkout. Wait for them to finish, or use a worktree for a separate branch."
    );

    assert_eq!(
        parse_branch_with_remote_names("origin/feature/x", &["origin", "origin/feature"]),
        Some(("origin/feature".to_owned(), "x".to_owned()))
    );
    assert_eq!(
        resolve_publish_branch_name("alice/fix", "origin"),
        "alice/fix"
    );
    assert_eq!(
        resolve_publish_branch_name("origin/alice/fix", "origin"),
        "alice/fix"
    );

    let mut clones = ProjectCloneTracker::new();
    assert!(clones.try_claim("p1", "/clones/app").is_ok());
    assert!(clones.try_claim("p2", "/clones/app").is_err());
    clones.set_phase("p1", ClonePhase::Failed);
    let superseded = clones
        .try_claim("p2", "/clones/app")
        .expect("failed clone does not block re-clone");
    assert_eq!(superseded, vec!["p1"]);
    clones.set_phase("p2", ClonePhase::Cancelled);
    assert!(clones.try_claim("p3", "/clones/app").is_ok());

    let mut race = TaskBranchCreationGuard::new(["/repo-a".to_owned(), "/repo-b".to_owned()]);
    race.record_outcome("/repo-a", true, None);
    race.record_outcome("/repo-b", false, Some(CREATE_WORKTREE_BRANCH_OPERATION));
    assert_eq!(race.branches_to_rollback(), &["/repo-a"]);
}

#[test]
fn worktree_slots_default_branch_push_stamps_and_ignore_bypass_work() {
    let dir = tempfile::tempdir().expect("tempdir");

    let empty_slot = dir.path().join("empty");
    fs::create_dir_all(&empty_slot).expect("mkdir");
    assert!(!is_checkout_slot_preoccupied(&empty_slot));

    let occupied_slot = dir.path().join("occupied");
    fs::create_dir_all(&occupied_slot).expect("mkdir");
    fs::write(occupied_slot.join(".git"), "gitdir: ../main/.git\n").expect("write");
    assert!(is_checkout_slot_preoccupied(&occupied_slot));
    assert!(should_skip_nested_repository(
        &PathBuf::from("/ws/outer/nested"),
        &[PathBuf::from("/ws/outer")],
    ));

    assert!(!should_skip_push_as_up_to_date(
        true,
        true,
        PushDestinationCheck::LocalUpstreamDot
    ));
    assert!(should_skip_push_as_up_to_date(
        true,
        true,
        PushDestinationCheck::MatchesUpstreamRemote {
            pushes_only_to_fetch_url: true,
        }
    ));

    let common_git = dir.path().join("main/.git");
    let wt_one = dir.path().join("wt-1");
    fs::create_dir_all(common_git.join("refs/heads")).expect("mkdir");
    fs::create_dir_all(common_git.join("refs/remotes")).expect("mkdir");
    fs::write(common_git.join("HEAD"), "ref: refs/heads/main\n").expect("head");
    fs::create_dir_all(&wt_one).expect("wt");

    let porcelain = format!(
        "worktree {}\nHEAD aaa\nbranch refs/heads/main\n\nworktree {}\nHEAD bbb\nbranch refs/heads/feat\n",
        common_git.display(),
        wt_one.display()
    );
    let parsed_wt = parse_worktree_branch_paths(&porcelain);
    assert_eq!(parsed_wt.len(), 2);
    let stamp = compute_worktree_branch_stamp(&parsed_wt, &common_git);
    assert!(stamp.contains("feat\t"));
    assert!(!stamp.contains("main\t"));
    let local_snap = read_ref_snapshot(&common_git).expect("local snap");
    let full_snap = read_ref_snapshot_with_worktrees(&common_git, Some(&stamp)).expect("full snap");
    assert!(full_snap.starts_with(&local_snap));
    assert!(full_snap.contains(&stamp));

    let ignored_proj = dir.path().join("ignored-ws");
    fs::create_dir_all(ignored_proj.join("sub-repo/.git")).expect("sub repo");
    fs::write(
        ignored_proj.join("legioncode.json"),
        r#"{ "ignoreRepositories": ["."] }"#,
    )
    .expect("write config");
    let ignored_paths = load_ignored_repository_paths(&ignored_proj);
    assert!(is_whole_project_ignored(&ignored_proj, &ignored_paths));
    assert_eq!(discover_project_folders(&ignored_proj), Vec::new());

    let folders = vec![WorkspaceFolder {
        path: "/ws/app".into(),
        name: "app".into(),
        state: WorkspaceFolderState::Git,
        discovered: Some(true),
    }];
    let subs = vec![
        folders[0].clone(),
        WorkspaceFolder {
            path: "/ws/app/vendor/mod".into(),
            name: "mod".into(),
            state: WorkspaceFolderState::Git,
            discovered: Some(true),
        },
    ];
    assert_eq!(
        collect_checkpoint_repository_paths(&folders, &subs),
        vec!["/ws/app", "/ws/app/vendor/mod"]
    );
}
