//! Pure Safe Rust reducers for Environments Map & batch/transitive P2P linking (`ccabed086a`,
//! `8e8ff9b243`, `6f644f6e91`), unified Side/Subagent/Linked Thread read-only pane (`1cb3a8c753`,
//! `7c6e889ddb`), and multi-repo project picker & pinned thread repos (`60863edc41`).

use std::collections::BTreeSet;

use legion_client_core::linked_threads::{
    LinkBatchCandidate, LinkBatchQueue, LinkListState, LinkableMachine, LinkedThreadTarget,
    LinkedThreadViewState, PinnedThreadRepository, ThreadRepositoryRef, append_tracked_folders,
    classify_linked_thread_failure, link_batch_candidates, link_list_state, pin_thread_repository,
    resolve_worktree_repository_selection, suggested_tracked_folders, thread_git_repositories,
    unpin_thread_repository,
};
use legion_contracts::ids::EnvironmentId;
use legion_contracts::peer_links::{PeerLinkFailureKind, PeerMcpClientAccess};
use legion_contracts::workspace_folders::{
    PROJECT_TRACKED_FOLDERS_MAX, ProjectTrackedFolder, WorkspaceFolder, WorkspaceFolderState,
    WorkspaceScannedFolder,
};

use crate::effects::{WebHostEffect, WebUiError, encode_rpc_request_frame_parts};
use crate::state::WebAppState;
use crate::state_features::{
    BatchLinkRowState, FleetEdgeKind, FleetEnvironmentEdge, FleetEnvironmentNode,
    LinkedPaneMessage, LinkedPaneSurfaceMode,
};

/// Populates Environments Map with local/tailnet client routes, direct P2P links, and
/// transitive full-access peer hops (`6f644f6e91`). Explicitly models ZERO Ping cloud relays.
pub fn set_environments_fleet(
    state: &mut WebAppState,
    nodes: Vec<FleetEnvironmentNode>,
    raw_edges: Vec<FleetEnvironmentEdge>,
) {
    let mut edges = raw_edges;
    let full_peers: Vec<&FleetEnvironmentNode> = nodes
        .iter()
        .filter(|n| n.linked && matches!(n.access, Some(PeerMcpClientAccess::FullAccess)))
        .collect();
    for peer in full_peers {
        for edge in edges.clone() {
            if edge.from_id == peer.id
                && matches!(edge.edge, FleetEdgeKind::DirectPeerLink)
                && edge.to_id != peer.id
                && !edges
                    .iter()
                    .any(|e| e.from_id == "local" && e.to_id == edge.to_id)
            {
                edges.push(FleetEnvironmentEdge {
                    from_id: "local".to_owned(),
                    to_id: edge.to_id,
                    edge: FleetEdgeKind::TransitivePeerLink {
                        via_peer_id: peer.id.clone(),
                    },
                });
            }
        }
    }

    let list_state = link_list_state(!nodes.is_empty(), false);
    state.features.environments_map.nodes = nodes;
    state.features.environments_map.edges = edges;
    state.features.environments_map.list_state = format!("{list_state:?}");
}

/// Starts concurrency-capped batch P2P linking (`8e8ff9b243`, `LINK_BATCH_CONCURRENCY = 2`).
pub fn start_batch_link(
    state: &mut WebAppState,
    candidate_ids: &[String],
) -> Result<Vec<WebHostEffect>, WebUiError> {
    let mut writable_environments = BTreeSet::new();
    let mut machines = Vec::new();
    for node in &state.features.environments_map.nodes {
        if !candidate_ids.is_empty() && !candidate_ids.contains(&node.id) {
            continue;
        }
        let Ok(env_id) = EnvironmentId::new(&node.id) else {
            continue;
        };
        if !matches!(node.access, Some(PeerMcpClientAccess::ReadOnly)) {
            writable_environments.insert(env_id.clone());
        }
        machines.push(LinkableMachine {
            environment_id: env_id,
            label: node.label.clone(),
            machine: "environment".to_owned(),
            connected: node.reachable,
            connect_only: false,
            linked: node.linked,
            urls: node.routes.clone(),
        });
    }
    let candidates: Vec<LinkBatchCandidate> =
        link_batch_candidates(&machines, &writable_environments, LinkListState::Loaded);
    let mut queue = LinkBatchQueue::from_candidates(&candidates);
    let started = queue.take_next_batch();
    let started_ids: BTreeSet<String> = started.iter().map(|id| id.as_str().to_owned()).collect();

    let mut rows = Vec::new();
    let mut effects = Vec::new();
    for cand in &candidates {
        if cand.blocked.is_some() {
            continue;
        }
        let env_id = cand.machine.environment_id.as_str().to_owned();
        let is_started = started_ids.contains(&env_id);
        rows.push(BatchLinkRowState {
            environment_id: env_id.clone(),
            status: if is_started {
                "linking".to_owned()
            } else {
                "pending".to_owned()
            },
            failure_kind: None,
        });
        if is_started {
            let req_id = state.client.alloc_request_id();
            let payload = serde_json::json!({ "environmentId": env_id });
            let parts = encode_rpc_request_frame_parts(req_id, "peer.linkStart", &payload, None)?;
            effects.push(WebHostEffect::WsSendFrameParts { parts });
        }
    }
    state.features.environments_map.batch_active = !rows.is_empty();
    state.features.environments_map.batch_rows = rows;
    Ok(effects)
}

