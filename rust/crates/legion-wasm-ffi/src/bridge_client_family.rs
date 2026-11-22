//! Thread-family cache action dispatch for [`crate::bridge_client::ClientCoreBridge`].

use std::collections::BTreeMap;

use legion_client_core::thread_families::{
    MountHandle, ThreadFamilyCache, ThreadFamilyRow, ThreadFamilySnapshot,
};
use legion_contracts::ids::{EnvironmentId, ThreadId};

use crate::bridge_client_dto::{FamilyRowInput, TouchFamilyRequest, TouchFamilyResponse};

#[derive(Default)]
struct FamilyResponseMeta {
    action: String,
    environment_id: Option<EnvironmentId>,
    thread_id: Option<ThreadId>,
    fetch_generation: Option<u64>,
    released: Option<bool>,
    evicted_count: Option<usize>,
    merged_threads: Option<Vec<ThreadFamilyRow<serde_json::Value>>>,
}

pub(crate) fn touch_family_step(
    families: &mut ThreadFamilyCache<ThreadFamilyRow<serde_json::Value>>,
    family_handles: &mut BTreeMap<(EnvironmentId, ThreadId), Vec<MountHandle>>,
    req: TouchFamilyRequest,
    now_ms: u64,
) -> Result<String, String> {
    let action = req.action.as_deref().unwrap_or("touch").to_owned();
    match action.as_str() {
        "evict" => {
            let evicted = families.evict_expired(now_ms);
            return serialize_family_response(
                families,
                FamilyResponseMeta {
                    action,
                    evicted_count: Some(evicted),
                    ..FamilyResponseMeta::default()
                },
            );
        }
        "setGeneration" | "set_generation" => {
            let env_id = parse_env_id(req.environment_id.as_deref())?;
            let generation = req
                .generation
                .ok_or_else(|| "setGeneration requires `generation`".to_owned())?;
            families.set_shell_generation(&env_id, generation, now_ms);
            return serialize_family_response(
                families,
                FamilyResponseMeta {
                    action,
                    environment_id: Some(env_id),
                    ..FamilyResponseMeta::default()
                },
            );
        }
        "merge" => {
            let env_id = parse_env_id(req.environment_id.as_deref())?;
            let shell_rows = convert_rows(req.shell_threads.unwrap_or_default())?;
            let mut removed_at = BTreeMap::new();
            for (raw_tid, seq) in req.removed_at {
                let tid = ThreadId::new(raw_tid).map_err(|err| err.to_string())?;
                removed_at.insert(tid, seq);
            }
            let merged = families.merged_threads(
                &env_id,
                &shell_rows,
                req.older_settled_as_of_sequence,
                &removed_at,
            );
            return serialize_family_response(
                families,
                FamilyResponseMeta {
                    action,
                    environment_id: Some(env_id),
                    merged_threads: Some(merged),
                    ..FamilyResponseMeta::default()
                },
            );
        }
        _ => {}
    }

    let env_id = parse_env_id(req.environment_id.as_deref())?;
    let thread_id = parse_thread_id(req.thread_id.as_deref())?;
    dispatch_thread_action(
        families,
        family_handles,
        action,
        env_id,
        thread_id,
        req,
        now_ms,
    )
}

fn dispatch_thread_action(
    families: &mut ThreadFamilyCache<ThreadFamilyRow<serde_json::Value>>,
    family_handles: &mut BTreeMap<(EnvironmentId, ThreadId), Vec<MountHandle>>,
    action: String,
    env_id: EnvironmentId,
    thread_id: ThreadId,
    req: TouchFamilyRequest,
    now_ms: u64,
) -> Result<String, String> {
    match action.as_str() {
        "touch" => handle_family_touch(
            families,
            family_handles,
            action,
            env_id,
            thread_id,
            req,
            now_ms,
        ),
        "mount" => {
            if let Some(gen_val) = req.generation {
                families.set_shell_generation(&env_id, gen_val, now_ms);
            }
            let handle = families.mount_family(&env_id, &thread_id, now_ms);
            family_handles
                .entry((env_id.clone(), thread_id.clone()))
                .or_default()
                .push(handle);
            serialize_family_response(
                families,
                FamilyResponseMeta {
                    action,
                    environment_id: Some(env_id),
                    thread_id: Some(thread_id),
                    ..FamilyResponseMeta::default()
                },
            )
        }
        "beginFetch" | "begin_fetch" => {
            let fetch_gen = families.begin_fetch(&env_id, &thread_id, now_ms);
            serialize_family_response(
                families,
                FamilyResponseMeta {
                    action,
                    environment_id: Some(env_id),
                    thread_id: Some(thread_id),
                    fetch_generation: fetch_gen,
                    ..FamilyResponseMeta::default()
                },
            )
        }
        "completeFetch" | "complete_fetch" => {
            let generation = req
                .generation
                .unwrap_or_else(|| families.shell_generation(&env_id));
            let enabled = req.enabled.unwrap_or(true);
            let result = build_family_fetch_result(
                req.snapshot_sequence.unwrap_or(0),
                req.threads,
                req.error,
            )?;
            families.complete_fetch(&env_id, &thread_id, generation, result, enabled, now_ms);
            serialize_family_response(
                families,
                FamilyResponseMeta {
                    action,
                    environment_id: Some(env_id),
                    thread_id: Some(thread_id),
                    fetch_generation: Some(generation),
                    ..FamilyResponseMeta::default()
                },
            )
        }
        "release" => {
            let released =
                release_family_handle(families, family_handles, &env_id, &thread_id, now_ms);
            serialize_family_response(
                families,
                FamilyResponseMeta {
                    action,
                    environment_id: Some(env_id),
                    thread_id: Some(thread_id),
                    released: Some(released),
                    ..FamilyResponseMeta::default()
                },
            )
        }
        other => Err(format!("unsupported touch_family action `{other}`")),
    }
}

