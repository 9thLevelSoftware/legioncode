#![forbid(unsafe_code)]
//! Integration tests for `legion_scm::stacked_action` (`commit`, `push`, `create_pr`,
//! `commit_push`, `commit_push_pr`) against a real bare git remote and stub `gh`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use legion_scm::stacked_action::{
    StackedActionKind, StackedActionPhase, StackedActionRequest, ToastCtaOutcome,
    execute_stacked_action, execute_stacked_action_with_gh, phases_for_action,
};
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn run_git(cwd: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let out = Command::new("git")
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(args)
        .output()?;
    if !out.status.success() {
        return Err(format!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn setup_repo_with_bare_remote() -> Result<(TempDir, PathBuf, PathBuf), Box<dyn std::error::Error>>
{
    let tmp = TempDir::new()?;
    let bare = tmp.path().join("remote.git");
    let work = tmp.path().join("work");
    fs::create_dir_all(&bare)?;
    fs::create_dir_all(&work)?;
    run_git(&bare, &["init", "--bare", "-b", "main"])?;
    run_git(&work, &["init", "-b", "main"])?;
    run_git(&work, &["config", "user.email", "test@legion.local"])?;
    run_git(&work, &["config", "user.name", "Legion Test"])?;
    fs::write(work.join("README.md"), "# init\n")?;
    run_git(&work, &["add", "."])?;
    run_git(&work, &["commit", "-m", "initial commit"])?;
    run_git(&work, &["remote", "add", "origin", &bare.to_string_lossy()])?;
    run_git(&work, &["push", "-u", "origin", "main"])?;
    run_git(&bare, &["symbolic-ref", "HEAD", "refs/heads/main"])?;
    Ok((tmp, bare, work))
}

fn install_stub_gh(
    bin_dir: &Path,
    list_json: &str,
    create_url: &str,
    view_json: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    fs::create_dir_all(bin_dir)?;
    let log_file = bin_dir.join("gh_args.log");
    let list_file = bin_dir.join("gh_list.json");
    let view_file = bin_dir.join("gh_view.json");
    fs::write(&list_file, list_json)?;
    fs::write(&view_file, view_json)?;

    #[cfg(windows)]
    {
        let script = format!(
            "@echo off\r\necho %* >> \"{}\"\r\nif \"%1 %2\"==\"pr list\" (\r\n  type \"{}\"\r\n  exit /b 0\r\n)\r\nif \"%1 %2\"==\"pr view\" (\r\n  type \"{}\"\r\n  exit /b 0\r\n)\r\nif \"%1 %2\"==\"pr create\" (\r\n  more > nul\r\n  echo {}\r\n  exit /b 0\r\n)\r\nexit /b 1\r\n",
            log_file.display(),
            list_file.display(),
            view_file.display(),
            create_url
        );
        let p = bin_dir.join("gh.cmd");
        fs::write(&p, script)?;
        Ok(p.to_string_lossy().into_owned())
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = format!(
            "#!/usr/bin/env sh\necho \"$*\" >> '{}'\nif [ \"$1 $2\" = \"pr list\" ]; then cat '{}'; exit 0; fi\nif [ \"$1 $2\" = \"pr view\" ]; then cat '{}'; exit 0; fi\nif [ \"$1 $2\" = \"pr create\" ]; then cat > /dev/null; echo '{}'; exit 0; fi\nexit 1\n",
            log_file.display(),
            list_file.display(),
            view_file.display(),
            create_url
        );
        let p = bin_dir.join("gh");
        fs::write(&p, script)?;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755))?;
        Ok(p.to_string_lossy().into_owned())
    }
}

#[tokio::test]
async fn stacked_action_commit_file_paths_push_and_up_to_date() -> TestResult {
    let (_tmp, _bare, work) = setup_repo_with_bare_remote()?;
    run_git(&work, &["checkout", "-b", "feat/work-1"])?;
    fs::write(work.join("keep.txt"), "hello\n")?;
    fs::write(work.join("skip.txt"), "untracked\n")?;

    let mut seen = Vec::new();
    let res = execute_stacked_action(
        &StackedActionRequest {
            cwd: work.clone(),
            action: StackedActionKind::CommitPush,
            commit_message: Some("feat: add keep.txt".to_owned()),
            feature_branch: false,
            file_paths: Some(vec!["keep.txt".to_owned()]),
        },
        |p, _| seen.push(p),
    )
    .await
    .map_err(|e| format!("{}: {}", e.operation, e.detail))?;

    assert_eq!(
        seen,
        vec![StackedActionPhase::Commit, StackedActionPhase::Push]
    );
    assert_eq!(res.commit.status, "created");
    assert_eq!(res.commit.subject.as_deref(), Some("feat: add keep.txt"));
    assert_eq!(res.push.status, "pushed");
    assert_eq!(res.push.set_upstream, Some(true));
    assert_eq!(
        res.push.upstream_branch.as_deref(),
        Some("origin/feat/work-1")
    );

    let status = run_git(&work, &["status", "--porcelain"])?;
    assert!(
        status.contains("?? skip.txt"),
        "expected skip.txt untouched: {status}"
    );

    let res2 = execute_stacked_action(
        &StackedActionRequest {
            cwd: work.clone(),
            action: StackedActionKind::Push,
            commit_message: None,
            feature_branch: false,
            file_paths: None,
        },
        |_, _| {},
    )
    .await
    .map_err(|e| format!("{}: {}", e.operation, e.detail))?;
    assert_eq!(res2.push.status, "skipped_up_to_date");
    Ok(())
}

#[tokio::test]
async fn stacked_action_commit_skipped_and_missing_remote_error() -> TestResult {
    let tmp = TempDir::new()?;
    let work = tmp.path().join("local-only");
    fs::create_dir_all(&work)?;
    run_git(&work, &["init", "-b", "main"])?;
    run_git(&work, &["config", "user.email", "test@legion.local"])?;
    run_git(&work, &["config", "user.name", "Legion Test"])?;
    fs::write(work.join("a.txt"), "1\n")?;
    run_git(&work, &["add", "."])?;
    run_git(&work, &["commit", "-m", "init"])?;

    let no_changes = execute_stacked_action(
        &StackedActionRequest {
            cwd: work.clone(),
            action: StackedActionKind::Commit,
            commit_message: None,
            feature_branch: false,
            file_paths: None,
        },
        |_, _| {},
    )
    .await
    .map_err(|e| format!("{}: {}", e.operation, e.detail))?;
    assert_eq!(no_changes.commit.status, "skipped_no_changes");

    let push_err = execute_stacked_action(
        &StackedActionRequest {
            cwd: work,
            action: StackedActionKind::Push,
            commit_message: None,
            feature_branch: false,
            file_paths: None,
        },
        |_, _| {},
    )
    .await
    .expect_err("push without remote must fail");
    assert_eq!(push_err.phase, Some(StackedActionPhase::Push));
    Ok(())
}

#[tokio::test]
async fn stacked_action_create_pr_created_and_opened_existing_with_stub_gh() -> TestResult {
    let (tmp, _bare, work) = setup_repo_with_bare_remote()?;
    let bin_dir = tmp.path().join("stub-bin");
    let view_json = r#"{"number":42,"url":"https://github.com/org/repo/pull/42","title":"feat: stub pr","baseRefName":"main","headRefName":"feat/pr-1"}"#;
    let gh_bin = install_stub_gh(
        &bin_dir,
        "[]",
        "https://github.com/org/repo/pull/42",
        view_json,
    )?;

    assert_eq!(
        phases_for_action(StackedActionKind::CommitPushPr, true),
        vec![
            StackedActionPhase::Branch,
            StackedActionPhase::Commit,
            StackedActionPhase::Push,
            StackedActionPhase::Pr,
        ]
    );

    fs::write(work.join("feature.txt"), "new work\n")?;
    let res = execute_stacked_action_with_gh(
        &StackedActionRequest {
            cwd: work.clone(),
            action: StackedActionKind::CommitPushPr,
            commit_message: Some("feat: stub pr".to_owned()),
            feature_branch: true,
            file_paths: None,
        },
        &gh_bin,
        |_, _| {},
    )
    .await
    .map_err(|e| format!("{}: {}", e.operation, e.detail))?;

    assert_eq!(res.branch.status, "created");
    assert_eq!(res.commit.status, "created");
    assert_eq!(res.push.status, "pushed");
    assert_eq!(res.pr.status, "created");
    assert_eq!(res.pr.number, Some(42));
    assert!(matches!(res.toast.cta, ToastCtaOutcome::OpenPr { .. }));

    let existing_list = format!("[{view_json}]");
    let gh_bin2 = install_stub_gh(&bin_dir, &existing_list, "unused", view_json)?;
    let existing = execute_stacked_action_with_gh(
        &StackedActionRequest {
            cwd: work,
            action: StackedActionKind::CreatePr,
            commit_message: None,
            feature_branch: false,
            file_paths: None,
        },
        &gh_bin2,
        |_, _| {},
    )
    .await
    .map_err(|e| format!("{}: {}", e.operation, e.detail))?;
    assert_eq!(existing.pr.status, "opened_existing");
    assert_eq!(existing.pr.number, Some(42));
    Ok(())
}

#[tokio::test]
async fn stacked_action_commit_file_paths_isolates_from_pre_staged_index() -> TestResult {
    let (_tmp, _bare, work) = setup_repo_with_bare_remote()?;
    run_git(&work, &["checkout", "-b", "feat/isolated-paths"])?;

    fs::write(work.join("pre_staged.txt"), "staged unrelated\n")?;
    run_git(&work, &["add", "pre_staged.txt"])?;
    fs::write(work.join("requested.txt"), "requested change\n")?;

    let res = execute_stacked_action(
        &StackedActionRequest {
            cwd: work.clone(),
            action: StackedActionKind::Commit,
            commit_message: Some("feat: only requested".to_owned()),
            feature_branch: false,
            file_paths: Some(vec!["requested.txt".to_owned()]),
        },
        |_, _| {},
    )
    .await
    .map_err(|e| format!("{}: {}", e.operation, e.detail))?;
    assert_eq!(res.commit.status, "created");

    let head_files = run_git(&work, &["show", "--name-only", "--format=", "HEAD"])?;
    let committed: Vec<&str> = head_files
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    assert_eq!(committed, vec!["requested.txt"]);

    let status = run_git(&work, &["status", "--porcelain"])?;
    assert!(
        status.contains("A  pre_staged.txt"),
        "expected pre_staged.txt to remain staged in index, got: {status}"
    );
    Ok(())
}
