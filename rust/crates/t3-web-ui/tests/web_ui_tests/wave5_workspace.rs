//! Integration tests for Environments Map (routes + direct/transitive P2P links, NO cloud
//! relay), bounded batch P2P linking, unified linked-thread read-only pane, and multi-repo picker.

use t3_client_core::linked_threads::LinkedThreadTarget;
use t3_contracts::ids::{EnvironmentId, ThreadId};
use t3_contracts::peer_links::PeerMcpClientAccess;
use t3_contracts::workspace_folders::{WorkspaceScannedFolder, WorkspaceScannedFolderSource};
use t3_web_ui::actions_features::{
    AdvanceBatchLinkPayload, OpenLinkedThreadPanePayload, ProjectRepoFolderPayload,
    SetEnvironmentsFleetPayload, SetProjectRepositoriesPayload, SyncLinkedThreadPanePayload,
    ThreadRepoPinPayload,
};
use t3_web_ui::state_features::{
    FleetEdgeKind, FleetEnvironmentEdge, FleetEnvironmentNode, LinkedPaneMessage,
    LinkedPaneSurfaceMode,
};
use t3_web_ui::{WebAppAction, WebAppEngine, WebHostEffect};

#[test]
fn environments_map_transitive_and_bounded_batch_p2p_linking() -> Result<(), String> {
    let mut engine = WebAppEngine::new("en", 1_000);
    engine
        .try_dispatch(WebAppAction::SetEnvironmentsMapOpen { open: true })
        .map_err(|e| e.to_string())?;
    engine
        .try_dispatch(WebAppAction::SetEnvironmentsFleet(
            SetEnvironmentsFleetPayload {
                nodes: vec![
                    FleetEnvironmentNode {
                        id: "peer-hub".to_owned(),
                        label: "Hub Mac".to_owned(),
                        routes: vec!["https://hub.tail.ts.net".to_owned()],
                        linked: true,
                        access: Some(PeerMcpClientAccess::FullAccess),
                        reachable: true,
                    },
                    FleetEnvironmentNode {
                        id: "peer-leaf-1".to_owned(),
                        label: "Build Box 1".to_owned(),
                        routes: vec!["http://100.64.0.10:3773".to_owned()],
                        linked: false,
                        access: Some(PeerMcpClientAccess::FullAccess),
                        reachable: true,
                    },
                    FleetEnvironmentNode {
                        id: "peer-leaf-2".to_owned(),
                        label: "Build Box 2".to_owned(),
                        routes: vec!["http://100.64.0.11:3773".to_owned()],
                        linked: false,
                        access: None,
                        reachable: true,
                    },
                    FleetEnvironmentNode {
                        id: "peer-readonly".to_owned(),
                        label: "Readonly Box".to_owned(),
                        routes: vec!["http://100.64.0.12:3773".to_owned()],
                        linked: false,
                        access: Some(PeerMcpClientAccess::ReadOnly),
                        reachable: true,
                    },
                ],
                edges: vec![FleetEnvironmentEdge {
                    from_id: "peer-hub".to_owned(),
                    to_id: "peer-leaf-1".to_owned(),
                    edge: FleetEdgeKind::DirectPeerLink,
                }],
            },
        ))
        .map_err(|e| e.to_string())?;

    // Full-access peer produces transitive P2P edge without any cloud relay.
    assert!(engine.state().features.environments_map.edges.iter().any(|e| {
        e.to_id == "peer-leaf-1"
            && matches!(
                &e.edge,
                FleetEdgeKind::TransitivePeerLink { via_peer_id } if via_peer_id == "peer-hub"
            )
    }));
    let html = engine.render_html();
    assert!(html.contains("via peer-hub"));
    assert!(!html.to_ascii_lowercase().contains("ping"));

    // Batch link selects eligible candidates (skipping ReadOnly & already-linked) with cap = 2.
    let fx = engine
        .try_dispatch(WebAppAction::StartBatchLink {
            candidate_ids: Vec::new(),
        })
        .map_err(|e| e.to_string())?;
    assert_eq!(fx.len(), 2);
    assert!(
        fx.iter()
            .all(|f| matches!(f, WebHostEffect::WsSendFrameParts { .. }))
    );
    assert_eq!(engine.state().features.environments_map.batch_rows.len(), 2);

    engine
        .try_dispatch(WebAppAction::AdvanceBatchLink(AdvanceBatchLinkPayload {
            environment_id: "peer-leaf-1".to_owned(),
            succeeded: true,
            failure_kind: None,
        }))
        .map_err(|e| e.to_string())?;
    assert!(engine.state().features.environments_map.nodes[1].linked);
    Ok(())
}