/// Records success or failure on an in-flight batch link row without halting sibling peers.
pub fn advance_batch_link(
    state: &mut WebAppState,
    environment_id: &str,
    succeeded: bool,
    failure_kind: Option<PeerLinkFailureKind>,
) {
    for row in &mut state.features.environments_map.batch_rows {
        if row.environment_id == environment_id {
            row.status = if succeeded {
                "linked".to_owned()
            } else {
                "failed".to_owned()
            };
            row.failure_kind = failure_kind;
        }
    }
    if succeeded {
        for node in &mut state.features.environments_map.nodes {
            if node.id == environment_id {
                node.linked = true;
            }
        }
    }
    let any_active = state
        .features
        .environments_map
        .batch_rows
        .iter()
        .any(|r| r.status == "linking" || r.status == "pending");
    state.features.environments_map.batch_active = any_active;
}

/// Opens the unified right-hand read-only pane (`SideChat`, `Subagent`, or `LinkedThread`).
pub fn open_linked_pane(
    state: &mut WebAppState,
    target: LinkedThreadTarget,
    title: String,
    mode: LinkedPaneSurfaceMode,
    working: bool,
    messages: Vec<LinkedPaneMessage>,
    has_earlier: bool,
) {
    state.features.linked_pane.open = true;
    state.features.linked_pane.last_target = Some(target.clone());
    state.features.linked_pane.target = Some(target);
    state.features.linked_pane.title = Some(title);
    state.features.linked_pane.mode = Some(mode);
    state.features.linked_pane.working = working;
    state.features.linked_pane.read_only = true;
    state.features.linked_pane.messages = messages;
    state.features.linked_pane.has_earlier = has_earlier;
    state.features.linked_pane.loading_earlier = false;
    state.features.linked_pane.error_message = None;
}

/// Applies typed `LinkedThreadViewState` updates or classifies failures inside the linked pane.
pub fn sync_linked_pane_view(
    state: &mut WebAppState,
    view: Option<&LinkedThreadViewState>,
    error_message: Option<&str>,
) {
    if let Some(view_state) = view {
        state.features.linked_pane.apply_view_state(view_state);
    }
    if let Some(msg) = error_message {
        let kind = classify_linked_thread_failure(None, "local", "peer");
        state.features.linked_pane.error_message = Some(format!("{}: {msg}", kind.title));
        state.features.linked_pane.loading_earlier = false;
    }
}

/// Closes the linked-thread pane while retaining `last_target` for quick reopen (`Close needs reopen`).
pub fn close_linked_pane(state: &mut WebAppState) {
    state.features.linked_pane.open = false;
}

/// Reopens the previously closed linked-thread pane when a `last_target` exists.
pub fn reopen_linked_pane(state: &mut WebAppState) {
    if state.features.linked_pane.target.is_some()
        || state.features.linked_pane.last_target.is_some()
    {
        state.features.linked_pane.open = true;
    }
}

fn make_tracked_folder(path: &str) -> ProjectTrackedFolder {
    let name = path.rsplit('/').next().unwrap_or(path).to_owned();
    ProjectTrackedFolder {
        path: path.to_owned(),
        name,
        discovered: None,
    }
}

fn make_workspace_folder(path: &str) -> WorkspaceFolder {
    let name = path.rsplit('/').next().unwrap_or(path).to_owned();
    WorkspaceFolder {
        name,
        path: path.to_owned(),
        state: WorkspaceFolderState::Git,
        discovered: None,
    }
}

