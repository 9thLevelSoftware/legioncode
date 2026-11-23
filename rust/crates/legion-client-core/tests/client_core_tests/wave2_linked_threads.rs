use std::collections::BTreeSet;
use std::error::Error;

use legion_client_core::linked_threads::{
    LINK_BATCH_CONCURRENCY, LinkBackBlock, LinkBackOffer, LinkBackSource, LinkBackTarget,
    LinkBatchQueue, LinkBlockReason, LinkListState, LinkableMachine, LinkedHistoryWindow,
    LinkedThreadOpen, LinkedThreadViewState, PinnedThreadRepository, RepositorySectionOrigin,
    RewindFileRestoreOutcome, ThreadRepositoryRef, addable_task_folder_repositories,
    append_tracked_folders, classify_linked_thread_failure, link_back_offer, link_batch_candidates,
    link_list_state, pin_thread_repository, pinnable_thread_repositories,
    resolve_linked_thread_open, resolve_rewind_file_restore, resolve_worktree_repository_selection,
    should_keep_loaded_history_on_snapshot, suggested_tracked_folders, thread_repository_sections,
    unpin_thread_repository,
};
use legion_contracts::ids::{CheckpointScopeId, EnvironmentId, ProjectId};
use legion_contracts::peer_links::{
    PeerLinkError, PeerLinkErrorReason, PeerLinkFailureKind, PeerLinkThreadReadError,
    PeerLinkThreadReadReason,
};
use legion_contracts::workspace_folders::{
    PROJECT_TRACKED_FOLDERS_MAX, ProjectTrackedFolder, TASK_FOLDER_DEFAULT_REPOSITORIES_MAX,
    TaskFolder, TaskFolderRepository, WorkspaceFolder, WorkspaceFolderState,
    WorkspaceScannedFolder, WorkspaceScannedFolderSource,
};

#[test]
fn linked_thread_stream_opening_failure_and_history_dedup() -> Result<(), Box<dyn Error>> {
    let via = EnvironmentId::new("env-via")?;
    let peer = EnvironmentId::new("env-peer")?;
    assert_eq!(
        resolve_linked_thread_open(&peer, &via, Some(true), true),
        LinkedThreadOpen::Direct
    );
    assert_eq!(
        resolve_linked_thread_open(&peer, &via, Some(false), true),
        LinkedThreadOpen::Linked
    );
    assert_eq!(
        resolve_linked_thread_open(&peer, &via, None, false),
        LinkedThreadOpen::Unavailable
    );

    let unreach = PeerLinkThreadReadError {
        reason: PeerLinkThreadReadReason::Unreachable,
        label: Some("VM 1".to_owned()),
        message: "timed out".to_owned(),
    };
    let retryable = classify_linked_thread_failure(Some(&unreach), "Local", "Peer");
    assert!(retryable.retryable);
    assert!(retryable.title.contains("VM 1"));

    let expired = PeerLinkThreadReadError {
        reason: PeerLinkThreadReadReason::Expired,
        label: None,
        message: "expired".to_owned(),
    };
    let fatal = classify_linked_thread_failure(Some(&expired), "Local", "Peer");
    assert!(!fatal.retryable);
    // Older host returning an unknown-method error stays retryable (#40545/#41020).
    let unknown = classify_linked_thread_failure(None, "Local", "Peer");
    assert!(unknown.retryable);

    let loaded = LinkedHistoryWindow {
        sequence: 1,
        cursor: "cur-1".to_owned(),
    };
    let same = LinkedHistoryWindow {
        sequence: 2,
        cursor: "cur-2".to_owned(),
    };
    assert!(should_keep_loaded_history_on_snapshot(2, &loaded, &same));
    assert!(!should_keep_loaded_history_on_snapshot(1, &loaded, &same));

    let mut view = LinkedThreadViewState::default();
    view.apply_snapshot(
        1,
        Some("Title 1".to_owned()),
        Some("cur-1".to_owned()),
        true,
        &[],
    );
    assert_eq!(view.begin_load_earlier().as_deref(), Some("cur-1"));
    assert_eq!(view.begin_load_earlier(), None); // single-flight dedup
    view.finish_history_page(None, false, false);
    assert_eq!(view.begin_load_earlier(), None);

    // Retryable failure preserves existing snapshot; non-retryable clears view.
    view.record_failure(retryable);
    assert!(view.has_snapshot);
    view.record_failure(fatal);
    assert!(!view.has_snapshot);
    Ok(())
}