#[test]
fn unified_linked_thread_pane_and_multi_repo_picker_roundtrip() -> Result<(), String> {
    let mut engine = WebAppEngine::new("es", 2_000);
    engine
        .try_dispatch(WebAppAction::SelectProject {
            project_id: "proj-1".to_owned(),
        })
        .map_err(|e| e.to_string())?;

    let target = LinkedThreadTarget {
        via_environment_id: EnvironmentId::new("peer-hub").map_err(|e| e.to_string())?,
        environment_id: EnvironmentId::new("peer-leaf").map_err(|e| e.to_string())?,
        thread_id: ThreadId::new("th-remote").map_err(|e| e.to_string())?,
        label: "Build Box".to_owned(),
        title: Some("Remote task".to_owned()),
    };
    engine
        .try_dispatch(WebAppAction::OpenLinkedThreadPane(
            OpenLinkedThreadPanePayload {
                target,
                title: "Subagent <audit>".to_owned(),
                mode: LinkedPaneSurfaceMode::Subagent,
                working: true,
                messages: vec![LinkedPaneMessage {
                    id: "m-1".to_owned(),
                    role: "assistant".to_owned(),
                    text: "<script>alert(1)</script>Checked diff".to_owned(),
                }],
                has_earlier: true,
            },
        ))
        .map_err(|e| e.to_string())?;

    let html = engine.render_html();
    assert!(html.contains(r#"data-readonly="true""#));
    assert!(html.contains(r#"data-mode="subagent""#));
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;"));

    engine
        .try_dispatch(WebAppAction::SyncLinkedThreadPane(
            SyncLinkedThreadPanePayload {
                working: Some(false),
                has_more_history: Some(false),
                error_message: Some("timeout".to_owned()),
            },
        ))
        .map_err(|e| e.to_string())?;
    assert!(!engine.state().features.linked_pane.working);
    assert!(engine.state().features.linked_pane.error_message.is_some());

    // Close + Reopen reverse state.
    engine
        .try_dispatch(WebAppAction::CloseLinkedThreadPane)
        .map_err(|e| e.to_string())?;
    assert!(!engine.state().features.linked_pane.open);
    assert!(engine.render_html().contains("Reabrir"));
    engine
        .try_dispatch(WebAppAction::ReopenLinkedThreadPane)
        .map_err(|e| e.to_string())?;
    assert!(engine.state().features.linked_pane.open);
    Ok(())
}

#[test]
fn multi_repo_picker_and_pinned_thread_repositories_roundtrip() -> Result<(), String> {
    let mut engine = WebAppEngine::new("es", 2_500);
    engine
        .try_dispatch(WebAppAction::SelectProject {
            project_id: "proj-1".to_owned(),
        })
        .map_err(|e| e.to_string())?;
    // Multi-repo project picker + thread repository pin toggle.
    engine
        .try_dispatch(WebAppAction::SetRepoPickerOpen { open: true })
        .map_err(|e| e.to_string())?;
    engine
        .try_dispatch(WebAppAction::SetProjectRepositories(
            SetProjectRepositoriesPayload {
                project_id: "proj-1".to_owned(),
                tracked_folders: vec!["/ws/web".to_owned()],
                scanned_folders: vec![WorkspaceScannedFolder {
                    name: "rust".to_owned(),
                    path: "/ws/rust".to_owned(),
                    source: WorkspaceScannedFolderSource::Child,
                    is_git_repository: true,
                }],
                default_worktree_repos: vec!["/ws/web".to_owned()],
            },
        ))
        .map_err(|e| e.to_string())?;
    engine
        .try_dispatch(WebAppAction::ToggleProjectRepository(
            ProjectRepoFolderPayload {
                project_id: "proj-1".to_owned(),
                folder_path: "/ws/rust".to_owned(),
            },
        ))
        .map_err(|e| e.to_string())?;
    assert_eq!(
        engine
            .state()
            .features
            .repo_picker
            .tracked_folders_by_project["proj-1"]
            .len(),
        2
    );
    assert!(
        engine
            .render_html()
            .contains(r#"data-repo-path="/ws/rust""#)
    );

    engine
        .try_dispatch(WebAppAction::TogglePinThreadRepository(
            ThreadRepoPinPayload {
                thread_id: "th-1".to_owned(),
                repo_path: "/ws/rust".to_owned(),
            },
        ))
        .map_err(|e| e.to_string())?;
    assert_eq!(
        engine.state().features.repo_picker.pinned_repos_by_thread["th-1"].len(),
        1
    );
    engine
        .try_dispatch(WebAppAction::TogglePinThreadRepository(
            ThreadRepoPinPayload {
                thread_id: "th-1".to_owned(),
                repo_path: "/ws/rust".to_owned(),
            },
        ))
        .map_err(|e| e.to_string())?;
    assert_eq!(
        engine.state().features.repo_picker.pinned_repos_by_thread["th-1"].len(),
        0
    );
    Ok(())
}