/// Sets tracked/scanned folders for a multi-repo project using `suggested_tracked_folders` (`60863edc41`).
pub fn set_project_repositories(
    state: &mut WebAppState,
    project_id: &str,
    tracked_folders: Vec<String>,
    scanned_folders: Vec<WorkspaceScannedFolder>,
    default_worktree_repos: &[String],
) {
    let tracked_vals: Vec<ProjectTrackedFolder> = tracked_folders
        .iter()
        .take(PROJECT_TRACKED_FOLDERS_MAX)
        .map(|p| make_tracked_folder(p))
        .collect();
    let _suggested = suggested_tracked_folders(&tracked_vals, &scanned_folders, None);
    let folders_for_worktree: Vec<WorkspaceFolder> = tracked_folders
        .iter()
        .take(PROJECT_TRACKED_FOLDERS_MAX)
        .map(|p| make_workspace_folder(p))
        .collect();
    let explicit_opt = (!default_worktree_repos.is_empty()).then_some(default_worktree_repos);
    let selected_worktrees =
        resolve_worktree_repository_selection(&folders_for_worktree, explicit_opt);
    state
        .features
        .repo_picker
        .tracked_folders_by_project
        .insert(
            project_id.to_owned(),
            tracked_folders
                .into_iter()
                .take(PROJECT_TRACKED_FOLDERS_MAX)
                .collect(),
        );
    state
        .features
        .repo_picker
        .scanned_folders_by_project
        .insert(project_id.to_owned(), scanned_folders);
    state
        .features
        .repo_picker
        .worktree_repos_by_project
        .insert(project_id.to_owned(), selected_worktrees);
}

/// Toggles tracking a scanned repository folder on a project (`append_tracked_folders`).
pub fn toggle_project_repository(state: &mut WebAppState, project_id: &str, folder_path: &str) {
    let entry = state
        .features
        .repo_picker
        .tracked_folders_by_project
        .entry(project_id.to_owned())
        .or_default();
    if let Some(pos) = entry.iter().position(|p| p == folder_path) {
        entry.remove(pos);
    } else if entry.len() < PROJECT_TRACKED_FOLDERS_MAX {
        let existing: Vec<ProjectTrackedFolder> =
            entry.iter().map(|p| make_tracked_folder(p)).collect();
        let added = make_tracked_folder(folder_path);
        let next = append_tracked_folders(&existing, &[added]);
        *entry = next.into_iter().map(|f| f.path).collect();
    }
}

/// Pins or unpins a repository in a thread's multi-repo switcher (`pin_thread_repository` / `unpin_thread_repository`).
pub fn toggle_pin_thread_repository(state: &mut WebAppState, thread_id: &str, repo_path: &str) {
    let entry = state
        .features
        .repo_picker
        .pinned_repos_by_thread
        .entry(thread_id.to_owned())
        .or_default();
    let is_pinned = entry.iter().any(|p| p.repository_path == repo_path);
    *entry = if is_pinned {
        unpin_thread_repository(entry, repo_path)
    } else {
        let name = repo_path.rsplit('/').next().unwrap_or(repo_path).to_owned();
        pin_thread_repository(
            entry,
            PinnedThreadRepository {
                repository_path: repo_path.to_owned(),
                name,
            },
        )
    };
}

/// Computes available repository refs for a project using `thread_git_repositories`.
#[must_use]
pub fn project_git_repositories(state: &WebAppState, project_id: &str) -> Vec<ThreadRepositoryRef> {
    let scanned = state
        .features
        .repo_picker
        .scanned_folders_by_project
        .get(project_id)
        .map_or(&[][..], Vec::as_slice);
    let tracked = state
        .features
        .repo_picker
        .tracked_folders_by_project
        .get(project_id)
        .map_or(&[][..], Vec::as_slice);
    let folders: Vec<WorkspaceFolder> = tracked
        .iter()
        .map(|path| WorkspaceFolder {
            name: scanned.iter().find(|s| s.path == *path).map_or_else(
                || path.rsplit('/').next().unwrap_or(path).to_owned(),
                |s| s.name.clone(),
            ),
            path: path.clone(),
            state: WorkspaceFolderState::Git,
            discovered: None,
        })
        .collect();
    thread_git_repositories(&[], None, &folders)
}