#[test]
fn link_batch_candidates_concurrency_and_retry_failed_only() -> Result<(), Box<dyn Error>> {
    let env_id = |name: &str| EnvironmentId::new(format!("env-{name}"));
    let machines: Vec<LinkableMachine> = ["a", "b", "c", "d", "e", "linked", "offline"]
        .into_iter()
        .map(|name| {
            let id = EnvironmentId::new(format!("env-{name}")).unwrap_or_else(|_| unreachable!());
            LinkableMachine {
                environment_id: id,
                label: name.to_owned(),
                machine: "server".to_owned(),
                connected: name != "offline",
                connect_only: false,
                linked: name == "linked",
                urls: vec![format!("https://{name}.example")],
            }
        })
        .collect();
    let writable: BTreeSet<EnvironmentId> = ["a", "b", "c", "d", "e", "linked", "offline"]
        .into_iter()
        .map(|n| env_id(n).unwrap_or_else(|_| unreachable!()))
        .collect();

    assert_eq!(link_list_state(false, false), LinkListState::Loading);
    assert_eq!(link_list_state(false, true), LinkListState::Failed);
    let gated = link_batch_candidates(&machines, &writable, LinkListState::Loading);
    assert!(
        gated
            .iter()
            .all(|c| c.blocked == Some(LinkBlockReason::LinksUnknown))
    );

    let cands = link_batch_candidates(&machines, &writable, LinkListState::Loaded);
    assert_eq!(cands[5].blocked, Some(LinkBlockReason::Linked));
    assert_eq!(cands[6].blocked, Some(LinkBlockReason::NotConnected));

    let mut q = LinkBatchQueue::from_candidates(&cands);
    let first_wave = q.take_next_batch();
    assert_eq!(first_wave.len(), LINK_BATCH_CONCURRENCY);
    assert_eq!(q.summary().pending, 1);

    q.record_outcome(&env_id("a")?, Ok(()));
    q.record_outcome(
        &env_id("b")?,
        Err(&PeerLinkError {
            reason: PeerLinkErrorReason::Unreachable,
            message: "offline".to_owned(),
            answered_as: None,
        }),
    );
    q.record_outcome(&env_id("c")?, Ok(()));
    q.record_outcome(&env_id("d")?, Ok(()));
    let second_wave = q.take_next_batch();
    assert_eq!(second_wave, vec![env_id("e")?]);
    q.record_outcome(&env_id("e")?, Ok(()));
    assert_eq!(q.summary().linked, 4);
    assert_eq!(q.summary().failed, 1);
    assert_eq!(
        q.status(&env_id("b")?),
        Some(
            &legion_client_core::linked_threads::LinkBatchItemStatus::Failed {
                kind: PeerLinkFailureKind::Unreachable,
                message: "offline".to_owned(),
            }
        )
    );
    assert_eq!(q.retry_failed(), vec![env_id("b")?]);
    Ok(())
}

