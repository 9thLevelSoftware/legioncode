//! Live CLI `stdin`/`env_clear` and temporary `git` repository integration tests for `LiveForgeClient`.

use legion_contracts::ids::SourceControlProviderKind;
use legion_scm::checkpoints::diff_query::DiffQueryOptions;
use legion_scm::forges::CliCommandSpec;
use legion_scm::live_client::LiveForgeClient;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn live_client_cli_stdin_and_env_clear_isolation() -> TestResult {
    let client = LiveForgeClient::new()?;

    let stdin_out = client
        .execute_cli(&CliCommandSpec {
            program: "git".to_owned(),
            args: vec!["stripspace".to_owned()],
            env: Vec::new(),
            stdin: Some("line one\n\n\nline two  \n".to_owned()),
        })
        .await?;
    assert!(stdin_out.success);
    assert_eq!(
        stdin_out.stdout.replace("\r\n", "\n"),
        "line one\n\nline two\n"
    );

    // Parent CARGO env var is NOT passed after env_clear(), while explicit override IS visible.
    let env_spec = if cfg!(windows) {
        CliCommandSpec {
            program: "cmd".to_owned(),
            args: vec![
                "/C".to_owned(),
                "echo EXP=%LEGION_EXPLICIT% LEAK=%CARGO%".to_owned(),
            ],
            env: vec![("LEGION_EXPLICIT".to_owned(), "ok".to_owned())],
            stdin: None,
        }
    } else {
        CliCommandSpec {
            program: "sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                "printf 'EXP=%s LEAK=%s' \"$LEGION_EXPLICIT\" \"$CARGO\"".to_owned(),
            ],
            env: vec![("LEGION_EXPLICIT".to_owned(), "ok".to_owned())],
            stdin: None,
        }
    };
    let env_out = client.execute_cli(&env_spec).await?;
    assert!(env_out.success);
    assert!(env_out.stdout.contains("EXP=ok LEAK="));
    assert!(!env_out.stdout.contains("cargo"));
    Ok(())
}

#[tokio::test]
async fn live_client_real_git_repo_discovery_and_diff() -> TestResult {
    let client = LiveForgeClient::new()?;
    let tmp = tempfile::tempdir()?;
    let repo = tmp.path();

    let init_cmds: &[&[&str]] = &[
        &["init"],
        &["config", "user.email", "test@legion.codes"],
        &["config", "user.name", "Legion Test"],
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/9thlevelsoftware/legioncode.git",
        ],
    ];
    for args in init_cmds {
        run_git(&client, repo, args).await?;
    }

    tokio::fs::create_dir_all(repo.join("spaced dir")).await?;
    let tracked_rel = "spaced dir/hello .txt";
    tokio::fs::write(repo.join(tracked_rel), "alpha\nbeta\n").await?;
    for args in &[&["add", "."][..], &["commit", "-m", "initial"][..]] {
        run_git(&client, repo, args).await?;
    }

    tokio::fs::write(repo.join(tracked_rel), "alpha\ngamma\n").await?;
    tokio::fs::write(repo.join("added.txt"), "new file line\n").await?;

    let remote = client.discover_workspace_remote(repo).await?;
    assert_eq!(remote.full_name, "9thlevelsoftware/legioncode");
    assert_eq!(
        remote
            .provider_hint
            .as_ref()
            .map(SourceControlProviderKind::as_str),
        Some("github")
    );

    let live_diff = client
        .compute_live_git_diff(repo, "HEAD", &DiffQueryOptions::default())
        .await?;
    assert_eq!(live_diff.files_changed, 2);
    assert!(live_diff.diff.contains("--- a/spaced dir/hello .txt\n"));
    assert!(live_diff.diff.contains("+gamma\n"));
    assert!(live_diff.diff.contains("+new file line\n"));
    Ok(())
}

async fn run_git(client: &LiveForgeClient, repo: &std::path::Path, args: &[&str]) -> TestResult {
    let out = client
        .execute_cli_in_dir(
            &CliCommandSpec {
                program: "git".to_owned(),
                args: args.iter().map(|s| (*s).to_owned()).collect(),
                env: Vec::new(),
                stdin: None,
            },
            repo,
        )
        .await?;
    assert!(out.success, "git {args:?} failed: {}", out.stderr);
    Ok(())
}
