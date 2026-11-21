//! Pure pull-request link and unlink command transitions.

use t3_contracts::ids::{CommandId, ThreadId};
use t3_contracts::orchestration::ThreadPullRequestLinkCommand;
use t3_contracts::pull_requests::{
    ThreadLinkedPullRequest, ThreadPullRequestLink, ThreadPullRequestLinkSource,
    effective_pull_requests, legacy_pull_request_key,
};

use crate::decider::thread_ops::{make_thread_event, require_live_thread};
use crate::decider::types::{CommandDecision, DeciderError};
use crate::sink::projection_apply::DeciderThreadState;

pub(crate) fn decide_pr_link(
    state: &DeciderThreadState,
    cmd: &ThreadPullRequestLinkCommand,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, &cmd.thread_id)?;
    let legacy = thread.linked_pull_request.as_ref().and_then(|o| o.as_ref());
    let mut links = effective_pull_requests(thread.pull_requests.as_deref(), legacy);
    let new_link = ThreadPullRequestLink {
        host: cmd.host.clone(),
        repository: cmd.repository.clone(),
        number: cmd.number,
        url: cmd.url.clone(),
        repository_web_url: cmd.repository_web_url.clone(),
        source: cmd.source,
        linked_at: now_iso.to_owned(),
        snapshot: None,
        stack: None,
        watch: None,
        ended_watch: None,
    };
    links.retain(|existing| {
        !(existing.host == new_link.host
            && existing.repository == new_link.repository
            && existing.number == new_link.number)
    });
    links.push(new_link.clone());
    thread.pull_requests = Some(links);
    thread.linked_pull_request = Some(Some(ThreadLinkedPullRequest {
        project_id: thread.project_id.clone(),
        repository: new_link.repository,
        number: new_link.number,
        url: new_link.url,
    }));
    now_iso.clone_into(&mut thread.updated_at);
    let event = make_thread_event(
        &cmd.command_id,
        0,
        &cmd.thread_id,
        now_iso,
        "thread.pull-request-synced",
        thread.clone(),
    )?;
    Ok(CommandDecision {
        command_id: cmd.command_id.clone(),
        command_type: "thread.pull-request.link".to_owned(),
        thread_id: cmd.thread_id.clone(),
        events: vec![event],
        outbox_effects: Vec::new(),
        rollback_intent: None,
        resulting_thread: Some(thread),
    })
}

pub(crate) fn decide_pr_unlink(
    state: &DeciderThreadState,
    command_id: &CommandId,
    thread_id: &ThreadId,
    host: &str,
    repository: &str,
    number: u32,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, thread_id)?;
    let legacy = thread.linked_pull_request.as_ref().and_then(|o| o.as_ref());
    let mut links = effective_pull_requests(thread.pull_requests.as_deref(), legacy);
    let was_stack = links.iter().any(|l| {
        l.host == host
            && l.repository == repository
            && l.number == number
            && l.source == ThreadPullRequestLinkSource::Stack
    });
    links.retain(|l| !(l.host == host && l.repository == repository && l.number == number));
    if was_stack {
        links.push(ThreadPullRequestLink {
            host: host.to_owned(),
            repository: repository.to_owned(),
            number,
            url: String::new(),
            repository_web_url: None,
            source: ThreadPullRequestLinkSource::StackDismissed,
            linked_at: now_iso.to_owned(),
            snapshot: None,
            stack: None,
            watch: None,
            ended_watch: None,
        });
    }
    thread.pull_requests = Some(links);
    if thread
        .linked_pull_request
        .as_ref()
        .and_then(|o| o.as_ref())
        .is_some_and(|pr| {
            let key = legacy_pull_request_key(pr, Some(host));
            key.host == host && key.repository == repository && key.number == number
        })
    {
        thread.linked_pull_request = Some(None);
    }
    if thread
        .branch_pull_request
        .as_ref()
        .and_then(|o| o.as_ref())
        .is_some_and(|pr| {
            let key = legacy_pull_request_key(pr, Some(host));
            key.host == host && key.repository == repository && key.number == number
        })
    {
        thread.branch_pull_request = Some(None);
    }
    now_iso.clone_into(&mut thread.updated_at);
    let event = make_thread_event(
        command_id,
        0,
        thread_id,
        now_iso,
        "thread.pull-request-synced",
        thread.clone(),
    )?;
    Ok(CommandDecision {
        command_id: command_id.clone(),
        command_type: "thread.pull-request.unlink".to_owned(),
        thread_id: thread_id.clone(),
        events: vec![event],
        outbox_effects: Vec::new(),
        rollback_intent: None,
        resulting_thread: Some(thread),
    })
}