fn handle_family_touch(
    families: &mut ThreadFamilyCache<ThreadFamilyRow<serde_json::Value>>,
    family_handles: &mut BTreeMap<(EnvironmentId, ThreadId), Vec<MountHandle>>,
    action: String,
    env_id: EnvironmentId,
    thread_id: ThreadId,
    req: TouchFamilyRequest,
    now_ms: u64,
) -> Result<String, String> {
    if let Some(gen_val) = req.generation {
        families.set_shell_generation(&env_id, gen_val, now_ms);
    }
    let key = (env_id.clone(), thread_id.clone());
    family_handles.entry(key).or_insert_with(|| {
        let handle = families.mount_family(&env_id, &thread_id, now_ms);
        vec![handle]
    });
    let mut fetch_generation = None;
    if req.threads.is_some() || req.error.is_some() {
        fetch_generation = families.begin_fetch(&env_id, &thread_id, now_ms);
        if let Some(generation) = fetch_generation {
            let result = build_family_fetch_result(
                req.snapshot_sequence.unwrap_or(0),
                req.threads,
                req.error,
            )?;
            families.complete_fetch(
                &env_id,
                &thread_id,
                generation,
                result,
                req.enabled.unwrap_or(true),
                now_ms,
            );
        }
    }
    serialize_family_response(
        families,
        FamilyResponseMeta {
            action,
            environment_id: Some(env_id),
            thread_id: Some(thread_id),
            fetch_generation,
            ..FamilyResponseMeta::default()
        },
    )
}

fn release_family_handle(
    families: &mut ThreadFamilyCache<ThreadFamilyRow<serde_json::Value>>,
    family_handles: &mut BTreeMap<(EnvironmentId, ThreadId), Vec<MountHandle>>,
    env_id: &EnvironmentId,
    thread_id: &ThreadId,
    now_ms: u64,
) -> bool {
    let key = (env_id.clone(), thread_id.clone());
    let Some(handles) = family_handles.get_mut(&key) else {
        return false;
    };
    let Some(mut handle) = handles.pop() else {
        family_handles.remove(&key);
        return false;
    };
    if handles.is_empty() {
        family_handles.remove(&key);
    }
    families.release(&mut handle, now_ms)
}

fn serialize_family_response(
    families: &ThreadFamilyCache<ThreadFamilyRow<serde_json::Value>>,
    meta: FamilyResponseMeta,
) -> Result<String, String> {
    let shell_generation = meta
        .environment_id
        .as_ref()
        .map(|env| families.shell_generation(env));
    let read_state = match (meta.environment_id.as_ref(), meta.thread_id.as_ref()) {
        (Some(env), Some(tid)) => Some(families.read_state(env, tid)),
        _ => None,
    };
    let mount_ref_count = match (meta.environment_id.as_ref(), meta.thread_id.as_ref()) {
        (Some(env), Some(tid)) => {
            Some(families.mount_ref_count(env, &format!("family:{}", tid.as_str())))
        }
        _ => None,
    };
    let response = TouchFamilyResponse {
        action: meta.action,
        environment_id: meta.environment_id,
        thread_id: meta.thread_id,
        shell_generation,
        read_state,
        mount_ref_count,
        cached_entry_count: families.cached_entry_count(),
        fetch_generation: meta.fetch_generation,
        released: meta.released,
        evicted_count: meta.evicted_count,
        merged_threads: meta.merged_threads,
    };
    serde_json::to_string(&response)
        .map_err(|err| format!("failed to serialize family cache state: {err}"))
}

fn parse_env_id(raw: Option<&str>) -> Result<EnvironmentId, String> {
    let text = raw.ok_or_else(|| "missing required `environmentId`".to_owned())?;
    EnvironmentId::new(text).map_err(|err| err.to_string())
}

fn parse_thread_id(raw: Option<&str>) -> Result<ThreadId, String> {
    let text = raw.ok_or_else(|| "missing required `threadId`".to_owned())?;
    ThreadId::new(text).map_err(|err| err.to_string())
}

fn convert_rows(
    inputs: Vec<FamilyRowInput>,
) -> Result<Vec<ThreadFamilyRow<serde_json::Value>>, String> {
    inputs.into_iter().map(FamilyRowInput::into_row).collect()
}

fn build_family_fetch_result(
    snapshot_sequence: u64,
    threads: Option<Vec<FamilyRowInput>>,
    error: Option<String>,
) -> Result<Result<ThreadFamilySnapshot<ThreadFamilyRow<serde_json::Value>>, String>, String> {
    if let Some(err) = error {
        return Ok(Err(err));
    }
    let rows = convert_rows(threads.unwrap_or_default())?;
    Ok(Ok(ThreadFamilySnapshot {
        snapshot_sequence,
        threads: rows,
    }))
}