#[test]
fn multi_repo_pin_sections_worktrees_and_rewind_restore() -> Result<(), Box<dyn Error>> {
    let tracked = suggested_tracked_folders(
        &[ProjectTrackedFolder {
            path: "/work/api".to_owned(),
            name: "api".to_owned(),
            discovered: None,
        }],
        &[
            WorkspaceScannedFolder {
                path: "/work/api/".to_owned(),
                name: "api".to_owned(),
                is_git_repository: true,
                source: WorkspaceScannedFolderSource::Child,
            },
            WorkspaceScannedFolder {
                path: "/work/web".to_owned(),
                name: "web".to_owned(),
                is_git_repository: true,
                source: WorkspaceScannedFolderSource::Child,
            },
        ],
        Some("/work"),
    );
    assert_eq!(tracked.len(), 1);
    let combined = append_tracked_folders(&[], &tracked);
    assert!(combined.len() <= PROJECT_TRACKED_FOLDERS_MAX);

    let changed = vec![ThreadRepositoryRef {
        scope_id: Some(CheckpointScopeId::new("scope-api")?),
        repository_path: "/work/api".to_owned(),
        name: "api".to_owned(),
        cwd: "/wt/task/api".to_owned(),
    }];
    let task_folder = TaskFolder {
        root: "/wt/task".to_owned(),
        project_id: ProjectId::new("p-1")?,
        project_root: "/work".to_owned(),
        anchor: "/work".to_owned(),
        project_path: "/wt/task".to_owned(),
        project_is_worktree: false,
        repositories: vec![
            TaskFolderRepository {
                repository_path: "/work/web".to_owned(),
                worktree_path: "/wt/task/web".to_owned(),
                name: "web".to_owned(),
            },
            TaskFolderRepository {
                repository_path: "/work/api".to_owned(),
                worktree_path: "/wt/task/api".to_owned(),
                name: "api".to_owned(),
            },
        ],
    };
    let sections = thread_repository_sections(&changed, &[], Some(&task_folder), false);
    assert_eq!(sections.visible[0].name, "api");
    assert_eq!(
        sections.visible[0].origin,
        RepositorySectionOrigin::Worktree
    );

    let pinned = pin_thread_repository(
        &[],
        PinnedThreadRepository {
            repository_path: "/work/docs".to_owned(),
            name: "docs".to_owned(),
        },
    );
    assert_eq!(pinned.len(), 1);
    assert_eq!(unpin_thread_repository(&pinned, "/work/docs").len(), 0);

    let folders: Vec<WorkspaceFolder> = (0..10)
        .map(|i| WorkspaceFolder {
            name: format!("repo-{i}"),
            path: format!("/work/repo-{i}"),
            state: WorkspaceFolderState::Git,
            discovered: Some(true),
        })
        .collect();
    assert_eq!(
        resolve_worktree_repository_selection(&folders, None).len(),
        TASK_FOLDER_DEFAULT_REPOSITORIES_MAX
    );
    assert_ne!(
        addable_task_folder_repositories(&task_folder, &folders),
        vec![]
    );
    assert_ne!(pinnable_thread_repositories(&changed, &pinned), vec![]);
    assert_eq!(
        resolve_rewind_file_restore(false, Some(&task_folder), true),
        RewindFileRestoreOutcome::Repositories
    );
    assert_eq!(
        resolve_rewind_file_restore(true, None, false),
        RewindFileRestoreOutcome::Files
    );
    Ok(())
}

#[test]
fn link_back_offer_evaluates_eligibility() -> Result<(), Box<dyn Error>> {
    let src_env = EnvironmentId::new("env-source")?;
    let urls = vec!["https://source.example".to_owned()];
    let source = LinkBackSource {
        environment_id: &src_env,
        connected: true,
        writable: true,
        urls: &urls,
    };
    assert_eq!(link_back_offer(None, &source), LinkBackOffer::Hidden);
    assert_eq!(
        link_back_offer(
            Some(&LinkBackTarget {
                connected: true,
                offers_links: false,
                writable: true,
                links: Some(&[]),
            }),
            &source
        ),
        LinkBackOffer::Blocked {
            reason: LinkBackBlock::TargetLinksOff
        }
    );
    assert_eq!(
        link_back_offer(
            Some(&LinkBackTarget {
                connected: true,
                offers_links: true,
                writable: true,
                links: Some(&[(&src_env, true)]),
            }),
            &source
        ),
        LinkBackOffer::Offered
    );
    assert_eq!(
        link_back_offer(
            Some(&LinkBackTarget {
                connected: true,
                offers_links: true,
                writable: true,
                links: Some(&[(&src_env, false)]),
            }),
            &source
        ),
        LinkBackOffer::Hidden
    );
    Ok(())
}
